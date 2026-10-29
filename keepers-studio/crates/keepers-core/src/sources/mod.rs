//! Importing from other photo apps' libraries: Lightroom Classic catalogs,
//! Apple Photos / iPhoto libraries and Google Photos Takeout exports.
//!
//! Every reader is read-only: the other app's files and databases are never
//! modified. Each produces one or more [`Collection`]s, which become folders in
//! the Keepers catalog with the source app's cull state carried over.

pub mod apple_photos;
pub mod lightroom;
pub mod takeout;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// One photo as the source app knows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExternalItem {
    pub path: PathBuf,
    pub pick: i8,
    pub rating: u8,
    /// Fallback capture time (epoch millis) when the file itself has no EXIF date.
    pub capture_time: Option<i64>,
    pub display_name: Option<String>,
}

/// A set of photos that becomes one folder in the Keepers catalog.
#[derive(Debug, Clone)]
pub struct Collection {
    pub root: PathBuf,
    pub label: String,
    pub items: Vec<ExternalItem>,
    /// Photos the source lists but which are not on disk (offline drive,
    /// iCloud-only originals, ...).
    pub missing: usize,
    /// Import every supported file under `root` (used when the source's own
    /// database couldn't be read).
    pub scan_all: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExternalSource {
    /// A Lightroom Classic catalog (`.lrcat`).
    Lightroom { path: String },
    /// An Apple Photos (`.photoslibrary`) or iPhoto library package.
    ApplePhotos { path: String },
    /// A Google Takeout export: an extracted folder, or one or more `.zip`
    /// files that are extracted into `destination` first.
    Takeout { paths: Vec<String>, destination: Option<String> },
}

impl ExternalSource {
    pub fn name(&self) -> &'static str {
        match self {
            ExternalSource::Lightroom { .. } => "Lightroom catalog",
            ExternalSource::ApplePhotos { .. } => "Photos library",
            ExternalSource::Takeout { .. } => "Google Takeout export",
        }
    }
}

/// Open another app's SQLite database strictly read-only. `immutable=1` means
/// no locks and no journal files are created, so it is safe even while the
/// owning app has the database open.
pub(crate) fn open_readonly(path: &Path) -> Result<Connection> {
    let uri = sqlite_uri(path);
    Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {}", path.display()))
}

fn sqlite_uri(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    let mut enc = String::new();
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => enc.push(b as char),
            _ => enc.push_str(&format!("%{b:02X}")),
        }
    }
    // Windows drive paths need an empty authority: file:///C:/...
    let prefix = if enc.starts_with('/') { "file://" } else { "file:///" };
    format!("{prefix}{enc}?immutable=1")
}

pub(crate) fn table_exists(c: &Connection, name: &str) -> bool {
    c.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |_| Ok(()))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris() {
        assert_eq!(sqlite_uri(Path::new("/Users/me/My Cat#1.lrcat")), "file:///Users/me/My%20Cat%231.lrcat?immutable=1");
        assert_eq!(sqlite_uri(Path::new(r"C:\Users\me\cat.lrcat")), "file:///C:/Users/me/cat.lrcat?immutable=1");
    }
}
