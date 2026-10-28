use anyhow::{anyhow, Result};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

/// Run decoder code that may panic on malformed files and turn the panic into
/// an error, so one corrupt photo is flagged instead of taking the app down.
pub fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "decoder panic".into());
            Err(anyhow!("file could not be decoded: {msg}"))
        }
    }
}

/// Memory-map a file read-only. Falls back to reading it when mapping fails
/// (some network filesystems refuse mmap).
pub enum Bytes {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Mapped(m) => m,
            Bytes::Owned(v) => v,
        }
    }
}

pub fn map_file(path: &Path) -> Result<Bytes> {
    let file = std::fs::File::open(path)?;
    // SAFETY: the mapping is read-only. If another process truncates the file
    // while we read it we may fault; originals are not expected to change
    // under us and every decoder runs inside `guard`.
    match unsafe { memmap2::Mmap::map(&file) } {
        Ok(m) => Ok(Bytes::Mapped(m)),
        Err(_) => Ok(Bytes::Owned(std::fs::read(path)?)),
    }
}

/// Apply an EXIF orientation (1..=8) to an image.
pub fn orient(img: image::DynamicImage, orientation: u16) -> image::DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
