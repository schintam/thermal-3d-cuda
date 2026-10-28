//! Local, model-free quality signals computed from the culling preview.
//! They are suggestions only: shown as badges, used to rank bursts, never to
//! hide or remove anything.

use image::{imageops::FilterType, DynamicImage, GrayImage};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
pub struct Scores {
    /// 0..100, focus of the sharpest regions of the frame.
    pub sharpness: f32,
    /// Mean luminance, 0..1 (display-referred).
    pub exposure: f32,
    /// Fraction of pixels blown out / crushed.
    pub clip_hi: f32,
    pub clip_lo: f32,
    /// 64-bit difference hash: coarse similarity, used for burst grouping.
    pub dhash: u64,
    /// 512-bit fingerprint (16x16 horizontal + vertical difference hashes)
    /// for near-duplicate detection.
    pub fp: [u64; 8],
}

/// Working size for the sharpness measure, so scores are comparable across
/// photos regardless of preview resolution.
const WORK: u32 = 768;

pub fn analyze(img: &DynamicImage) -> Scores {
    let small = if img.width().max(img.height()) > WORK { img.thumbnail(WORK, WORK) } else { img.clone() };
    let gray = small.to_luma8();
    let (exposure, clip_hi, clip_lo) = exposure(&gray);
    Scores { sharpness: sharpness(&gray), exposure, clip_hi, clip_lo, dhash: dhash(&gray), fp: fingerprint(&gray) }
}

/// RMS of the Laplacian over the sharpest 15% of an 8x8 tile grid. Using the
/// best tiles rewards a sharp subject against a deliberately soft background.
fn sharpness(g: &GrayImage) -> f32 {
    let (w, h) = (g.width() as usize, g.height() as usize);
    if w < 16 || h < 16 {
        return 0.0;
    }
    const T: usize = 8;
    let mut tiles = vec![(0f64, 0u32); T * T];
    let px = g.as_raw();
    for y in 1..h - 1 {
        let ty = y * T / h;
        for x in 1..w - 1 {
            let c = px[y * w + x] as i32;
            let lap = px[y * w + x - 1] as i32 + px[y * w + x + 1] as i32 + px[(y - 1) * w + x] as i32
                + px[(y + 1) * w + x] as i32
                - 4 * c;
            let t = &mut tiles[ty * T + x * T / w];
            t.0 += (lap * lap) as f64;
            t.1 += 1;
        }
    }
    let mut energy: Vec<f64> = tiles.iter().filter(|t| t.1 > 0).map(|t| t.0 / t.1 as f64).collect();
    energy.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let top = (energy.len() * 15 / 100).max(1);
    let rms = (energy[..top].iter().sum::<f64>() / top as f64).sqrt();
    (100.0 * rms / (rms + 18.0)) as f32
}

fn exposure(g: &GrayImage) -> (f32, f32, f32) {
    let n = g.as_raw().len().max(1) as f32;
    let (mut sum, mut hi, mut lo) = (0u64, 0u32, 0u32);
    for &p in g.as_raw() {
        sum += p as u64;
        hi += (p >= 250) as u32;
        lo += (p <= 4) as u32;
    }
    (sum as f32 / n / 255.0, hi as f32 / n, lo as f32 / n)
}

fn dhash(g: &GrayImage) -> u64 {
    let s = image::imageops::resize(g, 9, 8, FilterType::Triangle);
    let mut h = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            h = (h << 1) | (s.get_pixel(x, y)[0] < s.get_pixel(x + 1, y)[0]) as u64;
        }
    }
    h
}

fn fingerprint(g: &GrayImage) -> [u64; 8] {
    let s = image::imageops::resize(g, 17, 17, FilterType::Triangle);
    let mut out = [0u64; 8];
    for y in 0..16 {
        for x in 0..16 {
            let p = s.get_pixel(x, y)[0];
            let i = (y * 16 + x) as usize;
            out[i / 64] |= ((p < s.get_pixel(x + 1, y)[0]) as u64) << (i % 64);
            out[4 + i / 64] |= ((p < s.get_pixel(x, y + 1)[0]) as u64) << (i % 64);
        }
    }
    out
}

pub fn hamming_fp(a: &[u64; 8], b: &[u64; 8]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

pub fn fp_to_bytes(fp: &[u64; 8]) -> Vec<u8> {
    fp.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub fn fp_from_bytes(b: &[u8]) -> [u64; 8] {
    let mut out = [0u64; 8];
    for (i, c) in b.chunks_exact(8).take(8).enumerate() {
        out[i] = u64::from_le_bytes(c.try_into().unwrap());
    }
    out
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Single number used to rank frames inside a burst: sharpness first, then
/// penalise clipping and exposures far from a typical mid-tone mean.
pub fn quality(s: &Scores) -> f32 {
    s.sharpness - s.clip_hi * 150.0 - s.clip_lo * 60.0 - (s.exposure - 0.45).abs() * 25.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    fn checker(blur: u32) -> DynamicImage {
        let g = GrayImage::from_fn(800, 600, |x, y| Luma([if (x / 8 + y / 8) % 2 == 0 { 40 } else { 200 }]));
        let d = DynamicImage::ImageLuma8(g);
        if blur > 0 { d.blur(blur as f32) } else { d }
    }

    #[test]
    fn sharper_scores_higher() {
        let sharp = analyze(&checker(0));
        let soft = analyze(&checker(4));
        assert!(sharp.sharpness > soft.sharpness + 10.0, "{} vs {}", sharp.sharpness, soft.sharpness);
        assert!(quality(&sharp) > quality(&soft));
    }

    #[test]
    fn exposure_and_clipping() {
        let white = DynamicImage::ImageLuma8(GrayImage::from_pixel(100, 100, Luma([255])));
        let s = analyze(&white);
        assert!(s.exposure > 0.99 && s.clip_hi > 0.99 && s.clip_lo == 0.0);
    }

    #[test]
    fn dhash_tolerates_small_changes() {
        let a = analyze(&checker(1));
        let b = analyze(&checker(1).brighten(10));
        assert!(hamming(a.dhash, b.dhash) <= 4);
        assert!(hamming_fp(&a.fp, &b.fp) <= 12);
        assert_eq!(fp_from_bytes(&fp_to_bytes(&a.fp)), a.fp);
    }

    #[test]
    fn fingerprint_separates_shifted_content() {
        let boxed = |x0: u32| {
            DynamicImage::ImageLuma8(GrayImage::from_fn(800, 600, |x, y| {
                Luma([if x >= x0 && x < x0 + 200 && (150..450).contains(&y) { 220 } else { 60 + (y / 10) as u8 }])
            }))
        };
        let a = analyze(&boxed(100));
        let b = analyze(&boxed(450));
        let a2 = analyze(&boxed(100).brighten(3));
        assert!(hamming_fp(&a.fp, &b.fp) > 12, "{}", hamming_fp(&a.fp, &b.fp));
        assert!(hamming_fp(&a.fp, &a2.fp) <= 4);
    }
}
