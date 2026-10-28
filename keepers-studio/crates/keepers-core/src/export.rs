//! Export: full-resolution develop, resize, output sharpening, encode to a new
//! file. Never overwrites anything.

use crate::catalog::Photo;
use crate::develop::{pipeline, source, Edits, RenderOpts};
use anyhow::{Context, Result};
use image::{imageops::FilterType, DynamicImage, ImageBuffer};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Jpeg,
    Png,
    Tiff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputSharpen {
    None,
    Screen,
    Print,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSettings {
    pub format: Format,
    /// JPEG quality 1..100.
    pub quality: u8,
    /// Resize so the long edge is at most this; `None` = full size.
    pub long_edge: Option<u32>,
    /// 16 bits per channel (TIFF/PNG only).
    pub sixteen_bit: bool,
    pub sharpen: OutputSharpen,
    pub folder: String,
    /// File name template, see [`file_name`].
    pub naming: String,
}

pub fn presets() -> Vec<(&'static str, ExportSettings)> {
    let base = |format, quality, long_edge, sixteen_bit, sharpen| ExportSettings {
        format,
        quality,
        long_edge,
        sixteen_bit,
        sharpen,
        folder: String::new(),
        naming: "{name}".into(),
    };
    vec![
        ("Web", base(Format::Jpeg, 85, Some(2048), false, OutputSharpen::Screen)),
        ("Social", base(Format::Jpeg, 90, Some(1080), false, OutputSharpen::Screen)),
        ("Print", base(Format::Tiff, 100, None, true, OutputSharpen::Print)),
        ("Full-size JPEG", base(Format::Jpeg, 95, None, false, OutputSharpen::None)),
    ]
}

/// Expand a naming template. Tokens: `{name}` original file stem, `{seq}`
/// 4-digit sequence, `{date}` capture date YYYY-MM-DD, `{time}` HHMMSS,
/// `{camera}`, `{rating}`.
pub fn file_name(template: &str, photo: &Photo, seq: usize) -> String {
    let stem = Path::new(&photo.file_name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dt = photo.capture_time.and_then(chrono::DateTime::from_timestamp_millis);
    let out = template
        .replace("{name}", &stem)
        .replace("{seq}", &format!("{seq:04}"))
        .replace("{date}", &dt.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_else(|| "undated".into()))
        .replace("{time}", &dt.map(|d| d.format("%H%M%S").to_string()).unwrap_or_default())
        .replace("{camera}", photo.camera.as_deref().unwrap_or("camera"))
        .replace("{rating}", &photo.rating.to_string());
    sanitize(&out)
}

/// Make a string safe as a file name on Windows, macOS and Linux.
pub fn sanitize(s: &str) -> String {
    let cleaned: String =
        s.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_string();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "LPT1", "LPT2"];
    if cleaned.is_empty() || reserved.contains(&cleaned.to_uppercase().as_str()) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

/// First path of the form `dir/base.ext`, `dir/base-1.ext`, ... that does not exist.
pub fn unique_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{base}.{ext}"));
    let mut n = 1;
    while p.exists() {
        p = dir.join(format!("{base}-{n}.{ext}"));
        n += 1;
    }
    p
}

pub fn export_one(photo: &Photo, edits: &Edits, s: &ExportSettings, seq: usize) -> Result<PathBuf> {
    let path = Path::new(&photo.path);
    let lin = source::load(path, photo.kind, photo.orientation, None)?;
    let rendered = pipeline::render(&lin, edits, &RenderOpts { scale: 1.0, ..Default::default() });
    drop(lin);

    let mut img = DynamicImage::ImageRgb32F(
        ImageBuffer::from_raw(rendered.w as u32, rendered.h as u32, rendered.px.into_iter().flatten().collect())
            .context("render size mismatch")?,
    );
    if let Some(le) = s.long_edge {
        if img.width().max(img.height()) > le {
            img = img.resize(le, le, FilterType::Lanczos3);
        }
    }
    let (amount, radius) = match s.sharpen {
        OutputSharpen::None => (0.0, 0.0),
        OutputSharpen::Screen => (0.35, 0.6),
        OutputSharpen::Print => (0.6, 1.0),
    };
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut px: Vec<[f32; 3]> = img.into_rgb32f().into_raw().chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    if amount > 0.0 {
        let e = Edits { sharpen: amount * 100.0, sharpen_radius: radius, ..Default::default() };
        let tmp = source::Linear { w, h, px, is_raw: false };
        // Output sharpening operates on already display-encoded values; route
        // them through the identity tone path by marking the input linear.
        let enc = source::Linear { px: tmp.px.iter().map(|p| p.map(source::srgb_to_linear)).collect(), ..tmp };
        px = pipeline::render(&enc, &e, &RenderOpts { scale: 1.0, ..Default::default() }).px;
    }
    let out = pipeline::Rendered { w, h, px };

    let dir = Path::new(&s.folder);
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let ext = match s.format {
        Format::Jpeg => "jpg",
        Format::Png => "png",
        Format::Tiff => "tif",
    };
    let target = unique_path(dir, &file_name(&s.naming, photo, seq), ext);
    let tmp = target.with_extension(format!("{ext}.part"));
    {
        let file = std::fs::File::create(&tmp)?;
        let mut wr = std::io::BufWriter::new(file);
        match s.format {
            Format::Jpeg => {
                let rgb = pipeline::to_rgb8(&out);
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut wr, s.quality.clamp(1, 100))
                    .encode_image(&rgb)?;
            }
            Format::Png => {
                let img = if s.sixteen_bit { pipeline::to_rgb16(&out) } else { pipeline::to_rgb8(&out) };
                img.write_to(&mut wr, image::ImageFormat::Png)?;
            }
            Format::Tiff => {
                let img = if s.sixteen_bit { pipeline::to_rgb16(&out) } else { pipeline::to_rgb8(&out) };
                img.write_to(&mut wr, image::ImageFormat::Tiff)?;
            }
        }
        use std::io::Write;
        wr.flush()?;
    }
    std::fs::rename(&tmp, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::Kind;

    fn photo(path: &Path) -> Photo {
        Photo {
            id: 1,
            folder_id: 1,
            path: path.to_string_lossy().into(),
            file_name: path.file_name().unwrap().to_string_lossy().into(),
            kind: Kind::Jpeg,
            pair_path: None,
            processed: true,
            error: None,
            capture_time: Some(1714564800000),
            camera: Some("LEICA Q2".into()),
            lens: None,
            iso: None,
            shutter: None,
            aperture: None,
            focal: None,
            orientation: 1,
            width: None,
            height: None,
            sharpness: None,
            exposure: None,
            clip_hi: None,
            clip_lo: None,
            burst_id: None,
            burst_best: false,
            dup_of: None,
            pick: 1,
            rating: 3,
            edited: false,
        }
    }

    #[test]
    fn names() {
        let p = photo(Path::new("/x/L100.DNG"));
        assert_eq!(file_name("{date}_{name}_{seq}", &p, 7), "2024-05-01_L100_0007");
        assert_eq!(file_name("{camera}/{name}", &p, 1), "LEICA Q2_L100");
        assert_eq!(sanitize("con"), "_con");
        assert_eq!(sanitize("a:b?."), "a_b_");
    }

    #[test]
    fn exports_at_size_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("IMG_1.jpg");
        let img = image::RgbImage::from_fn(400, 300, |x, y| image::Rgb([x as u8, y as u8, 90]));
        img.save(&src).unwrap();
        let out_dir = dir.path().join("out");
        let mut s = presets()[0].1.clone();
        s.folder = out_dir.to_string_lossy().into();
        s.long_edge = Some(200);
        let p = photo(&src);
        let a = export_one(&p, &Edits::default(), &s, 1).unwrap();
        let b = export_one(&p, &Edits::default(), &s, 1).unwrap();
        assert_ne!(a, b);
        assert!(b.file_name().unwrap().to_string_lossy().ends_with("IMG_1-1.jpg"));
        let back = image::open(&a).unwrap();
        assert_eq!((back.width(), back.height()), (200, 150));

        s.format = Format::Tiff;
        s.long_edge = None;
        s.sharpen = OutputSharpen::None;
        let t = export_one(&p, &Edits::default(), &s, 1).unwrap();
        let back = image::open(&t).unwrap();
        assert_eq!((back.width(), back.height()), (400, 300));
        // Identity develop of a JPEG is pixel-faithful (16-bit TIFF, within JPEG decode rounding).
        let orig = image::open(&src).unwrap().to_rgb8();
        let got = back.to_rgb8();
        let max = orig.as_raw().iter().zip(got.as_raw()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(max <= 1, "max diff {max}");
        assert!(std::fs::read_dir(&out_dir).unwrap().all(|e| !e.unwrap().path().to_string_lossy().ends_with(".part")));
    }
}
