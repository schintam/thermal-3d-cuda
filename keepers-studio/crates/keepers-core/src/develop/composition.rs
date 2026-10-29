//! Composition analysis without machine learning: where the eye goes
//! (saliency from colour distinctiveness, focus and a mild centre bias),
//! crop suggestions that place the subject on classical guides (thirds,
//! golden section, centre) at common print and screen aspect ratios, and
//! horizon detection for levelling.

use super::pipeline::{blur, Rendered};
use super::style::luma;
use super::Crop;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Guide {
    Thirds,
    Golden,
    Centre,
    Level,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub label: String,
    pub guide: Guide,
    /// Crop in the straightened frame (`None` for levelling only).
    pub crop: Option<Crop>,
    /// Extra rotation to apply, degrees (levelling).
    pub angle: f32,
    /// Subject position inside the suggested frame, normalised.
    pub subject: [f32; 2],
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Composition {
    pub suggestions: Vec<Suggestion>,
    /// Subject centre in the full straightened frame, normalised.
    pub subject: [f32; 2],
    /// Detected horizon tilt correction, degrees, if confident.
    pub level: Option<f32>,
}

/// Saliency map in 0..1, same size as the image.
pub fn saliency(img: &Rendered) -> Vec<f32> {
    let (w, h) = (img.w, img.h);
    let n = w * h;
    // Opponent colour space.
    let l: Vec<f32> = img.px.iter().map(|p| luma(*p)).collect();
    let a: Vec<f32> = img.px.iter().map(|p| p[0] - p[1]).collect();
    let b: Vec<f32> = img.px.iter().map(|p| (p[0] + p[1]) * 0.5 - p[2]).collect();
    let sig = (w.max(h) as f32 / 300.0).max(0.6);
    let (lb, ab, bb) = (blur(&l, w, h, sig), blur(&a, w, h, sig), blur(&b, w, h, sig));
    let mean = |v: &[f32]| v.iter().sum::<f32>() / n.max(1) as f32;
    let (ml, ma, mb) = (mean(&lb), mean(&ab), mean(&bb));
    // Frequency-tuned saliency (Achanta et al. 2009): distance from the mean colour.
    let ft: Vec<f32> = (0..n).map(|i| ((lb[i] - ml).powi(2) + 2.0 * (ab[i] - ma).powi(2) + 2.0 * (bb[i] - mb).powi(2)).sqrt()).collect();
    // Focus: local detail energy (what the photographer focused on is sharp).
    let mut lap = vec![0f32; n];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            lap[i] = (4.0 * l[i] - l[i - 1] - l[i + 1] - l[i - w] - l[i + w]).abs();
        }
    }
    let focus = blur(&lap, w, h, sig * 4.0);
    let norm = |v: Vec<f32>| {
        let mut sorted: Vec<f32> = v.iter().copied().step_by(7).collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let hi = sorted.get(sorted.len() * 99 / 100).copied().unwrap_or(1.0).max(1e-6);
        v.into_iter().map(|x| (x / hi).min(1.0)).collect::<Vec<f32>>()
    };
    let (ft, focus) = (norm(ft), norm(focus));
    (0..n)
        .map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32 - 0.5, (i / w) as f32 / h as f32 - 0.5);
            let centre = (-(x * x + y * y) / (2.0 * 0.3 * 0.3)).exp();
            let s = 0.45 * ft[i] + 0.4 * focus[i] + 0.15 * centre;
            s * s
        })
        .collect()
}

/// Summed-area table of `v` (and of v·x, v·y for centroids).
struct Integral {
    w: usize,
    s: Vec<f64>,
    sx: Vec<f64>,
    sy: Vec<f64>,
}

impl Integral {
    fn new(v: &[f32], w: usize, h: usize) -> Integral {
        let (mut s, mut sx, mut sy) = (vec![0f64; (w + 1) * (h + 1)], vec![0f64; (w + 1) * (h + 1)], vec![0f64; (w + 1) * (h + 1)]);
        for y in 0..h {
            let (mut r, mut rx, mut ry) = (0f64, 0f64, 0f64);
            for x in 0..w {
                let val = v[y * w + x] as f64;
                r += val;
                rx += val * x as f64;
                ry += val * y as f64;
                let i = (y + 1) * (w + 1) + x + 1;
                s[i] = s[i - (w + 1)] + r;
                sx[i] = sx[i - (w + 1)] + rx;
                sy[i] = sy[i - (w + 1)] + ry;
            }
        }
        Integral { w: w + 1, s, sx, sy }
    }

    fn rect(t: &[f64], w: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> f64 {
        t[y1 * w + x1] - t[y0 * w + x1] - t[y1 * w + x0] + t[y0 * w + x0]
    }

    fn sum(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> f64 {
        Self::rect(&self.s, self.w, x0, y0, x1, y1)
    }

    fn centroid(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Option<(f64, f64)> {
        let s = self.sum(x0, y0, x1, y1);
        (s > 1e-9).then(|| (Self::rect(&self.sx, self.w, x0, y0, x1, y1) / s, Self::rect(&self.sy, self.w, x0, y0, x1, y1) / s))
    }
}

fn targets(g: Guide) -> &'static [(f32, f32)] {
    const T: f32 = 1.0 / 3.0;
    const G: f32 = 0.381_966;
    match g {
        Guide::Thirds => &[(T, T), (1.0 - T, T), (T, 1.0 - T), (1.0 - T, 1.0 - T)],
        Guide::Golden => &[(G, G), (1.0 - G, G), (G, 1.0 - G), (1.0 - G, 1.0 - G)],
        Guide::Centre | Guide::Level => &[(0.5, 0.5)],
    }
}

/// Analyse the straightened, uncropped frame (a small render is enough).
pub fn analyze(img: &Rendered) -> Composition {
    let (w, h) = (img.w, img.h);
    let sal = saliency(img);
    let ii = Integral::new(&sal, w, h);
    // Subject: centroid of the most salient few percent of the image,
    // weighted toward the peak (long edges like a horizon are salient but
    // rarely the subject).
    let mut sorted: Vec<f32> = sal.iter().copied().collect();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let thresh = sorted.get(sorted.len() / 25).copied().unwrap_or(0.0);
    let (mut sx, mut sy, mut sw) = (0f64, 0f64, 0f64);
    for (i, &s) in sal.iter().enumerate() {
        if s >= thresh {
            let wgt = (s as f64).powi(2);
            sx += wgt * (i % w) as f64;
            sy += wgt * (i / w) as f64;
            sw += wgt;
        }
    }
    let subject = if sw > 0.0 { [(sx / sw / w as f64) as f32, (sy / sw / h as f64) as f32] } else { [0.5, 0.5] };
    // Subject mass: just the salient peak, for "keep the subject" and placement.
    let peak: Vec<f32> = sal.iter().map(|&s| if s >= thresh { s * s } else { 0.0 }).collect();
    let pk = Integral::new(&peak, w, h);
    let peak_total = pk.sum(0, 0, w, h).max(1e-9);

    let img_r = w as f32 / h as f32;
    let landscape = img_r >= 1.0;
    let mut plans: Vec<(String, Guide, f32, &[f32])> = vec![
        (aspect_name(img_r), Guide::Thirds, img_r, &[1.0, 0.85, 0.72]),
        (aspect_name(img_r), Guide::Golden, img_r, &[1.0, 0.85, 0.72]),
        ("1:1".into(), Guide::Centre, 1.0, &[1.0, 0.85]),
        ("4:5".into(), Guide::Thirds, 0.8, &[1.0, 0.85]),
        ("Tight".into(), Guide::Thirds, img_r, &[0.62, 0.55]),
    ];
    if landscape {
        plans.push(("16:9".into(), Guide::Thirds, 16.0 / 9.0, &[1.0, 0.9]));
        plans.push(("2.39:1".into(), Guide::Golden, 2.39, &[1.0, 0.9]));
    } else {
        plans.push(("2:3".into(), Guide::Golden, 2.0 / 3.0, &[1.0, 0.85]));
    }

    let mut out: Vec<Suggestion> = Vec::new();
    for (aspect, guide, r, scales) in plans {
        let mut best: Option<Suggestion> = None;
        for &k in scales {
            // Largest window of ratio r, scaled.
            let (mut cw, mut ch) = if r * h as f32 <= w as f32 { (r * h as f32, h as f32) } else { (w as f32, w as f32 / r) };
            cw *= k;
            ch *= k;
            let (cw, ch) = (cw.round().max(8.0) as usize, ch.round().max(8.0) as usize);
            if cw > w || ch > h {
                continue;
            }
            let steps = 24;
            for yi in 0..=steps {
                for xi in 0..=steps {
                    let x0 = (w - cw) * xi / steps;
                    let y0 = (h - ch) * yi / steps;
                    let (x1, y1) = (x0 + cw, y0 + ch);
                    let inside = pk.sum(x0, y0, x1, y1) / peak_total;
                    if inside < 0.6 {
                        continue;
                    }
                    let Some((cx, cy)) = pk.centroid(x0, y0, x1, y1) else { continue };
                    let (u, v) = (((cx - x0 as f64) / cw as f64) as f32, ((cy - y0 as f64) / ch as f64) as f32);
                    let place = targets(guide)
                        .iter()
                        .map(|&(tx, ty)| (-((u - tx).powi(2) + (v - ty).powi(2)) / (2.0 * 0.05 * 0.05)).exp())
                        .fold(0.0, f32::max);
                    // Don't slice through the subject: penalise salient mass on the frame edge.
                    let bw = (cw.min(ch) / 20).max(1);
                    let area = (cw * ch) as f64;
                    let edge = ii.sum(x0, y0, x1, y1) - ii.sum(x0 + bw, y0 + bw, x1 - bw, y1 - bw);
                    let edge_area = area - ((cw - 2 * bw) * (ch - 2 * bw)) as f64;
                    let mean_in = ii.sum(x0, y0, x1, y1) / area;
                    let edge_ratio = ((edge / edge_area.max(1.0)) / mean_in.max(1e-9)) as f32;
                    let score = 0.35 * inside as f32 + 0.5 * place - 0.25 * (edge_ratio - 0.6).max(0.0) + 0.08 * k;
                    if best.as_ref().is_none_or(|b| score > b.score) {
                        best = Some(Suggestion {
                            label: String::new(),
                            guide,
                            crop: Some(Crop { x: x0 as f32 / w as f32, y: y0 as f32 / h as f32, w: cw as f32 / w as f32, h: ch as f32 / h as f32 }),
                            angle: 0.0,
                            subject: [u, v],
                            score,
                        });
                    }
                }
            }
        }
        if let Some(mut b) = best {
            b.label = match (aspect.as_str(), guide) {
                ("Tight", _) => "Tighter · subject on a third".to_string(),
                (a, Guide::Thirds) => format!("Rule of thirds · {a}"),
                (a, Guide::Golden) => format!("Golden section · {a}"),
                (a, _) => format!("Centred · {a}"),
            };
            if !out.iter().any(|o| iou(o.crop.as_ref().unwrap(), b.crop.as_ref().unwrap()) > 0.9) {
                out.push(b);
            }
        }
    }
    let level = detect_level(img);
    if let Some(a) = level {
        out.insert(0, Suggestion { label: format!("Level horizon ({a:+.1}°)"), guide: Guide::Level, crop: None, angle: a, subject, score: 1.0 });
    }
    Composition { suggestions: out, subject, level }
}

fn aspect_name(r: f32) -> String {
    for (name, v) in [("3:2", 1.5), ("2:3", 2.0 / 3.0), ("4:3", 4.0 / 3.0), ("3:4", 0.75), ("1:1", 1.0), ("16:9", 16.0 / 9.0), ("5:4", 1.25), ("4:5", 0.8)] {
        if (r - v).abs() < 0.02 {
            return name.into();
        }
    }
    "original".into()
}

fn iou(a: &Crop, b: &Crop) -> f32 {
    let ix = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let iy = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if ix <= 0.0 || iy <= 0.0 {
        return 0.0;
    }
    let i = ix * iy;
    i / (a.w * a.h + b.w * b.h - i)
}

/// Find a dominant near-horizontal line (horizon, sea, architecture) and
/// return the straighten angle that levels it, if clearly tilted.
pub fn detect_level(img: &Rendered) -> Option<f32> {
    let (w, h) = (img.w, img.h);
    if w < 32 || h < 32 {
        return None;
    }
    let l: Vec<f32> = img.px.iter().map(|p| luma(*p)).collect();
    // Strong, mostly-horizontal edges.
    let mut pts: Vec<(f32, f32, f32)> = Vec::new();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let gx = (l[i - w + 1] + 2.0 * l[i + 1] + l[i + w + 1]) - (l[i - w - 1] + 2.0 * l[i - 1] + l[i + w - 1]);
            let gy = (l[i + w - 1] + 2.0 * l[i + w] + l[i + w + 1]) - (l[i - w - 1] + 2.0 * l[i - w] + l[i - w + 1]);
            if gy.abs() > 0.15 && gy.abs() > 3.0 * gx.abs() {
                pts.push((x as f32, y as f32, gy.abs()));
            }
        }
    }
    if pts.len() < w / 4 {
        return None;
    }
    // Projection-profile sharpness for each candidate tilt.
    let energy = |deg: f32| -> f32 {
        let t = deg.to_radians();
        let (s, c) = (t.sin(), t.cos());
        let n = h + w;
        let mut bins = vec![0f32; n * 2];
        for &(x, y, g) in &pts {
            let r = y * c - x * s + w as f32;
            let i = (r as usize).min(bins.len() - 1);
            bins[i] += g;
        }
        bins.iter().map(|b| b * b).sum()
    };
    let e0 = energy(0.0);
    let mut best = (0.0f32, e0);
    let mut d = -10.0f32;
    while d <= 10.0 {
        let e = energy(d);
        if e > best.1 {
            best = (d, e);
        }
        d += 0.2;
    }
    // A line tilted by `best` degrees is levelled by rotating the opposite way.
    (best.0.abs() >= 0.4 && best.1 > e0 * 1.25).then(|| -best.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::source::Linear;
    use crate::develop::{pipeline, Edits, RenderOpts};

    fn scene(w: usize, h: usize, tilt_deg: f32, subject: (f32, f32)) -> Rendered {
        let t = tilt_deg.to_radians().tan();
        let px = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let horizon = h as f32 * 0.55 + (x - w as f32 / 2.0) * t;
                let mut p = if y < horizon { [0.55, 0.65, 0.85] } else { [0.25, 0.35, 0.2] };
                let (sx, sy) = (subject.0 * w as f32, subject.1 * h as f32);
                if (x - sx).powi(2) + (y - sy).powi(2) < (h as f32 * 0.07).powi(2) {
                    // A red, detailed subject.
                    p = if (x as i32 + y as i32) % 2 == 0 { [0.9, 0.15, 0.1] } else { [0.6, 0.05, 0.05] };
                }
                p
            })
            .collect();
        Rendered { w, h, px }
    }

    #[test]
    fn finds_subject_and_places_it_on_guides() {
        let img = scene(300, 200, 0.0, (0.7, 0.4));
        let c = analyze(&img);
        assert!((c.subject[0] - 0.7).abs() < 0.08 && (c.subject[1] - 0.4).abs() < 0.08, "{:?}", c.subject);
        let thirds = c.suggestions.iter().find(|s| s.guide == Guide::Thirds).unwrap();
        let near_third = [1.0 / 3.0, 2.0 / 3.0].iter().any(|t| (thirds.subject[0] - t).abs() < 0.08);
        assert!(near_third, "{thirds:?}");
        assert!(c.suggestions.iter().any(|s| s.guide == Guide::Centre));
        assert!(c.level.is_none(), "level on a level image: {:?}", c.level);
        for s in &c.suggestions {
            let k = s.crop.unwrap();
            assert!(k.x >= 0.0 && k.y >= 0.0 && k.x + k.w <= 1.001 && k.y + k.h <= 1.001, "{s:?}");
        }
    }

    #[test]
    fn levels_a_tilted_horizon() {
        let img = scene(400, 260, 3.0, (0.3, 0.3));
        let a = detect_level(&img).expect("tilt detected");
        // Applying the suggestion through the real pipeline levels the image.
        let lin = Linear { w: img.w, h: img.h, px: img.px.iter().map(|p| p.map(crate::develop::source::srgb_to_linear)).collect(), is_raw: false };
        let fixed = pipeline::render(&lin, &Edits { angle: a, ..Default::default() }, &RenderOpts::default());
        let after = detect_level(&fixed);
        assert!(after.is_none_or(|b| b.abs() < 0.8), "suggested {a}, residual {after:?}");
    }
}
