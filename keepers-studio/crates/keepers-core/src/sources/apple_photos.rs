//! Apple Photos (`.photoslibrary`) and legacy iPhoto libraries.
//!
//! Photos keeps originals in `originals/<x>/<UUID>.<ext>` (`Masters/` in older
//! libraries) and its catalog in `database/Photos.sqlite`. The importer reads
//! the catalog to restore each photo's original file name and favourite flag
//! (as a pick), and to skip trashed and hidden photos. If the database can't
//! be read, it falls back to importing whatever originals are on disk.
//!
//! macOS asks for permission the first time an app reads the Photos library.
//! Originals kept only in iCloud ("Optimise Mac Storage") are not on disk and
//! are reported as missing.

use super::{open_readonly, table_exists, Collection, ExternalItem};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Seconds between the Unix epoch and Core Data's reference date (2001-01-01).
const CORE_DATA_EPOCH: f64 = 978_307_200.0;

pub fn read(library: &Path) -> Result<Vec<Collection>> {
    if !library.is_dir() {
        bail!("{} is not a Photos library", library.display());
    }
    let name = library.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Photos".into());
    let originals = ["originals", "Masters"].iter().map(|d| library.join(d)).find(|p| p.is_dir());
    let Some(originals) = originals else {
        bail!(
            "No originals found in {}. If macOS asked for access to Photos, allow it and try again.",
            library.display()
        )
    };
    let label = format!("Apple Photos · {name}");
    let db = library.join("database").join("Photos.sqlite");
    let col = match read_db(&db, &originals) {
        Ok((items, missing)) => Collection { root: originals, label, items, missing, scan_all: false },
        Err(e) => {
            log::warn!("Photos database unreadable ({e:#}); importing originals folder as is");
            Collection { root: originals, label, items: Vec::new(), missing: 0, scan_all: true }
        }
    };
    Ok(vec![col])
}

fn read_db(db: &Path, originals: &Path) -> Result<(Vec<ExternalItem>, usize)> {
    let c = open_readonly(db)?;
    // macOS 11+ calls the table ZASSET; macOS 10.15 called it ZGENERICASSET.
    let table = ["ZASSET", "ZGENERICASSET"].into_iter().find(|t| table_exists(&c, t));
    let Some(table) = table else { bail!("unrecognised Photos database") };
    let attrs = table_exists(&c, "ZADDITIONALASSETATTRIBUTES");
    let sql = format!(
        "SELECT a.ZDIRECTORY, a.ZFILENAME, a.ZFAVORITE, a.ZDATECREATED, {orig}
         FROM {table} a {join}
         WHERE COALESCE(a.ZTRASHEDSTATE, 0) = 0 AND COALESCE(a.ZHIDDEN, 0) = 0
           AND a.ZFILENAME IS NOT NULL",
        orig = if attrs { "aa.ZORIGINALFILENAME" } else { "NULL" },
        join = if attrs { "LEFT JOIN ZADDITIONALASSETATTRIBUTES aa ON aa.ZASSET = a.Z_PK" } else { "" },
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<f64>>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    })?;
    let (mut items, mut missing) = (Vec::new(), 0);
    for row in rows {
        let (dir, file, fav, created, original) = row?;
        let path = match &dir {
            Some(d) if !d.is_empty() => originals.join(d).join(&file),
            _ => originals.join(&file),
        };
        // Videos and other non-photo assets are skipped by type.
        if crate::formats::classify(&path).is_none() {
            continue;
        }
        if !path.is_file() {
            missing += 1;
            continue;
        }
        items.push(ExternalItem {
            path,
            pick: if fav.unwrap_or(0) != 0 { 1 } else { 0 },
            rating: 0,
            capture_time: created.map(|s| ((s + CORE_DATA_EPOCH) * 1000.0) as i64),
            display_name: original.filter(|o| !o.is_empty()),
        });
    }
    Ok((items, missing))
}

/// Default library location on macOS.
pub fn default_library() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join("Pictures").join("Photos Library.photoslibrary");
    p.is_dir().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn reads_names_favourites_and_skips_trash() {
        let d = tempfile::tempdir().unwrap();
        let lib = d.path().join("Photos Library.photoslibrary");
        std::fs::create_dir_all(lib.join("originals/A")).unwrap();
        std::fs::create_dir_all(lib.join("database")).unwrap();
        for f in ["A/1111.heic", "A/2222.jpeg", "A/3333.jpeg", "A/4444.mov"] {
            std::fs::write(lib.join("originals").join(f), b"x").unwrap();
        }
        let c = Connection::open(lib.join("database/Photos.sqlite")).unwrap();
        c.execute_batch(
            "CREATE TABLE ZASSET (Z_PK INTEGER PRIMARY KEY, ZDIRECTORY TEXT, ZFILENAME TEXT, ZFAVORITE INTEGER,
                                  ZTRASHEDSTATE INTEGER, ZHIDDEN INTEGER, ZDATECREATED REAL);
             CREATE TABLE ZADDITIONALASSETATTRIBUTES (Z_PK INTEGER PRIMARY KEY, ZASSET INTEGER, ZORIGINALFILENAME TEXT);
             INSERT INTO ZASSET VALUES (1, 'A', '1111.heic', 1, 0, 0, 736257600.0);
             INSERT INTO ZASSET VALUES (2, 'A', '2222.jpeg', 0, 0, 0, NULL);
             INSERT INTO ZASSET VALUES (3, 'A', '3333.jpeg', 0, 1, 0, NULL);  -- trashed
             INSERT INTO ZASSET VALUES (4, 'A', '4444.mov', 0, 0, 0, NULL);   -- video
             INSERT INTO ZASSET VALUES (5, 'B', '5555.jpeg', 0, 0, 0, NULL);  -- iCloud only
             INSERT INTO ZADDITIONALASSETATTRIBUTES VALUES (1, 1, 'IMG_0042.HEIC');",
        )
        .unwrap();
        drop(c);

        let cols = read(&lib).unwrap();
        assert_eq!(cols.len(), 1);
        let col = &cols[0];
        assert_eq!(col.label, "Apple Photos · Photos Library");
        assert_eq!(col.missing, 1);
        assert_eq!(col.items.len(), 2);
        let a = &col.items.iter().find(|i| i.path.ends_with("1111.heic")).unwrap();
        assert_eq!(a.display_name.as_deref(), Some("IMG_0042.HEIC"));
        assert_eq!(a.pick, 1);
        assert_eq!(a.capture_time, Some(1714564800000));
    }

    #[test]
    fn falls_back_without_database() {
        let d = tempfile::tempdir().unwrap();
        let lib = d.path().join("iPhoto Library");
        std::fs::create_dir_all(lib.join("Masters/2010")).unwrap();
        let cols = read(&lib).unwrap();
        assert!(cols[0].items.is_empty() && cols[0].scan_all);
        assert!(cols[0].root.ends_with("Masters"));
        assert!(read(&d.path().join("nope")).is_err());
    }
}
