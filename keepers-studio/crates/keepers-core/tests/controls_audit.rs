//! Every develop control must visibly change the picture, in colour and in
//! black and white (where it applies). Catches controls that are wired but
//! have no effect.

use keepers_core::develop::source::{from_rendered, Linear};
use keepers_core::develop::{pipeline, ColorStock, Crop, Edits, FilmLook, Harmony, HarmonyTemplate, Hsl, LensFilter, Mono, MonoFilm, RenderOpts, Toner};

/// A colourful test scene at preview-like size: sky gradient, foliage, skin
/// tone, a red object and fine texture.
fn scene() -> Linear {
    let (w, h) = (900u32, 600u32);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let tex = if (x / 3 + y / 3) % 2 == 0 { 12.0 } else { -12.0 };
        let c: [f32; 3] = if fy < 0.45 {
            [70.0 + 60.0 * fy, 120.0 + 60.0 * fy, 220.0]
        } else if fx < 0.33 {
            [60.0 + tex, 130.0 + tex, 50.0 + tex]
        } else if fx < 0.66 {
            [215.0 + tex * 0.3, 160.0 + tex * 0.3, 130.0]
        } else {
            [200.0 + tex, 40.0, 35.0]
        };
        image::Rgb(c.map(|v| v.clamp(0.0, 255.0) as u8))
    });
    from_rendered(&image::DynamicImage::ImageRgb8(img))
}

fn diff(a: &pipeline::Rendered, b: &pipeline::Rendered) -> f32 {
    if (a.w, a.h) != (b.w, b.h) {
        return 1.0;
    }
    let s: f32 = a.px.iter().zip(&b.px).map(|(p, q)| (0..3).map(|c| (p[c] - q[c]).abs()).sum::<f32>()).sum();
    s / (a.px.len() as f32 * 3.0)
}

fn bw() -> Edits {
    Edits { mono: Mono { enabled: true, ..Default::default() }, ..Default::default() }
}

#[test]
fn every_control_has_a_visible_effect() {
    let src = scene();
    let opts = RenderOpts { scale: 0.25, ..Default::default() };
    let hsl = |i: usize, f: fn(&mut Hsl)| {
        let mut a = [Hsl::default(); 8];
        f(&mut a[i]);
        a
    };
    let colour: Vec<(&str, Edits)> = vec![
        ("exposure", Edits { exposure: 1.0, ..Default::default() }),
        ("contrast", Edits { contrast: 60.0, ..Default::default() }),
        ("highlights", Edits { highlights: -80.0, ..Default::default() }),
        ("shadows", Edits { shadows: 80.0, ..Default::default() }),
        ("whites", Edits { whites: 60.0, ..Default::default() }),
        ("blacks", Edits { blacks: -60.0, ..Default::default() }),
        ("temp", Edits { temp: 50.0, ..Default::default() }),
        ("tint", Edits { tint: 50.0, ..Default::default() }),
        ("vibrance", Edits { vibrance: 70.0, ..Default::default() }),
        ("saturation", Edits { saturation: 70.0, ..Default::default() }),
        ("curve", Edits { curve: vec![[0.0, 0.0], [0.5, 0.65], [1.0, 1.0]], ..Default::default() }),
        ("hsl hue (red)", Edits { hsl: hsl(0, |b| b.hue = 80.0), ..Default::default() }),
        ("hsl sat (blue)", Edits { hsl: hsl(5, |b| b.sat = -80.0), ..Default::default() }),
        ("hsl lum (green)", Edits { hsl: hsl(3, |b| b.lum = 80.0), ..Default::default() }),
        ("rotate", Edits { rotate: 1, ..Default::default() }),
        ("flip", Edits { flip_h: true, ..Default::default() }),
        ("straighten", Edits { angle: 5.0, ..Default::default() }),
        ("crop", Edits { crop: Some(Crop { x: 0.1, y: 0.1, w: 0.6, h: 0.6 }), ..Default::default() }),
        ("sharpen", Edits { sharpen: 120.0, ..Default::default() }),
        ("nr luma", Edits { nr_luma: 90.0, ..Default::default() }),
        ("nr color", Edits { nr_color: 90.0, ..Default::default() }),
        ("film stock", Edits { film: FilmLook { stock: ColorStock::VividSlide, strength: 100.0 }, ..Default::default() }),
        ("grain", Edits { grain: 60.0, ..Default::default() }),
        ("glow", Edits { glow: 70.0, ..Default::default() }),
        ("edge burn", Edits { edge_burn: 70.0, ..Default::default() }),
        ("harmony", Edits { harmony: Some(Harmony { template: HarmonyTemplate::Complementary, angle: 30.0, amount: 100.0 }), ..Default::default() }),
        ("zone N", Edits { zone_anchor: Some(5), zone_n: 1.5, ..Default::default() }),
        ("b&w", bw()),
    ];
    let mono_base = bw();
    let m = |f: fn(&mut Mono)| {
        let mut e = bw();
        f(&mut e.mono);
        e
    };
    let mono: Vec<(&str, Edits)> = vec![
        ("film classic", m(|m| m.film = MonoFilm::ClassicPan)),
        ("film ortho", m(|m| m.film = MonoFilm::Ortho)),
        ("film infrared", m(|m| m.film = MonoFilm::Infrared)),
        ("filter red", m(|m| m.filter = LensFilter::Red)),
        ("filter green", m(|m| m.filter = LensFilter::Green)),
        ("grade", m(|m| m.grade = 4.5)),
        ("soft", m(|m| m.soft = 80.0)),
        ("hard", m(|m| m.hard = 80.0)),
        ("toner sepia", m(|m| (m.toner, m.toner_amount) = (Toner::Sepia, 80.0))),
        ("toner selenium", m(|m| (m.toner, m.toner_amount) = (Toner::Selenium, 80.0))),
        ("mix with colour", m(|m| m.amount = 40.0)),
        ("exposure (bw)", Edits { exposure: 1.0, ..bw() }),
        ("contrast (bw)", Edits { contrast: 60.0, ..bw() }),
        ("shadows (bw)", Edits { shadows: 80.0, ..bw() }),
        ("temp (bw)", Edits { temp: 60.0, ..bw() }),
        ("hsl lum red (bw mix)", Edits { hsl: hsl(0, |b| b.lum = -80.0), ..bw() }),
        ("hsl lum blue (bw mix)", Edits { hsl: hsl(5, |b| b.lum = -80.0), ..bw() }),
        ("grain (bw)", Edits { grain: 60.0, ..bw() }),
        ("glow (bw)", Edits { glow: 70.0, ..bw() }),
        ("edge burn (bw)", Edits { edge_burn: 70.0, ..bw() }),
        ("zone N (bw)", Edits { zone_anchor: Some(5), zone_n: 1.5, ..bw() }),
    ];
    let base = pipeline::render(&src, &Edits::default(), &opts);
    let mbase = pipeline::render(&src, &mono_base, &opts);
    let mut dead = Vec::new();
    for (name, e) in &colour {
        let d = diff(&base, &pipeline::render(&src, e, &opts));
        println!("{name:24} {d:.4}");
        if d < 0.004 {
            dead.push(format!("{name} ({d:.4})"));
        }
    }
    for (name, e) in &mono {
        let d = diff(&mbase, &pipeline::render(&src, e, &opts));
        println!("{name:24} {d:.4}");
        if d < 0.004 {
            dead.push(format!("{name} ({d:.4})"));
        }
    }
    assert!(dead.is_empty(), "controls with no visible effect: {dead:?}");
}

/// Interactive renders must feel live. Run with --release --ignored to see timings.
#[test]
#[ignore]
fn render_speed_at_preview_size() {
    let img = image::RgbImage::from_fn(2048, 1365, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]));
    let src = from_rendered(&image::DynamicImage::ImageRgb8(img));
    let heavy = Edits { grain: 50.0, glow: 50.0, edge_burn: 50.0, sharpen: 60.0, nr_luma: 30.0, nr_color: 30.0, mono: Mono { enabled: true, toner: Toner::Sepia, toner_amount: 60.0, ..Default::default() }, ..Default::default() };
    for (name, e) in [("plain", Edits::default()), ("exposure", Edits { exposure: 0.5, ..Default::default() }), ("heavy", heavy)] {
        let t = std::time::Instant::now();
        let (img, _) = pipeline::render_image(&src, &e, &RenderOpts { scale: 0.4, ..Default::default() });
        let r = t.elapsed();
        let t2 = std::time::Instant::now();
        let _ = keepers_core::preview::encode_jpeg(&img, 90).unwrap();
        println!("{name:10} render {:?}  jpeg {:?}", r, t2.elapsed());
    }
}
