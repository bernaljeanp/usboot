//! What's inside an image, read from its first 34 KB.
use serde::Serialize;
use std::io::Read;
use std::path::Path;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    /// Has an MBR boot signature, so a raw copy boots (Linux "hybrid" ISOs, .img files).
    pub bootable: bool,
    /// A Windows install ISO: not hybrid, a raw copy will not boot.
    pub windows: bool,
    /// .xz / .gz / .zst: unpacked before flashing; `size` is the compressed size.
    pub compressed: bool,
}

/// Fills `buf` unless the reader ends first; returns how much was read.
pub fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

pub fn inspect(path: &str) -> Result<ImageInfo, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    // Compressed or not, look at the first decompressed bytes.
    let (mut f, _) = crate::unpack::open(path)?;
    let mut head = vec![0u8; 0x8800];
    let n = read_full(&mut f, &mut head).map_err(|e| format!("unpackFailed|{e}"))?;
    head.truncate(n);
    Ok(ImageInfo {
        path: path.to_string(),
        name: Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        size,
        bootable: head.get(510..512) == Some(&[0x55, 0xAA]),
        windows: is_windows_iso(&head),
        compressed: crate::unpack::is_compressed(path),
    })
}

/// ISO 9660 primary volume descriptor at 0x8000; Windows labels look like CCCOMA_X64FRE_EN-US_DV9.
fn is_windows_iso(head: &[u8]) -> bool {
    head.get(0x8001..0x8006) == Some(b"CD001")
        && head.get(0x8028..0x8048).is_some_and(|id| {
            let id = String::from_utf8_lossy(id);
            ["X64FRE", "X86FRE", "A64FRE"].iter().any(|tag| id.contains(tag))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_boot_signature_and_windows_label() {
        let mut img = vec![0u8; 0x8800];
        img[510] = 0x55;
        img[511] = 0xAA;
        img[0x8001..0x8006].copy_from_slice(b"CD001");
        img[0x8028..0x8028 + 23].copy_from_slice(b"CCCOMA_X64FRE_EN-US_DV9");
        let path = std::env::temp_dir().join("usboot-inspect-test.iso");
        std::fs::write(&path, &img).unwrap();
        let info = inspect(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(info.bootable && info.windows);
        assert_eq!(info.size, 0x8800);

        let tiny = std::env::temp_dir().join("usboot-inspect-tiny.img");
        std::fs::write(&tiny, b"hello").unwrap();
        let info = inspect(tiny.to_str().unwrap()).unwrap();
        std::fs::remove_file(&tiny).ok();
        assert!(!info.bootable && !info.windows);
    }
}
