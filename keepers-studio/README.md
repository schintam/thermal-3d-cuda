# Keepers Studio

Cull and develop photos locally, in one cross-platform desktop app. No account, no upload, no telemetry.

**Stack:** Tauri 2 (Rust) · React + TypeScript · SQLite · `rawler` RAW core.

```
keepers-studio/
├── crates/keepers-core/   Engine: catalog, import, previews, scores, bursts, develop, export (Rust, 50 tests)
├── src-tauri/             Desktop shell: commands, image protocols, events
└── src/                   UI (React)
```

## Run it on Windows

### Option A — installer (no dev tools needed)

1. Open the repo on GitHub → **Actions** → **Keepers Studio** → the latest run for the `claude/keepers-studio` branch.
2. Download the **keepers-studio-Windows** artifact. It contains an `.msi` and a `-setup.exe`.
3. Run either one. The build isn't code-signed yet, so SmartScreen will warn you: click **More info → Run anyway**.

### Option B — from source

You need these once:

- [Rust](https://rustup.rs) (stable, MSVC toolchain), with **Visual Studio Build Tools 2022** and the "Desktop development with C++" workload
- [Node.js 20+](https://nodejs.org)
- WebView2 (already part of Windows 10/11)

Then:

```powershell
git clone -b claude/keepers-studio https://github.com/schintam/thermal-3d-cuda.git
cd thermal-3d-cuda\keepers-studio
npm install
npm run tauri dev        # development build with hot reload
npm run tauri build      # release build + installers in target\release\bundle\
```

The first build compiles about 400 crates and takes several minutes. Later builds are incremental.

The engine tests run without the UI: `cargo test -p keepers-core`.

Two command-line helpers are useful for testing against real folders:

```powershell
# Import a folder headlessly and report timings, bursts, duplicates and unreadable files
cargo run -p keepers-core --release --example inspect -- "D:\Photos\Q2 test" --list
# Generate a messy synthetic shoot (DNG+JPEG bursts, a duplicate, corrupt files)
cargo run -p keepers-core --release --example make_shoot -- C:\temp\shoot
```

## Using it

1. **Import.** Use **Import…** (Ctrl+I) or drop a folder on the window.
   - *Add in place*: the catalog points at the files where they are.
   - *Copy to folder*: copies files (e.g. off an SD card) into dated subfolders and verifies each copy. Nothing is ever overwritten.

   - *Lightroom Classic*: choose your `.lrcat` catalog. Every photo is added where it already lives, with its flags and star ratings. Virtual copies are skipped, and Lightroom develop settings aren't carried over. The catalog is only read, so it's fine if Lightroom is open.
   - *Apple Photos* (macOS): choose your `.photoslibrary` (older iPhoto libraries work too). Originals are imported in place with their real file names; favourites become picks; trashed and hidden photos are skipped. HEIC photos are decoded by macOS, so they get full previews and develop. Photos kept only in iCloud have to be downloaded first. On Windows, iCloud Photos syncs to a normal folder (`Pictures\iCloud Photos\Photos`); import that as a folder.
   - *Google Photos*: Google doesn't let other apps read a Photos library directly; since 2025 its API only exposes photos an app uploaded itself. Instead, export with [Google Takeout](https://takeout.google.com) (select only Google Photos), then choose the downloaded zips. They're extracted (resumably, never overwriting anything), photos that appear in both albums and year folders are kept once, favourites become picks, and Google's metadata fills in missing capture dates.

   The grid fills in as files are scanned. Previews and scores are built in the background, and visible thumbnails jump the queue.
2. **Cull.** All of this is keyboard-first (press `?` in the app for the full list):

   | Key | Action |
   |---|---|
   | `P` / `X` / `U` | Pick / reject / unflag (auto-advances; `A` toggles) |
   | `0`–`5` | Stars |
   | Arrows, `Space` | Move, grid ↔ loupe |
   | `Z` or click | 100% zoom; drag to pan |
   | `C` | Compare 2–4 selected photos, zoom/pan synced |
   | `B` / `Shift+B` | Choose this burst frame as best / pick best and reject the rest of the burst |
   | `S` | Stack / unstack bursts |
   | `Ctrl+Z` | Undo |

3. **Develop** (`D`). The filmstrip shows only your picks. Available tools: white balance, exposure, contrast, highlights/shadows/whites/blacks, vibrance/saturation, tone curve, HSL, crop/straighten/rotate (`R`), sharpening, noise reduction, and presets. Copy and paste settings with Ctrl+Shift+C / V. Hold `\` for before/after, and press `J` for clipping warnings.
4. **Export** (Ctrl+E): Web / Social / Print / Full-size presets, a naming template, background progress, and a per-photo export history.

On macOS the shortcuts use Cmd in place of Ctrl.

## Decisions taken for the spec's `[DECIDE]` items

These were picked so the build could go ahead. All of them are easy to change.

| Spec item | Choice made | Why |
|---|---|---|
| Platforms | Windows + macOS in CI; Linux builds from the same code | Windows first, as requested |
| RAW core | [`rawler`](https://github.com/dnglab/dnglab) (pure Rust, from dnglab) | No C toolchain pain on Windows; supports DNG/CR3/NEF/ARW/RAF. **Licence is LGPL-2.1, see below** |
| Sidecars | JSON: `<file>.keepers.json` next to each photo (can be switched off in the sidebar) | Simple and lossless for the edit stack. XMP export can come later for Lightroom interop |
| RAW+JPEG | One stacked item, RAW is primary, JPEG is the preview fallback | Spec's suggested default |
| HEIC | Imported with metadata. Preview shown on macOS only (WebKit decodes it); Windows shows a placeholder. Not developable | Avoids shipping a HEVC decoder in v1 |
| Key map | Lightroom / Photo Mechanic style, as in §5 | |
| Import template | `{date} {name}` → `2026-10-28 Wedding` | |
| Export naming | `{name}` (original file name) | |
| Export history | Implemented (catalog table) | |
| Lens profiles | DNG-embedded opcodes (WarpRectilinear + FixVignetteRadial), which covers the Leica Q2. No Lensfun yet | Q2 first, per spec |

## Status against the spec

**Built**
- Slice 1: scaffold, folder import (reference + copy), SQLite catalog, virtualised grid (only on-screen rows are in the DOM), loupe with 100% zoom, pick/reject/stars, and embedded-JPEG previews for DNG/NEF/ARW/CR2 (TIFF IFD walk), CR3 (BMFF PRVW + full JPEG track) and RAF. Corrupt files are flagged and skipped, never fatal.
- Slice 2: burst grouping (≤2 s apart and visually similar), suggested best frame, one-key override, near-duplicate flags, sharpness and exposure/clipping scores shown as badges (they never hide or delete anything), and 2–4 way synced compare.
- Slice 3: the develop pipeline (half-size fast preview, full-resolution export), every v1 panel control, presets, copy/paste settings, before/after, histogram + clipping overlay, batch paste, and JPEG/TIFF/PNG export.
- Slice 4 (partial): export presets, error handling for corrupt files, installers via CI.

**Not built yet / known gaps**
- **Blink / closed-eye detection.** This needs a local face/eye model (ONNX Runtime) whose licence must be chosen first (spec §9).
- **GPU (wgpu) develop path.** Develop currently runs on the CPU with rayon. That's fine for the 2048 px interactive preview, but a full-resolution 47 MP Q2 export takes a few seconds.
- **Local-only updater.** The Tauri updater needs a signing key and an update URL; both are your call.
- **Code signing** for Windows and macOS installers.
- **Highlights/shadows are global tone controls**, not local (no guided-filter "clarity"-style adaptation yet).
- **Export doesn't copy EXIF** (capture date, camera) into the output files yet.
- **ICC / Display P3:** output is sRGB and the UI renders in the webview's sRGB.
- **Watermarks** on export.
- **HEIC on Windows**: still a placeholder. macOS decodes HEIC through the OS; Windows would need a bundled HEVC decoder, which raises patent-licensing questions.
- **Lightroom develop settings** are not translated on import. Picks, rejects and stars are.
- **Not yet validated on real camera files.** The RAW path is tested end to end on *synthetic* DNGs (real Bayer data, white balance, colour matrix, orientation, embedded previews, lens opcodes, corrupt files). This environment couldn't download real samples from any camera. Running `inspect` on your Q2 folder is the next step (§8).

### Licensing flag (spec §9)
`rawler` is **LGPL-2.1**. Rust links it statically, which makes LGPL compliance for a closed-source commercial app awkward: you'd have to provide object files so users can relink. Options:

1. Keep it and publish the app's source.
2. Ask for a commercial licence from the dnglab authors.
3. Move the RAW core behind a dynamic-library boundary.
4. Switch to LibRaw (LGPL/CDDL dual; CDDL is friendlier) via FFI.

This needs a decision before any commercial release.

## Privacy
The app makes no network requests. The catalog and preview cache live in the OS app-data folder (`%APPDATA%\studio.keepers.desktop` on Windows). Originals are opened read-only. The only files ever written next to photos are the optional `.keepers.json` sidecars.
