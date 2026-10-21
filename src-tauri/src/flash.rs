//! Erase a USB disk, copy an image onto it byte for byte, then read it back to verify.
use crate::drives::{self, Drive};
use crate::image::read_full;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel};
use std::time::{Duration, Instant};

// 8 MB blocks keep USB SSDs at full speed; DEPTH chunks are read ahead while one is being written.
const CHUNK: usize = 8 << 20;
const DEPTH: usize = 3;
// Raw disk I/O must be whole sectors; 4096 covers both 512 and 4K sector drives.
const SECTOR: usize = 4096;
pub const CANCELLED: &str = "cancelled";

#[derive(Serialize, Clone)]
pub struct Progress {
    pub phase: &'static str,
    pub done: u64,
    pub total: u64,
}

pub fn run(path: &str, picked: &Drive, cancel: &AtomicBool, emit: impl FnMut(Progress)) -> Result<(), String> {
    let total = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if total == 0 {
        return Err("emptyImage".into());
    }
    // Disk numbers can be reused after replugging: the drive must still be exactly the one shown.
    if !drives::list()?.contains(picked) {
        return Err("driveChanged".into());
    }
    if total > picked.size {
        return Err("tooLarge".into());
    }
    wipe(picked.number)?;
    let disk = format!(r"\\.\PhysicalDrive{}", picked.number);
    let result = write_and_verify(Path::new(path), Path::new(&disk), total, cancel, emit);
    // Let Windows read the new partition table and mount it.
    let _ = drives::powershell(&format!("Update-Disk -Number {}", picked.number));
    result
}

/// Back to a normal drive: one partition using the whole disk, formatted.
pub fn restore(picked: &Drive, fs: &str, label: &str) -> Result<(), String> {
    if !["exfat", "ntfs", "fat32"].contains(&fs) {
        return Err(format!("Unknown file system: {fs}"));
    }
    if !drives::list()?.contains(picked) {
        return Err("driveChanged".into());
    }
    // FAT labels max out at 11 characters; keep it to characters diskpart can't misread.
    let label: String = label.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ' ').take(11).collect();
    // MBR boots and mounts everywhere (TVs, consoles, old BIOS); only disks past 2 TB need GPT.
    let style = if picked.size > 2 << 40 { "gpt" } else { "mbr" };
    diskpart(&format!(
        "select disk {}\nclean\nconvert {style}\ncreate partition primary\nformat fs={fs} quick label=\"{}\"\nassign\n",
        picked.number,
        label.trim()
    ))
}

/// Removes the partition table so Windows lets go of the disk's volumes (same as etcher's `diskpart clean`).
fn wipe(number: u32) -> Result<(), String> {
    diskpart(&format!("select disk {number}\nclean\n"))
}

/// One diskpart at a time: parallel flashes share the script file and Windows' disk service.
static DISKPART: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn diskpart(commands: &str) -> Result<(), String> {
    let _one_at_a_time = DISKPART.lock().unwrap_or_else(|e| e.into_inner());
    let script = std::env::temp_dir().join("usboot-diskpart.txt");
    std::fs::write(&script, commands).map_err(|e| e.to_string())?;
    let out = Command::new("diskpart").arg("/s").arg(&script).creation_flags(0x0800_0000).output();
    let _ = std::fs::remove_file(&script);
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!("diskpart|{}", String::from_utf8_lossy(&o.stdout).trim())),
        Err(e) => Err(io_msg(e)),
    }
}

fn io_msg(e: std::io::Error) -> String {
    match e.raw_os_error() {
        Some(5) | Some(740) => "needsAdmin".into(),
        _ => e.to_string(),
    }
}

pub fn write_and_verify(
    src: &Path,
    dst: &Path,
    total: u64,
    cancel: &AtomicBool,
    mut emit: impl FnMut(Progress),
) -> Result<(), String> {
    let mut out = OpenOptions::new().read(true).write(true).open(dst).map_err(io_msg)?;
    let mut back = vec![0u8; CHUNK];

    for phase in ["write", "verify"] {
        out.seek(SeekFrom::Start(0)).map_err(io_msg)?;
        // A reader thread keeps the next chunks of the image ready, so the drive never waits on the source.
        std::thread::scope(|s| {
            let (full_tx, full_rx) = sync_channel::<(Vec<u8>, usize)>(DEPTH);
            let (free_tx, free_rx) = channel::<Vec<u8>>();
            for _ in 0..=DEPTH {
                free_tx.send(vec![0u8; CHUNK]).unwrap();
            }
            let reader = s.spawn(move || -> Result<(), String> {
                let mut input = File::open(src).map_err(io_msg)?;
                while let Ok(mut buf) = free_rx.recv() {
                    let n = read_full(&mut input, &mut buf).map_err(io_msg)?;
                    if n == 0 || full_tx.send((buf, n)).is_err() {
                        break;
                    }
                }
                Ok(())
            });

            let mut done = 0u64;
            let mut last = Instant::now();
            emit(Progress { phase, done, total });
            for (mut buf, n) in &full_rx {
                if cancel.load(Ordering::SeqCst) {
                    return Err(CANCELLED.into());
                }
                let n = n.min((total - done) as usize);
                let padded = n.div_ceil(SECTOR) * SECTOR;
                if phase == "write" {
                    buf[n..padded].fill(0);
                    out.write_all(&buf[..padded]).map_err(io_msg)?;
                } else {
                    read_full(&mut out, &mut back[..padded]).map_err(io_msg)?;
                    if let Some(i) = buf[..n].iter().zip(&back[..n]).position(|(a, b)| a != b) {
                        return Err(format!("verifyFailed|{}", done + i as u64));
                    }
                }
                done += n as u64;
                let _ = free_tx.send(buf);
                if last.elapsed() > Duration::from_millis(100) || done == total {
                    emit(Progress { phase, done, total });
                    last = Instant::now();
                }
                if done == total {
                    break;
                }
            }
            if done < total {
                drop(full_rx);
                reader.join().unwrap()?;
                return Err("imageChanged".into());
            }
            Ok(())
        })?;
        if phase == "write" {
            out.sync_all().map_err(io_msg)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_odd_sized_image_and_verifies() {
        let dir = std::env::temp_dir();
        let (src, dst) = (dir.join("usboot-src.img"), dir.join("usboot-dst.img"));
        let data: Vec<u8> = (0..CHUNK * 2 + 123).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        std::fs::write(&dst, b"old contents").unwrap();

        let mut phases = vec![];
        let ok = write_and_verify(&src, &dst, data.len() as u64, &AtomicBool::new(false), |p| phases.push((p.phase, p.done)));
        let written = std::fs::read(&dst).unwrap();
        let cancelled = write_and_verify(&src, &dst, data.len() as u64, &AtomicBool::new(true), |_| {});
        std::fs::remove_file(&src).ok();
        std::fs::remove_file(&dst).ok();

        assert_eq!(ok, Ok(()));
        assert_eq!(&written[..data.len()], &data[..]);
        assert_eq!(written.len() % SECTOR, 0, "writes are whole sectors");
        assert_eq!(phases.last(), Some(&("verify", data.len() as u64)));
        assert_eq!(cancelled, Err(CANCELLED.into()));
    }
}
