//! Synthetic test files (DNGs with real Bayer data, JPEGs with EXIF) for
//! tests and the `make_shoot` example. Not used by the app itself.
#![allow(non_snake_case, clippy::type_complexity)]

use crate::develop::source;
use crate::preview;
use std::path::Path;

pub struct DngOpts {
    pub width: usize,
    pub height: usize,
    pub orientation: u16,
    pub preview: bool,
    pub warp: bool,
    pub make: String,
    pub model: String,
    /// EXIF "YYYY:MM:DD HH:MM:SS".
    pub date: String,
    /// Linear sRGB scene radiance at sensor pixel (x, y).
    pub scene: Box<dyn Fn(usize, usize) -> [f32; 3]>,
}

pub struct Ifd(pub Vec<(u16, u16, u32, Vec<u8>)>);

impl Ifd {
    pub fn add(&mut self, tag: u16, typ: u16, count: u32, data: Vec<u8>) {
        self.0.push((tag, typ, count, data));
    }
    pub fn short(&mut self, tag: u16, v: &[u16]) {
        self.add(tag, 3, v.len() as u32, v.iter().flat_map(|x| x.to_le_bytes()).collect());
    }
    pub fn long(&mut self, tag: u16, v: &[u32]) {
        self.add(tag, 4, v.len() as u32, v.iter().flat_map(|x| x.to_le_bytes()).collect());
    }
    pub fn ascii(&mut self, tag: u16, s: &str) {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        self.add(tag, 2, b.len() as u32, b);
    }
    pub fn bytes(&mut self, tag: u16, typ: u16, v: &[u8]) {
        self.add(tag, typ, v.len() as u32, v.to_vec());
    }
    pub fn rational(&mut self, tag: u16, signed: bool, v: &[f64]) {
        let mut b = Vec::new();
        for x in v {
            let d = 10000i32;
            let n = (x * d as f64).round() as i32;
            b.extend_from_slice(&n.to_le_bytes());
            b.extend_from_slice(&d.to_le_bytes());
        }
        self.add(tag, if signed { 10 } else { 5 }, v.len() as u32, b);
    }

    /// Serialise as if placed at byte offset `at` of the file; out-of-line
    /// values follow the entries.
    pub fn write(mut self, at: usize, next: u32) -> Vec<u8> {
        self.0.sort_by_key(|e| e.0);
        let n = self.0.len();
        let mut data_at = at + 2 + n * 12 + 4;
        let mut head = Vec::new();
        let mut data = Vec::new();
        head.extend_from_slice(&(n as u16).to_le_bytes());
        for (tag, typ, count, bytes) in &self.0 {
            head.extend_from_slice(&tag.to_le_bytes());
            head.extend_from_slice(&typ.to_le_bytes());
            head.extend_from_slice(&count.to_le_bytes());
            if bytes.len() <= 4 {
                let mut v = bytes.clone();
                v.resize(4, 0);
                head.extend_from_slice(&v);
            } else {
                head.extend_from_slice(&(data_at as u32).to_le_bytes());
                data.extend_from_slice(bytes);
                if bytes.len() % 2 == 1 {
                    data.push(0);
                }
                data_at = at + 2 + n * 12 + 4 + data.len();
            }
        }
        head.extend_from_slice(&next.to_le_bytes());
        head.extend(data);
        head
    }
}

fn opcode_list(warp: bool) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&1u32.to_be_bytes());
    let mut params = 1u32.to_be_bytes().to_vec();
    // Strong barrel correction so the effect is measurable.
    let k: [f64; 8] = if warp { [1.0, -0.15, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5] } else { [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5] };
    for d in k {
        params.extend_from_slice(&d.to_be_bytes());
    }
    v.extend_from_slice(&1u32.to_be_bytes());
    v.extend_from_slice(&0x01030000u32.to_be_bytes());
    v.extend_from_slice(&1u32.to_be_bytes());
    v.extend_from_slice(&(params.len() as u32).to_be_bytes());
    v.extend(params);
    v
}

pub fn write_dng(path: &Path, o: &DngOpts) {
    let (W, H) = (o.width, o.height);
    let scene = |x: usize, y: usize| (o.scene)(x, y);
    // As-shot neutral: the camera sees grey as (0.5, 1, 0.6).
    let neutral = [0.5f32, 1.0, 0.6];
    let mut sensor = Vec::with_capacity(W * H * 2);
    for y in 0..H {
        for x in 0..W {
            let s = scene(x, y);
            // RGGB, camera RGB == linear sRGB (colour matrix below is XYZ->sRGB).
            let c = match (y % 2, x % 2) {
                (0, 0) => 0,
                (1, 1) => 2,
                _ => 1,
            };
            let v = (s[c] * neutral[c] * 65535.0).round() as u16;
            sensor.extend_from_slice(&v.to_le_bytes());
        }
    }
    let preview_jpeg = {
        let img = image::RgbImage::from_fn(W as u32 * 10, H as u32 * 10, |x, y| {
            let s = scene(x as usize / 10, y as usize / 10);
            image::Rgb(s.map(|v| (source::linear_to_srgb(v) * 255.0) as u8))
        });
        preview::encode_jpeg(&image::DynamicImage::ImageRgb8(img), 90).unwrap()
    };

    // Layout: header(8) | IFD0 + data | preview IFD + data | sensor | jpeg
    let xyz_to_srgb = [3.2404542, -1.5371385, -0.4985314, -0.9692660, 1.8760108, 0.0415560, 0.0556434, -0.2040259, 1.0572252];

    let build_ifd0 = |sub_at: Option<u32>, strip_at: u32| {
        let mut i = Ifd(Vec::new());
        i.long(0x00FE, &[0]);
        i.long(0x0100, &[W as u32]);
        i.long(0x0101, &[H as u32]);
        i.short(0x0102, &[16]);
        i.short(0x0103, &[1]);
        i.short(0x0106, &[32803]);
        i.ascii(0x010F, &o.make);
        i.ascii(0x0110, &o.model);
        i.long(0x0111, &[strip_at]);
        i.short(0x0112, &[o.orientation]);
        i.short(0x0115, &[1]);
        i.long(0x0116, &[H as u32]);
        i.long(0x0117, &[(W * H * 2) as u32]);
        i.short(0x011C, &[1]);
        i.ascii(0x0132, &o.date);
        if let Some(s) = sub_at {
            i.long(0x014A, &[s]);
        }
        i.short(0x828D, &[2, 2]);
        i.bytes(0x828E, 1, &[0, 1, 1, 2]);
        i.bytes(0xC612, 1, &[1, 4, 0, 0]);
        i.bytes(0xC613, 1, &[1, 1, 0, 0]);
        i.ascii(0xC614, &format!("{} {}", o.make, o.model));
        i.long(0xC61D, &[65535]);
        i.rational(0xC621, true, &xyz_to_srgb);
        i.rational(0xC628, false, &neutral.map(|v| v as f64));
        i.short(0xC65A, &[21]);
        if o.warp {
            i.bytes(0xC74E, 7, &opcode_list(true));
        }
        i
    };
    // Two passes: first to measure sizes, then with real offsets.
    let ifd0_len = build_ifd0(o.preview.then_some(0), 0).write(8, 0).len();
    let sub_at = 8 + ifd0_len;
    let build_sub = |jpeg_at: u32| {
        let mut s = Ifd(Vec::new());
        s.long(0x00FE, &[1]);
        s.long(0x0100, &[W as u32 * 10]);
        s.long(0x0101, &[H as u32 * 10]);
        s.short(0x0102, &[8, 8, 8]);
        s.short(0x0103, &[7]);
        s.short(0x0106, &[6]);
        s.long(0x0111, &[jpeg_at]);
        s.short(0x0115, &[3]);
        s.long(0x0116, &[H as u32 * 10]);
        s.long(0x0117, &[preview_jpeg.len() as u32]);
        s
    };
    let sub_len = if o.preview { build_sub(0).write(sub_at, 0).len() } else { 0 };
    let strip_at = (sub_at + sub_len) as u32;
    let jpeg_at = strip_at + sensor.len() as u32;
    let ifd0 = build_ifd0(o.preview.then_some(sub_at as u32), strip_at);
    let sub = build_sub(jpeg_at);

    let mut file = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
    file.extend(ifd0.write(8, 0));
    assert_eq!(file.len(), sub_at);
    if o.preview {
        file.extend(sub.write(sub_at, 0));
    }
    assert_eq!(file.len(), strip_at as usize);
    file.extend(sensor);
    if o.preview {
        file.extend(&preview_jpeg);
    }
    std::fs::write(path, file).unwrap();
}

/// Write a JPEG with a real EXIF block (camera, capture time, exposure, orientation).
pub fn write_jpeg(path: &Path, img: &image::RgbImage, make: &str, model: &str, date: &str, orientation: u16) {
    let jpeg = preview::encode_jpeg(&image::DynamicImage::ImageRgb8(img.clone()), 88).unwrap();
    // TIFF: header(8) | IFD0 | Exif IFD
    let ifd0 = |exif_at: u32| {
        let mut i = Ifd(Vec::new());
        i.ascii(0x010F, make);
        i.ascii(0x0110, model);
        i.short(0x0112, &[orientation]);
        i.long(0x8769, &[exif_at]);
        i
    };
    let exif_at = 8 + ifd0(0).write(8, 0).len();
    let mut exif = Ifd(Vec::new());
    exif.rational(0x829A, false, &[1.0 / 250.0]);
    exif.rational(0x829D, false, &[2.8]);
    exif.short(0x8827, &[400]);
    exif.ascii(0x9003, date);
    exif.rational(0x920A, false, &[28.0]);
    let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
    tiff.extend(ifd0(exif_at as u32).write(8, 0));
    tiff.extend(exif.write(exif_at, 0));
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend(tiff);
    out.extend_from_slice(&jpeg[2..]);
    std::fs::write(path, out).unwrap();
}

/// A simple test "scene": sky gradient, ground, and a subject disc whose
/// position varies with `shot` and whose edges soften with `blur` (pixels at
/// a 1000 px wide frame). Returns linear sRGB.
pub fn scene(shot: u32, blur: f32) -> impl Fn(f32, f32) -> [f32; 3] {
    let frac = |v: f32| v - v.floor();
    let s = shot as f32;
    let horizon = 0.35 + 0.35 * frac(s * 0.618);
    let (cx, cy) = (0.15 + 0.7 * frac(s * 0.381), 0.2 + 0.6 * frac(s * 0.233));
    let r = 0.08 + 0.12 * frac(s * 0.71);
    let (bx, by) = (frac(s * 0.529 + 0.3), frac(s * 0.147 + 0.6));
    let hue = s * 0.9;
    let bright = 0.6 + 0.8 * frac(s * 0.311);
    move |x: f32, y: f32| {
        let sky = [0.25 + 0.2 * y, 0.35 + 0.2 * y, 0.6].map(|v| v * bright);
        let ground = [0.12 + 0.1 * x, 0.2, 0.08].map(|v| v * bright);
        let t = smooth((y - horizon) * 1000.0 / (blur + 1.0));
        let mut c = [0.0; 3];
        for i in 0..3 {
            c[i] = sky[i] * (1.0 - t) + ground[i] * t;
        }
        // A dark block somewhere in the frame.
        let inb = smooth((0.12 - (x - bx).abs()) * 1000.0 / (blur + 1.0)) * smooth((0.18 - (y - by).abs()) * 1000.0 / (blur + 1.0));
        for v in c.iter_mut() {
            *v *= 1.0 - 0.8 * inb;
        }
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        let k = smooth((r - d) * 1000.0 / (blur + 1.0));
        let subj = [0.5 + 0.4 * hue.sin().abs(), 0.2 + 0.3 * (hue * 1.3).cos().abs(), 0.15];
        // Fine stripes on the subject give the sharpness score something to measure.
        let stripe = if ((x * 160.0).floor() as i32) % 2 == 0 { 1.0 } else { 0.55 };
        let stripe = 1.0 + (stripe - 1.0) / (1.0 + blur * 0.6);
        for i in 0..3 {
            c[i] = c[i] * (1.0 - k) + subj[i] * stripe * k;
        }
        c
    }
}

fn smooth(v: f32) -> f32 {
    (0.5 + 0.5 * v).clamp(0.0, 1.0)
}

/// Render a scene to an 8-bit sRGB image.
pub fn render_scene(w: u32, h: u32, f: &dyn Fn(f32, f32) -> [f32; 3]) -> image::RgbImage {
    image::RgbImage::from_fn(w, h, |x, y| {
        let c = f(x as f32 / w as f32, y as f32 / h as f32);
        image::Rgb(c.map(|v| (source::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8))
    })
}
