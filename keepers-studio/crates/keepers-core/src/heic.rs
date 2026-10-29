//! HEIC/HEIF decoding through the operating system.
//!
//! macOS ships a HEVC decoder and the `sips` tool, so HEIC photos (the iPhone
//! default, and most of an Apple Photos library) get full previews, scores and
//! develop there. Other platforms would need a bundled HEVC decoder, which has
//! patent-licensing implications, so they show a placeholder for now.

use anyhow::Result;
use std::path::Path;

pub const SUPPORTED: bool = cfg!(target_os = "macos");

/// Decode to an image whose long edge is at most `max` (0 = full size). The
/// result is unrotated; callers apply the EXIF orientation as for any file.
#[cfg(target_os = "macos")]
pub fn decode(path: &Path, max: u32) -> Result<image::DynamicImage> {
    use anyhow::{bail, Context};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let out = std::env::temp_dir().join(format!(
        "keepers-heic-{}-{}.tiff",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let mut cmd = std::process::Command::new("/usr/bin/sips");
    cmd.args(["-s", "format", "tiff"]);
    if max > 0 {
        cmd.args(["-Z", &max.to_string()]);
    }
    let status = cmd.arg(path).arg("--out").arg(&out).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().context("running sips")?;
    let res = if status.success() {
        image::open(&out).context("reading sips output")
    } else {
        bail!("macOS could not decode this HEIC file")
    };
    let _ = std::fs::remove_file(&out);
    res
}

#[cfg(not(target_os = "macos"))]
pub fn decode(_path: &Path, _max: u32) -> Result<image::DynamicImage> {
    anyhow::bail!("HEIC previews are available on macOS only in this version")
}
