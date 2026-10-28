//! Portable sidecars: `<photo file name>.keepers.json` next to the original.
//! They carry the cull decision and the edit stack so work survives moving
//! the folder or rebuilding the catalog. Originals are never written to.

use crate::catalog::Restored;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SUFFIX: &str = ".keepers.json";

#[derive(Serialize, Deserialize)]
struct Sidecar {
    app: String,
    version: u32,
    #[serde(default)]
    pick: i8,
    #[serde(default)]
    rating: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edits: Option<serde_json::Value>,
}

pub fn path_for(photo: &Path) -> PathBuf {
    let mut s = photo.as_os_str().to_owned();
    s.push(SUFFIX);
    PathBuf::from(s)
}

pub fn is_sidecar(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(SUFFIX))
}

pub fn read(photo: &Path) -> Option<Restored> {
    let text = std::fs::read_to_string(path_for(photo)).ok()?;
    let s: Sidecar = serde_json::from_str(&text).ok()?;
    Some(Restored {
        pick: s.pick.clamp(-1, 1),
        rating: s.rating.min(5),
        edits: s.edits.map(|v| v.to_string()),
    })
}

/// Write (or remove, when everything is back to default) the sidecar.
pub fn write(photo: &Path, pick: i8, rating: u8, edits: Option<&str>) -> std::io::Result<()> {
    let target = path_for(photo);
    let edits = edits.and_then(|e| serde_json::from_str::<serde_json::Value>(e).ok());
    if pick == 0 && rating == 0 && edits.is_none() {
        return match std::fs::remove_file(&target) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let s = Sidecar { app: "Keepers Studio".into(), version: 1, pick, rating, edits };
    let tmp = target.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&s)?)?;
    std::fs::rename(&tmp, &target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("L1000001.DNG");
        std::fs::write(&photo, b"x").unwrap();
        assert!(read(&photo).is_none());
        write(&photo, 1, 4, Some(r#"{"exposure":0.5}"#)).unwrap();
        assert!(dir.path().join("L1000001.DNG.keepers.json").exists());
        let r = read(&photo).unwrap();
        assert_eq!((r.pick, r.rating), (1, 4));
        assert!(r.edits.unwrap().contains("0.5"));
        write(&photo, 0, 0, None).unwrap();
        assert!(!path_for(&photo).exists());
        assert!(is_sidecar(&path_for(&photo)));
        assert_eq!(std::fs::read(&photo).unwrap(), b"x");
    }
}
