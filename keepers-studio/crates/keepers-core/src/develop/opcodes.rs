//! DNG opcode lens corrections (DNG spec 1.4, chapter 7).
//!
//! Cameras such as the Leica Q2 ship their lens profile inside every DNG as
//! `WarpRectilinear` (distortion + lateral CA) and `FixVignetteRadial`
//! opcodes; raw data is uncorrected, so applying them is essential.

use super::source::Linear;
use crate::embedded::{rd16, rd32, tiff_order, Order};
use rayon::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Per plane (or one for all): kr0..kr3, kt0, kt1; then centre.
    Warp { planes: Vec<[f64; 6]>, cx: f64, cy: f64 },
    Vignette { k: [f64; 5], cx: f64, cy: f64 },
}

const OPCODE_LIST2: u16 = 0xC741;
const OPCODE_LIST3: u16 = 0xC74E;

/// Find and parse lens-related opcodes in a DNG's IFDs.
pub fn read(buf: &[u8]) -> Vec<Op> {
    let Some(order) = tiff_order(buf) else { return vec![] };
    let mut ops = Vec::new();
    for tag in [OPCODE_LIST2, OPCODE_LIST3] {
        if let Some(data) = find_tag(buf, order, tag) {
            ops.extend(parse(data));
        }
    }
    ops
}

fn find_tag(buf: &[u8], order: Order, want: u16) -> Option<&[u8]> {
    let mut queue = vec![rd32(buf, 4, order)? as usize];
    let mut seen = Vec::new();
    while let Some(ifd) = queue.pop() {
        if ifd == 0 || ifd >= buf.len() || seen.contains(&ifd) || seen.len() > 32 {
            continue;
        }
        seen.push(ifd);
        let n = rd16(buf, ifd, order)? as usize;
        for i in 0..n.min(1000) {
            let e = ifd + 2 + i * 12;
            let tag = rd16(buf, e, order)?;
            let count = rd32(buf, e + 4, order)? as usize;
            if tag == want && count > 4 {
                let off = rd32(buf, e + 8, order)? as usize;
                return buf.get(off..off.checked_add(count)?);
            }
            if tag == 0x014A {
                let typ = rd16(buf, e + 2, order)?;
                if matches!(typ, 4 | 13) {
                    if count == 1 {
                        queue.push(rd32(buf, e + 8, order)? as usize);
                    } else {
                        let base = rd32(buf, e + 8, order)? as usize;
                        for k in 0..count.min(16) {
                            queue.extend(rd32(buf, base + k * 4, order).map(|v| v as usize));
                        }
                    }
                }
            }
        }
        if let Some(next) = rd32(buf, ifd + 2 + n * 12, order) {
            queue.push(next as usize);
        }
    }
    None
}

/// Opcode lists are always big-endian.
pub fn parse(d: &[u8]) -> Vec<Op> {
    let be32 = |o: usize| d.get(o..o + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let f64at = |o: usize| d.get(o..o + 8).map(|b| f64::from_be_bytes(b.try_into().unwrap()));
    let mut ops = Vec::new();
    let Some(count) = be32(0) else { return ops };
    let mut pos = 4;
    for _ in 0..count.min(64) {
        let (Some(id), Some(len)) = (be32(pos), be32(pos + 12)) else { break };
        let p = pos + 16;
        match id {
            1 => {
                if let Some(n) = be32(p) {
                    let n = n.min(4) as usize;
                    let planes: Option<Vec<[f64; 6]>> = (0..n)
                        .map(|i| {
                            let b = p + 4 + i * 48;
                            Some([f64at(b)?, f64at(b + 8)?, f64at(b + 16)?, f64at(b + 24)?, f64at(b + 32)?, f64at(b + 40)?])
                        })
                        .collect();
                    let c = p + 4 + n * 48;
                    if let (Some(planes), Some(cx), Some(cy)) = (planes, f64at(c), f64at(c + 8)) {
                        if !planes.is_empty() {
                            ops.push(Op::Warp { planes, cx, cy });
                        }
                    }
                }
            }
            3 => {
                let v: Option<Vec<f64>> = (0..7).map(|i| f64at(p + i * 8)).collect();
                if let Some(v) = v {
                    ops.push(Op::Vignette { k: [v[0], v[1], v[2], v[3], v[4]], cx: v[5], cy: v[6] });
                }
            }
            _ => {}
        }
        pos = p + len as usize;
    }
    ops
}

/// Max distance from the optical centre to any image corner, in pixels.
fn max_radius(w: f64, h: f64, x0: f64, y0: f64) -> f64 {
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
        .iter()
        .map(|(x, y)| ((x - x0).powi(2) + (y - y0).powi(2)).sqrt())
        .fold(0.0, f64::max)
        .max(1.0)
}

pub fn apply(ops: &[Op], img: &mut Linear) {
    for op in ops {
        match op {
            Op::Vignette { k, cx, cy } => vignette(img, k, *cx, *cy),
            Op::Warp { planes, cx, cy } => warp(img, planes, *cx, *cy),
        }
    }
}

fn vignette(img: &mut Linear, k: &[f64; 5], cx: f64, cy: f64) {
    let (w, h) = (img.w as f64, img.h as f64);
    let (x0, y0) = (cx * w, cy * h);
    let m2 = max_radius(w, h, x0, y0).powi(2);
    let iw = img.w;
    img.px.par_chunks_mut(iw).enumerate().for_each(|(y, row)| {
        let dy = y as f64 + 0.5 - y0;
        for (x, p) in row.iter_mut().enumerate() {
            let dx = x as f64 + 0.5 - x0;
            let r2 = (dx * dx + dy * dy) / m2;
            let g = 1.0 + r2 * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * (k[3] + r2 * k[4]))));
            let g = g as f32;
            p.iter_mut().for_each(|v| *v *= g);
        }
    });
}

fn warp(img: &mut Linear, planes: &[[f64; 6]], cx: f64, cy: f64) {
    let (w, h) = (img.w as f64, img.h as f64);
    let (x0, y0) = (cx * w, cy * h);
    let m = max_radius(w, h, x0, y0);
    let src = img.px.clone();
    let (iw, ih) = (img.w, img.h);
    img.px.par_chunks_mut(iw).enumerate().for_each(|(y, row)| {
        let dy = (y as f64 + 0.5 - y0) / m;
        for (x, p) in row.iter_mut().enumerate() {
            let dx = (x as f64 + 0.5 - x0) / m;
            let r2 = dx * dx + dy * dy;
            for c in 0..3 {
                let k = &planes[c.min(planes.len() - 1)];
                let f = k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3]));
                let dxt = k[4] * 2.0 * dx * dy + k[5] * (r2 + 2.0 * dx * dx);
                let dyt = k[5] * 2.0 * dx * dy + k[4] * (r2 + 2.0 * dy * dy);
                let sx = x0 + m * (f * dx + dxt) - 0.5;
                let sy = y0 + m * (f * dy + dyt) - 0.5;
                p[c] = sample(&src, iw, ih, sx as f32, sy as f32, c);
            }
        }
    });
}

/// Bilinear sample of one channel with edge clamping.
pub fn sample(px: &[[f32; 3]], w: usize, h: usize, x: f32, y: f32, c: usize) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (xi, yi) = (x as usize, y as usize);
    let (x1, y1) = ((xi + 1).min(w - 1), (yi + 1).min(h - 1));
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let a = px[yi * w + xi][c] * (1.0 - fx) + px[yi * w + x1][c] * fx;
    let b = px[y1 * w + xi][c] * (1.0 - fx) + px[y1 * w + x1][c] * fx;
    a * (1.0 - fy) + b * fy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opcode(id: u32, params: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&id.to_be_bytes());
        v.extend_from_slice(&0x01030000u32.to_be_bytes());
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&(params.len() as u32).to_be_bytes());
        v.extend_from_slice(params);
        v
    }

    fn doubles(v: &[f64]) -> Vec<u8> {
        v.iter().flat_map(|d| d.to_be_bytes()).collect()
    }

    #[test]
    fn parses_warp_and_vignette() {
        let mut warp = 1u32.to_be_bytes().to_vec();
        warp.extend(doubles(&[1.0, -0.05, 0.01, 0.0, 0.0, 0.0, 0.5, 0.5]));
        let vig = doubles(&[0.3, 0.1, 0.0, 0.0, 0.0, 0.5, 0.5]);
        let mut list = 3u32.to_be_bytes().to_vec();
        list.extend(opcode(1, &warp));
        list.extend(opcode(9, &[0; 12])); // unknown opcode is skipped
        list.extend(opcode(3, &vig));
        let ops = parse(&list);
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Op::Warp { planes, cx, .. } if planes.len() == 1 && planes[0][1] == -0.05 && *cx == 0.5));
        assert!(matches!(&ops[1], Op::Vignette { k, .. } if k[0] == 0.3));
    }

    #[test]
    fn identity_warp_is_noop_and_vignette_brightens_corners() {
        let mut img = Linear::new(20, 10, true);
        for (i, p) in img.px.iter_mut().enumerate() {
            *p = [(i % 7) as f32 / 7.0; 3];
        }
        let before = img.px.clone();
        apply(&[Op::Warp { planes: vec![[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]], cx: 0.5, cy: 0.5 }], &mut img);
        for (a, b) in img.px.iter().zip(&before) {
            assert!((a[0] - b[0]).abs() < 1e-5);
        }
        let mut flat = Linear::new(20, 10, true);
        flat.px.iter_mut().for_each(|p| *p = [0.5; 3]);
        apply(&[Op::Vignette { k: [0.5, 0.0, 0.0, 0.0, 0.0], cx: 0.5, cy: 0.5 }], &mut flat);
        assert!(flat.px[0][0] > flat.px[5 * 20 + 10][0]);
    }
}
