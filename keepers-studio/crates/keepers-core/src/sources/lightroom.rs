//! Lightroom Classic catalogs (`.lrcat`, a SQLite database).
//!
//! Imports every master photo (virtual copies are skipped) with its flag
//! (pick/reject) and star rating. Lightroom develop settings are not carried
//! over: they don't map onto this app's develop engine.

use super::{open_readonly, table_exists, Collection, ExternalItem};
use crate::meta::parse_exif_time;
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn read(catalog: &Path) -> Result<Vec<Collection>> {
    let c = open_readonly(catalog)?;
    for t in ["Adobe_images", "AgLibraryFile", "AgLibraryFolder", "AgLibraryRootFolder"] {
        if !table_exists(&c, t) {
            bail!("{} is not a Lightroom Classic catalog", catalog.display());
        }
    }
    let mut st = c.prepare(
        "SELECT rf.absolutePath, rf.name, fo.pathFromRoot, fi.baseName, fi.extension, i.pick, i.rating, i.captureTime
         FROM Adobe_images i
         JOIN AgLibraryFile fi ON i.rootFile = fi.id_local
         JOIN AgLibraryFolder fo ON fi.folder = fo.id_local
         JOIN AgLibraryRootFolder rf ON fo.rootFolder = rf.id_local
         WHERE i.masterImage IS NULL",
    )?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<f64>>(5)?,
            r.get::<_, Option<f64>>(6)?,
            r.get::<_, Option<String>>(7)?,
        ))
    })?;
    let cat_name = catalog.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut roots: BTreeMap<String, Collection> = BTreeMap::new();
    for row in rows {
        let (root, root_name, sub, base, ext, pick, rating, captured) = row?;
        let file = if ext.is_empty() { base } else { format!("{base}.{ext}") };
        let path = PathBuf::from(format!("{root}{sub}{file}"));
        let col = roots.entry(root.clone()).or_insert_with(|| {
            let name = root_name.unwrap_or_else(|| {
                root.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(&root).to_string()
            });
            Collection {
                root: PathBuf::from(root.trim_end_matches(['/', '\\'])),
                label: format!("Lightroom · {cat_name} · {name}"),
                items: Vec::new(),
                missing: 0,
                scan_all: false,
            }
        });
        if !path.is_file() {
            col.missing += 1;
            continue;
        }
        col.items.push(ExternalItem {
            path,
            pick: pick.map(|p| p.round() as i8).unwrap_or(0).clamp(-1, 1),
            rating: rating.map(|r| r.round() as u8).unwrap_or(0).min(5),
            // e.g. "2024-05-01T12:00:00.25" — fractional seconds dropped.
            capture_time: captured.and_then(|t| parse_exif_time(t.split('.').next().unwrap_or(&t), None)),
            display_name: None,
        });
    }
    Ok(roots.into_values().collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rusqlite::Connection;

    /// The subset of the Lightroom Classic schema the importer reads.
    pub fn fake_catalog(path: &Path, root: &Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE AgLibraryRootFolder (id_local INTEGER PRIMARY KEY, absolutePath TEXT, name TEXT);
             CREATE TABLE AgLibraryFolder (id_local INTEGER PRIMARY KEY, pathFromRoot TEXT, rootFolder INTEGER);
             CREATE TABLE AgLibraryFile (id_local INTEGER PRIMARY KEY, baseName TEXT, extension TEXT, folder INTEGER);
             CREATE TABLE Adobe_images (id_local INTEGER PRIMARY KEY, rootFile INTEGER, pick REAL, rating REAL,
                                        captureTime TEXT, masterImage INTEGER);",
        )
        .unwrap();
        let abs = format!("{}/", root.to_string_lossy().replace('\\', "/"));
        c.execute("INSERT INTO AgLibraryRootFolder VALUES (1, ?1, 'Pictures')", [abs]).unwrap();
        c.execute("INSERT INTO AgLibraryFolder VALUES (1, '2024/2024-05-01/', 1)", []).unwrap();
        c.execute_batch(
            "INSERT INTO AgLibraryFile VALUES (1, 'L1000001', 'DNG', 1), (2, 'L1000002', 'DNG', 1), (3, 'GONE', 'JPG', 1);
             INSERT INTO Adobe_images VALUES (1, 1, 1.0, 4.0, '2024-05-01T12:00:00.25', NULL);
             INSERT INTO Adobe_images VALUES (2, 2, -1.0, NULL, NULL, NULL);
             INSERT INTO Adobe_images VALUES (3, 1, 0.0, 2.0, NULL, 1);   -- virtual copy of 1
             INSERT INTO Adobe_images VALUES (4, 3, 0.0, 0.0, NULL, NULL); -- file not on disk",
        )
        .unwrap();
    }

    #[test]
    fn reads_flags_ratings_and_skips_virtual_copies() {
        let d = tempfile::tempdir().unwrap();
        let shoot = d.path().join("2024/2024-05-01");
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::write(shoot.join("L1000001.DNG"), b"x").unwrap();
        std::fs::write(shoot.join("L1000002.DNG"), b"x").unwrap();
        let cat = d.path().join("My Catalog.lrcat");
        fake_catalog(&cat, d.path());

        let cols = read(&cat).unwrap();
        assert_eq!(cols.len(), 1);
        let c = &cols[0];
        assert_eq!(c.label, "Lightroom · My Catalog · Pictures");
        assert_eq!(c.missing, 1);
        assert_eq!(c.items.len(), 2);
        let a = c.items.iter().find(|i| i.path.ends_with("L1000001.DNG")).unwrap();
        assert_eq!((a.pick, a.rating, a.capture_time), (1, 4, Some(1714564800000)));
        let b = c.items.iter().find(|i| i.path.ends_with("L1000002.DNG")).unwrap();
        assert_eq!((b.pick, b.rating), (-1, 0));
    }

    #[test]
    fn rejects_other_databases() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.lrcat");
        Connection::open(&p).unwrap().execute_batch("CREATE TABLE t (a)").unwrap();
        assert!(read(&p).is_err());
    }
}
