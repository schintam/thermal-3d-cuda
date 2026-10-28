//! File-type classification by extension.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Raw,
    Jpeg,
    Tiff,
    Png,
    Heic,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Raw => "raw",
            Kind::Jpeg => "jpeg",
            Kind::Tiff => "tiff",
            Kind::Png => "png",
            Kind::Heic => "heic",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "raw" => Kind::Raw,
            "jpeg" => Kind::Jpeg,
            "tiff" => Kind::Tiff,
            "png" => Kind::Png,
            "heic" => Kind::Heic,
            _ => return None,
        })
    }
}

/// RAW extensions we hand to the RAW core. The spec's v1 list (DNG, CR3, NEF,
/// ARW, RAF) plus close relatives the same decoder handles for free.
const RAW_EXTS: &[&str] = &[
    "dng", "cr3", "cr2", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "srw", "iiq", "3fr", "erf",
    "kdc", "mos", "mrw", "x3f",
];

pub fn classify(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "jpe" => Some(Kind::Jpeg),
        "tif" | "tiff" => Some(Kind::Tiff),
        "png" => Some(Kind::Png),
        "heic" | "heif" | "hif" => Some(Kind::Heic),
        e if RAW_EXTS.contains(&e) => Some(Kind::Raw),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_case_insensitively() {
        assert_eq!(classify(Path::new("L1000123.DNG")), Some(Kind::Raw));
        assert_eq!(classify(Path::new("a.Cr3")), Some(Kind::Raw));
        assert_eq!(classify(Path::new("a.JPG")), Some(Kind::Jpeg));
        assert_eq!(classify(Path::new("a.heic")), Some(Kind::Heic));
        assert_eq!(classify(Path::new("a.xmp")), None);
        assert_eq!(classify(Path::new("noext")), None);
    }
}
