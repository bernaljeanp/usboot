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

/// The Rufus-style tweaks, written as autounattend.xml at the root of the drive (Setup reads it on its own).
#[derive(serde::Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// Skip the TPM 2.0 / Secure Boot / CPU / RAM / storage checks.
    pub bypass_requirements: bool,
    /// Create this local user instead of asking for a Microsoft account.
    pub local_account: Option<String>,
    /// Answer the privacy and EULA screens.
    pub skip_privacy: bool,
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// Windows refuses these in user names, and they'd also break the `net user` command line.
pub fn valid_user(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && n.len() <= 20 && !n.chars().any(|c| "\\/[]:;|=,+*?<>@\"%".contains(c)) && !n.ends_with('.')
}

pub fn unattend(o: &Options) -> Option<String> {
    let user = o.local_account.as_deref().map(str::trim).filter(|n| !n.is_empty());
    if !o.bypass_requirements && user.is_none() && !o.skip_privacy {
        return None;
    }
    const COMP: &str = r#"processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS""#;
    let cmd = |order: usize, path: &str| {
        format!("<RunSynchronousCommand wcm:action=\"add\"><Order>{order}</Order><Path>{}</Path></RunSynchronousCommand>", xml_escape(path))
    };
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<unattend xmlns=\"urn:schemas-microsoft-com:unattend\" xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\">\n",
    );
    if o.bypass_requirements {
        let checks = ["BypassTPMCheck", "BypassSecureBootCheck", "BypassCPUCheck", "BypassRAMCheck", "BypassStorageCheck"];
        let cmds: String = checks
            .iter()
            .enumerate()
            .map(|(i, c)| cmd(i + 1, &format!(r"reg add HKLM\SYSTEM\Setup\LabConfig /v {c} /t REG_DWORD /d 1 /f")))
            .collect();
        xml += &format!(
            "<settings pass=\"windowsPE\"><component name=\"Microsoft-Windows-Setup\" {COMP}><RunSynchronous>{cmds}</RunSynchronous></component></settings>\n"
        );
    }
    if user.is_some() {
        // Without this, Windows 11 Home/Pro insists on a network and a Microsoft account.
        let nro = cmd(1, r"reg add HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\OOBE /v BypassNRO /t REG_DWORD /d 1 /f");
        xml += &format!(
            "<settings pass=\"specialize\"><component name=\"Microsoft-Windows-Deployment\" {COMP}><RunSynchronous>{nro}</RunSynchronous></component></settings>\n"
        );
    }
    if user.is_some() || o.skip_privacy {
        let mut shell = String::new();
        shell += "<OOBE>";
        if o.skip_privacy {
            shell += "<ProtectYourPC>3</ProtectYourPC><HideEULAPage>true</HideEULAPage>";
        }
        if user.is_some() {
            shell += "<HideOnlineAccountScreens>true</HideOnlineAccountScreens>";
        }
        shell += "</OOBE>";
        if let Some(name) = user {
            let n = xml_escape(name);
            // Blank password that must be changed at first sign-in, like Rufus.
            shell += &format!(
                "<UserAccounts><LocalAccounts><LocalAccount wcm:action=\"add\"><Name>{n}</Name><Group>Administrators</Group><Password><Value></Value><PlainText>true</PlainText></Password></LocalAccount></LocalAccounts></UserAccounts>\
                 <FirstLogonCommands><SynchronousCommand wcm:action=\"add\"><Order>1</Order><CommandLine>{}</CommandLine></SynchronousCommand></FirstLogonCommands>",
                xml_escape(&format!("net user \"{name}\" /logonpasswordchg:yes"))
            );
        }
        xml += &format!("<settings pass=\"oobeSystem\"><component name=\"Microsoft-Windows-Shell-Setup\" {COMP}>{shell}</component></settings>\n");
    }
    xml += "</unattend>\n";
    Some(xml)
}

pub fn run(iso: &str, picked: &Drive, opts: &Options, cancel: &AtomicBool, mut emit: impl FnMut(Progress)) -> Result<(), String> {
    if let Some(name) = opts.local_account.as_deref().filter(|n| !n.trim().is_empty()) {
        if !valid_user(name) {
            return Err("badUser".into());
        }
    }
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

    let result = copy_and_split(&src, &usb, cancel, &mut emit).and_then(|_| match unattend(opts) {
        Some(xml) => fs::write(usb.join("autounattend.xml"), xml).map_err(|e| e.to_string()),
        None => Ok(()),
    });
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

    #[test]
    fn unattend_only_has_what_was_asked() {
        assert_eq!(unattend(&Options::default()), None);

        let bypass = unattend(&Options { bypass_requirements: true, ..Default::default() }).unwrap();
        assert!(bypass.contains("BypassTPMCheck") && bypass.contains("pass=\"windowsPE\""));
        assert!(!bypass.contains("oobeSystem"));

        let user = unattend(&Options { local_account: Some("Ana <&> \"x\"".into()), skip_privacy: true, ..Default::default() }).unwrap();
        assert!(user.contains("BypassNRO") && user.contains("<ProtectYourPC>3</ProtectYourPC>"));
        assert!(user.contains("<Name>Ana &lt;&amp;&gt; &quot;x&quot;</Name>"), "names are XML-escaped");
        assert!(!user.contains("windowsPE"));

        assert!(valid_user("Jean") && valid_user("ana.perez"));
        assert!(!valid_user("") && !valid_user("a/b") && !valid_user("dot.") && !valid_user("abcdefghijklmnopqrstu"));
    }

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
