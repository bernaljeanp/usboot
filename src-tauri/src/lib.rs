mod drives;
mod flash;
mod image;
mod library;
mod unpack;
#[cfg(windows)]
mod windows;

/// Windows mode needs diskpart, DISM and Mount-DiskImage: Windows only for now.
#[cfg(not(windows))]
mod windows {
    #[derive(serde::Deserialize, Default, Debug)]
    pub struct Options {}
    pub fn run(
        _: &str,
        _: &crate::drives::Drive,
        _: &Options,
        _: &std::sync::atomic::AtomicBool,
        _: impl FnMut(crate::flash::Progress),
    ) -> Result<(), String> {
        Err("windowsModeOnly".into())
    }
}

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};

static CANCEL: AtomicBool = AtomicBool::new(false);

#[tauri::command(async)]
fn list_drives() -> Result<Vec<drives::Drive>, String> {
    drives::list()
}

#[tauri::command(async)]
fn inspect_image(path: String) -> Result<image::ImageInfo, String> {
    image::inspect(&path)
}

/// Progress of one drive; several drives can be flashing at once.
fn emit_progress(app: &AppHandle, drive: u32, p: flash::Progress) {
    let _ = app.emit("progress", serde_json::json!({ "drive": drive, "phase": p.phase, "done": p.done, "total": p.total }));
}

/// The same image onto every drive in parallel. One result per drive (None = written and verified),
/// so one bad stick doesn't hide that the others are fine.
#[tauri::command(async)]
fn flash(app: AppHandle, path: String, drives: Vec<drives::Drive>) -> Vec<Option<String>> {
    CANCEL.store(false, Ordering::SeqCst);
    // A compressed image is unpacked once, then every drive gets the same plain image.
    let path = match unpack::unpack(&path, &CANCEL, |p| drives.iter().for_each(|d| emit_progress(&app, d.number, p.clone()))) {
        Ok(p) => p,
        Err(e) => return drives.iter().map(|_| Some(e.clone())).collect(),
    };
    std::thread::scope(|s| {
        let jobs: Vec<_> = drives
            .iter()
            .map(|d| {
                let (app, path) = (&app, &path);
                s.spawn(move || flash::run(path, d, &CANCEL, |p| emit_progress(app, d.number, p)))
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap_or_else(|_| Err("crashed".into())).err()).collect()
    })
}

/// Windows install ISOs: GPT + FAT32 + copied files instead of a raw copy.
#[tauri::command(async)]
fn flash_windows(app: AppHandle, path: String, drive: drives::Drive, options: windows::Options) -> Result<(), String> {
    CANCEL.store(false, Ordering::SeqCst);
    windows::run(&path, &drive, &options, &CANCEL, |p| emit_progress(&app, drive.number, p))
}

#[tauri::command(async)]
fn restore_drive(drive: drives::Drive, fs: String, label: String) -> Result<(), String> {
    flash::restore(&drive, &fs, &label)
}

#[tauri::command(async)]
fn library() -> Vec<library::Entry> {
    library::list()
}

#[tauri::command(async)]
fn download_iso(app: AppHandle, id: String) -> Result<String, String> {
    library::download(&id, |p| {
        let _ = app.emit("download", p);
    })
}

#[tauri::command]
fn cancel_download(id: String) {
    library::cancel(&id);
}

#[tauri::command]
fn cancel_flash() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// Windows' display language (WebView2 can report its own instead).
#[tauri::command]
fn os_locale() -> String {
    sys_locale::get_locale().unwrap_or_else(|| "en".into())
}

/// Opening a physical disk at all needs admin, so this is the exact check that matters.
#[tauri::command]
fn is_admin() -> bool {
    #[cfg(windows)]
    return std::fs::File::open(r"\\.\PhysicalDrive0").is_ok();
    // Linux/macOS ask for the password per drive, through the system dialog.
    #[cfg(not(windows))]
    true
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![list_drives, inspect_image, flash, flash_windows, restore_drive, cancel_flash, is_admin, os_locale, library, download_iso, cancel_download])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

