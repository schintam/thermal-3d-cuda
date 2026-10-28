//! The engine behind the UI: owns the catalog, the preview cache and the
//! background workers. UI-framework agnostic; progress is reported through an
//! event callback.

use crate::bursts;
use crate::catalog::{path_str, Catalog, Folder, NewPhoto, Photo};
use crate::develop::{pipeline, source, Edits, RenderOpts};
use crate::export::{self, ExportSettings};
use crate::formats::{self, Kind};
use crate::preview::{self, Source};
use crate::{meta, scoring, sidecar, util};
use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Grid thumbnail long edge (crisp at 2x DPI for ~240px cells).
pub const THUMB_EDGE: u32 = 480;
/// Size scoring runs at.
const ANALYSIS_EDGE: u32 = 1024;
/// Loupe preview long edge.
pub const PREVIEW_EDGE: u32 = 2560;
/// Develop-mode working preview long edge.
pub const DEVELOP_EDGE: usize = 2048;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    #[serde(rename_all = "camelCase")]
    ImportStatus { message: String, done: usize, total: usize, finished: bool },
    #[serde(rename_all = "camelCase")]
    Progress { done: usize, total: usize },
    #[serde(rename_all = "camelCase")]
    PhotosUpdated { photos: Vec<Photo> },
    CatalogChanged,
    #[serde(rename_all = "camelCase")]
    ExportStatus { done: usize, total: usize, errors: Vec<String>, last: Option<String>, finished: bool },
    #[serde(rename_all = "camelCase")]
    Error { message: String },
}

pub type Emit = Box<dyn Fn(Event) + Send + Sync>;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyOptions {
    pub destination: String,
    /// Subfolder template: `{YYYY}`, `{MM}`, `{DD}`, `{date}` (YYYY-MM-DD), `{name}` (shoot name).
    pub template: String,
    pub shoot_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub name: String,
    pub edits: Edits,
    pub builtin: bool,
}

pub struct RenderResult {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub histogram: pipeline::Histogram,
}

pub struct Library {
    pub catalog: Catalog,
    cache: PathBuf,
    emit: Emit,
    work_tx: Sender<i64>,
    sidecar_tx: Sender<i64>,
    queued: AtomicUsize,
    done: AtomicUsize,
    updated: Mutex<Vec<i64>>,
    regroup: AtomicBool,
    develop_cache: Mutex<Vec<(i64, Arc<source::Linear>)>>,
    write_sidecars: AtomicBool,
}

impl Library {
    pub fn open(data_dir: &Path, emit: Emit) -> Result<Arc<Library>> {
        std::fs::create_dir_all(data_dir)?;
        let catalog = Catalog::open(&data_dir.join("catalog.sqlite"))?;
        let (work_tx, work_rx) = unbounded();
        let (sidecar_tx, sidecar_rx) = unbounded();
        let write_sidecars = catalog.setting("writeSidecars")?.map(|v| v != "false").unwrap_or(true);
        let lib = Arc::new(Library {
            catalog,
            cache: data_dir.join("cache"),
            emit,
            work_tx,
            sidecar_tx,
            queued: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            updated: Mutex::new(Vec::new()),
            regroup: AtomicBool::new(false),
            develop_cache: Mutex::new(Vec::new()),
            write_sidecars: AtomicBool::new(write_sidecars),
        });
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(1).clamp(2, 12);
        for i in 0..workers {
            let (lib, rx) = (Arc::downgrade(&lib), work_rx.clone());
            std::thread::Builder::new().name(format!("keepers-worker-{i}")).spawn(move || worker(lib, rx))?;
        }
        let weak = Arc::downgrade(&lib);
        std::thread::Builder::new().name("keepers-events".into()).spawn(move || event_pump(weak))?;
        let weak = Arc::downgrade(&lib);
        std::thread::Builder::new().name("keepers-sidecars".into()).spawn(move || sidecar_writer(weak, sidecar_rx))?;
        // Resume anything left unprocessed by a previous session.
        for id in lib.catalog.unprocessed()? {
            lib.enqueue(id);
        }
        Ok(lib)
    }

    fn enqueue(&self, id: i64) {
        self.queued.fetch_add(1, Ordering::SeqCst);
        let _ = self.work_tx.send(id);
    }

    // ------------------------------------------------------------ import --

    /// Import a folder in the background. With `copy`, files are first copied
    /// (e.g. off an SD card) into dated subfolders of the destination.
    pub fn import(self: &Arc<Self>, folder: PathBuf, copy: Option<CopyOptions>) {
        let lib = self.clone();
        std::thread::spawn(move || {
            let res = match copy {
                None => lib.import_reference(&folder).map(|_| ()),
                Some(c) => lib.import_copy(&folder, &c),
            };
            if let Err(e) = res {
                (lib.emit)(Event::Error { message: format!("Import failed: {e:#}") });
                (lib.emit)(Event::ImportStatus { message: "Import failed".into(), done: 0, total: 0, finished: true });
            }
        });
    }

    fn import_reference(&self, folder: &Path) -> Result<usize> {
        if !folder.is_dir() {
            return Err(anyhow!("{} is not a folder", folder.display()));
        }
        let folder_id = self.catalog.add_folder(folder)?;
        let status = |msg: String, done, total, finished| {
            (self.emit)(Event::ImportStatus { message: msg, done, total, finished })
        };
        status(format!("Scanning {}", folder.display()), 0, 0, false);
        let known = self.catalog.known_paths(folder_id)?;
        let found = scan(folder);
        let mut fresh: Vec<NewPhoto> = found.into_iter().filter(|p| !known.contains(&path_str(&p.path))).collect();
        let total = fresh.len();
        let mut added = 0;
        // Insert in chunks so the grid fills in while big folders are scanned.
        for chunk in fresh.chunks_mut(500) {
            // Header metadata only (a few KB per file), in parallel.
            use rayon::prelude::*;
            chunk.par_iter_mut().for_each(|p| {
                if p.kind != Kind::Heic {
                    p.meta = util::guard(|| Ok(meta::read(&p.path, p.kind, &util::map_file(&p.path)?))).ok();
                }
            });
            let ids = self.catalog.insert_photos(folder_id, chunk, |p| sidecar::read(p))?;
            added += ids.len();
            for id in ids {
                self.enqueue(id);
            }
            (self.emit)(Event::CatalogChanged);
            status(format!("Added {added} of {total}"), added, total, false);
        }
        self.regroup.store(true, Ordering::SeqCst);
        (self.emit)(Event::CatalogChanged);
        status(format!("Imported {added} new photos from {}", folder.display()), added, total, true);
        Ok(added)
    }

    fn import_copy(&self, from: &Path, c: &CopyOptions) -> Result<()> {
        let dest_root = PathBuf::from(&c.destination);
        let files: Vec<PathBuf> = walk(from).collect();
        let total = files.len();
        let mut targets: HashSet<PathBuf> = HashSet::new();
        let mut errors = 0;
        for (i, f) in files.iter().enumerate() {
            let kind = formats::classify(f).unwrap();
            let when = util::map_file(f).ok().and_then(|b| meta::read(f, kind, &b).capture_time).or_else(|| {
                std::fs::metadata(f).and_then(|m| m.modified()).ok().map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
                })
            });
            let sub = copy_subfolder(&c.template, &c.shoot_name, when);
            let dir = dest_root.join(&sub);
            let target = dir.join(f.file_name().unwrap());
            let res = (|| -> Result<()> {
                std::fs::create_dir_all(&dir)?;
                let src_len = std::fs::metadata(f)?.len();
                if target.exists() {
                    if std::fs::metadata(&target)?.len() == src_len {
                        return Ok(()); // Already copied earlier.
                    }
                    return Err(anyhow!("{} exists with different content; not overwritten", target.display()));
                }
                let part = target.with_extension("keepers-part");
                std::fs::copy(f, &part)?;
                if std::fs::metadata(&part)?.len() != src_len {
                    let _ = std::fs::remove_file(&part);
                    return Err(anyhow!("copy of {} is incomplete", f.display()));
                }
                std::fs::rename(&part, &target)?;
                Ok(())
            })();
            if let Err(e) = res {
                errors += 1;
                (self.emit)(Event::Error { message: format!("{e:#}") });
            }
            targets.insert(dir);
            if i % 10 == 0 || i + 1 == total {
                (self.emit)(Event::ImportStatus {
                    message: format!("Copying {} of {total}", i + 1),
                    done: i + 1,
                    total,
                    finished: false,
                });
            }
        }
        let mut dirs: Vec<_> = targets.into_iter().collect();
        dirs.sort();
        for d in dirs {
            self.import_reference(&d)?;
        }
        if errors > 0 {
            (self.emit)(Event::Error { message: format!("{errors} file(s) could not be copied") });
        }
        Ok(())
    }

    // ----------------------------------------------------- processing --

    /// Read metadata, build the thumbnail and compute scores for one photo.
    pub fn process(&self, id: i64) -> Result<()> {
        let Some(p) = self.catalog.photo(id)? else { return Ok(()) };
        let path = PathBuf::from(&p.path);
        let buf = match util::map_file(&path) {
            Ok(b) => b,
            Err(e) => {
                self.catalog.set_processed(id, &meta::Meta { orientation: 1, ..Default::default() }, None, None, Some(&format!("Cannot read file: {e}")))?;
                return Ok(());
            }
        };
        let m = util::guard(|| Ok(meta::read(&path, p.kind, &buf))).unwrap_or(meta::Meta { orientation: 1, ..Default::default() });
        drop(buf);
        if p.kind == Kind::Heic {
            self.catalog.set_processed(id, &m, None, None, None)?;
            return Ok(());
        }
        let pair = p.pair_path.as_ref().map(PathBuf::from);
        let src = Source { path: &path, kind: p.kind, orientation: m.orientation, pair: pair.as_deref() };
        match preview::load(&src, ANALYSIS_EDGE) {
            Ok(loaded) => {
                let scores = scoring::analyze(&loaded.image);
                let thumb = loaded.image.thumbnail(THUMB_EDGE, THUMB_EDGE);
                write_atomic(&self.thumb_path(id), &preview::encode_jpeg(&thumb, 82)?)?;
                self.catalog.set_processed(id, &m, Some((loaded.full_w, loaded.full_h)), Some(&scores), None)?;
            }
            Err(e) => {
                log::warn!("preview failed for {}: {e:#}", p.path);
                self.catalog.set_processed(id, &m, None, None, Some(&format!("{e:#}")))?;
            }
        }
        Ok(())
    }

    fn cache_file(&self, kind: &str, id: i64, suffix: &str) -> PathBuf {
        self.cache.join(kind).join(format!("{}", id / 1000)).join(format!("{id}{suffix}.jpg"))
    }

    fn thumb_path(&self, id: i64) -> PathBuf {
        self.cache_file("thumbs", id, "")
    }

    /// Grid thumbnail JPEG, generated on demand if the workers have not got
    /// to it yet (visible cells jump the queue).
    pub fn thumb(&self, id: i64) -> Result<Vec<u8>> {
        let p = self.thumb_path(id);
        if let Ok(b) = std::fs::read(&p) {
            return Ok(b);
        }
        let photo = self.catalog.photo(id)?.ok_or_else(|| anyhow!("no photo {id}"))?;
        if photo.kind == Kind::Heic {
            return Err(anyhow!("no HEIC preview"));
        }
        if let (true, Some(e)) = (photo.processed, &photo.error) {
            // Already known to be unreadable; don't retry on every request.
            return Err(anyhow!("{e}"));
        }
        self.process(id)?;
        self.updated.lock().push(id);
        std::fs::read(&p).with_context(|| format!("no thumbnail for {}", photo.file_name))
    }

    /// Loupe preview: `full` returns the largest available preview at native
    /// resolution (for 100% zoom), otherwise a 2560px version.
    pub fn preview(&self, id: i64, full: bool) -> Result<Vec<u8>> {
        let photo = self.catalog.photo(id)?.ok_or_else(|| anyhow!("no photo {id}"))?;
        // Camera JPEGs go to the webview untouched: it decodes them natively and
        // honours their orientation tag.
        if photo.kind == Kind::Jpeg && (full || std::fs::metadata(&photo.path).map(|m| m.len() < 6_000_000).unwrap_or(false)) {
            return Ok(std::fs::read(&photo.path)?);
        }
        if photo.kind == Kind::Heic {
            // WebKit (macOS) decodes HEIC natively; elsewhere the UI shows a placeholder.
            return Ok(std::fs::read(&photo.path)?);
        }
        let cache = self.cache_file("previews", id, if full { "_full" } else { "" });
        if let Ok(b) = std::fs::read(&cache) {
            return Ok(b);
        }
        let path = PathBuf::from(&photo.path);
        if photo.kind == Kind::Raw && photo.orientation <= 1 {
            // Unrotated embedded preview: serve the camera's JPEG bytes as is.
            let buf = util::map_file(&path)?;
            let jpegs = crate::embedded::find_jpegs(&buf);
            if let Some(j) = preview::choose(&jpegs, if full { 0 } else { PREVIEW_EDGE }) {
                if j.long_edge() >= 1000 && (full || j.long_edge() <= PREVIEW_EDGE * 3 / 2) {
                    return Ok(j.bytes(&buf).to_vec());
                }
            }
        }
        let pair = photo.pair_path.as_ref().map(PathBuf::from);
        let src = Source { path: &path, kind: photo.kind, orientation: photo.orientation, pair: pair.as_deref() };
        let loaded = preview::load(&src, if full { 0 } else { PREVIEW_EDGE })?;
        let bytes = preview::encode_jpeg(&loaded.image, if full { 92 } else { 88 })?;
        write_atomic(&cache, &bytes)?;
        Ok(bytes)
    }

    // -------------------------------------------------------- culling --

    pub fn set_pick(&self, ids: &[i64], pick: i8) -> Result<Vec<Photo>> {
        self.catalog.set_pick(ids, pick.clamp(-1, 1))?;
        self.after_flag_change(ids)
    }

    pub fn set_rating(&self, ids: &[i64], rating: u8) -> Result<Vec<Photo>> {
        self.catalog.set_rating(ids, rating)?;
        self.after_flag_change(ids)
    }

    fn after_flag_change(&self, ids: &[i64]) -> Result<Vec<Photo>> {
        for &id in ids {
            let _ = self.sidecar_tx.send(id);
        }
        self.catalog.photos_by_ids(ids)
    }

    pub fn set_burst_best(&self, id: i64) -> Result<Vec<Photo>> {
        let members = self.catalog.set_burst_best(id)?;
        self.catalog.photos_by_ids(&members)
    }

    pub fn regroup(&self) -> Result<()> {
        let shots = self.catalog.shots()?;
        let groups = bursts::group(&shots);
        self.catalog.set_groupings(&shots, &groups)
    }

    pub fn set_write_sidecars(&self, on: bool) -> Result<()> {
        self.write_sidecars.store(on, Ordering::SeqCst);
        self.catalog.set_setting("writeSidecars", if on { "true" } else { "false" })
    }

    pub fn write_sidecars(&self) -> bool {
        self.write_sidecars.load(Ordering::SeqCst)
    }

    pub fn photos(&self, folder: Option<i64>) -> Result<Vec<Photo>> {
        self.catalog.photos(folder)
    }

    pub fn folders(&self) -> Result<Vec<Folder>> {
        self.catalog.folders()
    }

    pub fn remove_folder(&self, id: i64) -> Result<()> {
        // Catalog only: never touches files on disk.
        self.catalog.remove_folder(id)
    }

    // -------------------------------------------------------- develop --

    fn develop_source(&self, photo: &Photo) -> Result<Arc<source::Linear>> {
        {
            let mut c = self.develop_cache.lock();
            if let Some(i) = c.iter().position(|(id, _)| *id == photo.id) {
                let e = c.remove(i);
                let s = e.1.clone();
                c.insert(0, e);
                return Ok(s);
            }
        }
        let lin = Arc::new(source::load(Path::new(&photo.path), photo.kind, photo.orientation, Some(DEVELOP_EDGE))?);
        let mut c = self.develop_cache.lock();
        c.insert(0, (photo.id, lin.clone()));
        c.truncate(4);
        Ok(lin)
    }

    /// Decode a photo for developing ahead of time (e.g. the next keeper).
    pub fn warm(&self, id: i64) {
        if let Ok(Some(p)) = self.catalog.photo(id) {
            let _ = self.develop_source(&p);
        }
    }

    pub fn render(&self, id: i64, edits: &Edits, ignore_crop: bool, clip_warnings: bool) -> Result<RenderResult> {
        let photo = self.catalog.photo(id)?.ok_or_else(|| anyhow!("no photo {id}"))?;
        let src = self.develop_source(&photo)?;
        let full_long = photo.width.zip(photo.height).map(|(w, h)| w.max(h) as f32).unwrap_or(src.long_edge() as f32);
        let opts = RenderOpts { ignore_crop, clip_warnings, scale: (src.long_edge() as f32 / full_long).min(1.0) };
        let (img, histogram) = pipeline::render_image(&src, edits, &opts);
        Ok(RenderResult { jpeg: preview::encode_jpeg(&img, 90)?, width: img.width(), height: img.height(), histogram })
    }

    pub fn photo_edited(&self, id: i64) -> bool {
        self.catalog.photo(id).ok().flatten().is_some_and(|p| p.edited)
    }

    pub fn edits(&self, id: i64) -> Result<Edits> {
        Ok(Edits::from_json(self.catalog.edits(id)?.as_deref()))
    }

    pub fn save_edits(&self, ids: &[i64], edits: &Edits) -> Result<Vec<Photo>> {
        let json = if edits.is_default() { None } else { Some(serde_json::to_string(edits)?) };
        for &id in ids {
            self.catalog.set_edits(id, json.as_deref())?;
            let _ = self.sidecar_tx.send(id);
        }
        self.catalog.photos_by_ids(ids)
    }

    pub fn presets(&self) -> Result<Vec<Preset>> {
        let mut all = builtin_presets();
        if let Some(s) = self.catalog.setting("presets")? {
            if let Ok(user) = serde_json::from_str::<Vec<Preset>>(&s) {
                all.extend(user.into_iter().map(|p| Preset { builtin: false, ..p }));
            }
        }
        Ok(all)
    }

    pub fn save_preset(&self, name: &str, edits: &Edits) -> Result<Vec<Preset>> {
        let mut user: Vec<Preset> = self.presets()?.into_iter().filter(|p| !p.builtin && p.name != name).collect();
        // Presets carry look, not framing.
        let edits = Edits { crop: None, angle: 0.0, rotate: 0, flip_h: false, ..edits.clone() };
        user.push(Preset { name: name.into(), edits, builtin: false });
        self.catalog.set_setting("presets", &serde_json::to_string(&user)?)?;
        self.presets()
    }

    pub fn delete_preset(&self, name: &str) -> Result<Vec<Preset>> {
        let user: Vec<Preset> = self.presets()?.into_iter().filter(|p| !p.builtin && p.name != name).collect();
        self.catalog.set_setting("presets", &serde_json::to_string(&user)?)?;
        self.presets()
    }

    // --------------------------------------------------------- export --

    pub fn export(self: &Arc<Self>, ids: Vec<i64>, settings: ExportSettings) {
        let lib = self.clone();
        std::thread::spawn(move || {
            let total = ids.len();
            let mut errors = Vec::new();
            let settings_json = serde_json::to_string(&settings).unwrap_or_default();
            for (i, id) in ids.iter().enumerate() {
                let res = (|| -> Result<PathBuf> {
                    let photo = lib.catalog.photo(*id)?.ok_or_else(|| anyhow!("photo {id} is gone"))?;
                    let edits = Edits::from_json(lib.catalog.edits(*id)?.as_deref());
                    let out = util::guard(|| export::export_one(&photo, &edits, &settings, i + 1))
                        .with_context(|| photo.file_name.clone())?;
                    lib.catalog.record_export(*id, &out, &settings_json)?;
                    Ok(out)
                })();
                let last = match res {
                    Ok(p) => Some(path_str(&p)),
                    Err(e) => {
                        errors.push(format!("{e:#}"));
                        None
                    }
                };
                (lib.emit)(Event::ExportStatus { done: i + 1, total, errors: errors.clone(), last, finished: i + 1 == total });
            }
            if total == 0 {
                (lib.emit)(Event::ExportStatus { done: 0, total: 0, errors, last: None, finished: true });
            }
        });
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id() as u64 ^ util::now_ms() as u64));
    std::fs::write(&tmp, bytes)?;
    if std::fs::rename(&tmp, path).is_err() {
        // Another thread won the race; theirs is identical.
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(())
}

pub fn builtin_presets() -> Vec<Preset> {
    let p = |name: &str, edits: Edits| Preset { name: name.into(), edits, builtin: true };
    vec![
        p("Neutral", Edits::default()),
        p("Punchy", Edits { contrast: 30.0, vibrance: 25.0, shadows: 15.0, highlights: -20.0, ..Default::default() }),
        p("Soft Matte", Edits { contrast: -20.0, curve: vec![[0.0, 0.06], [0.5, 0.5], [1.0, 0.96]], saturation: -10.0, ..Default::default() }),
        p("Black & White", Edits { saturation: -100.0, contrast: 25.0, ..Default::default() }),
        p("Warm", Edits { temp: 20.0, vibrance: 10.0, ..Default::default() }),
        p("Cool", Edits { temp: -20.0, ..Default::default() }),
    ]
}

/// Supported files under `root`, skipping hidden folders and our own sidecars.
fn walk(root: &Path) -> impl Iterator<Item = PathBuf> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| !sidecar::is_sidecar(p) && formats::classify(p).is_some())
}

/// Scan a folder into catalog items, stacking RAW+JPEG pairs (same folder,
/// same base name) into one item with the RAW as primary.
pub fn scan(root: &Path) -> Vec<NewPhoto> {
    let mut groups: HashMap<(PathBuf, String), Vec<(PathBuf, Kind)>> = HashMap::new();
    let mut order = Vec::new();
    for p in walk(root) {
        let kind = formats::classify(&p).unwrap();
        let key = (
            p.parent().map(Path::to_path_buf).unwrap_or_default(),
            p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default(),
        );
        let g = groups.entry(key.clone()).or_default();
        if g.is_empty() {
            order.push(key);
        }
        g.push((p, kind));
    }
    let stat = |p: &Path| {
        let m = std::fs::metadata(p).ok();
        let size = m.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = m
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        (size, mtime)
    };
    let mut out = Vec::new();
    for key in order {
        let mut files = groups.remove(&key).unwrap();
        files.sort();
        let raw = files.iter().position(|f| f.1 == Kind::Raw);
        let jpeg = files.iter().position(|f| f.1 == Kind::Jpeg);
        let paired = match (raw, jpeg) {
            (Some(r), Some(j)) => Some((r, j)),
            _ => None,
        };
        for (i, (path, kind)) in files.iter().enumerate() {
            if paired.is_some_and(|(_, j)| j == i) {
                continue;
            }
            let pair = paired.filter(|(r, _)| *r == i).map(|(_, j)| files[j].0.clone());
            let (size, mtime) = stat(path);
            out.push(NewPhoto { path: path.clone(), kind: *kind, pair, size, mtime, meta: None });
        }
    }
    out
}

pub fn copy_subfolder(template: &str, shoot: &str, when_ms: Option<i64>) -> PathBuf {
    let dt = when_ms.and_then(chrono::DateTime::from_timestamp_millis);
    let f = |fmt: &str| dt.map(|d| d.format(fmt).to_string()).unwrap_or_else(|| "undated".into());
    let t = if template.trim().is_empty() { "{date}" } else { template };
    let s = t
        .replace("{YYYY}", &f("%Y"))
        .replace("{MM}", &f("%m"))
        .replace("{DD}", &f("%d"))
        .replace("{date}", &f("%Y-%m-%d"))
        .replace("{name}", shoot.trim());
    // Each path component is sanitised separately so templates may nest folders.
    s.split(['/', '\\'])
        .map(str::trim)
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .map(export::sanitize)
        .collect()
}

fn worker(lib: std::sync::Weak<Library>, rx: Receiver<i64>) {
    while let Ok(id) = rx.recv() {
        let Some(lib) = lib.upgrade() else { return };
        if let Err(e) = lib.process(id) {
            log::error!("processing {id}: {e:#}");
        }
        lib.updated.lock().push(id);
        lib.done.fetch_add(1, Ordering::SeqCst);
    }
}

/// Batches per-photo updates into a few UI events per second and regroups
/// bursts once a queue drains.
fn event_pump(lib: std::sync::Weak<Library>) {
    let mut last_progress = (usize::MAX, usize::MAX);
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let Some(lib) = lib.upgrade() else { return };
        let ids: Vec<i64> = {
            let mut u = lib.updated.lock();
            let mut v: Vec<i64> = u.drain(..).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        if !ids.is_empty() {
            if let Ok(photos) = lib.catalog.photos_by_ids(&ids) {
                (lib.emit)(Event::PhotosUpdated { photos });
            }
        }
        let (done, total) = (lib.done.load(Ordering::SeqCst), lib.queued.load(Ordering::SeqCst));
        if (done, total) != last_progress {
            (lib.emit)(Event::Progress { done, total });
            last_progress = (done, total);
        }
        if done >= total && lib.regroup.swap(false, Ordering::SeqCst) {
            let t = Instant::now();
            match lib.regroup() {
                Ok(()) => (lib.emit)(Event::CatalogChanged),
                Err(e) => log::error!("regroup: {e:#}"),
            }
            log::info!("regrouped in {:?}", t.elapsed());
        } else if !ids.is_empty() && done < total {
            lib.regroup.store(true, Ordering::SeqCst);
        }
    }
}

/// Coalesces sidecar writes: a burst of keystrokes on one photo is one write.
fn sidecar_writer(lib: std::sync::Weak<Library>, rx: Receiver<i64>) {
    while let Ok(first) = rx.recv() {
        let mut ids = HashSet::from([first]);
        std::thread::sleep(Duration::from_millis(400));
        ids.extend(rx.try_iter());
        let Some(lib) = lib.upgrade() else { return };
        if !lib.write_sidecars() {
            continue;
        }
        for id in ids {
            if let Ok(Some((path, pick, rating, edits))) = lib.catalog.sidecar_state(id) {
                if let Err(e) = sidecar::write(Path::new(&path), pick, rating, edits.as_deref()) {
                    log::warn!("sidecar for {path}: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(path: &Path, w: u32, h: u32) {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 60]));
        img.save(path).unwrap();
    }

    #[test]
    fn scan_pairs_raw_and_jpeg() {
        let d = tempfile::tempdir().unwrap();
        for f in ["L1.DNG", "L1.JPG", "L2.jpg", "L3.dng", "notes.txt", "L2.jpg.keepers.json"] {
            std::fs::write(d.path().join(f), b"x").unwrap();
        }
        std::fs::create_dir(d.path().join(".hidden")).unwrap();
        std::fs::write(d.path().join(".hidden/x.jpg"), b"x").unwrap();
        let mut items = scan(d.path());
        items.sort_by(|a, b| a.path.cmp(&b.path));
        let names: Vec<_> = items.iter().map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["L1.DNG", "L2.jpg", "L3.dng"]);
        assert!(items[0].pair.as_ref().unwrap().ends_with("L1.JPG"));
        assert!(items[1].pair.is_none());
    }

    #[test]
    fn subfolder_templates() {
        let t = Some(1714564800000);
        assert_eq!(copy_subfolder("{date}", "", t), PathBuf::from("2024-05-01"));
        assert_eq!(copy_subfolder("{YYYY}/{MM}-{DD} {name}", "Wedding", t), PathBuf::from("2024").join("05-01 Wedding"));
        assert_eq!(copy_subfolder("../{date}", "", None), PathBuf::from("undated"));
    }

    #[test]
    fn end_to_end_import_cull_develop_export() {
        let shoot = tempfile::tempdir().unwrap();
        for i in 0..5 {
            jpeg(&shoot.path().join(format!("IMG_{i}.jpg")), 1200, 800);
        }
        std::fs::write(shoot.path().join("broken.jpg"), b"\xFF\xD8\xFF\xE0 not really a jpeg").unwrap();
        let data = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let lib = Library::open(data.path(), Box::new(move |e| ev.lock().push(e))).unwrap();
        assert_eq!(lib.import_reference(shoot.path()).unwrap(), 6);
        // Wait for the workers.
        let t = Instant::now();
        while lib.done.load(Ordering::SeqCst) < 6 {
            assert!(t.elapsed() < Duration::from_secs(60), "processing timed out");
            std::thread::sleep(Duration::from_millis(50));
        }
        let photos = lib.photos(None).unwrap();
        assert_eq!(photos.len(), 6);
        let broken = photos.iter().find(|p| p.file_name == "broken.jpg").unwrap();
        assert!(broken.error.is_some(), "corrupt file must be flagged");
        let good: Vec<_> = photos.iter().filter(|p| p.error.is_none()).collect();
        assert_eq!(good.len(), 5);
        assert!(good.iter().all(|p| p.width == Some(1200) && p.sharpness.is_some()));
        assert!(!lib.thumb(good[0].id).unwrap().is_empty());
        assert!(lib.thumb(broken.id).is_err());

        let ids: Vec<i64> = good.iter().map(|p| p.id).collect();
        let upd = lib.set_pick(&ids[..2], 1).unwrap();
        assert!(upd.iter().all(|p| p.pick == 1));
        lib.set_rating(&ids[..1], 4).unwrap();
        // Sidecar is written in the background.
        let sc = sidecar::path_for(Path::new(&good[0].path));
        let t = Instant::now();
        while !sc.exists() {
            assert!(t.elapsed() < Duration::from_secs(5), "sidecar not written");
            std::thread::sleep(Duration::from_millis(50));
        }

        let e = Edits { exposure: 0.5, ..Default::default() };
        let r = lib.render(ids[0], &e, false, false).unwrap();
        assert_eq!((r.width, r.height), (1200, 800));
        lib.save_edits(&ids[..1], &e).unwrap();
        assert_eq!(lib.edits(ids[0]).unwrap(), e);
        assert!(lib.photo_edited(ids[0]));
        let t = Instant::now();
        while !std::fs::read_to_string(&sc).unwrap_or_default().contains("exposure") {
            assert!(t.elapsed() < Duration::from_secs(5), "edits not written to sidecar");
            std::thread::sleep(Duration::from_millis(50));
        }

        let out = tempfile::tempdir().unwrap();
        let mut s = export::presets()[0].1.clone();
        s.folder = path_str(out.path());
        s.long_edge = Some(600);
        lib.export(ids[..2].to_vec(), s);
        let t = Instant::now();
        loop {
            let fin = events.lock().iter().any(|e| matches!(e, Event::ExportStatus { finished: true, .. }));
            if fin {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(60), "export timed out");
            std::thread::sleep(Duration::from_millis(50));
        }
        let files: Vec<_> = std::fs::read_dir(out.path()).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), 2);
        assert_eq!(image::open(&files[0]).unwrap().width(), 600);
        assert_eq!(lib.catalog.export_history(ids[0]).unwrap().len(), 1);

        // Rebuilding the catalog from scratch restores cull state from sidecars.
        let data2 = tempfile::tempdir().unwrap();
        let lib2 = Library::open(data2.path(), Box::new(|_| {})).unwrap();
        lib2.import_reference(shoot.path()).unwrap();
        let p = lib2.photos(None).unwrap().into_iter().find(|p| p.path == good[0].path).unwrap();
        assert_eq!((p.pick, p.rating, p.edited), (1, 4, true));
    }
}
