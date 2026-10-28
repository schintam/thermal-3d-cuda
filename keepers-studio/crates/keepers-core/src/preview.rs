//! Turning any supported file into a displayable image, as cheaply as the
//! requested size allows.
//!
//! RAWs use their embedded camera JPEG (decoded with DCT scaling for small
//! sizes), falling back to the paired JPEG of a RAW+JPEG stack, then the RAW
//! core's own preview, and only as a last resort a real demosaic.

use crate::embedded::{self, JpegRef};
use crate::formats::Kind;
use crate::util::{self, guard};
use anyhow::{anyhow, bail, Context, Result};
use image::{DynamicImage, ImageBuffer};
use std::path::Path;

pub struct Source<'a> {
    pub path: &'a Path,
    pub kind: Kind,
    pub orientation: u16,
    /// The JPEG half of a RAW+JPEG pair, used when the RAW has no usable preview.
    pub pair: Option<&'a Path>,
}

pub struct Loaded {
    /// Oriented image, long edge >= the requested size where the source allows.
    pub image: DynamicImage,
    /// Oriented size of the largest preview available (not the sensor size).
    pub full_w: u32,
    pub full_h: u32,
}

/// Decode `src` so that its long edge is at least `want` pixels (0 = full size).
pub fn load(src: &Source, want: u32) -> Result<Loaded> {
    guard(|| load_inner(src, want)).with_context(|| src.path.display().to_string())
}

fn load_inner(src: &Source, want: u32) -> Result<Loaded> {
    match src.kind {
        Kind::Jpeg => {
            let buf = util::map_file(src.path)?;
            let (img, w, h) = decode_jpeg(&buf, want)?;
            Ok(oriented(img, w, h, src.orientation))
        }
        Kind::Tiff | Kind::Png => {
            let img = image::ImageReader::open(src.path)?.with_guessed_format()?.decode()?;
            let (w, h) = (img.width(), img.height());
            let img = if want > 0 && img.width().max(img.height()) > want { img.thumbnail(want, want) } else { img };
            Ok(oriented(img, w, h, src.orientation))
        }
        Kind::Heic => bail!("HEIC previews are not supported on this platform yet"),
        Kind::Raw => load_raw(src, want),
    }
}

fn oriented(img: DynamicImage, w: u32, h: u32, orientation: u16) -> Loaded {
    let (full_w, full_h) = if (5..=8).contains(&orientation) { (h, w) } else { (w, h) };
    Loaded { image: util::orient(img, orientation), full_w, full_h }
}

/// Pick the embedded JPEG to decode for a target size: the smallest one that is
/// big enough, else the largest.
pub fn choose(jpegs: &[JpegRef], want: u32) -> Option<JpegRef> {
    if want == 0 {
        return jpegs.first().copied();
    }
    jpegs.iter().rev().find(|j| j.long_edge() >= want).or(jpegs.first()).copied()
}

/// Below this a RAW's embedded preview is only a thumbnail, not good enough
/// for the loupe.
const MIN_USEFUL_PREVIEW: u32 = 1000;

fn load_raw(src: &Source, want: u32) -> Result<Loaded> {
    let buf = util::map_file(src.path)?;
    let jpegs = embedded::find_jpegs(&buf);
    let largest = jpegs.first().map(|j| j.long_edge()).unwrap_or(0);
    let need = want.clamp(1, MIN_USEFUL_PREVIEW);
    if largest >= need {
        let j = choose(&jpegs, want).unwrap();
        if let Ok((img, _, _)) = decode_jpeg(j.bytes(&buf), want) {
            let big = jpegs[0];
            return Ok(oriented(img, big.width, big.height, src.orientation));
        }
    }
    if let Some(pair) = src.pair {
        // The camera JPEG carries its own orientation tag.
        let pbuf = util::map_file(pair)?;
        let o = crate::meta::read(pair, Kind::Jpeg, &pbuf).orientation;
        let (img, w, h) = decode_jpeg(&pbuf, want)?;
        return Ok(oriented(img, w, h, o));
    }
    // The RAW core's own preview extraction understands more containers
    // (maker-note previews, uncompressed RGB previews).
    let rawsrc = rawler::rawsource::RawSource::new(src.path)?;
    let dec = rawler::get_decoder(&rawsrc)?;
    let params = rawler::decoders::RawDecodeParams::default();
    for img in [dec.full_image(&rawsrc, &params), dec.preview_image(&rawsrc, &params)].into_iter().flatten().flatten() {
        if img.width().max(img.height()) >= need {
            let (w, h) = (img.width(), img.height());
            let img = if want > 0 { img.thumbnail(want, want) } else { img };
            return Ok(oriented(img, w, h, src.orientation));
        }
    }
    // No usable preview anywhere: develop the sensor data.
    let dev = crate::develop::source::decode_raw_preview(src.path, if want == 0 { 4096 } else { want.max(1024) })?;
    let (w, h) = (dev.width(), dev.height());
    Ok(Loaded { image: dev, full_w: w, full_h: h })
}

/// Decode a JPEG, using DCT-domain downscaling when only `want` pixels on the
/// long edge are needed. Returns the image (resized to <= `want` if needed)
/// and the stream's full dimensions.
pub fn decode_jpeg(bytes: &[u8], want: u32) -> Result<(DynamicImage, u32, u32)> {
    let mut dec = jpeg_decoder::Decoder::new(bytes);
    dec.read_info()?;
    let info = dec.info().ok_or_else(|| anyhow!("no JPEG header"))?;
    let (fw, fh) = (info.width as u32, info.height as u32);
    if want > 0 && fw.max(fh) > want * 2 {
        let s = want as f32 / fw.max(fh) as f32;
        let (rw, rh) = dec.scale(((fw as f32 * s).ceil() as u16).max(1), ((fh as f32 * s).ceil() as u16).max(1))?;
        let px = dec.decode()?;
        let info = dec.info().unwrap();
        let img = to_dynamic(px, rw as u32, rh as u32, info.pixel_format)?;
        let img = if img.width().max(img.height()) > want { img.thumbnail(want, want) } else { img };
        return Ok((img, fw, fh));
    }
    // Full-size decode: the image crate's decoder is considerably faster.
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)?;
    let img = if want > 0 && img.width().max(img.height()) > want { img.thumbnail(want, want) } else { img };
    Ok((img, fw, fh))
}

fn to_dynamic(px: Vec<u8>, w: u32, h: u32, fmt: jpeg_decoder::PixelFormat) -> Result<DynamicImage> {
    use jpeg_decoder::PixelFormat::*;
    let err = || anyhow!("decoded JPEG has unexpected size");
    Ok(match fmt {
        RGB24 => DynamicImage::ImageRgb8(ImageBuffer::from_raw(w, h, px).ok_or_else(err)?),
        L8 => DynamicImage::ImageLuma8(ImageBuffer::from_raw(w, h, px).ok_or_else(err)?),
        L16 => {
            let v: Vec<u16> = px.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            DynamicImage::ImageLuma16(ImageBuffer::from_raw(w, h, v).ok_or_else(err)?)
        }
        CMYK32 => {
            // Adobe-style inverted CMYK, as written by most software.
            let rgb: Vec<u8> = px
                .chunks_exact(4)
                .flat_map(|c| {
                    let k = c[3] as u32;
                    [(c[0] as u32 * k / 255) as u8, (c[1] as u32 * k / 255) as u8, (c[2] as u32 * k / 255) as u8]
                })
                .collect();
            DynamicImage::ImageRgb8(ImageBuffer::from_raw(w, h, rgb).ok_or_else(err)?)
        }
    })
}

pub fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let mut out = Vec::with_capacity((rgb.width() * rgb.height() / 4) as usize);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&rgb)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_decode_is_at_least_requested() {
        let img = image::RgbImage::from_fn(1600, 1000, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 7]));
        let bytes = encode_jpeg(&DynamicImage::ImageRgb8(img), 85).unwrap();
        let (small, fw, fh) = decode_jpeg(&bytes, 300).unwrap();
        assert_eq!((fw, fh), (1600, 1000));
        assert_eq!(small.width(), 300);
        let (full, _, _) = decode_jpeg(&bytes, 0).unwrap();
        assert_eq!((full.width(), full.height()), (1600, 1000));
    }

    #[test]
    fn choose_prefers_smallest_sufficient() {
        let mk = |w| JpegRef { offset: w as usize, len: 1, width: w, height: w / 2 };
        let list = vec![mk(6000), mk(1620), mk(160)];
        assert_eq!(choose(&list, 400).unwrap().width, 1620);
        assert_eq!(choose(&list, 3000).unwrap().width, 6000);
        assert_eq!(choose(&list, 9000).unwrap().width, 6000);
        assert_eq!(choose(&list, 0).unwrap().width, 6000);
    }
}
