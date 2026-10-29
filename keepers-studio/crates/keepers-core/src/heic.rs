//! HEIC/HEIF (and AVIF) decoding through the operating system's codecs.
//!
//! iPhones shoot HEIC, and Google Photos and Apple Photos hand those files
//! back unchanged. HEVC is patent-encumbered, so rather than bundling a
//! decoder this uses the codecs the OS already licenses:
//! - macOS: ImageIO via the built-in `sips` tool.
//! - Windows: Windows Imaging Component (WIC), with Microsoft's free
//!   "HEIF Image Extensions" + "HEVC Video Extensions" (AVIF: "AV1 Video
//!   Extension"), which many PCs ship with.
//!
//! HEIF stores rotation as container properties (`irot`/`imir`) rather than
//! only in EXIF; [`decode`] returns the image correctly oriented whichever
//! way the OS decoder behaves, so callers must not apply EXIF orientation.

use anyhow::Result;
use std::path::Path;

pub const SUPPORTED: bool = cfg!(any(target_os = "macos", windows));

/// Decode to an upright image whose long edge is at most `max` (0 = full size).
pub fn decode(path: &Path, max: u32) -> Result<image::DynamicImage> {
    let img = os_decode(path, max)?;
    let info = crate::util::map_file(path).ok().and_then(|b| heif_info(&b));
    Ok(match info {
        Some(i) => upright(img, &i),
        None => img,
    })
}

/// Geometry of a HEIF file's primary image.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HeifInfo {
    /// Stored (unrotated) size.
    pub width: u32,
    pub height: u32,
    /// Anticlockwise quarter turns to display it upright.
    pub rotation: u8,
    /// Mirror axis: 0 = about the vertical axis (left-right), 1 = horizontal.
    pub mirror: Option<u8>,
}

/// Apply the HEIF transform unless the OS decoder already did. A quarter-turn
/// swaps the aspect, so it's detectable; 180° and mirrors are assumed applied,
/// as every conforming decoder does.
fn upright(img: image::DynamicImage, i: &HeifInfo) -> image::DynamicImage {
    let stored_landscape = i.width >= i.height;
    let got_landscape = img.width() >= img.height();
    if i.rotation % 2 == 0 || i.width == i.height || stored_landscape != got_landscape {
        return img;
    }
    let img = match i.rotation {
        1 => img.rotate270(),
        3 => img.rotate90(),
        _ => img,
    };
    match i.mirror {
        Some(0) => img.fliph(),
        Some(1) => img.flipv(),
        _ => img,
    }
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_be_bytes(s.try_into().unwrap()))
}

fn be16(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_be_bytes(s.try_into().unwrap()))
}

/// Child boxes of `b[start..end]` as (type, payload start, end).
fn boxes(b: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut v = Vec::new();
    let mut p = start;
    while p + 8 <= end {
        let Some(size) = be32(b, p) else { break };
        let typ: [u8; 4] = b[p + 4..p + 8].try_into().unwrap();
        let (hdr, size) = match size {
            0 => (8, end - p),
            1 => match b.get(p + 8..p + 16) {
                Some(s) => (16, u64::from_be_bytes(s.try_into().unwrap()) as usize),
                None => break,
            },
            s => (8, s as usize),
        };
        if size < hdr || p + size > end {
            break;
        }
        v.push((typ, p + hdr, p + size));
        p += size;
    }
    v
}

/// Read the primary item's size and rotation/mirror properties.
pub fn heif_info(b: &[u8]) -> Option<HeifInfo> {
    if b.get(4..8)? != b"ftyp" {
        return None;
    }
    let meta = boxes(b, 0, b.len()).into_iter().find(|x| &x.0 == b"meta")?;
    // meta is a FullBox: skip version/flags.
    let kids = boxes(b, meta.1 + 4, meta.2);
    let pitm = kids.iter().find(|x| &x.0 == b"pitm")?;
    let primary = if b[pitm.1] == 0 { be16(b, pitm.1 + 4)? as u32 } else { be32(b, pitm.1 + 4)? };
    let iprp = kids.iter().find(|x| &x.0 == b"iprp")?;
    let iprp_kids = boxes(b, iprp.1, iprp.2);
    let ipco = iprp_kids.iter().find(|x| &x.0 == b"ipco")?;
    let props = boxes(b, ipco.1, ipco.2);
    let mut info = HeifInfo::default();
    let mut found_ispe = false;
    for ipma in iprp_kids.iter().filter(|x| &x.0 == b"ipma") {
        let version = b[ipma.1];
        let flags = be32(b, ipma.1)? & 0xFF_FFFF;
        let count = be32(b, ipma.1 + 4)?;
        let mut p = ipma.1 + 8;
        for _ in 0..count {
            let item = if version < 1 {
                let v = be16(b, p)? as u32;
                p += 2;
                v
            } else {
                let v = be32(b, p)?;
                p += 4;
                v
            };
            let n = *b.get(p)? as usize;
            p += 1;
            for _ in 0..n {
                let idx = if flags & 1 != 0 {
                    let v = be16(b, p)? & 0x7FFF;
                    p += 2;
                    v as usize
                } else {
                    let v = b.get(p)? & 0x7F;
                    p += 1;
                    v as usize
                };
                if item != primary || idx == 0 {
                    continue;
                }
                let Some(&(typ, s, _)) = props.get(idx - 1) else { continue };
                match &typ {
                    b"ispe" => {
                        info.width = be32(b, s + 4)?;
                        info.height = be32(b, s + 8)?;
                        found_ispe = true;
                    }
                    b"irot" => info.rotation = b.get(s)? & 3,
                    b"imir" => info.mirror = Some(b.get(s)? & 1),
                    _ => {}
                }
            }
        }
    }
    found_ispe.then_some(info)
}

#[cfg(target_os = "macos")]
fn os_decode(path: &Path, max: u32) -> Result<image::DynamicImage> {
    use anyhow::{bail, Context};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let out = std::env::temp_dir().join(format!("keepers-heic-{}-{}.tiff", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let mut cmd = std::process::Command::new("/usr/bin/sips");
    cmd.args(["-s", "format", "tiff"]);
    if max > 0 {
        cmd.args(["-Z", &max.to_string()]);
    }
    let status = cmd
        .arg(path)
        .arg("--out")
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("running sips")?;
    let res = if status.success() { image::open(&out).context("reading sips output") } else { bail!("macOS could not decode this file") };
    let _ = std::fs::remove_file(&out);
    res
}

#[cfg(windows)]
fn os_decode(path: &Path, max: u32) -> Result<image::DynamicImage> {
    use anyhow::anyhow;
    use windows::core::{Interface, HSTRING};
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;

    /// WIC: no decoder installed for this format.
    const COMPONENT_NOT_FOUND: i32 = 0x8898_2F50u32 as i32;
    let missing = |e: &windows::core::Error| e.code().0 == COMPONENT_NOT_FOUND;
    let codec_help = |path: &Path| {
        let avif = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("avif"));
        if avif {
            anyhow!("Windows needs the free \"AV1 Video Extension\" from the Microsoft Store to open AVIF photos")
        } else {
            anyhow!("Windows needs the free \"HEIF Image Extensions\" and \"HEVC Video Extensions\" from the Microsoft Store to open HEIC photos")
        }
    };

    // SAFETY: plain COM calls on interfaces owned by this function; COM is
    // initialised for this (worker) thread and stays initialised.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let decoder = factory
            .CreateDecoderFromFilename(&HSTRING::from(path.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .map_err(|e| if missing(&e) { codec_help(path) } else { anyhow!("Windows could not open this file: {e}") })?;
        let frame = decoder.GetFrame(0).map_err(|e| if missing(&e) { codec_help(path) } else { e.into() })?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h)?;
        let mut source: IWICBitmapSource = frame.cast()?;
        let (mut ow, mut oh) = (w, h);
        if max > 0 && w.max(h) > max {
            let s = max as f32 / w.max(h) as f32;
            ow = ((w as f32 * s).round() as u32).max(1);
            oh = ((h as f32 * s).round() as u32).max(1);
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(&source, ow, oh, WICBitmapInterpolationModeFant)?;
            source = scaler.cast()?;
        }
        let conv = factory.CreateFormatConverter()?;
        conv.Initialize(&source, &GUID_WICPixelFormat24bppRGB, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let stride = ow * 3;
        let mut buf = vec![0u8; (stride * oh) as usize];
        conv.CopyPixels(std::ptr::null(), stride, &mut buf)?;
        let img = image::RgbImage::from_raw(ow, oh, buf).ok_or_else(|| anyhow!("unexpected decoder output size"))?;
        Ok(image::DynamicImage::ImageRgb8(img))
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn os_decode(_path: &Path, _max: u32) -> Result<image::DynamicImage> {
    anyhow::bail!("HEIC previews are available on Windows and macOS only in this version")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(payload);
        v
    }

    /// A minimal HEIF header: primary item 1 with ispe 4032x3024 and irot.
    fn heif(rot: u8) -> Vec<u8> {
        let mut ispe = vec![0, 0, 0, 0];
        ispe.extend(4032u32.to_be_bytes());
        ispe.extend(3024u32.to_be_bytes());
        let ipco = [bx(b"ispe", &ispe), bx(b"irot", &[rot])].concat();
        // ipma v0, flags 0: 1 entry, item 1, 2 associations (props 1 and 2).
        let ipma = [0u8, 0, 0, 0, 0, 0, 0, 1, 0, 1, 2, 0x81, 0x02];
        let iprp = bx(b"iprp", &[bx(b"ipco", &ipco), bx(b"ipma", &ipma)].concat());
        let pitm = bx(b"pitm", &[0, 0, 0, 0, 0, 1]);
        let meta = bx(b"meta", &[vec![0, 0, 0, 0], pitm, iprp].concat());
        [bx(b"ftyp", b"heic\0\0\0\0mif1heic"), meta].concat()
    }

    #[test]
    fn reads_heif_rotation() {
        let i = heif_info(&heif(3)).unwrap();
        assert_eq!((i.width, i.height, i.rotation, i.mirror), (4032, 3024, 3, None));
        assert!(heif_info(b"not a heif file at all").is_none());
    }

    #[test]
    fn rotates_only_when_decoder_did_not() {
        let info = HeifInfo { width: 40, height: 30, rotation: 3, mirror: None };
        // Decoder returned the stored landscape pixels: rotate to portrait.
        let raw = image::DynamicImage::new_rgb8(40, 30);
        assert_eq!((upright(raw, &info).width(), 30), (30, 30));
        // Decoder already rotated: leave alone.
        let done = image::DynamicImage::new_rgb8(30, 40);
        let out = upright(done, &info);
        assert_eq!((out.width(), out.height()), (30, 40));
    }
}
