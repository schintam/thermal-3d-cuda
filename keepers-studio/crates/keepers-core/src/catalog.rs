//! The local SQLite catalog.

use crate::bursts::{Grouping, Shot};
use crate::formats::Kind;
use crate::meta::Meta;
use crate::scoring::Scores;
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub struct Catalog {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  added_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS photos (
  id INTEGER PRIMARY KEY,
  folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  path TEXT NOT NULL UNIQUE,
  file_name TEXT NOT NULL,
  kind TEXT NOT NULL,
  pair_path TEXT,
  file_size INTEGER NOT NULL DEFAULT 0,
  mtime INTEGER NOT NULL DEFAULT 0,
  processed INTEGER NOT NULL DEFAULT 0,
  error TEXT,
  capture_time INTEGER,
  camera TEXT, lens TEXT, iso INTEGER, shutter TEXT, aperture REAL, focal REAL,
  orientation INTEGER NOT NULL DEFAULT 1,
  width INTEGER, height INTEGER,
  sharpness REAL, exposure REAL, clip_hi REAL, clip_lo REAL, dhash INTEGER, fp BLOB,
  burst_id INTEGER, burst_best INTEGER NOT NULL DEFAULT 0, dup_of INTEGER,
  pick INTEGER NOT NULL DEFAULT 0,
  rating INTEGER NOT NULL DEFAULT 0,
  edits TEXT,
  edited_at INTEGER
);
CREATE INDEX IF NOT EXISTS photos_folder ON photos(folder_id, capture_time);
CREATE INDEX IF NOT EXISTS photos_pending ON photos(processed) WHERE processed = 0;
CREATE TABLE IF NOT EXISTS exports (
  id INTEGER PRIMARY KEY,
  photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
  out_path TEXT NOT NULL,
  settings TEXT NOT NULL,
  at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: i64,
    pub path: String,
    pub count: i64,
    /// Display name for folders that came from another app's library.
    pub label: Option<String>,
}

/// One catalog item, as the UI sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Photo {
    pub id: i64,
    pub folder_id: i64,
    pub path: String,
    pub file_name: String,
    pub kind: Kind,
    pub pair_path: Option<String>,
    pub processed: bool,
    pub error: Option<String>,
    pub capture_time: Option<i64>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    pub shutter: Option<String>,
    pub aperture: Option<f32>,
    pub focal: Option<f32>,
    pub orientation: u16,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sharpness: Option<f32>,
    pub exposure: Option<f32>,
    pub clip_hi: Option<f32>,
    pub clip_lo: Option<f32>,
    pub burst_id: Option<i64>,
    pub burst_best: bool,
    pub dup_of: Option<i64>,
    pub pick: i8,
    pub rating: u8,
    pub edited: bool,
}

const PHOTO_COLS: &str = "id, folder_id, path, file_name, kind, pair_path, processed, error, capture_time, camera, lens, \
    iso, shutter, aperture, focal, orientation, width, height, sharpness, exposure, clip_hi, clip_lo, burst_id, \
    burst_best, dup_of, pick, rating, edits IS NOT NULL";

fn photo_from_row(r: &Row) -> rusqlite::Result<Photo> {
    Ok(Photo {
        id: r.get(0)?,
        folder_id: r.get(1)?,
        path: r.get(2)?,
        file_name: r.get(3)?,
        kind: Kind::parse(&r.get::<_, String>(4)?).unwrap_or(Kind::Jpeg),
        pair_path: r.get(5)?,
        processed: r.get(6)?,
        error: r.get(7)?,
        capture_time: r.get(8)?,
        camera: r.get(9)?,
        lens: r.get(10)?,
        iso: r.get(11)?,
        shutter: r.get(12)?,
        aperture: r.get(13)?,
        focal: r.get(14)?,
        orientation: r.get(15)?,
        width: r.get(16)?,
        height: r.get(17)?,
        sharpness: r.get(18)?,
        exposure: r.get(19)?,
        clip_hi: r.get(20)?,
        clip_lo: r.get(21)?,
        burst_id: r.get(22)?,
        burst_best: r.get(23)?,
        dup_of: r.get(24)?,
        pick: r.get(25)?,
        rating: r.get(26)?,
        edited: r.get(27)?,
    })
}

pub struct NewPhoto {
    pub path: PathBuf,
    pub kind: Kind,
    pub pair: Option<PathBuf>,
    pub size: u64,
    pub mtime: i64,
    /// Header metadata read during the scan, so photos sort by capture time
    /// from the moment they appear.
    pub meta: Option<Meta>,
    /// Name to show instead of the file name (e.g. Apple Photos stores
    /// originals under UUIDs but remembers the camera's file name).
    pub display_name: Option<String>,
    /// Pick and rating carried over from another app; a Keepers sidecar wins.
    pub flags: Option<(i8, u8)>,
}

/// Saved cull state and edits restored from a sidecar.
pub struct Restored {
    pub pick: i8,
    pub rating: u8,
    pub edits: Option<String>,
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl Catalog {
    pub fn open(path: &Path) -> Result<Catalog> {
        let conn = Connection::open(path).with_context(|| format!("opening catalog {}", path.display()))?;
        Self::init(conn)
    }

    pub fn in_memory() -> Result<Catalog> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Catalog> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        // Migrations for catalogs created by earlier versions.
        let has_label: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('folders') WHERE name = 'label'")?
            .exists([])?;
        if !has_label {
            conn.execute_batch("ALTER TABLE folders ADD COLUMN label TEXT")?;
        }
        Ok(Catalog { conn: Mutex::new(conn) })
    }

    pub fn add_folder(&self, path: &Path) -> Result<i64> {
        self.add_folder_labeled(path, None)
    }

    pub fn add_folder_labeled(&self, path: &Path, label: Option<&str>) -> Result<i64> {
        let c = self.conn.lock();
        let p = path_str(path);
        c.execute(
            "INSERT OR IGNORE INTO folders(path, added_at, label) VALUES (?1, ?2, ?3)",
            params![p, crate::util::now_ms(), label],
        )?;
        if label.is_some() {
            c.execute("UPDATE folders SET label = ?2 WHERE path = ?1", params![p, label])?;
        }
        Ok(c.query_row("SELECT id FROM folders WHERE path = ?1", [p], |r| r.get(0))?)
    }

    pub fn folders(&self) -> Result<Vec<Folder>> {
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT f.id, f.path, (SELECT COUNT(*) FROM photos p WHERE p.folder_id = f.id), f.label FROM folders f
             ORDER BY f.label IS NULL, f.label, f.path",
        )?;
        let rows =
            st.query_map([], |r| Ok(Folder { id: r.get(0)?, path: r.get(1)?, count: r.get(2)?, label: r.get(3)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_folder(&self, id: i64) -> Result<()> {
        self.conn.lock().execute("DELETE FROM folders WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Insert new files (existing paths are left alone). Returns ids of rows
    /// actually inserted.
    pub fn insert_photos(
        &self,
        folder_id: i64,
        photos: &[NewPhoto],
        restore: impl Fn(&Path) -> Option<Restored>,
    ) -> Result<Vec<i64>> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let mut ids = Vec::new();
        {
            let mut ins = tx.prepare(
                "INSERT OR IGNORE INTO photos(folder_id, path, file_name, kind, pair_path, file_size, mtime, pick, rating, edits,
                   capture_time, camera, lens, iso, shutter, aperture, focal, orientation)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            )?;
            let mut upd_pair = tx.prepare("UPDATE photos SET pair_path = ?2 WHERE path = ?1 AND pair_path IS NULL")?;
            for p in photos {
                let name = p
                    .display_name
                    .clone()
                    .unwrap_or_else(|| p.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                let r = restore(&p.path)
                    .or_else(|| p.flags.map(|(pick, rating)| Restored { pick: pick.clamp(-1, 1), rating: rating.min(5), edits: None }));
                let n = ins.execute(params![
                    folder_id,
                    path_str(&p.path),
                    name,
                    p.kind.as_str(),
                    p.pair.as_deref().map(path_str),
                    p.size as i64,
                    p.mtime,
                    r.as_ref().map(|r| r.pick).unwrap_or(0),
                    r.as_ref().map(|r| r.rating).unwrap_or(0),
                    r.as_ref().and_then(|r| r.edits.clone()),
                    p.meta.as_ref().and_then(|m| m.capture_time),
                    p.meta.as_ref().and_then(|m| m.camera.clone()),
                    p.meta.as_ref().and_then(|m| m.lens.clone()),
                    p.meta.as_ref().and_then(|m| m.iso),
                    p.meta.as_ref().and_then(|m| m.shutter.clone()),
                    p.meta.as_ref().and_then(|m| m.aperture),
                    p.meta.as_ref().and_then(|m| m.focal),
                    p.meta.as_ref().map(|m| m.orientation).unwrap_or(1),
                ])?;
                if n == 1 {
                    ids.push(tx.last_insert_rowid());
                } else if let Some(pair) = &p.pair {
                    // A JPEG partner appeared next to an already-catalogued RAW.
                    upd_pair.execute(params![path_str(&p.path), path_str(pair)])?;
                }
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    pub fn known_paths(&self, folder_id: i64) -> Result<std::collections::HashSet<String>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT path FROM photos WHERE folder_id = ?1 UNION SELECT pair_path FROM photos WHERE folder_id = ?1 AND pair_path IS NOT NULL")?;
        let rows = st.query_map([folder_id], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Mark a folder's unreadable photos for another attempt; returns their ids.
    pub fn reset_errors(&self, folder_id: i64) -> Result<Vec<i64>> {
        let c = self.conn.lock();
        let mut st = c.prepare("UPDATE photos SET processed = 0, error = NULL WHERE folder_id = ?1 AND error IS NOT NULL RETURNING id")?;
        let rows = st.query_map([folder_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn unprocessed(&self) -> Result<Vec<i64>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT id FROM photos WHERE processed = 0 ORDER BY folder_id, capture_time, file_name")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_processed(
        &self,
        id: i64,
        meta: &Meta,
        dims: Option<(u32, u32)>,
        scores: Option<&Scores>,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE photos SET processed = 1, error = ?2, capture_time = COALESCE(?3, capture_time), camera = ?4, lens = ?5, iso = ?6,
               shutter = ?7, aperture = ?8, focal = ?9, orientation = ?10, width = ?11, height = ?12,
               sharpness = ?13, exposure = ?14, clip_hi = ?15, clip_lo = ?16, dhash = ?17, fp = ?18
             WHERE id = ?1",
            params![
                id,
                error,
                meta.capture_time,
                meta.camera,
                meta.lens,
                meta.iso,
                meta.shutter,
                meta.aperture,
                meta.focal,
                meta.orientation,
                dims.map(|d| d.0),
                dims.map(|d| d.1),
                scores.map(|s| s.sharpness),
                scores.map(|s| s.exposure),
                scores.map(|s| s.clip_hi),
                scores.map(|s| s.clip_lo),
                scores.map(|s| s.dhash as i64),
                scores.map(|s| crate::scoring::fp_to_bytes(&s.fp)),
            ],
        )?;
        Ok(())
    }

    pub fn photos(&self, folder_id: Option<i64>) -> Result<Vec<Photo>> {
        let c = self.conn.lock();
        let sql = format!(
            "SELECT {PHOTO_COLS} FROM photos WHERE (?1 IS NULL OR folder_id = ?1)
             ORDER BY capture_time IS NULL, capture_time, file_name"
        );
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map([folder_id], photo_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn photo(&self, id: i64) -> Result<Option<Photo>> {
        let c = self.conn.lock();
        Ok(c.query_row(&format!("SELECT {PHOTO_COLS} FROM photos WHERE id = ?1"), [id], photo_from_row)
            .optional()?)
    }

    pub fn photos_by_ids(&self, ids: &[i64]) -> Result<Vec<Photo>> {
        ids.iter().filter_map(|&id| self.photo(id).transpose()).collect()
    }

    pub fn set_pick(&self, ids: &[i64], pick: i8) -> Result<()> {
        self.update_many(ids, "UPDATE photos SET pick = ?2 WHERE id = ?1", pick as i64)
    }

    pub fn set_rating(&self, ids: &[i64], rating: u8) -> Result<()> {
        self.update_many(ids, "UPDATE photos SET rating = ?2 WHERE id = ?1", rating.min(5) as i64)
    }

    fn update_many(&self, ids: &[i64], sql: &str, v: i64) -> Result<()> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare(sql)?;
            for id in ids {
                st.execute(params![id, v])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Cull state + edits for writing a sidecar.
    pub fn sidecar_state(&self, id: i64) -> Result<Option<(String, i8, u8, Option<String>)>> {
        let c = self.conn.lock();
        Ok(c.query_row("SELECT path, pick, rating, edits FROM photos WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .optional()?)
    }

    pub fn shots(&self) -> Result<Vec<Shot>> {
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT id, folder_id, capture_time, sharpness, exposure, clip_hi, clip_lo, dhash, fp FROM photos WHERE processed = 1",
        )?;
        let rows = st.query_map([], |r| {
            let sharp: Option<f32> = r.get(3)?;
            Ok(Shot {
                id: r.get(0)?,
                folder_id: r.get(1)?,
                capture_time: r.get(2)?,
                scores: match sharp {
                    Some(sharpness) => Some(Scores {
                        sharpness,
                        exposure: r.get(4)?,
                        clip_hi: r.get(5)?,
                        clip_lo: r.get(6)?,
                        dhash: r.get::<_, i64>(7)? as u64,
                        fp: crate::scoring::fp_from_bytes(&r.get::<_, Option<Vec<u8>>>(8)?.unwrap_or_default()),
                    }),
                    None => None,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Store burst groupings. A user's manual choice of burst best survives
    /// regrouping as long as the burst itself is unchanged.
    pub fn set_groupings(&self, shots: &[Shot], groups: &[Grouping]) -> Result<()> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let manual: std::collections::HashMap<i64, i64> = {
            let mut st = tx.prepare("SELECT burst_id, id FROM photos WHERE burst_best = 2")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        {
            let mut st = tx.prepare("UPDATE photos SET burst_id = ?2, burst_best = ?3, dup_of = ?4 WHERE id = ?1")?;
            for (s, g) in shots.iter().zip(groups) {
                let best = match g.burst_id.and_then(|b| manual.get(&b)) {
                    Some(&m) => if m == s.id { 2 } else { 0 },
                    None => g.best as i64,
                };
                st.execute(params![s.id, g.burst_id, best, g.dup_of])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Make `id` the chosen frame of its burst (user override). Returns the burst members.
    pub fn set_burst_best(&self, id: i64) -> Result<Vec<i64>> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let burst: Option<i64> = tx.query_row("SELECT burst_id FROM photos WHERE id = ?1", [id], |r| r.get(0))?;
        let Some(b) = burst else { return Ok(vec![]) };
        tx.execute("UPDATE photos SET burst_best = 0 WHERE burst_id = ?1", [b])?;
        tx.execute("UPDATE photos SET burst_best = 2 WHERE id = ?1", [id])?;
        let members = {
            let mut st = tx.prepare("SELECT id FROM photos WHERE burst_id = ?1")?;
            let rows = st.query_map([b], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<i64>>>()?
        };
        tx.commit()?;
        Ok(members)
    }

    pub fn edits(&self, id: i64) -> Result<Option<String>> {
        let c = self.conn.lock();
        Ok(c.query_row("SELECT edits FROM photos WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten())
    }

    pub fn set_edits(&self, id: i64, edits: Option<&str>) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE photos SET edits = ?2, edited_at = ?3 WHERE id = ?1",
            params![id, edits, crate::util::now_ms()],
        )?;
        Ok(())
    }

    pub fn record_export(&self, photo_id: i64, out: &Path, settings: &str) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO exports(photo_id, out_path, settings, at) VALUES (?1, ?2, ?3, ?4)",
            params![photo_id, path_str(out), settings, crate::util::now_ms()],
        )?;
        Ok(())
    }

    pub fn export_history(&self, photo_id: i64) -> Result<Vec<(String, String, i64)>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT out_path, settings, at FROM exports WHERE photo_id = ?1 ORDER BY at DESC")?;
        let rows = st.query_map([photo_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        let c = self.conn.lock();
        Ok(c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .lock()
            .execute("INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2", [key, value])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_list_flag() {
        let c = Catalog::in_memory().unwrap();
        let f = c.add_folder(Path::new("/shoot")).unwrap();
        assert_eq!(c.add_folder(Path::new("/shoot")).unwrap(), f);
        let np = |p: &str, k| NewPhoto { path: p.into(), kind: k, pair: None, size: 1, mtime: 0, meta: None, display_name: None, flags: None };
        let ids = c.insert_photos(f, &[np("/shoot/a.dng", Kind::Raw), np("/shoot/b.jpg", Kind::Jpeg)], |_| None).unwrap();
        assert_eq!(ids.len(), 2);
        // Re-import is idempotent.
        assert!(c.insert_photos(f, &[np("/shoot/a.dng", Kind::Raw)], |_| None).unwrap().is_empty());
        c.set_pick(&ids, 1).unwrap();
        c.set_rating(&ids[..1], 9).unwrap();
        let all = c.photos(Some(f)).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|p| p.pick == 1));
        assert_eq!(c.photo(ids[0]).unwrap().unwrap().rating, 5);
        assert_eq!(c.unprocessed().unwrap().len(), 2);
        assert_eq!(c.folders().unwrap()[0].count, 2);
    }

    #[test]
    fn manual_burst_best_survives_regroup() {
        let c = Catalog::in_memory().unwrap();
        let f = c.add_folder(Path::new("/s")).unwrap();
        let np = |p: &str| NewPhoto { path: p.into(), kind: Kind::Jpeg, pair: None, size: 1, mtime: 0, meta: None, display_name: None, flags: None };
        let ids = c.insert_photos(f, &[np("/s/1.jpg"), np("/s/2.jpg")], |_| None).unwrap();
        let sc = Scores { sharpness: 10.0, ..Default::default() };
        for (i, id) in ids.iter().enumerate() {
            let m = Meta { capture_time: Some(i as i64 * 100), orientation: 1, ..Default::default() };
            let s = Scores { sharpness: 10.0 + i as f32 * 50.0, dhash: 1, ..sc };
            c.set_processed(*id, &m, None, Some(&s), None).unwrap();
        }
        let shots = c.shots().unwrap();
        c.set_groupings(&shots, &crate::bursts::group(&shots)).unwrap();
        assert!(c.photo(ids[1]).unwrap().unwrap().burst_best);
        c.set_burst_best(ids[0]).unwrap();
        let shots = c.shots().unwrap();
        c.set_groupings(&shots, &crate::bursts::group(&shots)).unwrap();
        assert!(c.photo(ids[0]).unwrap().unwrap().burst_best);
        assert!(!c.photo(ids[1]).unwrap().unwrap().burst_best);
    }
}
