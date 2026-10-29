//! Loading a photo as linear-light sRGB for developing.
//!
//! RAW decoding (container parsing, decompression, demosaicing) is done by the
//! `rawler` RAW core. White balance and the camera-to-sRGB matrix are applied
//! here so the fast half-size preview path and the full-resolution export path
//! produce identical colour.

use super::opcodes;
use crate::formats::Kind;
use crate::util::{self, guard};
use anyhow::{bail, Result};
use rawler::decoders::RawDecodeParams;
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::imgop::Rect;
use rawler::rawimage::RawPhotometricInterpretation;
use rawler::rawsource::RawSource;
use std::path::Path;

/// Linear-light, sRGB-primaries image. RAW values may exceed 1.0 only before
/// highlight clipping; after loading they are in 0..=1.
#[derive(Clone)]
pub struct Linear {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 3]>,
    /// RAW sources get a default base tone curve; rendered sources do not.
    pub is_raw: bool,
}

impl Linear {
    pub fn new(w: usize, h: usize, is_raw: bool) -> Linear {
        Linear { w, h, px: vec![[0.0; 3]; w * h], is_raw }
    }

    pub fn long_edge(&self) -> usize {
        self.w.max(self.h)
    }

    /// Box-filter downscale so the long edge is at most `max`.
    pub fn downscale(&self, max: usize) -> Linear {
        if self.long_edge() <= max {
            return self.clone();
        }
        let s = max as f32 / self.long_edge() as f32;
        let (nw, nh) = (((self.w as f32 * s).round() as usize).max(1), ((self.h as f32 * s).round() as usize).max(1));
        let mut out = Linear::new(nw, nh, self.is_raw);
        let (fx, fy) = (self.w as f32 / nw as f32, self.h as f32 / nh as f32);
        use rayon::prelude::*;
        out.px.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
            let y0 = (y as f32 * fy) as usize;
            let y1 = (((y + 1) as f32 * fy) as usize).clamp(y0 + 1, self.h);
            for (x, o) in row.iter_mut().enumerate() {
                let x0 = (x as f32 * fx) as usize;
                let x1 = (((x + 1) as f32 * fx) as usize).clamp(x0 + 1, self.w);
                let mut acc = [0f32; 3];
                for yy in y0..y1 {
                    for p in &self.px[yy * self.w + x0..yy * self.w + x1] {
                        acc[0] += p[0];
                        acc[1] += p[1];
                        acc[2] += p[2];
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as f32;
                *o = [acc[0] / n, acc[1] / n, acc[2] / n];
            }
        });
        out
    }

    /// Apply an EXIF orientation.
    pub fn orient(self, o: u16) -> Linear {
        if o <= 1 || o > 8 {
            return self;
        }
        let (w, h) = (self.w, self.h);
        let swap = o >= 5;
        let (nw, nh) = if swap { (h, w) } else { (w, h) };
        let mut out = Linear::new(nw, nh, self.is_raw);
        for y in 0..nh {
            for x in 0..nw {
                // Source coordinate for destination (x, y).
                let (sx, sy) = match o {
                    2 => (w - 1 - x, y),
                    3 => (w - 1 - x, h - 1 - y),
                    4 => (x, h - 1 - y),
                    5 => (y, x),
                    6 => (y, h - 1 - x),
                    7 => (w - 1 - y, h - 1 - x),
                    8 => (w - 1 - y, x),
                    _ => (x, y),
                };
                out.px[y * nw + x] = self.px[sy * w + sx];
            }
        }
        out
    }
}

pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Load for developing. `max_edge` limits the long edge (for interactive
/// previews); `None` loads full resolution for export.
pub fn load(path: &Path, kind: Kind, orientation: u16, max_edge: Option<usize>) -> Result<Linear> {
    guard(|| match kind {
        Kind::Raw => load_raw(path, max_edge),
        Kind::Jpeg | Kind::Tiff | Kind::Png | Kind::Webp | Kind::Gif => {
            let img = image::ImageReader::open(path)?.with_guessed_format()?.decode()?;
            let img = match max_edge {
                Some(m) if img.width().max(img.height()) as usize > m * 2 => img.thumbnail(m as u32 * 2, m as u32 * 2),
                _ => img,
            };
            let lin = from_rendered(&img);
            let lin = lin.orient(orientation);
            Ok(match max_edge {
                Some(m) => lin.downscale(m),
                None => lin,
            })
        }
        Kind::Heic => {
            // Upright already: HEIF carries its own rotation.
            let lin = from_rendered(&crate::heic::decode(path, 0)?);
            Ok(match max_edge {
                Some(m) => lin.downscale(m),
                None => lin,
            })
        }
    })
}

/// A rendered (display-referred sRGB) image to linear light.
pub fn from_rendered(img: &image::DynamicImage) -> Linear {
    let lut: Vec<f32> = (0..=255).map(|v| srgb_to_linear(v as f32 / 255.0)).collect();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = match img {
        image::DynamicImage::ImageRgb8(b) => {
            b.as_raw().chunks_exact(3).map(|c| [lut[c[0] as usize], lut[c[1] as usize], lut[c[2] as usize]]).collect()
        }
        _ => {
            let b = img.to_rgb32f();
            b.as_raw()
                .chunks_exact(3)
                .map(|c| [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2])])
                .collect()
        }
    };
    Linear { w, h, px, is_raw: false }
}

fn load_raw(path: &Path, max_edge: Option<usize>) -> Result<Linear> {
    let src = RawSource::new(path)?;
    let dec = rawler::get_decoder(&src)?;
    let params = RawDecodeParams::default();
    let mut raw = dec.raw_image(&src, &params, false)?;
    let orientation = raw.orientation.to_u16();
    let ops = util::map_file(path).ok().map(|b| opcodes::read(&b)).unwrap_or_default();

    let bayer = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(c) => {
            c.cfa.is_rgb() && c.sensor == rawler::imgop::sensor::SensorType::Bayer && raw.cpp == 1
        }
        _ => false,
    };

    let cam2rgb = cam_to_srgb(&raw);
    let wb = white_balance(&raw);
    let active = raw.active_area.unwrap_or(Rect::new(rawler::imgop::Point::zero(), raw.dim()));
    let crop = raw.crop_area.map(|c| c.adapt(&active));

    // Half-size "superpixel" demosaic is plenty for a screen preview and ~5x faster.
    let half_ok = matches!(max_edge, Some(m) if bayer && active.d.w.max(active.d.h) / 2 >= m);

    let (mut lin, scale) = if half_ok {
        raw.apply_scaling()?;
        let cfa = match &raw.photometric {
            RawPhotometricInterpretation::Cfa(c) => c.cfa.clone(),
            _ => unreachable!(),
        };
        let data = raw.data.as_f32();
        let (w, h) = (active.d.w / 2, active.d.h / 2);
        let mut cam = vec![[0f32; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0f32; 3];
                let mut n = [0f32; 3];
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (ry, rx) = (active.p.y + y * 2 + dy, active.p.x + x * 2 + dx);
                        let c = cfa.color_at(ry, rx).min(2);
                        acc[c] += data[ry * raw.width + rx];
                        n[c] += 1.0;
                    }
                }
                cam[y * w + x] = [acc[0] / n[0].max(1.0), acc[1] / n[1].max(1.0), acc[2] / n[2].max(1.0)];
            }
        }
        (to_srgb(cam, w, h, wb, &cam2rgb), 0.5)
    } else if bayer {
        let dev = RawDevelop::new_with(&[ProcessingStep::Rescale, ProcessingStep::Demosaic, ProcessingStep::CropActiveArea]);
        match dev.develop_intermediate(&raw)? {
            Intermediate::ThreeColor(p) => {
                let (w, h) = (p.width, p.height);
                (to_srgb(p.data, w, h, wb, &cam2rgb), 1.0)
            }
            _ => bail!("unexpected RAW layout"),
        }
    } else {
        // Non-Bayer sensors (X-Trans, 4-colour, linear DNG): let the RAW core
        // do the whole colour pipeline up to linear sRGB.
        let dev = RawDevelop::new_with(&[
            ProcessingStep::Rescale,
            ProcessingStep::Demosaic,
            ProcessingStep::FujiRotate,
            ProcessingStep::CropActiveArea,
            ProcessingStep::WhiteBalance,
            ProcessingStep::Calibrate,
            ProcessingStep::CropDefault,
        ]);
        let lin = match dev.develop_intermediate(&raw)? {
            Intermediate::ThreeColor(p) => {
                let (w, h) = (p.width, p.height);
                Linear { w, h, px: p.data, is_raw: true }
            }
            Intermediate::Monochrome(p) => {
                let (w, h) = (p.width, p.height);
                Linear { w, h, px: p.data.iter().map(|&v| [v, v, v]).collect(), is_raw: true }
            }
            _ => bail!("unexpected RAW layout"),
        };
        let lin = lin.orient(orientation);
        return Ok(match max_edge {
            Some(m) => lin.downscale(m),
            None => lin,
        });
    };

    // DNG lens corrections (e.g. the Leica Q2's distortion and vignetting
    // profile) are defined relative to the active area.
    opcodes::apply(&ops, &mut lin);

    if let Some(c) = crop {
        lin = crop_linear(&lin, (c.p.x as f32 * scale) as usize, (c.p.y as f32 * scale) as usize, (c.d.w as f32 * scale) as usize, (c.d.h as f32 * scale) as usize);
    }
    let lin = lin.orient(orientation);
    Ok(match max_edge {
        Some(m) => lin.downscale(m),
        None => lin,
    })
}

fn crop_linear(l: &Linear, x: usize, y: usize, w: usize, h: usize) -> Linear {
    let x = x.min(l.w.saturating_sub(1));
    let y = y.min(l.h.saturating_sub(1));
    let w = w.min(l.w - x).max(1);
    let h = h.min(l.h - y).max(1);
    if (x, y, w, h) == (0, 0, l.w, l.h) {
        return l.clone();
    }
    let mut px = Vec::with_capacity(w * h);
    for row in y..y + h {
        px.extend_from_slice(&l.px[row * l.w + x..row * l.w + x + w]);
    }
    Linear { w, h, px, is_raw: l.is_raw }
}

/// As-shot white balance normalised to green = 1.
fn white_balance(raw: &rawler::RawImage) -> [f32; 3] {
    let wb = raw.wb_coeffs;
    if wb[0].is_nan() || wb[1] <= 0.0 || wb.iter().take(3).any(|v| !v.is_finite() || *v <= 0.0) {
        return [1.0; 3];
    }
    [wb[0] / wb[1], 1.0, wb[2] / wb[1]]
}

const SRGB_TO_XYZ_D65: [[f32; 3]; 3] =
    [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];

/// Camera RGB -> linear sRGB, dcraw style: invert the row-normalised
/// (XYZ->camera x sRGB->XYZ) matrix.
fn cam_to_srgb(raw: &rawler::RawImage) -> [[f32; 3]; 3] {
    use rawler::imgop::xyz::Illuminant;
    let Some((_, m)) = raw.color_matrix_find_first([
        Illuminant::D65,
        Illuminant::D55,
        Illuminant::D50,
        Illuminant::Daylight,
        Illuminant::D75,
        Illuminant::Flash,
        Illuminant::A,
    ]) else {
        return IDENTITY;
    };
    if m.len() < 9 {
        return IDENTITY;
    }
    let xyz2cam = [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]];
    let mut rgb2cam = mul3(&xyz2cam, &SRGB_TO_XYZ_D65);
    for row in rgb2cam.iter_mut() {
        let s: f32 = row.iter().sum();
        if s.abs() > 1e-6 {
            row.iter_mut().for_each(|v| *v /= s);
        }
    }
    inv3(&rgb2cam).unwrap_or(IDENTITY)
}

const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn mul3(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut o = [[0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

pub(crate) fn inv3(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-9 {
        return None;
    }
    let id = 1.0 / det;
    Some([
        [(e * i - f * h) * id, (c * h - b * i) * id, (b * f - c * e) * id],
        [(f * g - d * i) * id, (a * i - c * g) * id, (c * d - a * f) * id],
        [(d * h - e * g) * id, (b * g - a * h) * id, (a * e - b * d) * id],
    ])
}

fn to_srgb(cam: Vec<[f32; 3]>, w: usize, h: usize, wb: [f32; 3], m: &[[f32; 3]; 3]) -> Linear {
    use rayon::prelude::*;
    let px = cam
        .into_par_iter()
        .map(|p| {
            // White-balance, then clip each channel at sensor saturation so
            // blown highlights go neutral instead of magenta.
            let r = (p[0] * wb[0]).clamp(0.0, 1.0);
            let g = (p[1] * wb[1]).clamp(0.0, 1.0);
            let b = (p[2] * wb[2]).clamp(0.0, 1.0);
            [
                (m[0][0] * r + m[0][1] * g + m[0][2] * b).max(0.0),
                (m[1][0] * r + m[1][1] * g + m[1][2] * b).max(0.0),
                (m[2][0] * r + m[2][1] * g + m[2][2] * b).max(0.0),
            ]
        })
        .collect();
    Linear { w, h, px, is_raw: true }
}

/// Quick look at a RAW with no usable embedded preview: half-size develop with
/// default settings.
pub fn decode_raw_preview(path: &Path, max_edge: u32) -> Result<image::DynamicImage> {
    let lin = load(path, Kind::Raw, 1, Some(max_edge as usize))?;
    Ok(super::pipeline::render_image(&lin, &super::Edits::default(), &super::RenderOpts::default()).0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip() {
        for i in 0..=255 {
            let v = i as f32 / 255.0;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5);
        }
    }

    #[test]
    fn inverse() {
        let m = [[2.0, 0.5, 0.0], [0.1, 1.0, 0.3], [0.0, 0.2, 1.5]];
        let i = inv3(&m).unwrap();
        let p = mul3(&m, &i);
        for r in 0..3 {
            for c in 0..3 {
                assert!((p[r][c] - if r == c { 1.0 } else { 0.0 }).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn orientations_preserve_pixels() {
        let mut l = Linear::new(3, 2, false);
        for (i, p) in l.px.iter_mut().enumerate() {
            *p = [i as f32, 0.0, 0.0];
        }
        let r = l.clone().orient(6);
        assert_eq!((r.w, r.h), (2, 3));
        // Rotating 90° clockwise puts the bottom-left pixel at the top-left.
        assert_eq!(r.px[0][0], 3.0);
        assert_eq!(r.px[1][0], 0.0);
        let back = r.orient(8);
        assert_eq!(back.px, l.px);
    }

    #[test]
    fn downscale_averages() {
        let mut l = Linear::new(4, 2, false);
        l.px = vec![[1.0; 3], [3.0; 3], [5.0; 3], [7.0; 3], [1.0; 3], [3.0; 3], [5.0; 3], [7.0; 3]];
        let d = l.downscale(2);
        assert_eq!((d.w, d.h), (2, 1));
        assert_eq!(d.px[0][0], 2.0);
        assert_eq!(d.px[1][0], 6.0);
    }
}
