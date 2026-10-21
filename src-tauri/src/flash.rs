//! Erase a USB disk, copy an image onto it byte for byte, then read it back to verify.
use crate::drives::{self, Drive};
use crate::image::read_full;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
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
    os::prepare(picked)?;
    let result = write_and_verify(Path::new(path), Path::new(&picked.device), total, cancel, emit);
    os::settle(picked);
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
    // FAT labels max out at 11 characters; keep it to characters no shell or diskpart can misread.
    let label: String = label.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(11).collect();
    let label = if label.is_empty() { "USB".to_string() } else { label };
    // MBR boots and mounts everywhere (TVs, consoles, old BIOS); only disks past 2 TB need GPT.
    let gpt = picked.size > 2 << 40;
    os::restore(picked, fs, &label, gpt)
}

fn io_msg(e: std::io::Error) -> String {
    match e.raw_os_error() {
        Some(5) | Some(740) | Some(13) | Some(1) => "needsAdmin".into(),
        _ => e.to_string(),
    }
}

#[cfg(windows)]
pub use os::diskpart;

/// The parts that differ per OS: letting go of the disk, raw access rights, and formatting.
#[cfg(windows)]
mod os {
    use super::{Drive, io_msg};
    use crate::drives;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    /// Removes the partition table so Windows lets go of the disk's volumes (same as etcher's `diskpart clean`).
    pub fn prepare(d: &Drive) -> Result<(), String> {
        diskpart(&format!("select disk {}\nclean\n", d.number))
    }

    /// Let Windows read the new partition table and mount it.
    pub fn settle(d: &Drive) {
        let _ = drives::powershell(&format!("Update-Disk -Number {}", d.number));
    }

    pub fn restore(d: &Drive, fs: &str, label: &str, gpt: bool) -> Result<(), String> {
        let style = if gpt { "gpt" } else { "mbr" };
        diskpart(&format!(
            "select disk {}\nclean\nconvert {style}\ncreate partition primary\nformat fs={fs} quick label=\"{label}\"\nassign\n",
            d.number
        ))
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
}

/// Linux and macOS: the app runs as the user and asks for the password once per drive, through the
/// system's own dialog, only to unmount the disk and hand the user its device node. The copy itself
/// then runs as the user.
#[cfg(unix)]
mod os {
    use super::Drive;
    use std::process::Command;

    /// Runs a root shell script through the OS password dialog (pkexec / macOS admin prompt).
    fn elevated(script: &str) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        let out = Command::new("pkexec").args(["sh", "-c", script]).output();
        #[cfg(target_os = "macos")]
        let out = Command::new("osascript")
            .arg("-e")
            .arg(format!("do shell script \"{}\" with administrator privileges", script.replace('\\', "\\\\").replace('"', "\\\"")))
            .output();
        match out {
            Ok(o) if o.status.success() => Ok(()),
            // 126/127: pkexec dialog dismissed; -128 in stderr: macOS prompt cancelled
            Ok(o) if matches!(o.status.code(), Some(126) | Some(127)) || String::from_utf8_lossy(&o.stderr).contains("-128") => {
                Err("needsAdmin".into())
            }
            Ok(o) => Err(format!("system|{}", String::from_utf8_lossy(&o.stderr).trim())),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Only plain /dev paths reach a root shell.
    fn safe(dev: &str) -> Result<&str, String> {
        if dev.starts_with("/dev/") && dev[5..].chars().all(|c| c.is_ascii_alphanumeric()) { Ok(dev) } else { Err("driveChanged".into()) }
    }

    #[cfg(target_os = "linux")]
    pub fn prepare(d: &Drive) -> Result<(), String> {
        let dev = safe(&d.device)?;
        elevated(&format!(
            "for p in {dev}?*; do umount \"$p\" 2>/dev/null; done; wipefs -a {dev} >/dev/null && chown \"$PKEXEC_UID\" {dev}"
        ))
    }

    #[cfg(target_os = "macos")]
    fn whole(d: &Drive) -> Result<String, String> {
        Ok(safe(&d.device)?.replacen("/dev/r", "/dev/", 1))
    }

    #[cfg(target_os = "macos")]
    pub fn prepare(d: &Drive) -> Result<(), String> {
        let (raw, disk) = (safe(&d.device)?, whole(d)?);
        let uid = String::from_utf8_lossy(&Command::new("id").arg("-u").output().map_err(|e| e.to_string())?.stdout).trim().to_string();
        elevated(&format!("diskutil unmountDisk force {disk} && chown {uid} {raw} {disk}"))
    }

    /// Linux: the kernel re-reads the partition table when the stick is replugged. macOS: diskutil remounts.
    pub fn settle(_d: &Drive) {
        #[cfg(target_os = "macos")]
        if let Ok(disk) = whole(_d) {
            let _ = Command::new("diskutil").args(["mountDisk", &disk]).output();
        }
    }

    #[cfg(target_os = "linux")]
    pub fn restore(d: &Drive, fs: &str, label: &str, gpt: bool) -> Result<(), String> {
        let dev = safe(&d.device)?;
        let part = if dev.ends_with(|c: char| c.is_ascii_digit()) { format!("{dev}p1") } else { format!("{dev}1") };
        let mkfs = match fs {
            "exfat" => format!("mkfs.exfat -L {label} {part}"),
            "ntfs" => format!("mkfs.ntfs -f -L {label} {part}"),
            _ => format!("mkfs.vfat -F 32 -n {label} {part}"),
        };
        let table = if gpt { "gpt" } else { "msdos" };
        elevated(&format!(
            "for p in {dev}?*; do umount \"$p\" 2>/dev/null; done; wipefs -a {dev} >/dev/null && \
             parted -s {dev} mklabel {table} mkpart primary 1MiB 100% && partprobe {dev}; udevadm settle; {mkfs}"
        ))
    }

    #[cfg(target_os = "macos")]
    pub fn restore(d: &Drive, fs: &str, label: &str, gpt: bool) -> Result<(), String> {
        let format = match fs {
            "exfat" => "ExFAT",
            "fat32" => "FAT32",
            _ => return Err("ntfsOnMac".into()),
        };
        let scheme = if gpt { "GPT" } else { "MBR" };
        elevated(&format!("diskutil eraseDisk {format} {label} {scheme} {}", whole(d)?))
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
            #[cfg(target_os = "linux")]
            // SAFETY: plain syscall on a file descriptor we own; drops the now-clean cached pages.
            unsafe {
                use std::os::fd::AsRawFd;
                libc::posix_fadvise(out.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
            }
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
