//! Photographic craft, modelled rather than faked: black-and-white film and
//! lens filters, darkroom paper grades and split-grade printing, chemical
//! toners, colour film renditions, film grain, halation and glow, print edge
//! burning, colour harmony and Ansel Adams' Zone System.
//!
//! Everything is deterministic image processing; no models, no network.

use super::source::{linear_to_srgb, srgb_to_linear};
use serde::{Deserialize, Serialize};

// ------------------------------------------------------- black & white ----

/// Spectral character of the black-and-white film being emulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MonoFilm {
    /// Luminance as the eye sees it.
    #[default]
    Panchromatic,
    /// Classic 400-speed cubic-grain film: a little extra red sensitivity,
    /// punchy midtones, long toe.
    ClassicPan,
    /// Modern tabular-grain film: smooth, straight curve, gentle highlights.
    FineGrain,
    /// Orthochromatic: blind to red, so lips and skin darken and skies pale.
    Ortho,
    /// Infrared: foliage glows white, blue skies go nearly black.
    Infrared,
}

/// Coloured filter on the lens, as used with black-and-white film.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LensFilter {
    #[default]
    None,
    /// Natural sky separation.
    Yellow,
    /// Darker skies, smoother skin.
    Orange,
    /// Dramatic, near-black skies.
    Red,
    /// Lighter foliage, ruddier skin.
    Green,
    /// Haze and atmosphere.
    Blue,
}

/// Chemical toner applied to the print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Toner {
    #[default]
    None,
    /// Deepens and cools the shadows first.
    Selenium,
    /// Warms highlights and midtones to brown.
    Sepia,
    /// Cool blue-black highlights.
    Gold,
    /// Warm neutral image with soft blacks, like platinum/palladium.
    Platinum,
    /// Prussian-blue cyanotype.
    Cyanotype,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Mono {
    pub enabled: bool,
    /// 0..100: how far the photo is converted (100 = fully monochrome).
    pub amount: f32,
    pub film: MonoFilm,
    pub filter: LensFilter,
    /// Paper contrast grade, 0 (soft) ..5 (hard); 2 is normal.
    pub grade: f32,
    /// Split-grade printing, -100..100: extra soft-filter exposure (highlight
    /// density) and hard-filter exposure (shadow depth).
    pub soft: f32,
    pub hard: f32,
    pub toner: Toner,
    /// 0..100.
    pub toner_amount: f32,
}

impl Default for Mono {
    fn default() -> Self {
        Mono {
            enabled: false,
            amount: 100.0,
            film: MonoFilm::Panchromatic,
            filter: LensFilter::None,
            grade: 2.0,
            soft: 0.0,
            hard: 0.0,
            toner: Toner::None,
            toner_amount: 0.0,
        }
    }
}

/// Weights mixing linear R, G, B into the film's response, after the lens
/// filter. Normalised so a neutral grey keeps its value (the photographer
/// compensates the filter factor with exposure).
pub fn mono_weights(film: MonoFilm, filter: LensFilter) -> [f32; 3] {
    let base = match film {
        MonoFilm::Panchromatic => [0.2126, 0.7152, 0.0722],
        MonoFilm::ClassicPan => [0.30, 0.59, 0.11],
        MonoFilm::FineGrain => [0.25, 0.62, 0.13],
        MonoFilm::Ortho => [0.03, 0.57, 0.40],
        MonoFilm::Infrared => [0.35, 1.0, -0.35],
    };
    let t = match filter {
        LensFilter::None => [1.0, 1.0, 1.0],
        LensFilter::Yellow => [1.0, 0.92, 0.45],
        LensFilter::Orange => [1.0, 0.65, 0.15],
        LensFilter::Red => [1.0, 0.22, 0.04],
        LensFilter::Green => [0.35, 1.0, 0.40],
        LensFilter::Blue => [0.25, 0.50, 1.0],
    };
    let w = [base[0] * t[0], base[1] * t[1], base[2] * t[2]];
    let sum: f32 = w.iter().sum();
    let sum = if sum.abs() < 0.05 { 0.05 } else { sum };
    w.map(|v| v / sum)
}

/// Characteristic curve of the film plus the paper it is printed on, in
/// display-encoded space. Built as a 1D table by the pipeline.
pub fn mono_print_curve(x: f32, m: &Mono) -> f32 {
    // Film character.
    let (contrast, toe, shoulder) = match m.film {
        MonoFilm::Panchromatic => (0.0, 0.0, 0.0),
        MonoFilm::ClassicPan => (0.22, 0.0, 0.15),
        MonoFilm::FineGrain => (0.08, 0.0, 0.35),
        MonoFilm::Ortho => (0.15, 0.0, 0.1),
        MonoFilm::Infrared => (0.25, 0.0, 0.2),
    };
    let x = film_curve(x, contrast, toe, shoulder);

    // Paper: work in print density, D = -log10(reflectance).
    let lin = srgb_to_linear(x).max(1e-4);
    let d = -lin.log10();
    let d_mid = -(0.18f32).log10();
    let gamma = 0.6 + 0.2 * m.grade.clamp(0.0, 5.0);
    let mut d2 = d_mid + (d - d_mid) * gamma;
    // Split grade: the soft filter prints the highlights, the hard filter the shadows.
    let hi_w = 1.0 - smoothstep(0.2, 1.1, d);
    let sh_w = smoothstep(0.5, 1.7, d);
    d2 += m.soft.clamp(-100.0, 100.0) / 100.0 * 0.22 * hi_w;
    d2 += m.hard.clamp(-100.0, 100.0) / 100.0 * 0.35 * sh_w;
    // Glossy fibre paper: paper white to Dmax ~2.3.
    let d2 = d2.clamp(0.0, 2.3);
    linear_to_srgb(10f32.powf(-d2).min(1.0))
}

/// Toner colour for a monochrome value `l` (encoded). Returns RGB.
pub fn tone_print(l: f32, toner: Toner, amount: f32) -> [f32; 3] {
    let a = (amount / 100.0).clamp(0.0, 1.0);
    if a == 0.0 || toner == Toner::None {
        return [l, l, l];
    }
    let mix = |tint: [f32; 3], w: f32, l: f32| tint.map(|t| l * (1.0 + (t - 1.0) * w));
    match toner {
        Toner::None => [l, l, l],
        Toner::Selenium => {
            let s = (1.0 - l).powi(2);
            let l2 = l * (1.0 - 0.15 * a * s);
            mix([0.94, 0.90, 0.97], a * s * 1.2, l2)
        }
        Toner::Sepia => {
            let w = a * (1.0 - (1.0 - l).powi(3));
            mix([1.10, 0.97, 0.78], w, l)
        }
        Toner::Gold => mix([0.94, 0.98, 1.07], a * l, l),
        Toner::Platinum => {
            // Platinum's matte surface can't hold a deep black.
            let l2 = 0.06 * a + l * (1.0 - 0.06 * a);
            mix([1.06, 1.0, 0.89], a * 0.9, l2)
        }
        Toner::Cyanotype => {
            let dark = [0.02, 0.10, 0.27];
            let light = [0.93, 0.97, 1.0];
            let c = [0, 1, 2].map(|i| dark[i] + (light[i] - dark[i]) * l.powf(0.9));
            [0, 1, 2].map(|i| l + (c[i] - l) * a)
        }
    }
}

// --------------------------------------------------------- colour film ----

/// Colour film renditions. Named for what they do rather than for products.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorStock {
    #[default]
    None,
    /// Gentle contrast, flattering skin, pastel greens.
    PortraitNegative,
    /// Saturated, contrasty transparency film for landscapes.
    VividSlide,
    /// Deep reds and yellows, dense blues, rich contrast.
    ClassicSlide,
    /// Tungsten-balanced motion-picture film: cool, with red halation.
    TungstenCine,
    /// An aged print: lifted blacks, softened colour, warm cast.
    FadedPrint,
    /// Restrained palette that keeps earth tones and calms the rest.
    MutedEarth,
}

struct StockParams {
    contrast: f32,
    toe: f32,
    shoulder: f32,
    sat: f32,
    /// Channel gammas: different curve shape per layer gives film's crossover.
    gamma: [f32; 3],
    shadow_tint: [f32; 3],
    high_tint: [f32; 3],
    /// (hue centre, half width, hue shift, saturation multiplier)
    twists: &'static [(f32, f32, f32, f32)],
    /// Red glow around highlights from light scattering off the film base.
    halation: f32,
}

fn stock_params(s: ColorStock) -> Option<StockParams> {
    Some(match s {
        ColorStock::None => return None,
        ColorStock::PortraitNegative => StockParams {
            contrast: -0.12,
            toe: 0.025,
            shoulder: 0.3,
            sat: 0.9,
            gamma: [0.98, 1.0, 1.03],
            shadow_tint: [0.98, 1.0, 1.01],
            high_tint: [1.03, 1.0, 0.96],
            twists: &[(120.0, 50.0, -10.0, 0.8), (25.0, 25.0, 3.0, 0.95)],
            halation: 0.08,
        },
        ColorStock::VividSlide => StockParams {
            contrast: 0.35,
            toe: -0.01,
            shoulder: 0.1,
            sat: 1.35,
            gamma: [1.0, 1.0, 0.97],
            shadow_tint: [0.97, 0.99, 1.04],
            high_tint: [1.0, 1.0, 1.0],
            twists: &[(220.0, 40.0, 6.0, 1.15), (110.0, 40.0, 5.0, 1.1)],
            halation: 0.0,
        },
        ColorStock::ClassicSlide => StockParams {
            contrast: 0.25,
            toe: 0.0,
            shoulder: 0.15,
            sat: 1.12,
            gamma: [0.96, 1.0, 1.06],
            shadow_tint: [1.0, 0.99, 1.0],
            high_tint: [1.03, 1.01, 0.95],
            twists: &[(0.0, 30.0, -4.0, 1.25), (50.0, 25.0, -3.0, 1.15), (215.0, 35.0, -8.0, 0.95)],
            halation: 0.0,
        },
        ColorStock::TungstenCine => StockParams {
            contrast: 0.1,
            toe: 0.015,
            shoulder: 0.2,
            sat: 1.0,
            gamma: [1.02, 1.0, 0.96],
            shadow_tint: [0.95, 1.0, 1.04],
            high_tint: [0.96, 1.0, 1.06],
            twists: &[(30.0, 30.0, -6.0, 1.05), (190.0, 40.0, -8.0, 1.1)],
            halation: 0.55,
        },
        ColorStock::FadedPrint => StockParams {
            contrast: -0.25,
            toe: 0.08,
            shoulder: 0.35,
            sat: 0.78,
            gamma: [0.95, 1.0, 1.05],
            shadow_tint: [1.03, 0.98, 1.02],
            high_tint: [1.04, 1.01, 0.93],
            twists: &[(200.0, 60.0, 10.0, 0.85)],
            halation: 0.12,
        },
        ColorStock::MutedEarth => StockParams {
            contrast: 0.05,
            toe: 0.02,
            shoulder: 0.2,
            sat: 0.72,
            gamma: [1.0, 1.0, 1.0],
            shadow_tint: [1.0, 1.0, 0.99],
            high_tint: [1.02, 1.0, 0.97],
            twists: &[(30.0, 30.0, 0.0, 1.3), (110.0, 50.0, -15.0, 0.85), (220.0, 50.0, 0.0, 0.8)],
            halation: 0.0,
        },
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FilmLook {
    pub stock: ColorStock,
    /// 0..100.
    pub strength: f32,
}

impl Default for FilmLook {
    fn default() -> Self {
        FilmLook { stock: ColorStock::None, strength: 100.0 }
    }
}

pub struct StockOp {
    p: StockParams,
    strength: f32,
}

impl StockOp {
    pub fn new(f: &FilmLook) -> Option<StockOp> {
        let p = stock_params(f.stock)?;
        let strength = (f.strength / 100.0).clamp(0.0, 1.0);
        (strength > 0.0).then_some(StockOp { p, strength })
    }

    #[inline]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let p = &self.p;
        let mut c = [0, 1, 2].map(|i| film_curve(rgb[i].clamp(0.0, 1.0).powf(p.gamma[i]), p.contrast, p.toe, p.shoulder));
        let l = luma(c);
        // Split tint by tone.
        let hw = smoothstep(0.35, 0.95, l);
        let sw = 1.0 - smoothstep(0.05, 0.55, l);
        for i in 0..3 {
            c[i] *= 1.0 + (p.high_tint[i] - 1.0) * hw + (p.shadow_tint[i] - 1.0) * sw;
        }
        // Saturation response (films desaturate toward the shoulder) and hue twists.
        let (mut h, mut s, v) = rgb_to_hsv(c);
        let mut sat = p.sat * (1.0 - 0.25 * smoothstep(0.85, 1.0, l));
        for &(centre, half, shift, mul) in p.twists {
            let d = ((h - centre + 540.0).rem_euclid(360.0) - 180.0).abs();
            let w = (1.0 - d / half).max(0.0);
            h = (h + shift * w).rem_euclid(360.0);
            sat *= 1.0 + (mul - 1.0) * w;
        }
        s = (s * sat).clamp(0.0, 1.0);
        let out = hsv_to_rgb(h, s, v.clamp(0.0, 1.0));
        [0, 1, 2].map(|i| rgb[i] + (out[i] - rgb[i]) * self.strength)
    }

    pub fn halation(&self) -> f32 {
        self.p.halation * self.strength
    }
}

// ------------------------------------------------------- colour harmony ----

/// Hue templates from colour-harmony research (Matsuda; Cohen-Or et al. 2006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HarmonyTemplate {
    /// One narrow family of hues.
    #[default]
    Monochromatic,
    /// A neighbourhood of related hues.
    Analogous,
    /// Two opposite hues.
    Complementary,
    /// A main family plus a narrow complementary accent.
    SplitAccent,
    /// A hue and its neighbour at a right angle.
    Square,
    /// Two broad opposite families.
    DualAnalogous,
}

impl HarmonyTemplate {
    pub const ALL: [HarmonyTemplate; 6] = [
        HarmonyTemplate::Monochromatic,
        HarmonyTemplate::Analogous,
        HarmonyTemplate::Complementary,
        HarmonyTemplate::SplitAccent,
        HarmonyTemplate::Square,
        HarmonyTemplate::DualAnalogous,
    ];

    /// Sectors as (offset from template angle, width), degrees.
    pub fn sectors(self) -> &'static [(f32, f32)] {
        match self {
            HarmonyTemplate::Monochromatic => &[(0.0, 18.0)],
            HarmonyTemplate::Analogous => &[(0.0, 93.6)],
            HarmonyTemplate::Complementary => &[(0.0, 18.0), (180.0, 18.0)],
            HarmonyTemplate::SplitAccent => &[(0.0, 93.6), (180.0, 18.0)],
            HarmonyTemplate::Square => &[(0.0, 18.0), (90.0, 79.2)],
            HarmonyTemplate::DualAnalogous => &[(0.0, 93.6), (180.0, 93.6)],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HarmonyTemplate::Monochromatic => "Monochromatic",
            HarmonyTemplate::Analogous => "Analogous",
            HarmonyTemplate::Complementary => "Complementary",
            HarmonyTemplate::SplitAccent => "Accent",
            HarmonyTemplate::Square => "Right-angle",
            HarmonyTemplate::DualAnalogous => "Dual analogous",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Harmony {
    pub template: HarmonyTemplate,
    /// Template rotation, degrees.
    pub angle: f32,
    /// 0..100.
    pub amount: f32,
}

fn hue_dist(a: f32, b: f32) -> f32 {
    ((a - b + 540.0).rem_euclid(360.0) - 180.0).abs()
}

/// Distance from hue `h` to the nearest sector of a template; also returns
/// that sector's centre and width.
pub fn nearest_sector(h: f32, t: HarmonyTemplate, angle: f32) -> (f32, f32, f32) {
    t.sectors()
        .iter()
        .map(|&(off, w)| {
            let c = (angle + off).rem_euclid(360.0);
            let d = (hue_dist(h, c) - w / 2.0).max(0.0);
            (d, c, w)
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .unwrap()
}

pub struct HarmonyOp {
    h: Harmony,
}

impl HarmonyOp {
    pub fn new(h: &Option<Harmony>) -> Option<HarmonyOp> {
        h.filter(|h| h.amount > 0.0).map(|h| HarmonyOp { h })
    }

    /// Pull the hue into the nearest template sector: hues inside stay near
    /// where they are, outliers are drawn to the sector edge (Cohen-Or's
    /// Gaussian remapping), weighted by saturation so greys are untouched.
    #[inline]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let (h, s, v) = rgb_to_hsv(rgb);
        if s < 0.02 {
            return rgb;
        }
        let (_, c, w) = nearest_sector(h, self.h.template, self.h.angle);
        let signed = (h - c + 540.0).rem_euclid(360.0) - 180.0;
        let sigma = w / 2.0;
        let g = (-(signed * signed) / (2.0 * sigma * sigma)).exp();
        let target = c + signed.signum() * (w / 2.0) * (1.0 - g);
        let delta = (target - h + 540.0).rem_euclid(360.0) - 180.0;
        let a = (self.h.amount / 100.0).clamp(0.0, 1.0) * s.sqrt().min(1.0);
        hsv_to_rgb((h + delta * a).rem_euclid(360.0), s, v)
    }
}

/// Saturation-weighted hue histogram (36 bins of 10°).
pub fn hue_histogram(px: &[[f32; 3]]) -> [f32; 36] {
    let mut hist = [0f32; 36];
    for p in px {
        let (h, s, v) = rgb_to_hsv(*p);
        let w = s * v;
        if w > 0.02 {
            hist[((h / 10.0) as usize).min(35)] += w;
        }
    }
    hist
}

/// Best rotation of each template for this histogram; returns (template,
/// angle, score 0..100) sorted best first. Broad templates fit anything, so
/// the score is penalised by the share of the wheel a template covers.
pub fn fit_harmony(hist: &[f32; 36]) -> Vec<(HarmonyTemplate, f32, f32)> {
    let total: f32 = hist.iter().sum::<f32>().max(1e-6);
    let mut out: Vec<(HarmonyTemplate, f32, f32)> = HarmonyTemplate::ALL
        .iter()
        .map(|&t| {
            let mut best = (f32::MAX, 0.0);
            for step in 0..180 {
                let angle = step as f32 * 2.0;
                let cost: f32 = hist
                    .iter()
                    .enumerate()
                    .map(|(i, &w)| w * nearest_sector(i as f32 * 10.0 + 5.0, t, angle).0)
                    .sum();
                if cost < best.0 {
                    best = (cost, angle);
                }
            }
            let coverage: f32 = t.sectors().iter().map(|s| s.1).sum::<f32>() / 360.0;
            let fit = 1.0 - (best.0 / total / 90.0).min(1.0);
            (t, best.1, (100.0 * fit * (1.0 - 0.35 * coverage)).max(0.0))
        })
        .collect();
    out.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    out
}

// --------------------------------------------------------- zone system ----

/// Linear luminance at the centre of zone `z` (0..=10). Zones are equal
/// perceptual steps of the print's range (CIE L* = 10·z): zone 0 is the
/// deepest black, zone V is middle grey (L* 50, ~18% reflectance), zone X is
/// paper white — the usual digital reading of Adams' print zones.
pub fn zone_value(z: f32) -> f32 {
    lstar_to_linear(z.clamp(0.0, 10.0) * 10.0)
}

/// Zone (0..=10) of a linear luminance.
pub fn zone_of(lin: f32) -> u8 {
    (linear_to_lstar(lin) / 10.0).round().clamp(0.0, 10.0) as u8
}

pub fn linear_to_lstar(y: f32) -> f32 {
    let y = y.clamp(0.0, 1.0);
    if y > 0.008856 { 116.0 * y.cbrt() - 16.0 } else { 903.3 * y }
}

pub fn lstar_to_linear(l: f32) -> f32 {
    if l > 8.0 { ((l + 16.0) / 116.0).powi(3) } else { l / 903.3 }
}

/// N+/N- development: expand or contract tones about the placed zone.
#[inline]
pub fn zone_develop(v: f32, anchor_zone: Option<u8>, n: f32) -> f32 {
    match anchor_zone {
        Some(z) if n != 0.0 => {
            let pivot = zone_value(z as f32);
            pivot * 2f32.powf((v.max(1e-6) / pivot).log2() * (1.0 + 0.2 * n.clamp(-2.0, 2.0)))
        }
        _ => v,
    }
}

// --------------------------------------------------------------- shared ----

/// Contrast S-curve with toe lift and shoulder compression, monotone for the
/// parameter ranges used here.
pub fn film_curve(x: f32, contrast: f32, toe: f32, shoulder: f32) -> f32 {
    let mut y = x.clamp(0.0, 1.0);
    if contrast != 0.0 {
        y -= contrast.clamp(-1.0, 1.0) * 0.12 * (2.0 * std::f32::consts::PI * y).sin();
    }
    if shoulder != 0.0 {
        let k = 0.12 * shoulder;
        y = y * (1.0 - k * y * y) / (1.0 - k);
    }
    if toe != 0.0 {
        y = toe + (1.0 - toe) * y;
    }
    y.clamp(0.0, 1.0)
}

pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
pub fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

pub fn rgb_to_hsv(p: [f32; 3]) -> (f32, f32, f32) {
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

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let c = v * s;
    let hp = h.rem_euclid(360.0) / 60.0;
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

/// Deterministic per-pixel noise in [-1, 1] (integer hash, no RNG state).
#[inline]
pub fn hash_noise(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6b343) ^ y.wrapping_mul(0xd8163841) ^ seed.wrapping_mul(0xcb1ab31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1e995);
    h ^= h >> 15;
    (h as f32 / u32::MAX as f32) * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_weights_keep_grey() {
        for film in [MonoFilm::Panchromatic, MonoFilm::ClassicPan, MonoFilm::Ortho, MonoFilm::Infrared] {
            for f in [LensFilter::None, LensFilter::Red, LensFilter::Blue, LensFilter::Green] {
                let w = mono_weights(film, f);
                assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-4);
            }
        }
        // A red filter darkens a blue sky relative to no filter.
        let sky = [0.1f32, 0.25, 0.6];
        let dot = |w: [f32; 3]| w[0] * sky[0] + w[1] * sky[1] + w[2] * sky[2];
        assert!(dot(mono_weights(MonoFilm::Panchromatic, LensFilter::Red)) < dot(mono_weights(MonoFilm::Panchromatic, LensFilter::None)) * 0.7);
    }

    #[test]
    fn print_curve_is_monotone_and_grade_adds_contrast() {
        for grade in [0.0, 2.0, 5.0] {
            for (soft, hard) in [(0.0, 0.0), (100.0, 100.0), (-100.0, -100.0)] {
                let m = Mono { enabled: true, grade, soft, hard, film: MonoFilm::ClassicPan, ..Default::default() };
                let mut prev = -1.0;
                for i in 0..=200 {
                    let y = mono_print_curve(i as f32 / 200.0, &m);
                    assert!(y >= prev - 1e-4, "grade {grade} soft {soft} at {i}");
                    prev = y;
                }
            }
        }
        let soft = Mono { grade: 0.0, ..Default::default() };
        let hard = Mono { grade: 5.0, ..Default::default() };
        let span = |m: &Mono| mono_print_curve(0.75, m) - mono_print_curve(0.25, m);
        assert!(span(&hard) > span(&soft) + 0.1);
    }

    #[test]
    fn toners_tint_as_expected() {
        let s = tone_print(0.7, Toner::Sepia, 100.0);
        assert!(s[0] > s[2] + 0.05, "sepia warm {s:?}");
        let c = tone_print(0.5, Toner::Cyanotype, 100.0);
        assert!(c[2] > c[0] + 0.1, "cyanotype blue {c:?}");
        let g = tone_print(0.8, Toner::Gold, 100.0);
        assert!(g[2] > g[0]);
        assert_eq!(tone_print(0.4, Toner::Selenium, 0.0), [0.4, 0.4, 0.4]);
        let p = tone_print(0.0, Toner::Platinum, 100.0);
        assert!(p[1] > 0.03, "platinum soft blacks {p:?}");
    }

    #[test]
    fn harmony_fits_and_pulls_hues() {
        // Orange and teal: complementary.
        let mut hist = [0f32; 36];
        hist[3] = 10.0;
        hist[21] = 8.0;
        let fits = fit_harmony(&hist);
        assert_eq!(fits[0].0, HarmonyTemplate::Complementary, "{fits:?}");
        let op = HarmonyOp::new(&Some(Harmony { template: fits[0].0, angle: fits[0].1, amount: 100.0 })).unwrap();
        // A stray green gets pulled toward a sector.
        let green = hsv_to_rgb(120.0, 0.8, 0.8);
        let (h2, _, _) = rgb_to_hsv(op.apply(green));
        let d_before = nearest_sector(120.0, fits[0].0, fits[0].1).0;
        let d_after = nearest_sector(h2, fits[0].0, fits[0].1).0;
        assert!(d_after < d_before, "{d_before} -> {d_after}");
        // Greys untouched.
        assert_eq!(op.apply([0.5, 0.5, 0.5]), [0.5, 0.5, 0.5]);
    }

    #[test]
    fn zones() {
        assert_eq!(zone_of(0.18), 5);
        assert_eq!(zone_of(0.0), 0);
        assert_eq!(zone_of(1.0), 10);
        for z in 0..=10 {
            assert_eq!(zone_of(zone_value(z as f32)), z as u8);
        }
        assert!(zone_value(8.0) < 1.0);
        assert!((zone_develop(zone_value(5.0), Some(5), 1.0) - zone_value(5.0)).abs() < 1e-6);
        assert!(zone_develop(zone_value(7.0), Some(5), 1.0) > zone_value(7.0));
        assert!(zone_develop(zone_value(3.0), Some(5), 1.0) < zone_value(3.0));
    }

    #[test]
    fn stocks_are_bounded() {
        for stock in [ColorStock::PortraitNegative, ColorStock::VividSlide, ColorStock::ClassicSlide, ColorStock::TungstenCine, ColorStock::FadedPrint, ColorStock::MutedEarth] {
            let op = StockOp::new(&FilmLook { stock, strength: 100.0 }).unwrap();
            for r in 0..6 {
                for g in 0..6 {
                    for b in 0..6 {
                        let o = op.apply([r as f32 / 5.0, g as f32 / 5.0, b as f32 / 5.0]);
                        assert!(o.iter().all(|v| v.is_finite() && *v >= -0.01 && *v <= 1.1), "{stock:?} {o:?}");
                    }
                }
            }
        }
    }
}
