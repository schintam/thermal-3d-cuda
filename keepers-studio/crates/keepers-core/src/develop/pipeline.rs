//! The develop pipeline: geometry -> white balance/exposure -> tone -> colour
//! -> noise reduction -> sharpening. Runs on CPU with rayon; a 2 MP preview
//! renders in a few milliseconds per core.

use super::opcodes::sample;
use super::source::{linear_to_srgb, srgb_to_linear, Linear};
use super::style::{self, HarmonyOp, StockOp};
use super::{Edits, RenderOpts};
use image::{DynamicImage, ImageBuffer};
use rayon::prelude::*;

/// Display-referred sRGB result, values 0..=1.
pub struct Rendered {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 3]>,
}

pub type Histogram = [[u32; 256]; 3];

pub fn render(src: &Linear, e: &Edits, o: &RenderOpts) -> Rendered {
    let geo = geometry(src, e, o.ignore_crop);
    let pipe = PixelPipe::new(e, src.is_raw);
    let mut px: Vec<[f32; 3]> = geo.px.par_iter().map(|p| pipe.apply(*p)).collect();
    let scale = if o.scale > 0.0 { o.scale } else { 1.0 };
    let (w, h) = (geo.w, geo.h);
    let glow = e.glow / 100.0 + pipe.halation();
    if glow > 0.0 {
        let mono = e.mono.enabled && e.mono.amount >= 50.0;
        style_glow(&mut px, w, h, glow, mono);
    }
    if e.nr_color > 0.0 || e.nr_luma > 0.0 {
        noise_reduction(&mut px, w, h, e.nr_luma / 100.0, e.nr_color / 100.0, scale);
    }
    if e.sharpen > 0.0 {
        sharpen(&mut px, w, h, e.sharpen / 100.0, e.sharpen_radius.clamp(0.3, 5.0) * scale);
    }
    if e.grain > 0.0 {
        let mono = e.mono.enabled && e.mono.amount >= 50.0;
        film_grain(&mut px, w, h, e.grain / 100.0, e.grain_size, mono);
    }
    if e.edge_burn > 0.0 {
        edge_burn(&mut px, w, h, e.edge_burn / 100.0);
    }
    if o.zones {
        px.par_iter_mut().for_each(|p| {
            let lin = srgb_to_linear(style::luma(*p).clamp(0.0, 1.0));
            let v = linear_to_srgb(style::zone_value(style::zone_of(lin) as f32));
            *p = [v, v, v];
        });
    }
    if o.clip_warnings {
        px.par_iter_mut().for_each(|p| {
            if p.iter().any(|&v| v >= 0.999) {
                *p = [1.0, 0.0, 0.0];
            } else if p.iter().all(|&v| v <= 0.001) {
                *p = [0.0, 0.35, 1.0];
            }
        });
    }
    Rendered { w: geo.w, h: geo.h, px }
}

/// Render to an 8-bit image plus its histogram.
pub fn render_image(src: &Linear, e: &Edits, o: &RenderOpts) -> (DynamicImage, Histogram) {
    let r = render(src, e, o);
    let img = to_rgb8(&r);
    let hist = histogram(img.as_rgb8().unwrap().as_raw());
    (img, hist)
}

pub fn to_rgb8(r: &Rendered) -> DynamicImage {
    let data: Vec<u8> = r.px.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    DynamicImage::ImageRgb8(ImageBuffer::from_raw(r.w as u32, r.h as u32, data).unwrap())
}

pub fn to_rgb16(r: &Rendered) -> DynamicImage {
    let data: Vec<u16> = r.px.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16)).collect();
    DynamicImage::ImageRgb16(ImageBuffer::from_raw(r.w as u32, r.h as u32, data).unwrap())
}

pub fn histogram(rgb: &[u8]) -> Histogram {
    rgb.par_chunks(3 * 4096)
        .fold(
            || [[0u32; 256]; 3],
            |mut h, chunk| {
                for p in chunk.chunks_exact(3) {
                    h[0][p[0] as usize] += 1;
                    h[1][p[1] as usize] += 1;
                    h[2][p[2] as usize] += 1;
                }
                h
            },
        )
        .reduce(
            || [[0u32; 256]; 3],
            |mut a, b| {
                for c in 0..3 {
                    for i in 0..256 {
                        a[c][i] += b[c][i];
                    }
                }
                a
            },
        )
}

// ----------------------------------------------------------- geometry ----

/// Size of the image after rotation/straighten/crop, for a source of `w`x`h`.
pub fn output_size(w: usize, h: usize, e: &Edits) -> (usize, usize) {
    let (w, h) = if e.rotate % 2 == 1 { (h, w) } else { (w, h) };
    match e.crop {
        Some(c) => (((c.w.clamp(0.0, 1.0) * w as f32).round() as usize).max(1), ((c.h.clamp(0.0, 1.0) * h as f32).round() as usize).max(1)),
        None => (w, h),
    }
}

fn geometry(src: &Linear, e: &Edits, ignore_crop: bool) -> Linear {
    let quarter = match e.rotate % 4 {
        1 => 6,
        2 => 3,
        3 => 8,
        _ => 1,
    };
    let needs_copy = quarter != 1 || e.flip_h || e.angle.abs() > 0.01 || (e.crop.is_some() && !ignore_crop);
    if !needs_copy {
        return src.clone();
    }
    let mut img = src.clone().orient(quarter);
    if e.flip_h {
        img = img.orient(2);
    }
    if e.angle.abs() > 0.01 {
        img = straighten(&img, e.angle.clamp(-45.0, 45.0));
    }
    if let (Some(c), false) = (e.crop, ignore_crop) {
        let x = ((c.x.clamp(0.0, 1.0) * img.w as f32) as usize).min(img.w - 1);
        let y = ((c.y.clamp(0.0, 1.0) * img.h as f32) as usize).min(img.h - 1);
        let w = ((c.w.clamp(0.0, 1.0) * img.w as f32).round() as usize).clamp(1, img.w - x);
        let h = ((c.h.clamp(0.0, 1.0) * img.h as f32).round() as usize).clamp(1, img.h - y);
        let mut px = Vec::with_capacity(w * h);
        for row in y..y + h {
            px.extend_from_slice(&img.px[row * img.w + x..row * img.w + x + w]);
        }
        img = Linear { w, h, px, is_raw: img.is_raw };
    }
    img
}

/// Rotate about the centre, scaled up just enough that no empty corners show.
fn straighten(img: &Linear, deg: f32) -> Linear {
    let t = deg.to_radians();
    let (s, c) = (t.sin().abs(), t.cos().abs());
    let (w, h) = (img.w as f32, img.h as f32);
    let cover = ((w * c + h * s) / w).max((w * s + h * c) / h);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (sin, cos) = (t.sin(), t.cos());
    let mut out = Linear::new(img.w, img.h, img.is_raw);
    let iw = img.w;
    out.px.par_chunks_mut(iw).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let dx = (x as f32 + 0.5 - cx) / cover;
            let dy = (y as f32 + 0.5 - cy) / cover;
            // Inverse rotation maps output back into the source.
            let sx = cos * dx + sin * dy + cx - 0.5;
            let sy = -sin * dx + cos * dy + cy - 0.5;
            *p = [0, 1, 2].map(|ch| sample(&img.px, img.w, img.h, sx, sy, ch));
        }
    });
    out
}

// --------------------------------------------------------------- tone ----

const LUT_N: usize = 4096;

/// Per-channel lookup from linear input (0..1) to display-encoded output,
/// combining white balance, exposure and every global tone control. Indexed
/// in sqrt space for precision in the shadows.
struct ToneLut {
    t: [Vec<f32>; 3],
}

impl ToneLut {
    fn new(e: &Edits, is_raw: bool, m: [f32; 3]) -> ToneLut {
        let curve = Spline::new(&e.curve);
        let t = [0, 1, 2].map(|c| {
            (0..LUT_N)
                .map(|i| {
                    let s = i as f32 / (LUT_N - 1) as f32;
                    tone(s * s * m[c], e, is_raw, &curve)
                })
                .collect()
        });
        ToneLut { t }
    }

    #[inline]
    fn apply(&self, c: usize, v: f32) -> f32 {
        let f = v.clamp(0.0, 1.0).sqrt() * (LUT_N - 1) as f32;
        let i = (f as usize).min(LUT_N - 2);
        let fr = f - i as f32;
        let t = &self.t[c];
        t[i] + (t[i + 1] - t[i]) * fr
    }
}

/// White balance shift as per-channel linear gains (luminance-neutral).
fn wb_gains(e: &Edits) -> [f32; 3] {
    let t = e.temp / 100.0;
    let u = e.tint / 100.0;
    let (r, g, b) = ((0.35 * t).exp(), (-0.3 * u).exp(), (-0.35 * t).exp());
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    [r / lum, g / lum, b / lum]
}

fn ev_gain(e: &Edits) -> f32 {
    2f32.powf(e.exposure.clamp(-5.0, 5.0))
}

/// Everything that happens to one pixel, in order: white balance, exposure,
/// tone; then either the black-and-white print path (film/filter mix, paper,
/// toner) or the colour path (HSL, harmony, film rendition).
pub struct PixelPipe {
    lut: ToneLut,
    color: ColorOps,
    harmony: Option<HarmonyOp>,
    stock: Option<StockOp>,
    mono: Option<MonoPipe>,
}

struct MonoPipe {
    /// Film/filter weights folded together with white balance.
    w: [f32; 3],
    lut: ToneLut,
    print: Vec<f32>,
    toner: style::Toner,
    toner_amount: f32,
    amount: f32,
}

const PRINT_N: usize = 1024;

impl PixelPipe {
    pub fn new(e: &Edits, is_raw: bool) -> PixelPipe {
        let wb = wb_gains(e);
        let ev = ev_gain(e);
        let mono = (e.mono.enabled && e.mono.amount > 0.0).then(|| {
            let fw = style::mono_weights(e.mono.film, e.mono.filter);
            MonoPipe {
                w: [fw[0] * wb[0], fw[1] * wb[1], fw[2] * wb[2]],
                lut: ToneLut::new(e, is_raw, [ev; 3]),
                print: (0..PRINT_N).map(|i| style::mono_print_curve(i as f32 / (PRINT_N - 1) as f32, &e.mono)).collect(),
                toner: e.mono.toner,
                toner_amount: e.mono.toner_amount,
                amount: (e.mono.amount / 100.0).clamp(0.0, 1.0),
            }
        });
        PixelPipe {
            lut: ToneLut::new(e, is_raw, wb.map(|g| g * ev)),
            color: ColorOps::new(e),
            harmony: HarmonyOp::new(&e.harmony),
            stock: StockOp::new(&e.film),
            mono,
        }
    }

    pub fn halation(&self) -> f32 {
        if self.mono.is_some() { 0.0 } else { self.stock.as_ref().map(|s| s.halation()).unwrap_or(0.0) }
    }

    #[inline]
    pub fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let colour = |p: [f32; 3]| {
            let mut rgb = [self.lut.apply(0, p[0]), self.lut.apply(1, p[1]), self.lut.apply(2, p[2])];
            rgb = self.color.apply(rgb);
            if let Some(h) = &self.harmony {
                rgb = h.apply(rgb);
            }
            if let Some(s) = &self.stock {
                rgb = s.apply(rgb);
            }
            rgb
        };
        let Some(m) = &self.mono else { return colour(p) };
        let v = (m.w[0] * p[0] + m.w[1] * p[1] + m.w[2] * p[2]).max(0.0);
        let x = m.lut.apply(0, v);
        let f = x.clamp(0.0, 1.0) * (PRINT_N - 1) as f32;
        let i = (f as usize).min(PRINT_N - 2);
        let l = m.print[i] + (m.print[i + 1] - m.print[i]) * (f - i as f32);
        let bw = style::tone_print(l, m.toner, m.toner_amount);
        if m.amount >= 1.0 {
            return bw;
        }
        let c = colour(p);
        [0, 1, 2].map(|k| c[k] + (bw[k] - c[k]) * m.amount)
    }
}

/// Output (display-encoded) value of the pixel at normalised position
/// (`nx`, `ny`) of the rendered image, ignoring spatial effects. Averages a
/// small neighbourhood. Used by the Zone System placement tool.
pub fn probe(src: &Linear, e: &Edits, nx: f32, ny: f32) -> [f32; 3] {
    let geo = geometry(src, e, false);
    let pipe = PixelPipe::new(e, src.is_raw);
    let cx = (nx.clamp(0.0, 1.0) * (geo.w - 1) as f32) as i64;
    let cy = (ny.clamp(0.0, 1.0) * (geo.h - 1) as f32) as i64;
    let r = ((geo.w.max(geo.h) as f32 * 0.004) as i64).max(1);
    let mut acc = [0f32; 3];
    let mut n = 0.0;
    for y in (cy - r).max(0)..=(cy + r).min(geo.h as i64 - 1) {
        for x in (cx - r).max(0)..=(cx + r).min(geo.w as i64 - 1) {
            let o = pipe.apply(geo.px[y as usize * geo.w + x as usize]);
            for c in 0..3 {
                acc[c] += o[c];
            }
            n += 1.0;
        }
    }
    acc.map(|v| v / n)
}

/// Zone System placement: find the exposure that puts the tone at
/// (`nx`, `ny`) in `zone`, and anchor N+/N- development there.
pub fn place_zone(src: &Linear, e: &Edits, nx: f32, ny: f32, zone: u8) -> Edits {
    let target = style::zone_value(zone.min(10) as f32).min(1.0);
    let lum = |ev: f32| {
        let t = Edits { exposure: ev, ..e.clone() };
        srgb_to_linear(style::luma(probe(src, &t, nx, ny)).clamp(0.0, 1.0))
    };
    let (mut lo, mut hi) = (-5.0f32, 5.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if lum(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Edits { exposure: ((lo + hi) / 2.0 * 100.0).round() / 100.0, zone_anchor: Some(zone.min(10)), ..e.clone() }
}

/// Linear value -> tone-mapped display value.
fn tone(v: f32, e: &Edits, is_raw: bool, curve: &Spline) -> f32 {
    let v = style::zone_develop(v, e.zone_anchor, e.zone_n);
    let v = if is_raw {
        // Soft shoulder so pushed highlights roll off instead of clipping hard.
        if v <= 0.8 { v } else { 0.8 + 0.2 * (1.0 - (-(v - 0.8) / 0.2).exp()) }
    } else {
        v.min(1.0)
    };
    let mut x = linear_to_srgb(v.max(0.0));

    let k = |s: f32| s.clamp(-100.0, 100.0) / 100.0;
    let (wh, bl, hi, sh) = (k(e.whites), k(e.blacks), k(e.highlights), k(e.shadows));
    // RAW files get a gentle base contrast, as every RAW converter does.
    let ct = k(e.contrast) + if is_raw { 0.3 } else { 0.0 };

    if wh != 0.0 {
        x *= 1.0 + 0.25 * wh * x * x * x;
    }
    if bl != 0.0 {
        let y = 1.0 - x;
        x += 0.15 * bl * y * y * y * y;
    }
    x = x.clamp(0.0, 1.0);
    if hi != 0.0 {
        x += 0.14 * 6.75 * hi * x * x * (1.0 - x);
    }
    if sh != 0.0 {
        x += 0.14 * 6.75 * sh * x * (1.0 - x) * (1.0 - x);
    }
    x = x.clamp(0.0, 1.0);
    if ct != 0.0 {
        x -= ct.clamp(-1.0, 1.3) * 0.12 * (2.0 * std::f32::consts::PI * x).sin();
    }
    curve.eval(x.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// Monotone cubic (Fritsch-Carlson) interpolation through curve points.
pub struct Spline {
    xs: Vec<f32>,
    ys: Vec<f32>,
    ms: Vec<f32>,
    identity: bool,
}

impl Spline {
    pub fn new(points: &[[f32; 2]]) -> Spline {
        let mut p: Vec<[f32; 2]> =
            points.iter().map(|q| [q[0].clamp(0.0, 1.0), q[1].clamp(0.0, 1.0)]).collect();
        p.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
        p.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-4);
        let identity = p.len() < 2 || p.iter().all(|q| (q[0] - q[1]).abs() < 1e-6) && p[0][0] == 0.0 && p[p.len() - 1][0] == 1.0;
        if p.len() < 2 {
            return Spline { xs: vec![], ys: vec![], ms: vec![], identity: true };
        }
        let n = p.len();
        let xs: Vec<f32> = p.iter().map(|q| q[0]).collect();
        let ys: Vec<f32> = p.iter().map(|q| q[1]).collect();
        let d: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
        let mut ms = vec![0f32; n];
        ms[0] = d[0];
        ms[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            ms[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
        }
        for i in 0..n - 1 {
            if d[i] == 0.0 {
                ms[i] = 0.0;
                ms[i + 1] = 0.0;
            } else {
                let a = ms[i] / d[i];
                let b = ms[i + 1] / d[i];
                let s = a * a + b * b;
                if s > 9.0 {
                    let t = 3.0 / s.sqrt();
                    ms[i] = t * a * d[i];
                    ms[i + 1] = t * b * d[i];
                }
            }
        }
        Spline { xs, ys, ms, identity }
    }

    pub fn eval(&self, x: f32) -> f32 {
        if self.identity {
            return x;
        }
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let i = self.xs.partition_point(|&v| v <= x).saturating_sub(1).min(n - 2);
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[i]
            + (t3 - 2.0 * t2 + t) * h * self.ms[i]
            + (-2.0 * t3 + 3.0 * t2) * self.ys[i + 1]
            + (t3 - t2) * h * self.ms[i + 1]
    }
}

// -------------------------------------------------------------- colour ----

/// Hue centres (degrees) of the eight HSL bands.
const BANDS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 275.0, 315.0];

struct ColorOps {
    active: bool,
    sat: f32,
    vib: f32,
    hsl: [[f32; 3]; 8],
    any_hsl: bool,
}

impl ColorOps {
    fn new(e: &Edits) -> ColorOps {
        let hsl = e.hsl.map(|b| [b.hue / 100.0, b.sat / 100.0, b.lum / 100.0]);
        let any_hsl = hsl.iter().flatten().any(|&v| v != 0.0);
        ColorOps {
            active: any_hsl || e.saturation != 0.0 || e.vibrance != 0.0,
            sat: e.saturation / 100.0,
            vib: e.vibrance / 100.0,
            hsl,
            any_hsl,
        }
    }

    #[inline]
    fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if !self.active {
            return rgb;
        }
        let (mut h, mut s, mut v) = rgb_to_hsv(rgb);
        if self.any_hsl && s > 0.0 {
            let (mut dh, mut ds, mut dl) = (0.0, 0.0, 0.0);
            for (i, &c) in BANDS.iter().enumerate() {
                let w = band_weight(h, i, c);
                if w > 0.0 {
                    dh += w * self.hsl[i][0];
                    ds += w * self.hsl[i][1];
                    dl += w * self.hsl[i][2];
                }
            }
            h = (h + dh * 30.0).rem_euclid(360.0);
            s *= 1.0 + ds;
            // Luminance mostly affects saturated pixels of that colour.
            v *= 1.0 + dl * 0.5 * s.min(1.0);
        }
        s *= 1.0 + self.sat;
        if self.vib != 0.0 {
            let room = (1.0 - s).clamp(0.0, 1.0);
            s *= 1.0 + self.vib * room * room;
        }
        hsv_to_rgb(h, s.clamp(0.0, 1.0), v.clamp(0.0, 1.0))
    }
}

/// Triangular weight of hue `h` for band `i`, falling to zero at neighbours.
fn band_weight(h: f32, i: usize, c: f32) -> f32 {
    let prev = BANDS[(i + 7) % 8];
    let next = BANDS[(i + 1) % 8];
    let d = (h - c + 540.0).rem_euclid(360.0) - 180.0;
    let span = if d < 0.0 { (c - prev + 360.0).rem_euclid(360.0) } else { (next - c + 360.0).rem_euclid(360.0) };
    (1.0 - d.abs() / span).max(0.0)
}

fn rgb_to_hsv(p: [f32; 3]) -> (f32, f32, f32) {
    let [r, g, b] = p;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 1e-7 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, if max > 0.0 { d / max } else { 0.0 }, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let c = v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

// ------------------------------------------------------------- spatial ----

/// Gaussian blur approximated by three box blurs, one channel.
pub fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma < 0.3 {
        return src.to_vec();
    }
    // Box radius for 3 passes (Kovesi).
    let wideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let r = (((wideal - 1.0) / 2.0).floor() as usize).max(1);
    let mut a = src.to_vec();
    let mut b = vec![0f32; a.len()];
    for _ in 0..3 {
        box_h(&a, &mut b, w, r);
        box_v(&b, &mut a, w, h, r);
    }
    a
}

fn box_h(src: &[f32], dst: &mut [f32], w: usize, r: usize) {
    dst.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(d, s)| {
        let n = (2 * r + 1) as f32;
        let mut acc = s[0] * (r + 1) as f32;
        for k in 1..=r {
            acc += s[k.min(w - 1)];
        }
        for x in 0..w {
            d[x] = acc / n;
            let add = s[(x + r + 1).min(w - 1)];
            let sub = s[x.saturating_sub(r)];
            acc += add - sub;
        }
    });
}

fn box_v(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let n = (2 * r + 1) as f32;
    // Process in column strips for cache friendliness.
    let cols: Vec<usize> = (0..w).collect();
    let results: Vec<(usize, Vec<f32>)> = cols
        .par_chunks(64)
        .map(|chunk| {
            let x0 = chunk[0];
            let cw = chunk.len();
            let mut out = vec![0f32; cw * h];
            for (ci, x) in chunk.iter().enumerate() {
                let at = |y: usize| src[y * w + x];
                let mut acc = at(0) * (r + 1) as f32;
                for y in 1..=r {
                    acc += at(y.min(h - 1));
                }
                for y in 0..h {
                    out[y * cw + ci] = acc / n;
                    acc += at((y + r + 1).min(h - 1)) - at(y.saturating_sub(r));
                }
            }
            (x0, out)
        })
        .collect();
    for (x0, out) in results {
        let cw = out.len() / h;
        for y in 0..h {
            dst[y * w + x0..y * w + x0 + cw].copy_from_slice(&out[y * cw..(y + 1) * cw]);
        }
    }
}

fn luma(p: &[f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn noise_reduction(px: &mut [[f32; 3]], w: usize, h: usize, luma_amt: f32, color_amt: f32, scale: f32) {
    let y: Vec<f32> = px.par_iter().map(luma).collect();
    if color_amt > 0.0 {
        // Blur colour-difference channels; detail lives in luminance.
        let cb: Vec<f32> = px.par_iter().zip(&y).map(|(p, l)| p[2] - l).collect();
        let cr: Vec<f32> = px.par_iter().zip(&y).map(|(p, l)| p[0] - l).collect();
        let sigma = (1.0 + 5.0 * color_amt) * scale;
        let (cb2, cr2) = rayon::join(|| blur(&cb, w, h, sigma), || blur(&cr, w, h, sigma));
        px.par_iter_mut().enumerate().for_each(|(i, p)| {
            let l = y[i];
            let b = l + cb[i] + (cb2[i] - cb[i]) * color_amt.min(1.0);
            let r = l + cr[i] + (cr2[i] - cr[i]) * color_amt.min(1.0);
            // Recover green from luminance.
            let g = (l - 0.2126 * r - 0.0722 * b) / 0.7152;
            *p = [r, g, b];
        });
    }
    if luma_amt > 0.0 {
        // Edge-aware: suppress low-amplitude detail (noise), keep strong edges.
        let yb = blur(&y, w, h, 1.2 * scale.max(0.5));
        let t = 0.01 + 0.04 * luma_amt;
        px.par_iter_mut().enumerate().for_each(|(i, p)| {
            let d = y[i] - yb[i];
            let keep = 1.0 - luma_amt * (-(d * d) / (2.0 * t * t)).exp();
            let delta = d * keep - d;
            p.iter_mut().for_each(|v| *v += delta);
        });
    }
}

/// Halation (colour: red-orange glow scattering around highlights) or bloom
/// (black and white: a soft, luminous diffusion like a printing diffuser).
/// Radii are proportional to the image, so preview and export match.
fn style_glow(px: &mut [[f32; 3]], w: usize, h: usize, amount: f32, mono: bool) {
    let long = w.max(h) as f32;
    let threshold = if mono { 0.45 } else { 0.7 };
    let mask: Vec<f32> = px.par_iter().map(|p| style::smoothstep(threshold, 1.0, style::luma(*p))).collect();
    let sigma = long * if mono { 0.012 } else { 0.006 };
    let b = blur(&mask, w, h, sigma.max(0.5));
    let tint = if mono { [1.0, 1.0, 1.0] } else { [1.0, 0.32, 0.12] };
    let k = amount.clamp(0.0, 1.5) * if mono { 0.5 } else { 0.8 };
    px.par_iter_mut().zip(b.par_iter()).for_each(|(p, &g)| {
        for c in 0..3 {
            let a = (g * tint[c] * k).min(1.0);
            // Screen blend: light adds without clipping hard.
            p[c] = 1.0 - (1.0 - p[c].clamp(0.0, 1.0)) * (1.0 - a);
        }
    });
}

/// Film grain: deterministic noise, clumped to a size relative to the frame,
/// strongest in the midtones as on film. Colour film adds a little chroma grain.
fn film_grain(px: &mut [[f32; 3]], w: usize, h: usize, amount: f32, size: f32, mono: bool) {
    let long = w.max(h) as f32;
    // Size 1 ≈ 35 mm 400-speed film: grain clumps ~1/3000 of the frame.
    let sigma = size.clamp(0.25, 3.0) * long / 3000.0;
    let field = |seed: u32| -> Vec<f32> {
        let n: Vec<f32> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % w) as u32, (i / w) as u32);
                style::hash_noise(x, y, seed) + style::hash_noise(x, y, seed ^ 0x9e37)
            })
            .collect();
        if sigma < 0.6 {
            return n;
        }
        let b = blur(&n, w, h, sigma);
        // Re-normalise: blurring shrinks the variance.
        let var = b.iter().map(|v| v * v).sum::<f32>() / b.len().max(1) as f32;
        let k = 0.8 / var.sqrt().max(1e-6);
        b.into_iter().map(|v| v * k).collect()
    };
    let lum = field(17);
    let chroma = if mono { None } else { Some((field(29), field(43))) };
    let a = amount.clamp(0.0, 1.0) * 0.11;
    px.par_iter_mut().enumerate().for_each(|(i, p)| {
        let l = style::luma(*p).clamp(0.0, 1.0);
        let strength = a * (4.0 * l * (1.0 - l)).max(0.0).powf(0.6) + a * 0.08;
        let g = lum[i] * strength;
        match &chroma {
            None => p.iter_mut().for_each(|v| *v = (*v + g).clamp(0.0, 1.0)),
            Some((c1, c2)) => {
                let (d1, d2) = (c1[i] * strength * 0.25, c2[i] * strength * 0.25);
                p[0] = (p[0] + g + d1).clamp(0.0, 1.0);
                p[1] = (p[1] + g - 0.5 * (d1 + d2)).clamp(0.0, 1.0);
                p[2] = (p[2] + g + d2).clamp(0.0, 1.0);
            }
        }
    });
}

/// Darkroom edge burning: extra print exposure toward the borders, following
/// the frame's rectangle (unlike a round lens vignette).
fn edge_burn(px: &mut [[f32; 3]], w: usize, h: usize, amount: f32) {
    let band = 0.25;
    px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let ny = (y as f32 + 0.5) / h as f32;
        let by = 1.0 - style::smoothstep(0.0, band, ny.min(1.0 - ny));
        for (x, p) in row.iter_mut().enumerate() {
            let nx = (x as f32 + 0.5) / w as f32;
            let bx = 1.0 - style::smoothstep(0.0, band * h as f32 / w as f32 * 1.2, nx.min(1.0 - nx));
            let burn = 1.0 - (1.0 - bx.powf(1.5)) * (1.0 - by.powf(1.5));
            // One stop at full strength, applied in linear light like real exposure.
            let gain = 2f32.powf(-amount.clamp(0.0, 1.0) * burn);
            for v in p.iter_mut() {
                *v = linear_to_srgb(srgb_to_linear(v.clamp(0.0, 1.0)) * gain);
            }
        }
    });
}

fn sharpen(px: &mut [[f32; 3]], w: usize, h: usize, amount: f32, radius: f32) {
    let y: Vec<f32> = px.par_iter().map(luma).collect();
    let yb = blur(&y, w, h, radius.max(0.3));
    px.par_iter_mut().enumerate().for_each(|(i, p)| {
        let d = (y[i] - yb[i]) * amount;
        p.iter_mut().for_each(|v| *v = (*v + d).clamp(0.0, 1.0));
    });
}

#[cfg(test)]
mod tests {
    use super::super::source::{from_rendered, srgb_to_linear};
    use super::*;
    use crate::develop::{Crop, Hsl};

    fn gradient() -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8])
        }))
    }

    #[test]
    fn identity_edit_reproduces_rendered_source() {
        let src = gradient();
        let lin = from_rendered(&src);
        let (out, _) = render_image(&lin, &Edits::default(), &RenderOpts::default());
        let a = src.to_rgb8();
        let b = out.to_rgb8();
        let max = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap();
        assert!(max <= 1, "max diff {max}");
    }

    #[test]
    fn exposure_brightens_and_curve_is_monotone() {
        let lin = from_rendered(&gradient());
        let mean = |e: &Edits| {
            let r = render(&lin, e, &RenderOpts::default());
            r.px.iter().map(|p| p[1]).sum::<f32>() / r.px.len() as f32
        };
        let base = mean(&Edits::default());
        assert!(mean(&Edits { exposure: 1.0, ..Default::default() }) > base + 0.05);
        assert!(mean(&Edits { exposure: -1.0, ..Default::default() }) < base - 0.05);
        for e in [
            Edits { contrast: 100.0, ..Default::default() },
            Edits { contrast: -100.0, highlights: -100.0, shadows: 100.0, ..Default::default() },
            Edits { whites: 100.0, blacks: -100.0, highlights: 100.0, shadows: -100.0, ..Default::default() },
        ] {
            let curve = Spline::new(&e.curve);
            let mut prev = -1.0;
            for i in 0..=1000 {
                let v = tone(srgb_to_linear(i as f32 / 1000.0), &e, false, &curve);
                assert!(v >= prev - 1e-4, "not monotone at {i}: {v} < {prev} for {e:?}");
                prev = v;
            }
        }
    }

    #[test]
    fn spline_hits_points_and_is_monotone() {
        let s = Spline::new(&[[0.0, 0.0], [0.25, 0.15], [0.75, 0.9], [1.0, 1.0]]);
        assert!((s.eval(0.25) - 0.15).abs() < 1e-5);
        assert!((s.eval(0.75) - 0.9).abs() < 1e-5);
        let mut prev = -1.0;
        for i in 0..=100 {
            let v = s.eval(i as f32 / 100.0);
            assert!(v >= prev);
            prev = v;
        }
        assert_eq!(Spline::new(&[[0.0, 0.0], [1.0, 1.0]]).eval(0.37), 0.37);
    }

    #[test]
    fn saturation_zero_is_grey_and_hsl_targets_band() {
        let mut lin = Linear::new(2, 1, false);
        lin.px = vec![[0.6, 0.05, 0.05], [0.05, 0.05, 0.6]];
        let r = render(&lin, &Edits { saturation: -100.0, ..Default::default() }, &RenderOpts::default());
        for p in &r.px {
            assert!((p[0] - p[1]).abs() < 1e-4 && (p[1] - p[2]).abs() < 1e-4);
        }
        let mut hsl = [Hsl::default(); 8];
        hsl[0].sat = -100.0; // desaturate reds only
        let r = render(&lin, &Edits { hsl, ..Default::default() }, &RenderOpts::default());
        assert!((r.px[0][0] - r.px[0][1]).abs() < 1e-3, "red not desaturated: {:?}", r.px[0]);
        assert!(r.px[1][2] > r.px[1][0] + 0.3, "blue affected: {:?}", r.px[1]);
    }

    #[test]
    fn geometry_sizes() {
        let lin = from_rendered(&gradient());
        let e = Edits { rotate: 1, crop: Some(Crop { x: 0.25, y: 0.0, w: 0.5, h: 0.5 }), angle: 3.0, ..Default::default() };
        let r = render(&lin, &e, &RenderOpts::default());
        assert_eq!((r.w, r.h), output_size(64, 48, &e));
        assert_eq!((r.w, r.h), (24, 32));
        let r = render(&lin, &e, &RenderOpts { ignore_crop: true, ..Default::default() });
        assert_eq!((r.w, r.h), (48, 64));
    }

    #[test]
    fn zone_placement_lands_in_the_chosen_zone() {
        let lin = from_rendered(&gradient());
        let looks = [
            Edits::default(),
            crate::develop::looks::apply("platinum", &Edits::default(), 100.0, &Default::default()),
            crate::develop::looks::apply("vivid-slide", &Edits::default(), 100.0, &Default::default()),
        ];
        for base in looks {
            for zone in [3u8, 5, 8] {
                let e = place_zone(&lin, &base, 0.5, 0.5, zone);
                let got = style::zone_of(srgb_to_linear(style::luma(probe(&lin, &e, 0.5, 0.5))));
                assert_eq!(got, zone, "placed in {zone}, landed in {got} (exposure {})", e.exposure);
                assert_eq!(e.zone_anchor, Some(zone));
            }
        }
    }

    #[test]
    fn mono_and_styles_render() {
        let lin = from_rendered(&gradient());
        let e = crate::develop::looks::apply("street", &Edits::default(), 100.0, &Default::default());
        let r = render(&lin, &e, &RenderOpts::default());
        // Untoned black and white is neutral.
        assert!(r.px.iter().all(|p| (p[0] - p[1]).abs() < 1e-4 && (p[1] - p[2]).abs() < 1e-4));
        let z = render(&lin, &Edits::default(), &RenderOpts { zones: true, ..Default::default() });
        let levels: std::collections::BTreeSet<u8> = z.px.iter().map(|p| (p[0] * 255.0) as u8).collect();
        assert!(levels.len() <= 11, "{levels:?}");
    }

    #[test]
    fn spatial_filters_run() {
        let lin = from_rendered(&gradient());
        let e = Edits { sharpen: 80.0, nr_luma: 50.0, nr_color: 50.0, ..Default::default() };
        let r = render(&lin, &e, &RenderOpts { clip_warnings: true, ..Default::default() });
        assert!(r.px.iter().all(|p| p.iter().all(|v| v.is_finite())));
        let flat = vec![0.5f32; 100 * 80];
        let b = blur(&flat, 100, 80, 3.0);
        assert!(b.iter().all(|v| (v - 0.5).abs() < 1e-5));
    }
}
