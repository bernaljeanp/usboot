mod drives;
mod flash;
mod image;
mod library;
mod windows;

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

#[tauri::command(async)]
fn flash(app: AppHandle, path: String, drive: drives::Drive) -> Result<(), String> {
    CANCEL.store(false, Ordering::SeqCst);
    flash::run(&path, &drive, &CANCEL, |p| {
        let _ = app.emit("progress", p);
    })
}

/// Windows install ISOs: GPT + FAT32 + copied files instead of a raw copy.
#[tauri::command(async)]
fn flash_windows(app: AppHandle, path: String, drive: drives::Drive, options: windows::Options) -> Result<(), String> {
    CANCEL.store(false, Ordering::SeqCst);
    windows::run(&path, &drive, &options, &CANCEL, |p| {
        let _ = app.emit("progress", p);
    })
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
    std::fs::File::open(r"\\.\PhysicalDrive0").is_ok()
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

