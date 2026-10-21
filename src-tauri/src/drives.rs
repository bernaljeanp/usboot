//! Removable USB disks that are safe to offer as targets, on Windows, Linux and macOS.
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    pub number: u32,
    pub name: String,
    pub size: u64,
    pub serial: String,
    /// Windows drive letters ("E"); empty elsewhere.
    pub letters: Vec<String>,
    /// What gets opened for raw writes: \\.\PhysicalDrive2, /dev/sdb, /dev/rdisk4.
    pub device: String,
}

fn run(cmd: &mut Command) -> Result<String, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(windows)]
pub fn powershell(script: &str) -> Result<String, String> {
    let script = format!("[Console]::OutputEncoding = [Text.Encoding]::UTF8\n{script}");
    run(Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]))
}

// USB bus only, never the disk Windows booted from or lives on.
#[cfg(windows)]
const LIST: &str = r#"
ConvertTo-Json -Compress -InputObject @(
  Get-Disk | Where-Object { $_.BusType -eq 'USB' -and -not $_.IsBoot -and -not $_.IsSystem } | ForEach-Object {
    [pscustomobject]@{
      number  = $_.Number
      name    = $_.FriendlyName
      size    = $_.Size
      serial  = ([string]$_.SerialNumber).Trim()
      letters = @(Get-Partition -DiskNumber $_.Number -ErrorAction SilentlyContinue |
                  Where-Object DriveLetter | ForEach-Object { [string]$_.DriveLetter })
      device  = "\\.\PhysicalDrive" + $_.Number
    }
  })"#;

#[cfg(windows)]
pub fn list() -> Result<Vec<Drive>, String> {
    serde_json::from_str(&powershell(LIST)?).map_err(|e| e.to_string())
}

/// USB whole disks from lsblk, skipping any disk that holds /, /boot or swap.
#[cfg(target_os = "linux")]
pub fn list() -> Result<Vec<Drive>, String> {
    let out = run(Command::new("lsblk").args(["-J", "-b", "-o", "NAME,PATH,SIZE,TRAN,MODEL,SERIAL,TYPE,MOUNTPOINT"]))?;
    Ok(parse_lsblk(&out))
}

#[cfg(any(target_os = "linux", test))]
pub fn parse_lsblk(json: &str) -> Vec<Drive> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let system = |d: &serde_json::Value| {
        let mut stack = vec![d];
        while let Some(n) = stack.pop() {
            if let Some(m) = n["mountpoint"].as_str() {
                if m == "/" || m.starts_with("/boot") || m == "[SWAP]" {
                    return true;
                }
            }
            stack.extend(n["children"].as_array().into_iter().flatten());
        }
        false
    };
    let text = |d: &serde_json::Value, k: &str| d[k].as_str().unwrap_or("").trim().to_string();
    v["blockdevices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["type"] == "disk" && d["tran"] == "usb" && !system(d))
        .map(|d| Drive {
            number: stable_id(&text(d, "name")),
            name: Some(text(d, "model")).filter(|m| !m.is_empty()).unwrap_or_else(|| text(d, "name")),
            size: d["size"].as_u64().or_else(|| d["size"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0),
            serial: text(d, "serial"),
            letters: vec![],
            device: text(d, "path"),
        })
        .collect()
}

/// A stable small id from the device name, so the UI keeps its selection across refreshes.
#[cfg(any(unix, test))]
fn stable_id(name: &str) -> u32 {
    name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

/// External physical disks on a USB bus, from diskutil.
#[cfg(target_os = "macos")]
pub fn list() -> Result<Vec<Drive>, String> {
    let all: plist::Value = plist::from_bytes(run(Command::new("diskutil").args(["list", "-plist", "external", "physical"]))?.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut drives = vec![];
    for id in all.as_dictionary().and_then(|d| d.get("WholeDisks")).and_then(|w| w.as_array()).into_iter().flatten() {
        let Some(id) = id.as_string() else { continue };
        let info: plist::Value = match run(Command::new("diskutil").args(["info", "-plist", id])) {
            Ok(s) => plist::from_bytes(s.as_bytes()).map_err(|e| e.to_string())?,
            Err(_) => continue,
        };
        let Some(d) = info.as_dictionary() else { continue };
        let get = |k: &str| d.get(k).and_then(|v| v.as_string()).unwrap_or("").trim().to_string();
        if get("BusProtocol") != "USB" || d.get("Internal").and_then(|v| v.as_boolean()) == Some(true) {
            continue;
        }
        drives.push(Drive {
            number: stable_id(id),
            name: Some(get("MediaName")).filter(|m| !m.is_empty()).unwrap_or_else(|| id.to_string()),
            size: d.get("TotalSize").or_else(|| d.get("Size")).and_then(|v| v.as_unsigned_integer()).unwrap_or(0),
            serial: get("DiskUUID"),
            letters: vec![],
            // the raw node skips the buffer cache: several times faster than /dev/diskN
            device: format!("/dev/r{id}"),
        });
    }
    Ok(drives)
}

#[cfg(test)]
mod tests {
    #[test]
    fn lsblk_keeps_usb_sticks_and_skips_the_system_disk() {
        let json = r#"{"blockdevices":[
          {"name":"nvme0n1","path":"/dev/nvme0n1","size":512110190592,"tran":"nvme","model":"Samsung","serial":"S1","type":"disk","mountpoint":null,
           "children":[{"name":"nvme0n1p2","mountpoint":"/","type":"part"}]},
          {"name":"sdb","path":"/dev/sdb","size":"32017047552","tran":"usb","model":"SanDisk Ultra ","serial":"4C53","type":"disk","mountpoint":null,
           "children":[{"name":"sdb1","mountpoint":"/media/me/USB","type":"part"}]},
          {"name":"sdc","path":"/dev/sdc","size":64000000000,"tran":"usb","model":"Live","serial":"X","type":"disk","mountpoint":null,
           "children":[{"name":"sdc1","mountpoint":"/boot/efi","type":"part"}]}
        ]}"#;
        let drives = super::parse_lsblk(json);
        assert_eq!(drives.len(), 1, "system disk and a USB holding /boot are both skipped");
        assert_eq!((drives[0].name.as_str(), drives[0].size, drives[0].device.as_str()), ("SanDisk Ultra", 32017047552, "/dev/sdb"));
    }
}
