//! Locate the JPEG previews cameras embed inside RAW files.
//!
//! Culling never demosaics: it shows the camera's own preview. This module
//! walks just enough of each container (TIFF IFDs, Fuji RAF header, Canon CR3
//! ISO-BMFF boxes) to find those JPEG streams without reading the sensor data,
//! so a memory-mapped 50 MB RAW only faults in a few pages.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegRef {
    pub offset: usize,
    pub len: usize,
    pub width: u32,
    pub height: u32,
}

impl JpegRef {
    pub fn pixels(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    pub fn long_edge(&self) -> u32 {
        self.width.max(self.height)
    }

    pub fn bytes<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        &buf[self.offset..self.offset + self.len]
    }
}

/// All decodable (baseline/progressive DCT) JPEG streams embedded in `buf`,
/// de-duplicated, largest first.
pub fn find_jpegs(buf: &[u8]) -> Vec<JpegRef> {
    let mut out = Vec::new();
    if buf.len() < 16 {
        return out;
    }
    if buf.starts_with(b"FUJIFILMCCD-RAW") {
        raf(buf, &mut out);
    } else if &buf[4..8] == b"ftyp" {
        bmff(buf, &mut out);
    } else if let Some(order) = tiff_order(buf) {
        tiff(buf, order, &mut out);
    } else if buf.starts_with(&[0xFF, 0xD8]) {
        if let Some(r) = probe(buf, 0, None) {
            out.push(r);
        }
    }
    out.sort_by(|a, b| b.pixels().cmp(&a.pixels()).then(a.offset.cmp(&b.offset)));
    out.dedup_by_key(|r| r.offset);
    out
}

/// Check that a JPEG stream starts at `offset`, is DCT-coded (not the lossless
/// JPEG some RAWs use for sensor data) and work out its size. When `len` is not
/// known from the container, the end-of-image marker is located by scanning.
pub fn probe(buf: &[u8], offset: usize, len: Option<usize>) -> Option<JpegRef> {
    let end_limit = match len {
        Some(l) => offset.checked_add(l)?.min(buf.len()),
        None => buf.len(),
    };
    let data = buf.get(offset..end_limit)?;
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut pos = 2;
    let mut dims = None;
    loop {
        // Skip fill bytes to the next marker.
        while pos < data.len() && data[pos] != 0xFF {
            pos += 1;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        let marker = *data.get(pos)?;
        pos += 1;
        match marker {
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            0xD9 => return None,
            _ => {}
        }
        let seg_len = u16::from_be_bytes([*data.get(pos)?, *data.get(pos + 1)?]) as usize;
        if seg_len < 2 {
            return None;
        }
        match marker {
            0xC0 | 0xC1 | 0xC2 => {
                let h = u16::from_be_bytes([*data.get(pos + 3)?, *data.get(pos + 4)?]) as u32;
                let w = u16::from_be_bytes([*data.get(pos + 5)?, *data.get(pos + 6)?]) as u32;
                let comps = *data.get(pos + 7)?;
                if w == 0 || h == 0 || !(comps == 1 || comps == 3) {
                    return None;
                }
                dims = Some((w, h));
            }
            // Lossless, hierarchical and arithmetic-coded frames: not a preview.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            0xDA => {
                let (w, h) = dims?;
                let total = match len {
                    Some(_) => data.len(),
                    None => find_eoi(data, pos + seg_len)?,
                };
                return Some(JpegRef { offset, len: total, width: w, height: h });
            }
            _ => {}
        }
        pos += seg_len;
    }
}

/// Byte length of the JPEG up to and including EOI, scanning entropy-coded data
/// (which escapes 0xFF as 0xFF00) and any further scans of a progressive file.
fn find_eoi(data: &[u8], mut pos: usize) -> Option<usize> {
    while pos + 1 < data.len() {
        if data[pos] == 0xFF {
            let m = data[pos + 1];
            match m {
                0x00 | 0xFF | 0xD0..=0xD7 => pos += if m == 0xFF { 1 } else { 2 },
                0xD9 => return Some(pos + 2),
                _ => {
                    // A marker segment between scans (DHT, SOS, ...): jump over it.
                    let l = u16::from_be_bytes([*data.get(pos + 2)?, *data.get(pos + 3)?]) as usize;
                    pos += 2 + l;
                }
            }
        } else {
            // Fast skip to the next 0xFF.
            match data[pos..].iter().position(|&b| b == 0xFF) {
                Some(p) => pos += p,
                None => return None,
            }
        }
    }
    None
}

// ---------------------------------------------------------------- TIFF ----

#[derive(Clone, Copy)]
pub(crate) enum Order {
    Le,
    Be,
}

pub(crate) fn tiff_order(buf: &[u8]) -> Option<Order> {
    match &buf[0..2] {
        // Standard TIFF/DNG/NEF/ARW/CR2 ("*"), Olympus ("RO"/"RS"), Panasonic (0x55).
        b"II" if matches!(buf[2], 0x2A | b'R' | 0x55) => Some(Order::Le),
        b"MM" if buf[3] == 0x2A => Some(Order::Be),
        _ => None,
    }
}

pub(crate) fn rd16(buf: &[u8], o: usize, order: Order) -> Option<u16> {
    let b = buf.get(o..o + 2)?;
    Some(match order {
        Order::Le => u16::from_le_bytes([b[0], b[1]]),
        Order::Be => u16::from_be_bytes([b[0], b[1]]),
    })
}

pub(crate) fn rd32(buf: &[u8], o: usize, order: Order) -> Option<u32> {
    let b = buf.get(o..o + 4)?;
    Some(match order {
        Order::Le => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        Order::Be => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
    })
}

/// One entry's values as u32s (for SHORT/LONG/IFD typed tags).
fn tag_values(buf: &[u8], entry: usize, order: Order) -> Vec<u32> {
    let (Some(typ), Some(count)) = (rd16(buf, entry + 2, order), rd32(buf, entry + 4, order)) else {
        return Vec::new();
    };
    let size = match typ {
        3 => 2,
        4 | 13 => 4,
        _ => return Vec::new(),
    };
    let count = count.min(64) as usize;
    let base = if size * count <= 4 {
        entry + 8
    } else {
        match rd32(buf, entry + 8, order) {
            Some(o) => o as usize,
            None => return Vec::new(),
        }
    };
    (0..count)
        .filter_map(|i| {
            let o = base + i * size;
            if size == 2 { rd16(buf, o, order).map(u32::from) } else { rd32(buf, o, order) }
        })
        .collect()
}

fn tiff(buf: &[u8], order: Order, out: &mut Vec<JpegRef>) {
    let Some(first) = rd32(buf, 4, order) else { return };
    let mut queue = vec![first as usize];
    let mut seen = Vec::new();
    while let Some(ifd) = queue.pop() {
        if ifd == 0 || ifd >= buf.len() || seen.contains(&ifd) || seen.len() > 64 {
            continue;
        }
        seen.push(ifd);
        let Some(n) = rd16(buf, ifd, order) else { continue };
        let n = n as usize;
        if n > 1000 {
            continue;
        }
        let (mut jif, mut jif_len) = (None, None);
        let (mut strip, mut strip_len, mut compression) = (None, None, None);
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            let Some(tag) = rd16(buf, e, order) else { break };
            let vals = || tag_values(buf, e, order);
            match tag {
                0x0103 => compression = vals().first().copied(),
                // Single strip or single tile only: multi-part streams are sensor data.
                0x0111 | 0x0144 => strip = single(vals()),
                0x0117 | 0x0145 => strip_len = single(vals()),
                0x0201 => jif = vals().first().copied(),
                0x0202 => jif_len = vals().first().copied(),
                0x014A | 0x8769 => queue.extend(vals().into_iter().map(|v| v as usize)),
                _ => {}
            }
        }
        if let (Some(o), Some(l)) = (jif, jif_len) {
            out.extend(probe(buf, o as usize, Some(l as usize)));
        }
        if let (Some(o), Some(l), Some(6 | 7)) = (strip, strip_len, compression) {
            out.extend(probe(buf, o as usize, Some(l as usize)));
        }
        if let Some(next) = rd32(buf, ifd + 2 + n * 12, order) {
            queue.push(next as usize);
        }
    }
}

fn single(v: Vec<u32>) -> Option<u32> {
    if v.len() == 1 { Some(v[0]) } else { None }
}

// ----------------------------------------------------------------- RAF ----

fn raf(buf: &[u8], out: &mut Vec<JpegRef>) {
    let (Some(o), Some(l)) = (rd32(buf, 84, Order::Be), rd32(buf, 88, Order::Be)) else { return };
    out.extend(probe(buf, o as usize, Some(l as usize)));
}

// ------------------------------------------------------- CR3 (ISO BMFF) ----

const CANON_PRVW_UUID: [u8; 16] =
    [0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16];

/// Iterate (type, payload_start, end) over boxes in `buf[start..end]`.
fn boxes(buf: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut v = Vec::new();
    let mut pos = start;
    while pos + 8 <= end {
        let Some(size32) = rd32(buf, pos, Order::Be) else { break };
        let typ: [u8; 4] = buf[pos + 4..pos + 8].try_into().unwrap();
        let (hdr, size) = match size32 {
            0 => (8, end - pos),
            1 => match buf.get(pos + 8..pos + 16) {
                Some(b) => (16, u64::from_be_bytes(b.try_into().unwrap()) as usize),
                None => break,
            },
            s => (8, s as usize),
        };
        if size < hdr || pos + size > end {
            break;
        }
        v.push((typ, pos + hdr, pos + size));
        pos += size;
    }
    v
}

fn bmff(buf: &[u8], out: &mut Vec<JpegRef>) {
    for (typ, start, end) in boxes(buf, 0, buf.len()) {
        match &typ {
            // Canon's preview container holds a ~1620px PRVW JPEG.
            b"uuid" if buf.get(start..start + 16) == Some(&CANON_PRVW_UUID[..]) => {
                let region = &buf[start..end];
                if let Some(p) = region.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]) {
                    out.extend(probe(buf, start + p, Some(end - start - p)));
                }
            }
            // Track 1 of a CR3 is a full-resolution JPEG.
            b"moov" => {
                if let Some(trak) = boxes(buf, start, end).into_iter().find(|b| &b.0 == b"trak") {
                    if let Some((o, l)) = first_sample(buf, trak.1, trak.2) {
                        out.extend(probe(buf, o, Some(l)));
                    }
                }
            }
            _ => {}
        }
    }
}

fn first_sample(buf: &[u8], start: usize, end: usize) -> Option<(usize, usize)> {
    let child = |s, e, t: &[u8; 4]| boxes(buf, s, e).into_iter().find(|b| &b.0 == t);
    let mdia = child(start, end, b"mdia")?;
    let minf = child(mdia.1, mdia.2, b"minf")?;
    let stbl = child(minf.1, minf.2, b"stbl")?;
    let stsz = child(stbl.1, stbl.2, b"stsz")?;
    // stsz: version/flags(4) sample_size(4) count(4) [sizes...]
    let mut size = rd32(buf, stsz.1 + 4, Order::Be)? as usize;
    if size == 0 {
        size = rd32(buf, stsz.1 + 12, Order::Be)? as usize;
    }
    let offset = if let Some(co64) = child(stbl.1, stbl.2, b"co64") {
        let b = buf.get(co64.1 + 8..co64.1 + 16)?;
        u64::from_be_bytes(b.try_into().ok()?) as usize
    } else {
        let stco = child(stbl.1, stbl.2, b"stco")?;
        rd32(buf, stco.1 + 8, Order::Be)? as usize
    };
    Some((offset, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn tiny_jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 7) as u8, (y * 5) as u8, 128]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img).unwrap();
        out
    }

    #[test]
    fn probes_plain_jpeg_without_length() {
        let j = tiny_jpeg(40, 30);
        let mut buf = vec![0u8; 13];
        buf.extend_from_slice(&j);
        buf.extend_from_slice(&[0xAB; 50]);
        let r = probe(&buf, 13, None).unwrap();
        assert_eq!((r.width, r.height, r.len), (40, 30, j.len()));
    }

    #[test]
    fn rejects_lossless_jpeg() {
        // SOI, SOF3 frame header, nothing else.
        let buf = [0xFF, 0xD8, 0xFF, 0xC3, 0, 11, 8, 0, 10, 0, 10, 1, 1, 0x11, 0];
        assert!(probe(&buf, 0, None).is_none());
    }

    /// Minimal little-endian TIFF: IFD0 -> SubIFD with a JPEG strip, plus a
    /// JPEGInterchangeFormat thumbnail in IFD1.
    #[test]
    fn walks_tiff_subifds() {
        let big = tiny_jpeg(64, 48);
        let small = tiny_jpeg(16, 12);
        let mut buf = vec![b'I', b'I', 0x2A, 0, 8, 0, 0, 0];
        let entry = |tag: u16, typ: u16, count: u32, val: u32| {
            let mut e = Vec::new();
            e.extend_from_slice(&tag.to_le_bytes());
            e.extend_from_slice(&typ.to_le_bytes());
            e.extend_from_slice(&count.to_le_bytes());
            e.extend_from_slice(&val.to_le_bytes());
            e
        };
        // IFD0 at 8: 1 entry (SubIFDs) -> next IFD1
        let ifd0 = 8usize;
        let ifd1 = ifd0 + 2 + 12 + 4;
        let sub = ifd1 + 2 + 24 + 4;
        let data_big = sub + 2 + 36 + 4;
        let data_small = data_big + big.len();
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend(entry(0x014A, 4, 1, sub as u32));
        buf.extend_from_slice(&(ifd1 as u32).to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend(entry(0x0201, 4, 1, data_small as u32));
        buf.extend(entry(0x0202, 4, 1, small.len() as u32));
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&3u16.to_le_bytes());
        buf.extend(entry(0x0103, 3, 1, 7));
        buf.extend(entry(0x0111, 4, 1, data_big as u32));
        buf.extend(entry(0x0117, 4, 1, big.len() as u32));
        buf.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(buf.len(), data_big);
        buf.extend_from_slice(&big);
        buf.extend_from_slice(&small);

        let found = find_jpegs(&buf);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].width, found[0].height), (64, 48));
        assert_eq!((found[1].width, found[1].height), (16, 12));
    }

    #[test]
    fn garbage_is_harmless() {
        for n in [0, 3, 16, 100, 4096] {
            let buf: Vec<u8> = (0..n).map(|i| (i * 31 % 251) as u8).collect();
            let _ = find_jpegs(&buf);
        }
        let mut evil = vec![b'I', b'I', 0x2A, 0, 8, 0, 0, 0, 0xFF, 0xFF];
        evil.extend_from_slice(&[0xFF; 64]);
        let _ = find_jpegs(&evil);
    }
}
