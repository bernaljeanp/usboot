//! Removable USB disks that are safe to offer as targets.
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    pub number: u32,
    pub name: String,
    pub size: u64,
    pub serial: String,
    pub letters: Vec<String>,
}

// USB bus only, never the disk Windows booted from or lives on.
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
    }
  })"#;

pub fn powershell(script: &str) -> Result<String, String> {
    let script = format!("[Console]::OutputEncoding = [Text.Encoding]::UTF8\n{script}");
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn list() -> Result<Vec<Drive>, String> {
    serde_json::from_str(&powershell(LIST)?).map_err(|e| e.to_string())
}
