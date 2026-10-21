//! Compressed images (.img.xz, .iso.gz, .img.zst — how Raspberry Pi OS, SteamOS and others ship):
//! unpacked once to a temp file, then flashed and verified like any other image.
use crate::flash::{CANCELLED, Progress};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn kind(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "xz" => Some("xz"),
        "gz" => Some("gz"),
        "zst" => Some("zst"),
        _ => None,
    }
}

pub fn is_compressed(path: &str) -> bool {
    kind(Path::new(path)).is_some()
}

/// Counts compressed bytes as the decoder pulls them, so progress follows the file on disk.
struct Counting<R> {
    inner: R,
    read: Arc<AtomicU64>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// A reader over the decompressed bytes, plus a counter of compressed bytes consumed.
pub fn open(path: &str) -> Result<(Box<dyn Read>, Arc<AtomicU64>), String> {
    let read = Arc::new(AtomicU64::new(0));
    let file = Counting { inner: File::open(path).map_err(|e| e.to_string())?, read: read.clone() };
    let input = BufReader::with_capacity(1 << 20, file);
    let decoder: Box<dyn Read> = match kind(Path::new(path)) {
        Some("xz") => Box::new(xz2::read::XzDecoder::new_multi_decoder(input)),
        Some("gz") => Box::new(flate2::read::MultiGzDecoder::new(input)),
        Some("zst") => Box::new(zstd::stream::read::Decoder::with_buffer(input).map_err(|e| e.to_string())?),
        _ => Box::new(input),
    };
    Ok((decoder, read))
}

/// "raspios.img.xz" -> "raspios.img"; a bare "image.xz" becomes "image.img".
fn unpacked_name(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if Path::new(&stem).extension().is_some() { stem } else { format!("{stem}.img") }
}

/// Unpacks `path` (if compressed) and returns the path to flash. Re-uses an earlier unpack of the same file.
pub fn unpack(path: &str, cancel: &AtomicBool, mut emit: impl FnMut(Progress)) -> Result<String, String> {
    let src = Path::new(path);
    if kind(src).is_none() {
        return Ok(path.to_string());
    }
    let meta = fs::metadata(src).map_err(|e| e.to_string())?;
    let stamp = format!("{path}|{}|{:?}", meta.len(), meta.modified().ok());
    let dir = std::env::temp_dir().join("usboot-unpacked");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(unpacked_name(src));
    let marker = PathBuf::from(format!("{}.src", out.display()));
    if out.exists() && fs::read_to_string(&marker).ok().as_deref() == Some(stamp.as_str()) {
        return Ok(out.to_string_lossy().into_owned());
    }

    let total = meta.len();
    let (mut reader, read) = open(path)?;
    let part = PathBuf::from(format!("{}.part", out.display()));
    let result = (|| -> Result<(), String> {
        let mut to = std::io::BufWriter::with_capacity(4 << 20, File::create(&part).map_err(|e| e.to_string())?);
        let mut buf = vec![0u8; 4 << 20];
        let mut last = Instant::now();
        emit(Progress { phase: "unpack", done: 0, total });
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(CANCELLED.into());
            }
            let n = reader.read(&mut buf).map_err(|e| format!("unpackFailed|{e}"))?;
            if n == 0 {
                break;
            }
            to.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            if last.elapsed() > Duration::from_millis(150) {
                emit(Progress { phase: "unpack", done: read.load(Ordering::Relaxed).min(total), total });
                last = Instant::now();
            }
        }
        to.flush().map_err(|e| e.to_string())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    fs::rename(&part, &out).map_err(|e| e.to_string())?;
    let _ = fs::write(&marker, stamp);
    emit(Progress { phase: "unpack", done: total, total });
    Ok(out.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_xz_gz_and_zst_identically() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8 ^ (i >> 13) as u8).collect();
        let dir = std::env::temp_dir().join("usboot-unpack-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut xz = xz2::write::XzEncoder::new(Vec::new(), 3);
        xz.write_all(&data).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&data).unwrap();
        let files = [
            ("a.img.xz", xz.finish().unwrap()),
            ("b.img.gz", gz.finish().unwrap()),
            ("c.img.zst", zstd::encode_all(&data[..], 3).unwrap()),
        ];
        for (name, bytes) in files {
            let src = dir.join(name);
            fs::write(&src, &bytes).unwrap();
            let mut last = 0;
            let out = unpack(src.to_str().unwrap(), &AtomicBool::new(false), |p| last = p.done).unwrap();
            assert_eq!(fs::read(&out).unwrap(), data, "{name}");
            assert!(out.ends_with(&name[..5]), "{out}");
            assert_eq!(last, bytes.len() as u64);
            // second time: re-used, no progress
            let mut calls = 0;
            unpack(src.to_str().unwrap(), &AtomicBool::new(false), |_| calls += 1).unwrap();
            assert_eq!(calls, 0);
            let _ = fs::remove_file(&out);
            let _ = fs::remove_file(format!("{out}.src"));
        }
        let _ = fs::remove_dir_all(&dir);
        assert!(!is_compressed("x.iso") && is_compressed("x.img.XZ"));
    }
}
