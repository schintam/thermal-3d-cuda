//! Google Photos via Google Takeout.
//!
//! Google offers no local library, and since 2025 its Photos API only lets
//! third-party apps see photos they uploaded themselves, so the supported
//! route is a Takeout export (takeout.google.com → Google Photos). Takeout
//! delivers zips of `Takeout/Google Photos/<album or "Photos from YYYY">/`
//! folders with a JSON metadata file next to each photo. This module extracts
//! the zips and reads that JSON for capture time (when EXIF is missing),
//! favourites (as picks) and the original title.

use super::{Collection, ExternalItem};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Meta {
    title: Option<String>,
    photo_taken_time: Option<Timestamp>,
    #[serde(default)]
    favorited: bool,
}

#[derive(Deserialize)]
struct Timestamp {
    timestamp: String,
}

/// Read an extracted Takeout folder (or any folder containing one).
pub fn read(root: &Path) -> Result<Vec<Collection>> {
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    // Index JSON files per directory and gather media.
    let mut jsons: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut media = Vec::new();
    for e in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()) {
        let p = e.into_path();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.to_ascii_lowercase().ends_with(".json") {
            jsons.entry(p.parent().unwrap().to_path_buf()).or_default().push(name);
        } else if crate::formats::classify(&p).is_some() && !crate::sidecar::is_sidecar(&p) {
            media.push(p);
        }
    }
    media.sort();

    // Albums repeat photos that are also in "Photos from YYYY": keep one copy,
    // preferring the year folder.
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let in_year = |p: &Path| {
        p.parent().and_then(|d| d.file_name()).is_some_and(|n| n.to_string_lossy().starts_with("Photos from "))
    };
    media.sort_by_key(|p| (!in_year(p), p.clone()));
    let mut seen = HashSet::new();
    media.retain(|p| {
        let key = (p.file_name().map(|n| n.to_string_lossy().to_lowercase()), size(p));
        seen.insert(key)
    });

    let mut items = Vec::new();
    for p in media {
        let dir = p.parent().unwrap();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let meta = jsons
            .get(dir)
            .and_then(|list| sidecar_for(&name, list))
            .and_then(|j| std::fs::read_to_string(dir.join(j)).ok())
            .and_then(|t| serde_json::from_str::<Meta>(&t).ok())
            .unwrap_or_default();
        let capture_time = meta
            .photo_taken_time
            .and_then(|t| t.timestamp.parse::<i64>().ok())
            .filter(|&s| s > 0)
            .map(|s| s * 1000);
        // Takeout truncates long file names; the JSON keeps the full title.
        let display_name = meta.title.filter(|t| t.len() > name.len() && t.starts_with(stem(&name)));
        items.push(ExternalItem { path: p, pick: meta.favorited as i8, rating: 0, capture_time, display_name });
    }
    let label = "Google Photos (Takeout)".to_string();
    Ok(vec![Collection { root: root.to_path_buf(), label, items, missing: 0, scan_all: false }])
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// Find the JSON sidecar Takeout wrote for `name` among `candidates`. Handles
/// `<name>.json`, `<name>.supplemental-metadata.json` and its truncations,
/// `name(1).jpg` → `name.jpg(1).json`, and `-edited` copies.
pub fn sidecar_for<'a>(name: &str, candidates: &'a [String]) -> Option<&'a str> {
    let try_base = |base: &str, dup: Option<&str>| -> Option<&'a str> {
        let long = format!("{base}.supplemental-metadata");
        candidates
            .iter()
            .filter_map(|c| {
                let s = c.strip_suffix(".json").or_else(|| c.strip_suffix(".JSON"))?;
                let s = match dup {
                    Some(d) => s.strip_suffix(&format!("({d})"))?,
                    None => s,
                };
                // Exact, or a (possibly truncated) prefix of the long form that
                // still covers most of the file name.
                let ok = s == base || (long.starts_with(s) && s.len() >= base.len().min(40));
                ok.then_some((s.len(), c.as_str()))
            })
            .max()
            .map(|(_, c)| c)
    };
    if let Some(j) = try_base(name, None) {
        return Some(j);
    }
    let (st, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let dot_ext = if ext.is_empty() { String::new() } else { format!(".{ext}") };
    // "IMG_1(2).jpg" -> base "IMG_1.jpg", duplicate number 2
    if let Some(open) = st.rfind('(') {
        let num = &st[open + 1..st.len().saturating_sub(1)];
        if st.ends_with(')') && !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
            let base = format!("{}{dot_ext}", &st[..open]);
            if let Some(j) = try_base(&base, Some(num)) {
                return Some(j);
            }
        }
    }
    if let Some(orig) = st.strip_suffix("-edited") {
        return try_base(&format!("{orig}{dot_ext}"), None);
    }
    None
}

/// Extract the photos and their JSON from Takeout zips into `dest`, keeping
/// the archive's folder structure. Existing files of the same size are
/// skipped, so an interrupted extraction can be resumed. Returns the number
/// of files written.
pub fn extract(zips: &[PathBuf], dest: &Path, mut progress: impl FnMut(usize, usize, &str)) -> Result<usize> {
    std::fs::create_dir_all(dest)?;
    let mut written = 0;
    for (zi, zp) in zips.iter().enumerate() {
        let file = std::fs::File::open(zp).with_context(|| format!("opening {}", zp.display()))?;
        let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
            .with_context(|| format!("{} is not a valid zip", zp.display()))?;
        let n = zip.len();
        for i in 0..n {
            let mut entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            // `enclosed_name` rejects absolute paths and `..` (zip-slip).
            let Some(rel) = entry.enclosed_name() else { continue };
            let lower = rel.to_string_lossy().to_lowercase();
            if !(lower.ends_with(".json") || crate::formats::classify(&rel).is_some()) {
                continue;
            }
            let out = dest.join(&rel);
            if std::fs::metadata(&out).map(|m| m.len() == entry.size()).unwrap_or(false) {
                continue;
            }
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let part = out.with_extension("keepers-part");
            let mut w = std::fs::File::create(&part)?;
            std::io::copy(&mut entry, &mut w)?;
            drop(w);
            std::fs::rename(&part, &out)?;
            written += 1;
            if i % 25 == 0 || i + 1 == n {
                progress(zi + 1, zips.len(), &rel.to_string_lossy());
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn matches_takeout_sidecar_names() {
        let c = names(&[
            "IMG_1.JPG.json",
            "IMG_2.jpg.supplemental-metadata.json",
            "PXL_20240501_120000123.MP.jpg.supplemental-me.json",
            "IMG_3.jpg(1).json",
            "IMG_4.jpg.supplemental-metadata(2).json",
            "IMG_5.HEIC.json",
        ]);
        assert_eq!(sidecar_for("IMG_1.JPG", &c), Some("IMG_1.JPG.json"));
        assert_eq!(sidecar_for("IMG_2.jpg", &c), Some("IMG_2.jpg.supplemental-metadata.json"));
        assert_eq!(sidecar_for("PXL_20240501_120000123.MP.jpg", &c), Some("PXL_20240501_120000123.MP.jpg.supplemental-me.json"));
        assert_eq!(sidecar_for("IMG_3(1).jpg", &c), Some("IMG_3.jpg(1).json"));
        assert_eq!(sidecar_for("IMG_4(2).jpg", &c), Some("IMG_4.jpg.supplemental-metadata(2).json"));
        assert_eq!(sidecar_for("IMG_5-edited.HEIC", &c), Some("IMG_5.HEIC.json"));
        assert_eq!(sidecar_for("IMG_9.jpg", &c), None);
        // A short name must not match another photo's longer sidecar.
        assert_eq!(sidecar_for("IMG_.jpg", &names(&["IMG_1.jpg.json"])), None);
    }

    fn takeout_tree(root: &Path) {
        let y = root.join("Takeout/Google Photos/Photos from 2024");
        let a = root.join("Takeout/Google Photos/Holiday");
        std::fs::create_dir_all(&y).unwrap();
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(y.join("IMG_1.jpg"), b"one").unwrap();
        std::fs::write(y.join("IMG_1.jpg.supplemental-metadata.json"), r#"{"title":"IMG_1.jpg","photoTakenTime":{"timestamp":"1714564800"},"favorited":true}"#).unwrap();
        std::fs::write(y.join("IMG_2.jpg"), b"two").unwrap();
        // The album repeats IMG_1 (same size) and adds IMG_3.
        std::fs::write(a.join("IMG_1.jpg"), b"one").unwrap();
        std::fs::write(a.join("IMG_3.jpg"), b"three").unwrap();
        std::fs::write(a.join("metadata.json"), r#"{"title":"Holiday"}"#).unwrap();
    }

    #[test]
    fn reads_takeout_folder() {
        let d = tempfile::tempdir().unwrap();
        takeout_tree(d.path());
        let cols = read(d.path()).unwrap();
        let items = &cols[0].items;
        let names: Vec<_> = items.iter().map(|i| i.path.strip_prefix(d.path()).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names.iter().any(|n| n.ends_with("Photos from 2024/IMG_1.jpg")));
        assert!(!names.iter().any(|n| n.ends_with("Holiday/IMG_1.jpg")));
        let one = items.iter().find(|i| i.path.ends_with("IMG_1.jpg")).unwrap();
        assert_eq!((one.pick, one.capture_time), (1, Some(1714564800000)));
    }

    #[test]
    fn extracts_zip_safely_and_resumably() {
        let d = tempfile::tempdir().unwrap();
        let zp = d.path().join("takeout-001.zip");
        {
            let f = std::fs::File::create(&zp).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("Takeout/Google Photos/Photos from 2024/IMG_1.jpg", o).unwrap();
            std::io::Write::write_all(&mut z, b"jpegdata").unwrap();
            z.start_file("Takeout/Google Photos/Photos from 2024/IMG_1.jpg.json", o).unwrap();
            std::io::Write::write_all(&mut z, br#"{"favorited":true}"#).unwrap();
            z.start_file("Takeout/archive_browser.html", o).unwrap();
            std::io::Write::write_all(&mut z, b"<html>").unwrap();
            z.start_file("../escape.jpg", o).unwrap();
            std::io::Write::write_all(&mut z, b"bad").unwrap();
            z.finish().unwrap();
        }
        let dest = d.path().join("out");
        assert_eq!(extract(&[zp.clone()], &dest, |_, _, _| {}).unwrap(), 2);
        assert!(dest.join("Takeout/Google Photos/Photos from 2024/IMG_1.jpg").exists());
        assert!(!dest.join("Takeout/archive_browser.html").exists());
        assert!(!d.path().join("escape.jpg").exists());
        // Second run skips what is already there.
        assert_eq!(extract(&[zp], &dest, |_, _, _| {}).unwrap(), 0);
        let cols = read(&dest).unwrap();
        assert_eq!(cols[0].items[0].pick, 1);
    }
}
