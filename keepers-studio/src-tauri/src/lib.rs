//! Tauri shell: exposes the engine to the UI as commands, streams images over
//! custom URI schemes, and forwards engine events.

use keepers_core::catalog::{Folder, Photo};
use keepers_core::develop::Edits;
use keepers_core::export::{self, ExportSettings};
use keepers_core::library::{CopyOptions, Preset};
use keepers_core::{Event, Library};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::http::{Response as HttpResponse, StatusCode};
use tauri::ipc::Response;
use tauri::{Emitter, Manager, State};

struct App {
    lib: Arc<Library>,
    /// Bounded pool for image requests so a fast scroll can't spawn hundreds
    /// of decoders at once.
    pool: Arc<Pool>,
}

struct Pool(std::sync::Mutex<std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>>>);

impl Pool {
    fn new(n: usize) -> Pool {
        let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let rx = Arc::new(std::sync::Mutex::new(rx));
        for i in 0..n {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("keepers-images-{i}"))
                .spawn(move || loop {
                    let job = rx.lock().unwrap().recv();
                    match job {
                        Ok(f) => f(),
                        Err(_) => return,
                    }
                })
                .expect("spawn image thread");
        }
        Pool(std::sync::Mutex::new(tx))
    }

    fn run(&self, f: impl FnOnce() + Send + 'static) {
        let _ = self.0.lock().unwrap().send(Box::new(f));
    }
}

type Res<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Run blocking engine work off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(err)
}

#[tauri::command]
async fn folders(app: State<'_, App>) -> Res<Vec<Folder>> {
    let lib = app.lib.clone();
    blocking(move || lib.folders()).await
}

#[tauri::command]
async fn photos(app: State<'_, App>, folder_id: Option<i64>) -> Res<Vec<Photo>> {
    let lib = app.lib.clone();
    blocking(move || lib.photos(folder_id)).await
}

#[tauri::command]
fn import_folder(app: State<'_, App>, path: String, copy: Option<CopyOptions>) {
    app.lib.import(PathBuf::from(path), copy);
}

#[tauri::command]
async fn remove_folder(app: State<'_, App>, id: i64) -> Res<()> {
    let lib = app.lib.clone();
    blocking(move || lib.remove_folder(id)).await
}

#[tauri::command]
async fn set_pick(app: State<'_, App>, ids: Vec<i64>, pick: i8) -> Res<Vec<Photo>> {
    let lib = app.lib.clone();
    blocking(move || lib.set_pick(&ids, pick)).await
}

#[tauri::command]
async fn set_rating(app: State<'_, App>, ids: Vec<i64>, rating: u8) -> Res<Vec<Photo>> {
    let lib = app.lib.clone();
    blocking(move || lib.set_rating(&ids, rating)).await
}

#[tauri::command]
async fn set_burst_best(app: State<'_, App>, id: i64) -> Res<Vec<Photo>> {
    let lib = app.lib.clone();
    blocking(move || lib.set_burst_best(id)).await
}

#[tauri::command]
async fn get_edits(app: State<'_, App>, id: i64) -> Res<Edits> {
    let lib = app.lib.clone();
    blocking(move || lib.edits(id)).await
}

#[tauri::command]
async fn save_edits(app: State<'_, App>, ids: Vec<i64>, edits: Edits) -> Res<Vec<Photo>> {
    let lib = app.lib.clone();
    blocking(move || lib.save_edits(&ids, &edits)).await
}

/// Binary response: u32 width, u32 height, 3x256 u32 histogram (all
/// little-endian), then the JPEG.
#[tauri::command]
async fn render(app: State<'_, App>, id: i64, edits: Edits, ignore_crop: bool, clip_warnings: bool) -> Result<Response, String> {
    let lib = app.lib.clone();
    let r = blocking(move || lib.render(id, &edits, ignore_crop, clip_warnings)).await?;
    let mut out = Vec::with_capacity(8 + 3 * 256 * 4 + r.jpeg.len());
    out.extend_from_slice(&r.width.to_le_bytes());
    out.extend_from_slice(&r.height.to_le_bytes());
    for ch in &r.histogram {
        for v in ch {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out.extend_from_slice(&r.jpeg);
    Ok(Response::new(out))
}

#[tauri::command]
fn warm(app: State<'_, App>, id: i64) {
    let lib = app.lib.clone();
    std::thread::spawn(move || lib.warm(id));
}

#[tauri::command]
async fn presets(app: State<'_, App>) -> Res<Vec<Preset>> {
    let lib = app.lib.clone();
    blocking(move || lib.presets()).await
}

#[tauri::command]
async fn save_preset(app: State<'_, App>, name: String, edits: Edits) -> Res<Vec<Preset>> {
    let lib = app.lib.clone();
    blocking(move || lib.save_preset(&name, &edits)).await
}

#[tauri::command]
async fn delete_preset(app: State<'_, App>, name: String) -> Res<Vec<Preset>> {
    let lib = app.lib.clone();
    blocking(move || lib.delete_preset(&name)).await
}

#[derive(Serialize)]
struct NamedExport {
    name: &'static str,
    settings: ExportSettings,
}

#[tauri::command]
fn export_presets() -> Vec<NamedExport> {
    export::presets().into_iter().map(|(name, settings)| NamedExport { name, settings }).collect()
}

#[tauri::command]
fn export_photos(app: State<'_, App>, ids: Vec<i64>, settings: ExportSettings) {
    app.lib.export(ids, settings);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportRecord {
    path: String,
    settings: String,
    at: i64,
}

#[tauri::command]
async fn export_history(app: State<'_, App>, id: i64) -> Res<Vec<ExportRecord>> {
    let lib = app.lib.clone();
    blocking(move || {
        Ok(lib.catalog.export_history(id)?.into_iter().map(|(path, settings, at)| ExportRecord { path, settings, at }).collect())
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    write_sidecars: bool,
    platform: &'static str,
    pictures_dir: Option<String>,
}

#[tauri::command]
fn settings(app: State<'_, App>, handle: tauri::AppHandle) -> Settings {
    Settings {
        write_sidecars: app.lib.write_sidecars(),
        platform: std::env::consts::OS,
        pictures_dir: handle.path().picture_dir().ok().map(|p| p.to_string_lossy().into_owned()),
    }
}

#[tauri::command]
async fn set_write_sidecars(app: State<'_, App>, on: bool) -> Res<()> {
    let lib = app.lib.clone();
    blocking(move || lib.set_write_sidecars(on)).await
}

/// `kthumb://localhost/<id>`, `kprev://localhost/<id>`, `kfull://localhost/<id>`
/// (on Windows: `http://kthumb.localhost/<id>` etc.).
fn image_protocol(
    which: &'static str,
) -> impl Fn(tauri::UriSchemeContext<'_, tauri::Wry>, tauri::http::Request<Vec<u8>>, tauri::UriSchemeResponder) + Send + Sync + 'static
{
    move |ctx, req, responder| {
        let app = ctx.app_handle().state::<App>();
        let lib = app.lib.clone();
        let id: Option<i64> = req.uri().path().trim_start_matches('/').split('/').next().and_then(|s| s.parse().ok());
        app.pool.run(move || {
            let result = match id {
                None => Err(anyhow::anyhow!("bad image URL")),
                Some(id) => match which {
                    "thumb" => lib.thumb(id),
                    "full" => lib.preview(id, true),
                    _ => lib.preview(id, false),
                },
            };
            let resp = match result {
                Ok(bytes) => HttpResponse::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", sniff(&bytes))
                    .header("Cache-Control", "max-age=31536000")
                    .header("Access-Control-Allow-Origin", "*")
                    .body(bytes),
                Err(e) => HttpResponse::builder()
                    .status(StatusCode::NOT_FOUND)
                    .header("Content-Type", "text/plain")
                    .header("Access-Control-Allow-Origin", "*")
                    .body(format!("{e:#}").into_bytes()),
            };
            responder.respond(resp.expect("valid response"));
        });
    }
}

fn sniff(b: &[u8]) -> &'static str {
    if b.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if b.starts_with(b"\x89PNG") {
        "image/png"
    } else if b.len() > 12 && &b[4..8] == b"ftyp" {
        "image/heic"
    } else {
        "application/octet-stream"
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol("kthumb", image_protocol("thumb"))
        .register_asynchronous_uri_scheme_protocol("kprev", image_protocol("prev"))
        .register_asynchronous_uri_scheme_protocol("kfull", image_protocol("full"))
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let handle = app.handle().clone();
            let lib = Library::open(
                &dir,
                Box::new(move |e: Event| {
                    let _ = handle.emit("keepers", e);
                }),
            )?;
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
            app.manage(App { lib, pool: Arc::new(Pool::new(threads)) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            folders,
            photos,
            import_folder,
            remove_folder,
            set_pick,
            set_rating,
            set_burst_best,
            get_edits,
            save_edits,
            render,
            warm,
            presets,
            save_preset,
            delete_preset,
            export_presets,
            export_photos,
            export_history,
            settings,
            set_write_sidecars,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Keepers Studio");
}
