//! End-to-end checks of the RAW path on synthetic DNGs: a Bayer sensor with
//! a known scene, white balance, colour matrix, orientation, optional embedded
//! preview and optional lens-correction opcodes.

use keepers_core::develop::{opcodes, pipeline, source, Edits, RenderOpts};
use keepers_core::formats::Kind;
use keepers_core::testkit::{write_dng, DngOpts};
use keepers_core::{embedded, meta, preview, util};
use std::path::Path;

const W: usize = 120;
const H: usize = 80;

struct Opts {
    orientation: u16,
    preview: bool,
    warp: bool,
}

/// Left half neutral grey, right half saturated red.
fn make_dng(path: &Path, o: Opts) {
    write_dng(
        path,
        &DngOpts {
            width: W,
            height: H,
            orientation: o.orientation,
            preview: o.preview,
            warp: o.warp,
            make: "Keepers".into(),
            model: "TestCam".into(),
            date: "2024:05:01 12:00:00".into(),
            scene: Box::new(|x, _| if x < W / 2 { [0.4, 0.4, 0.4] } else { [0.6, 0.08, 0.05] }),
        },
    );
}

fn avg(l: &source::Linear, x0: f32, x1: f32) -> [f32; 3] {
    let mut acc = [0f32; 3];
    let mut n = 0.0;
    for y in l.h / 4..l.h * 3 / 4 {
        for x in (l.w as f32 * x0) as usize..(l.w as f32 * x1) as usize {
            let p = l.px[y * l.w + x];
            for c in 0..3 {
                acc[c] += p[c];
            }
            n += 1.0;
        }
    }
    acc.map(|v| v / n)
}

#[test]
fn raw_develop_half_and_full_agree_and_colours_are_right() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("TEST.DNG");
    make_dng(&p, Opts { orientation: 1, preview: false, warp: false });

    for max_edge in [Some(40), None] {
        let lin = source::load(&p, Kind::Raw, 1, max_edge).unwrap_or_else(|e| panic!("decode {max_edge:?}: {e:#}"));
        assert!(lin.is_raw);
        let grey = avg(&lin, 0.1, 0.4);
        let red = avg(&lin, 0.6, 0.9);
        // White balance neutralises grey; matrix keeps red red.
        assert!((grey[0] - grey[1]).abs() < 0.03 && (grey[2] - grey[1]).abs() < 0.03, "grey {grey:?} at {max_edge:?}");
        assert!((grey[1] - 0.4).abs() < 0.05, "grey level {grey:?}");
        assert!(red[0] > 0.5 && red[1] < 0.15 && red[2] < 0.15, "red {red:?} at {max_edge:?}");
        if max_edge.is_none() {
            assert_eq!((lin.w, lin.h), (W, H));
        } else {
            assert!(lin.w <= 40);
        }
    }
}

#[test]
fn orientation_rotates_raw_and_preview() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ROT.DNG");
    make_dng(&p, Opts { orientation: 6, preview: true, warp: false });
    let lin = source::load(&p, Kind::Raw, 6, None).unwrap();
    assert_eq!((lin.w, lin.h), (H, W));

    let buf = util::map_file(&p).unwrap();
    let m = meta::read(&p, Kind::Raw, &buf);
    assert_eq!(m.orientation, 6);
    assert_eq!(m.camera.as_deref(), Some("Keepers TestCam"));
    assert_eq!(m.capture_time, Some(1714564800000));

    let jpegs = embedded::find_jpegs(&buf);
    assert_eq!(jpegs.len(), 1);
    assert_eq!((jpegs[0].width, jpegs[0].height), (W as u32 * 10, H as u32 * 10));

    let src = preview::Source { path: &p, kind: Kind::Raw, orientation: m.orientation, pair: None };
    let l = preview::load(&src, 300).unwrap();
    // Portrait after rotation, from the embedded preview.
    assert!(l.image.height() > l.image.width());
    assert_eq!((l.full_w, l.full_h), (H as u32 * 10, W as u32 * 10));
}

#[test]
fn raw_without_preview_falls_back_to_develop() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("NOPREV.DNG");
    make_dng(&p, Opts { orientation: 1, preview: false, warp: false });
    let src = preview::Source { path: &p, kind: Kind::Raw, orientation: 1, pair: None };
    let l = preview::load(&src, 300).unwrap();
    assert!(l.image.width() > 0);
    // Rendered with the default RAW look: left grey, right red.
    let rgb = l.image.to_rgb8();
    let left = rgb.get_pixel(rgb.width() / 4, rgb.height() / 2);
    let right = rgb.get_pixel(rgb.width() * 3 / 4, rgb.height() / 2);
    assert!((left[0] as i32 - left[1] as i32).abs() < 10, "{left:?}");
    assert!(right[0] > 150 && right[1] < 90, "{right:?}");
}

#[test]
fn dng_opcodes_are_found_and_applied() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("PLAIN.DNG");
    let warped = dir.path().join("WARP.DNG");
    make_dng(&plain, Opts { orientation: 1, preview: false, warp: false });
    make_dng(&warped, Opts { orientation: 1, preview: false, warp: true });
    let ops = opcodes::read(&util::map_file(&warped).unwrap());
    assert!(matches!(ops.as_slice(), [opcodes::Op::Warp { .. }]), "{ops:?}");
    assert!(opcodes::read(&util::map_file(&plain).unwrap()).is_empty());

    let a = source::load(&plain, Kind::Raw, 1, None).unwrap();
    let b = source::load(&warped, Kind::Raw, 1, None).unwrap();
    assert_eq!((a.w, a.h), (b.w, b.h));
    // Barrel correction pulls the grey/red boundary: pixels at the same spot differ near the edge.
    let diff: f32 = a.px.iter().zip(&b.px).map(|(p, q)| (p[0] - q[0]).abs()).sum();
    assert!(diff > 1.0, "warp had no effect");
}

#[test]
fn corrupt_raw_is_an_error_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("BROKEN.DNG");
    make_dng(&p, Opts { orientation: 1, preview: false, warp: false });
    let mut bytes = std::fs::read(&p).unwrap();
    bytes.truncate(400);
    std::fs::write(&p, &bytes).unwrap();
    assert!(source::load(&p, Kind::Raw, 1, None).is_err());
    let src = preview::Source { path: &p, kind: Kind::Raw, orientation: 1, pair: None };
    assert!(preview::load(&src, 300).is_err());

    std::fs::write(&p, b"II*\0garbage").unwrap();
    assert!(source::load(&p, Kind::Raw, 1, Some(100)).is_err());
}

#[test]
fn develop_pipeline_on_raw_renders() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("DEV.DNG");
    make_dng(&p, Opts { orientation: 1, preview: false, warp: false });
    let lin = source::load(&p, Kind::Raw, 1, Some(60)).unwrap();
    let (img, hist) = pipeline::render_image(&lin, &Edits { exposure: 0.5, ..Default::default() }, &RenderOpts::default());
    assert_eq!(hist[1].iter().sum::<u32>(), img.width() * img.height());
}
