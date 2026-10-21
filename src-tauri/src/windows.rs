//! Windows mode: a Windows install ISO isn't a hybrid image, so a raw copy won't boot.
//! Instead: one GPT + FAT32 partition (every Windows 11 PC boots that over UEFI), the ISO's
//! files copied over, and install.wim split by DISM into pieces under FAT32's 4 GB limit.
use crate::drives::{self, Drive};
use crate::flash::{CANCELLED, Progress, diskpart};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const FAT32_MAX: u64 = (4 << 30) - 1;
// diskpart only formats FAT32 up to 32 GB; that's plenty for any Windows ISO.
const PARTITION_MB: u64 = 32 * 1024;
const SPLIT_MB: u32 = 3800;

pub fn run(iso: &str, picked: &Drive, cancel: &AtomicBool, mut emit: impl FnMut(Progress)) -> Result<(), String> {
    if !drives::list()?.contains(picked) {
        return Err("driveChanged".into());
    }
    let iso_size = fs::metadata(iso).map_err(|e| e.to_string())?.len();
    if iso_size > picked.size {
        return Err("tooLarge".into());
    }
    emit(Progress { phase: "prepare", done: 0, total: 1 });

    let size = if picked.size / (1 << 20) > PARTITION_MB { format!(" size={PARTITION_MB}") } else { String::new() };
    diskpart(&format!(
        "select disk {}\nclean\nconvert gpt\ncreate partition primary{size}\nformat fs=fat32 quick label=\"WINUSB\"\nassign\n",
        picked.number
    ))?;
    let usb = drives::powershell(&format!(
        "(Get-Partition -DiskNumber {} | Where-Object DriveLetter | Select-Object -First 1).DriveLetter",
        picked.number
    ))?;
    let usb = usb.chars().next().filter(char::is_ascii_alphabetic).ok_or("diskpart|no drive letter")?;
    let usb = PathBuf::from(format!("{usb}:\\"));

    let esc = iso.replace('\'', "''");
    let src = drives::powershell(&format!("(Mount-DiskImage -ImagePath '{esc}' -PassThru | Get-Volume).DriveLetter"))?;
    let src = src.chars().next().filter(char::is_ascii_alphabetic).ok_or("Could not open the ISO")?;
    let src = PathBuf::from(format!("{src}:\\"));

    let result = copy_and_split(&src, &usb, cancel, &mut emit);
    let _ = drives::powershell(&format!("Dismount-DiskImage -ImagePath '{esc}' | Out-Null"));
    result
}

fn files(dir: &Path, out: &mut Vec<(PathBuf, u64)>) -> std::io::Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let meta = e.metadata()?;
        if meta.is_dir() {
            files(&e.path(), out)?;
        } else {
            out.push((e.path(), meta.len()));
        }
    }
    Ok(())
}

fn copy_and_split(src: &Path, usb: &Path, cancel: &AtomicBool, emit: &mut impl FnMut(Progress)) -> Result<(), String> {
    let mut list = vec![];
    files(src, &mut list).map_err(|e| e.to_string())?;
    let total: u64 = list.iter().map(|(_, n)| n).sum();
    let mut done = 0u64;
    let mut last = Instant::now();
    let mut buf = vec![0u8; 8 << 20];
    let mut big = vec![];

    for (path, len) in &list {
        let rel = path.strip_prefix(src).unwrap();
        if *len > FAT32_MAX {
            big.push((path.clone(), rel.to_path_buf(), *len));
            continue;
        }
        let dest = usb.join(rel);
        fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut from = File::open(path).map_err(|e| e.to_string())?;
        let mut to = File::create(&dest).map_err(|e| e.to_string())?;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(CANCELLED.into());
            }
            let n = from.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            to.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if last.elapsed() > Duration::from_millis(100) {
                emit(Progress { phase: "copy", done, total });
                last = Instant::now();
            }
        }
        to.sync_all().map_err(|e| e.to_string())?;
    }

    // install.wim > 4 GB: DISM splits it into install.swm, install2.swm…; Setup reads them as one.
    for (path, rel, len) in big {
        let swm = usb.join(rel.with_extension("swm"));
        fs::create_dir_all(swm.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut child = Command::new("dism")
            .arg("/Split-Image")
            .arg(format!("/ImageFile:{}", path.display()))
            .arg(format!("/SWMFile:{}", swm.display()))
            .arg(format!("/FileSize:{SPLIT_MB}"))
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| e.to_string())?;
        let dir = swm.parent().unwrap().to_path_buf();
        let stem = swm.file_stem().unwrap().to_string_lossy().to_lowercase();
        loop {
            if cancel.load(Ordering::SeqCst) {
                let _ = child.kill();
                return Err(CANCELLED.into());
            }
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    return Err(format!("DISM could not split {} (exit {status})", rel.display()));
                }
                break;
            }
            // Progress = size of the .swm pieces written so far.
            let written: u64 = fs::read_dir(&dir)
                .map(|it| {
                    it.filter_map(|e| e.ok())
                        .filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with(&stem))
                        .filter_map(|e| e.metadata().ok().map(|m| m.len()))
                        .sum()
                })
                .unwrap_or(0);
            emit(Progress { phase: "split", done: done + written.min(len), total });
            std::thread::sleep(Duration::from_millis(250));
        }
        done += len;
    }
    emit(Progress { phase: "split", done: total, total });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The copy path on plain folders: small files copied, a >4 GB file would be held for DISM.
    #[test]
    fn copies_tree_and_reports_progress() {
        let root = std::env::temp_dir().join("usboot-winmode-test");
        let (src, usb) = (root.join("iso"), root.join("usb"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(src.join("sources")).unwrap();
        fs::create_dir_all(&usb).unwrap();
        fs::write(src.join("setup.exe"), vec![7u8; 3 << 20]).unwrap();
        fs::write(src.join("sources").join("boot.wim"), vec![9u8; 5 << 20]).unwrap();

        let mut last = (0, 0);
        copy_and_split(&src, &usb, &AtomicBool::new(false), &mut |p: Progress| last = (p.done, p.total)).unwrap();
        let copied = fs::read(usb.join("sources").join("boot.wim")).unwrap();
        let _ = fs::remove_dir_all(&root);

        assert_eq!(copied.len(), 5 << 20);
        assert_eq!(last, (8 << 20, 8 << 20));
    }
}
