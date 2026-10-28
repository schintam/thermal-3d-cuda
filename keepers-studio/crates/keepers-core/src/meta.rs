//! Capture metadata (time, camera, exposure) for the catalog.
//!
//! TIFF-family RAWs and JPEGs are read with a small zero-copy TIFF reader over
//! the memory-mapped file so only the header pages are touched. Other
//! containers fall back to the RAW core or a full EXIF parser.

use crate::embedded::{self, rd16, rd32, Order};
use crate::formats::Kind;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct Meta {
    /// Milliseconds since the Unix epoch, treating the camera clock as UTC.
    /// Only used for ordering and burst detection, never displayed as UTC.
    pub capture_time: Option<i64>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    pub shutter: Option<String>,
    pub aperture: Option<f32>,
    pub focal: Option<f32>,
    pub orientation: u16,
}

pub fn read(path: &Path, kind: Kind, buf: &[u8]) -> Meta {
    let mut m = Meta { orientation: 1, ..Default::default() };
    let parsed = match kind {
        Kind::Jpeg => jpeg_exif(buf).map(|t| tiff_meta(t, &mut m)).is_some(),
        Kind::Tiff => tiff_meta(buf, &mut m),
        Kind::Raw => {
            if embedded::tiff_order(buf).is_some() {
                tiff_meta(buf, &mut m)
            } else if buf.starts_with(b"FUJIFILMCCD-RAW") {
                // The RAF preview JPEG carries the full EXIF block.
                embedded::find_jpegs(buf)
                    .first()
                    .and_then(|j| jpeg_exif(j.bytes(buf)))
                    .map(|t| tiff_meta(t, &mut m))
                    .is_some()
            } else {
                false
            }
        }
        Kind::Png | Kind::Heic => false,
    };
    if !parsed || m.capture_time.is_none() {
        match kind {
            Kind::Raw => rawler_meta(path, &mut m),
            Kind::Png | Kind::Heic => kamadak_meta(path, &mut m),
            _ => {}
        }
    }
    m
}

/// The TIFF structure inside a JPEG's APP1 "Exif" segment.
pub fn jpeg_exif(buf: &[u8]) -> Option<&[u8]> {
    if !buf.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut pos = 2;
    while pos + 4 <= buf.len() && buf[pos] == 0xFF {
        let marker = buf[pos + 1];
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
        if marker == 0xE1 && buf.get(pos + 4..pos + 10) == Some(b"Exif\0\0") {
            return buf.get(pos + 10..pos + 2 + len);
        }
        pos += 2 + len;
    }
    None
}

fn tiff_meta(t: &[u8], m: &mut Meta) -> bool {
    let Some(order) = embedded::tiff_order(t) else { return false };
    let Some(ifd0) = rd32(t, 4, order) else { return false };
    let r = Reader { t, order };
    let mut make = None;
    let mut model = None;
    let mut dt = None;
    let mut exif_ifd = None;
    r.each(ifd0 as usize, |tag, e| match tag {
        0x010F => make = r.ascii(e),
        0x0110 => model = r.ascii(e),
        0x0112 => m.orientation = r.short(e).unwrap_or(1),
        0x0132 => dt = r.ascii(e),
        0x8769 => exif_ifd = r.long(e),
        _ => {}
    });
    let mut dto = None;
    let mut subsec = None;
    if let Some(off) = exif_ifd {
        r.each(off as usize, |tag, e| match tag {
            0x829A => {
                m.shutter = r.rational(e).map(|(n, d)| {
                    if n == 0 || d == 0 {
                        "?".into()
                    } else if n >= d {
                        format!("{}s", trim(n as f64 / d as f64))
                    } else {
                        format!("1/{}", (d as f64 / n as f64).round())
                    }
                })
            }
            0x829D => m.aperture = r.rational(e).filter(|r| r.1 != 0).map(|(n, d)| n as f32 / d as f32),
            0x8827 => m.iso = r.short(e).map(u32::from).or_else(|| r.long(e)),
            0x9003 => dto = r.ascii(e),
            0x9291 => subsec = r.ascii(e),
            0x920A => m.focal = r.rational(e).filter(|r| r.1 != 0).map(|(n, d)| n as f32 / d as f32),
            0xA434 => m.lens = r.ascii(e).filter(|s| !s.is_empty()),
            _ => {}
        });
    }
    m.camera = join_make_model(make, model);
    m.capture_time = dto.or(dt).and_then(|s| parse_exif_time(&s, subsec.as_deref()));
    true
}

struct Reader<'a> {
    t: &'a [u8],
    order: Order,
}

impl Reader<'_> {
    fn each(&self, ifd: usize, mut f: impl FnMut(u16, usize)) {
        let Some(n) = rd16(self.t, ifd, self.order) else { return };
        for i in 0..(n as usize).min(1000) {
            let e = ifd + 2 + i * 12;
            match rd16(self.t, e, self.order) {
                Some(tag) => f(tag, e),
                None => return,
            }
        }
    }

    fn value_offset(&self, e: usize, bytes: usize) -> Option<usize> {
        if bytes <= 4 { Some(e + 8) } else { rd32(self.t, e + 8, self.order).map(|o| o as usize) }
    }

    fn ascii(&self, e: usize) -> Option<String> {
        let count = rd32(self.t, e + 4, self.order)? as usize;
        let o = self.value_offset(e, count)?;
        let raw = self.t.get(o..o + count.min(256))?;
        let s: String = String::from_utf8_lossy(raw).trim_end_matches('\0').trim().into();
        Some(s)
    }

    fn short(&self, e: usize) -> Option<u16> {
        (rd16(self.t, e + 2, self.order)? == 3).then(|| rd16(self.t, e + 8, self.order))?
    }

    fn long(&self, e: usize) -> Option<u32> {
        (matches!(rd16(self.t, e + 2, self.order)?, 4 | 13)).then(|| rd32(self.t, e + 8, self.order))?
    }

    fn rational(&self, e: usize) -> Option<(u32, u32)> {
        if !matches!(rd16(self.t, e + 2, self.order)?, 5 | 10) {
            return None;
        }
        let o = rd32(self.t, e + 8, self.order)? as usize;
        Some((rd32(self.t, o, self.order)?, rd32(self.t, o + 4, self.order)?))
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.1}");
    s.trim_end_matches(".0").to_string()
}

fn join_make_model(make: Option<String>, model: Option<String>) -> Option<String> {
    match (make, model) {
        (Some(mk), Some(md)) if md.to_lowercase().starts_with(&mk.to_lowercase().split(' ').next()?.to_string()) => {
            Some(md)
        }
        (Some(mk), Some(md)) => Some(format!("{mk} {md}")),
        (a, b) => a.or(b).filter(|s| !s.is_empty()),
    }
}

/// "YYYY:MM:DD HH:MM:SS" (+ optional sub-second digits) -> epoch millis.
pub fn parse_exif_time(s: &str, subsec: Option<&str>) -> Option<i64> {
    let dt = chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y:%m:%d %H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S"))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    let mut ms = dt.and_utc().timestamp_millis();
    if let Some(ss) = subsec.map(str::trim).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())) {
        let digits: String = ss.chars().take(3).collect();
        let v: i64 = digits.parse().ok()?;
        ms += v * 10i64.pow(3 - digits.len() as u32);
    }
    Some(ms)
}

fn rawler_meta(path: &Path, m: &mut Meta) {
    let res = crate::util::guard(|| -> anyhow::Result<rawler::decoders::RawMetadata> {
        let src = rawler::rawsource::RawSource::new(path)?;
        let dec = rawler::get_decoder(&src)?;
        Ok(dec.raw_metadata(&src, &rawler::decoders::RawDecodeParams::default())?)
    });
    let Ok(md) = res else { return };
    let e = &md.exif;
    m.camera = m.camera.take().or_else(|| join_make_model(Some(md.make.clone()), Some(md.model.clone())));
    m.lens = m.lens.take().or_else(|| e.lens_model.clone());
    m.iso = m.iso.or(e.iso_speed_ratings.map(u32::from)).or(e.iso_speed);
    if m.shutter.is_none() {
        m.shutter = e.exposure_time.map(|r| {
            if r.n >= r.d { format!("{}s", trim(r.n as f64 / r.d.max(1) as f64)) } else { format!("1/{}", (r.d as f64 / r.n.max(1) as f64).round()) }
        });
    }
    m.aperture = m.aperture.or(e.fnumber.filter(|r| r.d != 0).map(|r| r.n as f32 / r.d as f32));
    m.focal = m.focal.or(e.focal_length.filter(|r| r.d != 0).map(|r| r.n as f32 / r.d as f32));
    if let Some(o) = e.orientation {
        m.orientation = o;
    }
    if m.capture_time.is_none() {
        m.capture_time = e
            .date_time_original
            .as_deref()
            .or(e.create_date.as_deref())
            .and_then(|s| parse_exif_time(s, e.sub_sec_time_original.as_deref()));
    }
}

fn kamadak_meta(path: &Path, m: &mut Meta) {
    let Ok(file) = std::fs::File::open(path) else { return };
    let mut r = std::io::BufReader::new(file);
    let Ok(ex) = exif::Reader::new().read_from_container(&mut r) else { return };
    use exif::{In, Tag, Value};
    let s = |t| ex.get_field(t, In::PRIMARY).map(|f| f.display_value().to_string().trim_matches('"').to_string());
    m.camera = join_make_model(s(Tag::Make), s(Tag::Model));
    m.lens = s(Tag::LensModel);
    if let Some(f) = ex.get_field(Tag::Orientation, In::PRIMARY) {
        m.orientation = f.value.get_uint(0).unwrap_or(1) as u16;
    }
    if let Some(f) = ex.get_field(Tag::PhotographicSensitivity, In::PRIMARY) {
        m.iso = f.value.get_uint(0);
    }
    if let Some(Value::Rational(v)) = ex.get_field(Tag::FNumber, In::PRIMARY).map(|f| &f.value) {
        m.aperture = v.first().map(|r| r.to_f32());
    }
    if let Some(Value::Rational(v)) = ex.get_field(Tag::FocalLength, In::PRIMARY).map(|f| &f.value) {
        m.focal = v.first().map(|r| r.to_f32());
    }
    m.shutter = s(Tag::ExposureTime);
    if let Some(Value::Ascii(v)) = ex.get_field(Tag::DateTimeOriginal, In::PRIMARY).map(|f| &f.value) {
        if let Some(b) = v.first() {
            m.capture_time = parse_exif_time(&String::from_utf8_lossy(b), s(Tag::SubSecTimeOriginal).as_deref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_times() {
        assert_eq!(parse_exif_time("2024:05:01 12:00:00", None), Some(1714564800000));
        assert_eq!(parse_exif_time("2024:05:01 12:00:00", Some("25")), Some(1714564800250));
        assert_eq!(parse_exif_time("0000:00:00 00:00:00", None), None);
        assert_eq!(parse_exif_time("garbage", None), None);
    }

    #[test]
    fn camera_names() {
        assert_eq!(join_make_model(Some("LEICA CAMERA AG".into()), Some("LEICA Q2".into())).unwrap(), "LEICA Q2");
        assert_eq!(join_make_model(Some("Canon".into()), Some("Canon EOS R5".into())).unwrap(), "Canon EOS R5");
        assert_eq!(join_make_model(Some("SONY".into()), Some("ILCE-7M4".into())).unwrap(), "SONY ILCE-7M4");
        assert_eq!(join_make_model(None, None), None);
    }
}
