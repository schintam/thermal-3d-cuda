//! Generate a messy test shoot: RAW+JPEG pairs, bursts with one sharp frame,
//! a duplicate, a rotated frame, a corrupt RAW and a truncated JPEG.
//!
//!     cargo run -p keepers-core --release --example make_shoot -- <out-dir> [jpeg-count]

use keepers_core::testkit::{render_scene, scene, write_dng, write_jpeg, DngOpts};
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().expect("usage: make_shoot <out-dir> [jpeg-count]"));
    let jpegs: u32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(40);
    std::fs::create_dir_all(&out).unwrap();
    let time = |s: u32| format!("2026:10:28 {:02}:{:02}:{:02}", 9 + s / 3600, (s / 60) % 60, s % 60);

    // Bursts: 3 RAW+JPEG frames one second apart; the middle one is sharp.
    let mut n = 1;
    for burst in 0..6u32 {
        for frame in 0..3u32 {
            let blur = if frame == 1 { 0.0 } else { 6.0 + frame as f32 * 4.0 };
            let f = scene(burst, blur);
            let base = format!("L100{n:04}");
            let (w, h) = (300usize, 200usize);
            write_dng(
                &out.join(format!("{base}.DNG")),
                &DngOpts {
                    width: w,
                    height: h,
                    orientation: 1,
                    preview: true,
                    warp: true,
                    make: "LEICA CAMERA AG".into(),
                    model: "LEICA Q2".into(),
                    date: time(burst * 120 + frame),
                    scene: Box::new(move |x, y| f(x as f32 / w as f32, y as f32 / h as f32)),
                },
            );
            let f = scene(burst, blur);
            write_jpeg(&out.join(format!("{base}.JPG")), &render_scene(1500, 1000, &f), "LEICA CAMERA AG", "LEICA Q2", &time(burst * 120 + frame), 1);
            n += 1;
        }
    }
    // Singles, 30 s apart, one portrait, one exact duplicate.
    for i in 0..jpegs {
        let f = scene(100 + i, (i % 5) as f32 * 2.0);
        let img = render_scene(2400, 1600, &f);
        let orientation = if i == 3 { 6 } else { 1 };
        write_jpeg(&out.join(format!("IMG_{:04}.JPG", i + 1)), &img, "Canon", "Canon EOS R6", &time(2000 + i * 30), orientation);
    }
    std::fs::copy(out.join("IMG_0001.JPG"), out.join("IMG_0001 copy.JPG")).unwrap();
    // Bad files the app must flag and skip.
    std::fs::write(out.join("CORRUPT.NEF"), b"MM\0*\0\0\0\x08this is not a nef").unwrap();
    let good = std::fs::read(out.join("IMG_0002.JPG")).unwrap();
    std::fs::write(out.join("TRUNCATED.JPG"), &good[..good.len() / 20]).unwrap();
    println!("wrote test shoot to {}", out.display());
}
