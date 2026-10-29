//! Looks: complete photographic renditions built from the craft tools in
//! [`super::style`]. Each one is grounded in a real process or tradition and
//! applies on top of the photo's own exposure, white balance and crop.

use super::pipeline::Rendered;
use super::style::{self, ColorStock, FilmLook, Harmony, HarmonyTemplate, LensFilter, Mono, MonoFilm, Toner};
use super::Edits;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Look {
    pub id: &'static str,
    pub name: &'static str,
    /// "Black & white" or "Colour".
    pub group: &'static str,
    pub description: &'static str,
}

const BW: &str = "Black & white";
const COLOUR: &str = "Colour";

pub fn catalog() -> Vec<Look> {
    let l = |id, name, group, description| Look { id, name, group, description };
    vec![
        l("silver", "Silver Gelatin", BW, "Classic panchromatic film printed on fibre paper; a yellow filter keeps skies natural."),
        l("dramatic-sky", "Dramatic Sky", BW, "Red filter turns blue sky nearly black; printed hard with burned-in edges."),
        l("platinum", "Platinum Print", BW, "The long, delicate tonal scale and warm, soft blacks of a platinum/palladium print."),
        l("selenium", "Selenium Deep", BW, "Hard-grade print toned in selenium for rich, cool, deep shadows."),
        l("sepia", "Sepia Heritage", BW, "Bleach-and-redevelop sepia: warm browns in the highlights and midtones."),
        l("cyanotype", "Cyanotype", BW, "The Prussian-blue iron print of the 1840s, with darkened print edges."),
        l("infrared", "Infrared", BW, "Infrared film: foliage glows white, skies go dark, highlights bloom."),
        l("street", "Pushed Street", BW, "Film pushed two stops and printed hard: grit, grain and graphic contrast."),
        l("ortho", "Orthochromatic", BW, "Red-blind early film: pale skies, darkened skin and lips, a vintage portrait feel."),
        l("portrait-neg", "Portrait Negative", COLOUR, "Gentle contrast, flattering skin and pastel greens of portrait colour negative."),
        l("vivid-slide", "Vivid Slide", COLOUR, "Saturated, contrasty transparency film loved by landscape photographers."),
        l("classic-slide", "Classic Slide", COLOUR, "Deep reds and yellows, dense blues: the mid-century reportage slide."),
        l("tungsten-cine", "Tungsten Cine", COLOUR, "Tungsten-balanced motion-picture film: cool tones and red halation around lights."),
        l("faded", "Faded Print", COLOUR, "A family-album print: lifted blacks, softened colour, a warm cast."),
        l("muted-earth", "Muted Earth", COLOUR, "A restrained palette: earth tones kept, everything else quietened."),
        l("harmony", "Colour Harmony", COLOUR, "Finds the photo's own colour scheme and gently brings stray hues into it."),
        l("golden-light", "Golden Light", COLOUR, "Warm late-afternoon light with a soft glow in the highlights."),
    ]
}

/// Context computed from the photo that some looks need.
#[derive(Debug, Clone, Default)]
pub struct LookContext {
    /// Best-fitting harmony template and its angle.
    pub harmony: Option<(HarmonyTemplate, f32)>,
}

/// The photo's own adjustments with every style field reset.
pub fn without_style(base: &Edits) -> Edits {
    let d = Edits::default();
    Edits {
        mono: d.mono,
        film: d.film,
        grain: d.grain,
        grain_size: d.grain_size,
        glow: d.glow,
        edge_burn: d.edge_burn,
        harmony: d.harmony,
        ..base.clone()
    }
}

/// Apply look `id` at `strength` (0..100) on top of `base`.
pub fn apply(id: &str, base: &Edits, strength: f32, ctx: &LookContext) -> Edits {
    let s = (strength / 100.0).clamp(0.0, 1.5);
    let mut e = without_style(base);
    let mono = |film, filter, grade: f32, soft: f32, hard: f32, toner, toner_amount: f32| Mono {
        enabled: true,
        amount: 100.0,
        film,
        filter,
        grade: 2.0 + (grade - 2.0) * s,
        soft: soft * s,
        hard: hard * s,
        toner,
        toner_amount: (toner_amount * s).min(100.0),
    };
    let film = |stock| FilmLook { stock, strength: (100.0 * s).min(100.0) };
    match id {
        "silver" => {
            e.mono = mono(MonoFilm::ClassicPan, LensFilter::Yellow, 2.5, 0.0, 10.0, Toner::None, 0.0);
            e.grain = 22.0 * s;
        }
        "dramatic-sky" => {
            e.mono = mono(MonoFilm::Panchromatic, LensFilter::Red, 3.5, -10.0, 30.0, Toner::None, 0.0);
            e.edge_burn = 40.0 * s;
            e.grain = 15.0 * s;
        }
        "platinum" => {
            e.mono = mono(MonoFilm::FineGrain, LensFilter::None, 1.4, -15.0, 0.0, Toner::Platinum, 85.0);
            e.edge_burn = 12.0 * s;
        }
        "selenium" => {
            e.mono = mono(MonoFilm::ClassicPan, LensFilter::Orange, 3.0, 0.0, 25.0, Toner::Selenium, 75.0);
            e.grain = 12.0 * s;
        }
        "sepia" => {
            e.mono = mono(MonoFilm::ClassicPan, LensFilter::None, 2.2, 0.0, 0.0, Toner::Sepia, 80.0);
            e.grain = 18.0 * s;
            e.edge_burn = 15.0 * s;
        }
        "cyanotype" => {
            e.mono = mono(MonoFilm::Ortho, LensFilter::None, 2.0, 0.0, 0.0, Toner::Cyanotype, 100.0);
            e.edge_burn = 25.0 * s;
        }
        "infrared" => {
            e.mono = mono(MonoFilm::Infrared, LensFilter::None, 3.0, 0.0, 15.0, Toner::None, 0.0);
            e.glow = 45.0 * s;
            e.grain = 30.0 * s;
            e.grain_size = 1.2;
        }
        "street" => {
            e.mono = mono(MonoFilm::ClassicPan, LensFilter::Yellow, 4.2, 0.0, 30.0, Toner::None, 0.0);
            e.grain = 60.0 * s;
            e.grain_size = 1.5;
            e.edge_burn = 25.0 * s;
        }
        "ortho" => {
            e.mono = mono(MonoFilm::Ortho, LensFilter::None, 2.6, 0.0, 10.0, Toner::Sepia, 20.0);
            e.grain = 20.0 * s;
        }
        "portrait-neg" => {
            e.film = film(ColorStock::PortraitNegative);
            e.grain = 15.0 * s;
            e.grain_size = 0.8;
        }
        "vivid-slide" => {
            e.film = film(ColorStock::VividSlide);
            e.grain_size = 0.6;
            e.grain = 6.0 * s;
        }
        "classic-slide" => {
            e.film = film(ColorStock::ClassicSlide);
            e.grain = 10.0 * s;
            e.grain_size = 0.7;
        }
        "tungsten-cine" => {
            e.film = film(ColorStock::TungstenCine);
            e.grain = 22.0 * s;
            e.glow = 15.0 * s;
        }
        "faded" => {
            e.film = film(ColorStock::FadedPrint);
            e.grain = 15.0 * s;
            e.edge_burn = 10.0 * s;
        }
        "muted-earth" => {
            e.film = film(ColorStock::MutedEarth);
            e.grain = 8.0 * s;
        }
        "harmony" => {
            let (template, angle) = ctx.harmony.unwrap_or((HarmonyTemplate::Analogous, 30.0));
            e.harmony = Some(Harmony { template, angle, amount: (75.0 * s).min(100.0) });
        }
        "golden-light" => {
            e.temp = base.temp + 22.0 * s;
            e.tint = base.tint + 4.0 * s;
            e.highlights = base.highlights - 12.0 * s;
            e.glow = 28.0 * s;
            e.film = FilmLook { stock: ColorStock::PortraitNegative, strength: (40.0 * s).min(100.0) };
        }
        _ => {}
    }
    e
}

// ------------------------------------------------------- colour analysis ----

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Swatch {
    /// Display sRGB, 0..255.
    pub rgb: [u8; 3],
    /// Share of the image, 0..1.
    pub share: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarmonyFit {
    pub template: HarmonyTemplate,
    pub label: &'static str,
    pub angle: f32,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColourAnalysis {
    pub palette: Vec<Swatch>,
    pub harmonies: Vec<HarmonyFit>,
    /// Saturation-weighted hue histogram, 36 bins of 10°.
    pub hues: Vec<f32>,
}

/// Dominant colours (k-means, deterministic) and harmony fit.
pub fn analyze_colour(img: &Rendered) -> ColourAnalysis {
    let step = (img.px.len() / 6000).max(1);
    let samples: Vec<[f32; 3]> = img.px.iter().step_by(step).copied().collect();
    let k = 5.min(samples.len().max(1));
    // Farthest-point initialisation from the mean: deterministic.
    let mean = samples.iter().fold([0f32; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]).map(|v| v / samples.len().max(1) as f32);
    let d2 = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
    let mut centres = vec![mean];
    while centres.len() < k {
        let far = samples
            .iter()
            .max_by(|a, b| {
                let da = centres.iter().map(|c| d2(**a, *c)).fold(f32::MAX, f32::min);
                let db = centres.iter().map(|c| d2(**b, *c)).fold(f32::MAX, f32::min);
                da.partial_cmp(&db).unwrap()
            })
            .copied()
            .unwrap_or(mean);
        centres.push(far);
    }
    let mut counts = vec![0usize; k];
    for _ in 0..12 {
        let mut sums = vec![[0f32; 3]; k];
        counts = vec![0; k];
        for p in &samples {
            let (i, _) = centres.iter().enumerate().map(|(i, c)| (i, d2(*p, *c))).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
            for c in 0..3 {
                sums[i][c] += p[c];
            }
            counts[i] += 1;
        }
        for i in 0..k {
            if counts[i] > 0 {
                centres[i] = sums[i].map(|v| v / counts[i] as f32);
            }
        }
    }
    let n = samples.len().max(1) as f32;
    let mut palette: Vec<Swatch> = centres
        .iter()
        .zip(&counts)
        .filter(|(_, &c)| c > 0)
        .map(|(c, &cnt)| Swatch { rgb: c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8), share: cnt as f32 / n })
        .collect();
    palette.sort_by(|a, b| b.share.partial_cmp(&a.share).unwrap());

    let hist = style::hue_histogram(&img.px);
    let harmonies = style::fit_harmony(&hist)
        .into_iter()
        .map(|(template, angle, score)| HarmonyFit { template, label: template.label(), angle, score })
        .collect();
    ColourAnalysis { palette, harmonies, hues: hist.to_vec() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_look_changes_something_and_keeps_basics() {
        let base = Edits { exposure: 0.7, crop: Some(super::super::Crop { x: 0.1, y: 0.1, w: 0.5, h: 0.5 }), ..Default::default() };
        let ctx = LookContext { harmony: Some((HarmonyTemplate::Complementary, 20.0)) };
        for look in catalog() {
            let e = apply(look.id, &base, 100.0, &ctx);
            assert_ne!(e, without_style(&base), "{} does nothing", look.id);
            assert_eq!(e.exposure, base.exposure);
            assert_eq!(e.crop, base.crop);
            assert_eq!(e.mono.enabled, look.group == BW, "{}", look.id);
        }
        // Strength 0 of a colour look is (almost) the original.
        let z = apply("vivid-slide", &base, 0.0, &ctx);
        assert_eq!(z.film.strength, 0.0);
    }

    #[test]
    fn palette_finds_dominant_colours() {
        let mut px = vec![[0.9f32, 0.1, 0.1]; 700];
        px.extend(vec![[0.1f32, 0.2, 0.9]; 300]);
        let a = analyze_colour(&Rendered { w: 100, h: 10, px });
        assert!(a.palette[0].rgb[0] > 200 && a.palette[0].share > 0.6, "{:?}", a.palette);
        assert!(a.palette.iter().any(|s| s.rgb[2] > 200));
        assert_eq!(a.harmonies.len(), 6);
    }
}
