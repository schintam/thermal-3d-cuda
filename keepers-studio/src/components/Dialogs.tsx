import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { CopyOptions, ExportSettings, ExternalSource, SuggestedSources, api, modLabel } from "../api";

function useStored<T>(key: string, initial: T): [T, (v: T) => void] {
  const [v, setV] = useState<T>(() => {
    try {
      const s = localStorage.getItem(key);
      return s ? { ...initial, ...JSON.parse(s) } : initial;
    } catch {
      return initial;
    }
  });
  const set = (n: T) => {
    setV(n);
    try {
      localStorage.setItem(key, JSON.stringify(n));
    } catch {
      /* storage unavailable: not fatal */
    }
  };
  return [v, set];
}

async function pickFolder(title: string): Promise<string | null> {
  const r = await openDialog({ directory: true, multiple: false, title });
  return typeof r === "string" ? r : null;
}

type ImportFrom = "folder" | "lightroom" | "apple" | "takeout";

const IMPORT_TABS: { key: ImportFrom; label: string }[] = [
  { key: "folder", label: "Folder or card" },
  { key: "lightroom", label: "Lightroom Classic" },
  { key: "apple", label: "Apple Photos" },
  { key: "takeout", label: "Google Photos" },
];

export function ImportDialog({ onClose, initialSource }: { onClose: () => void; initialSource?: string }) {
  const [from, setFrom] = useState<ImportFrom>(() => {
    const s = initialSource?.toLowerCase() ?? "";
    if (s.endsWith(".lrcat")) return "lightroom";
    if (s.endsWith(".photoslibrary")) return "apple";
    if (s.endsWith(".zip")) return "takeout";
    return "folder";
  });
  const tabs = (
    <div className="segmented import-tabs">
      {IMPORT_TABS.map((t) => (
        <button key={t.key} className={from === t.key ? "on" : ""} onClick={() => setFrom(t.key)}>
          {t.label}
        </button>
      ))}
    </div>
  );
  return from === "folder" ? (
    <FolderImport onClose={onClose} initialSource={initialSource} tabs={tabs} />
  ) : (
    <ExternalImport key={from} from={from} onClose={onClose} initialSource={initialSource} tabs={tabs} />
  );
}

async function pickFile(title: string, name: string, extensions: string[], multiple = false): Promise<string[]> {
  const r = await openDialog({ title, multiple, directory: false, filters: [{ name, extensions }] });
  if (r == null) return [];
  return Array.isArray(r) ? r : [r];
}

function ExternalImport({
  from,
  onClose,
  initialSource,
  tabs,
}: {
  from: Exclude<ImportFrom, "folder">;
  onClose: () => void;
  initialSource?: string;
  tabs: React.ReactNode;
}) {
  const [path, setPath] = useState(initialSource ?? "");
  const [zips, setZips] = useState<string[]>(initialSource?.toLowerCase().endsWith(".zip") ? [initialSource] : []);
  const [dest, setDest] = useStored("keepers.takeoutDest", { folder: "" });
  const [suggested, setSuggested] = useState<SuggestedSources | null>(null);
  const [platform, setPlatform] = useState("");

  useEffect(() => {
    api.suggestedSources().then((s) => {
      setSuggested(s);
      if (!path) {
        if (from === "lightroom" && s.lightroom.length) setPath(s.lightroom[0]);
        if (from === "apple" && s.applePhotos) setPath(s.applePhotos);
      }
    });
    api.settings().then((st) => {
      setPlatform(st.platform);
      if (!dest.folder && st.picturesDir) setDest({ folder: st.picturesDir + (st.platform === "windows" ? "\\" : "/") + "Google Photos" });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const source: ExternalSource | null =
    from === "lightroom"
      ? path
        ? { kind: "lightroom", path }
        : null
      : from === "apple"
        ? path
          ? { kind: "applePhotos", path }
          : null
        : zips.length || path
          ? { kind: "takeout", paths: zips.length ? zips : [path], destination: zips.length ? dest.folder : null }
          : null;

  const start = () => {
    if (!source) return;
    api.importExternal(source);
    onClose();
  };

  return (
    <Modal title="Import photos" onClose={onClose}>
      {tabs}
      {from === "lightroom" && (
        <>
          <div className="field">
            <label>Lightroom Classic catalog (.lrcat)</label>
            <div className="row">
              <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="Choose a catalog…" />
              <button onClick={async () => setPath((await pickFile("Choose a Lightroom catalog", "Lightroom catalog", ["lrcat"]))[0] ?? path)}>
                Browse…
              </button>
            </div>
            {suggested && suggested.lightroom.length > 1 && (
              <div className="segmented">
                {suggested.lightroom.map((c) => (
                  <button key={c} className={c === path ? "on" : ""} onClick={() => setPath(c)}>
                    {c.split(/[\\/]/).pop()}
                  </button>
                ))}
              </div>
            )}
          </div>
          <p className="hint">
            Brings in every photo in the catalog where it already lives, with its flags (pick/reject) and star ratings. Virtual
            copies are skipped. Lightroom develop settings can't be carried over. The catalog is only read, never changed —
            it's fine if Lightroom is open. For Lightroom (cloud), use its "Export originals" and import that folder.
          </p>
        </>
      )}
      {from === "apple" && (
        <>
          <div className="field">
            <label>Photos library</label>
            <div className="row">
              <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="~/Pictures/Photos Library.photoslibrary" />
              <button
                onClick={async () =>
                  setPath((await pickFile("Choose a Photos library", "Photos library", ["photoslibrary", "photolibrary"]))[0] ?? path)
                }
              >
                Browse…
              </button>
            </div>
          </div>
          {platform === "windows" ? (
            <p className="hint">
              Apple Photos libraries live on a Mac. On Windows, iCloud for Windows syncs your photos to a normal folder
              {suggested?.icloudPhotos ? (
                <>
                  {" "}
                  (<code>{suggested.icloudPhotos}</code>)
                </>
              ) : (
                " (usually Pictures\\iCloud Photos\\Photos)"
              )}{" "}
              — import that with <b>Folder or card</b>.
            </p>
          ) : (
            <p className="hint">
              Imports your originals in place with their real file names; favourites become picks; trashed and hidden photos are
              skipped. The library is only read. macOS may ask for permission to access Photos — allow it. Photos stored only in
              iCloud ("Optimise Mac Storage") can't be imported until they're downloaded. Older iPhoto libraries work too.
            </p>
          )}
        </>
      )}
      {from === "takeout" && (
        <>
          <p className="hint">
            Google doesn't let other apps read your Google Photos library directly. Export it with{" "}
            <b>Google Takeout</b> (takeout.google.com → select only Google Photos), download the zip files, then choose them
            here. Favourites become picks; capture dates come from Google's metadata when a photo has none.
          </p>
          <div className="field">
            <label>Takeout zip files</label>
            <div className="row">
              <input readOnly value={zips.length ? `${zips.length} zip file${zips.length > 1 ? "s" : ""} selected` : ""} placeholder="Choose takeout-*.zip…" />
              <button
                onClick={async () => {
                  const z = await pickFile("Choose Takeout zips", "Zip archives", ["zip"], true);
                  if (z.length) setZips(z);
                }}
              >
                Choose zips…
              </button>
            </div>
          </div>
          {zips.length > 0 ? (
            <div className="field">
              <label>Extract photos to</label>
              <div className="row">
                <input value={dest.folder} onChange={(e) => setDest({ folder: e.target.value })} />
                <button
                  onClick={async () => {
                    const d = await pickFolder("Extract Google Photos to…");
                    if (d) setDest({ folder: d });
                  }}
                >
                  Browse…
                </button>
              </div>
            </div>
          ) : (
            <div className="field">
              <label>…or an already-extracted Takeout folder</label>
              <div className="row">
                <input value={path} onChange={(e) => setPath(e.target.value)} />
                <button onClick={async () => setPath((await pickFolder("Choose the extracted Takeout folder")) ?? path)}>Browse…</button>
              </div>
            </div>
          )}
        </>
      )}
      <div className="actions">
        <button onClick={onClose}>Cancel</button>
        <button className="primary" disabled={!source || (from === "takeout" && zips.length > 0 && !dest.folder)} onClick={start}>
          Import
        </button>
      </div>
    </Modal>
  );
}

function FolderImport({ onClose, initialSource, tabs }: { onClose: () => void; initialSource?: string; tabs: React.ReactNode }) {
  const [source, setSource] = useState(initialSource ?? "");
  const [opts, setOpts] = useStored("keepers.import", {
    mode: "reference" as "reference" | "copy",
    destination: "",
    template: "{date} {name}",
  });
  const [shoot, setShoot] = useState("");

  useEffect(() => {
    if (!opts.destination)
      api.settings().then((s) => s.picturesDir && setOpts({ ...opts, destination: s.picturesDir }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const start = () => {
    const copy: CopyOptions | null =
      opts.mode === "copy" ? { destination: opts.destination, template: opts.template, shootName: shoot } : null;
    api.importFolder(source, copy);
    onClose();
  };

  const today = new Date().toISOString().slice(0, 10);
  const example = (opts.template || "{date}")
    .replace("{date}", today)
    .replace("{YYYY}", today.slice(0, 4))
    .replace("{MM}", today.slice(5, 7))
    .replace("{DD}", today.slice(8, 10))
    .replace("{name}", shoot)
    .trim();

  return (
    <Modal title="Import photos" onClose={onClose}>
      {tabs}
      <div className="field">
        <label>Source folder or card</label>
        <div className="row">
          <input autoFocus value={source} onChange={(e) => setSource(e.target.value)} placeholder="Choose a folder…" />
          <button onClick={async () => setSource((await pickFolder("Choose photos to import")) ?? source)}>Browse…</button>
        </div>
      </div>
      <div className="field">
        <label>Mode</label>
        <div className="segmented">
          <button className={opts.mode === "reference" ? "on" : ""} onClick={() => setOpts({ ...opts, mode: "reference" })}>
            Add in place
          </button>
          <button className={opts.mode === "copy" ? "on" : ""} onClick={() => setOpts({ ...opts, mode: "copy" })}>
            Copy to folder
          </button>
        </div>
        <p className="hint">
          {opts.mode === "reference"
            ? "Photos stay where they are; the catalog points at them."
            : "Copies files (e.g. off an SD card) into dated subfolders, verifies each copy, then catalogs the copies. Existing files are never overwritten."}
        </p>
      </div>
      {opts.mode === "copy" && (
        <>
          <div className="field">
            <label>Destination</label>
            <div className="row">
              <input value={opts.destination} onChange={(e) => setOpts({ ...opts, destination: e.target.value })} />
              <button
                onClick={async () => {
                  const d = await pickFolder("Copy photos to…");
                  if (d) setOpts({ ...opts, destination: d });
                }}
              >
                Browse…
              </button>
            </div>
          </div>
          <div className="field two">
            <div>
              <label>Subfolder template</label>
              <input value={opts.template} onChange={(e) => setOpts({ ...opts, template: e.target.value })} />
            </div>
            <div>
              <label>Shoot name</label>
              <input value={shoot} onChange={(e) => setShoot(e.target.value)} placeholder="optional" />
            </div>
          </div>
          <p className="hint">
            Tokens: {"{date}"} {"{YYYY}"} {"{MM}"} {"{DD}"} {"{name}"} — use / for nested folders. Example: <code>{example || today}</code>
          </p>
        </>
      )}
      <div className="actions">
        <button onClick={onClose}>Cancel</button>
        <button className="primary" disabled={!source || (opts.mode === "copy" && !opts.destination)} onClick={start}>
          Import
        </button>
      </div>
    </Modal>
  );
}

export type ExportScope = "selected" | "picks" | "visible";

export function ExportDialog({
  counts,
  onClose,
  onExport,
}: {
  counts: Record<ExportScope, number>;
  onClose: () => void;
  onExport: (scope: ExportScope, s: ExportSettings) => void;
}) {
  const [presets, setPresets] = useState<{ name: string; settings: ExportSettings }[]>([]);
  const [s, setS] = useStored<ExportSettings>("keepers.export", {
    format: "jpeg",
    quality: 85,
    longEdge: 2048,
    sixteenBit: false,
    sharpen: "screen",
    folder: "",
    naming: "{name}",
  });
  const [scope, setScope] = useState<ExportScope>(counts.selected > 1 ? "selected" : counts.picks > 0 ? "picks" : "selected");

  useEffect(() => {
    api.exportPresets().then(setPresets).catch(() => {});
    if (!s.folder) api.settings().then((st) => st.picturesDir && setS({ ...s, folder: st.picturesDir + (st.platform === "windows" ? "\\" : "/") + "Keepers Export" }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <Modal title="Export" onClose={onClose}>
      <div className="field">
        <label>Preset</label>
        <div className="segmented">
          {presets.map((p) => (
            <button key={p.name} onClick={() => setS({ ...p.settings, folder: s.folder, naming: s.naming })}>
              {p.name}
            </button>
          ))}
        </div>
      </div>
      <div className="field">
        <label>Photos</label>
        <div className="segmented">
          {(["selected", "picks", "visible"] as const).map((k) => (
            <button key={k} className={scope === k ? "on" : ""} disabled={counts[k] === 0} onClick={() => setScope(k)}>
              {k === "selected" ? "Selected" : k === "picks" ? "All picks" : "All shown"} ({counts[k]})
            </button>
          ))}
        </div>
      </div>
      <div className="field two">
        <div>
          <label>Format</label>
          <select value={s.format} onChange={(e) => setS({ ...s, format: e.target.value as ExportSettings["format"] })}>
            <option value="jpeg">JPEG</option>
            <option value="tiff">TIFF</option>
            <option value="png">PNG</option>
          </select>
        </div>
        {s.format === "jpeg" ? (
          <div>
            <label>Quality {s.quality}</label>
            <input type="range" min={40} max={100} value={s.quality} onChange={(e) => setS({ ...s, quality: Number(e.target.value) })} />
          </div>
        ) : (
          <div>
            <label>Bit depth</label>
            <select value={s.sixteenBit ? "16" : "8"} onChange={(e) => setS({ ...s, sixteenBit: e.target.value === "16" })}>
              <option value="8">8-bit</option>
              <option value="16">16-bit</option>
            </select>
          </div>
        )}
      </div>
      <div className="field two">
        <div>
          <label>Long edge (px)</label>
          <input
            type="number"
            placeholder="Full size"
            value={s.longEdge ?? ""}
            onChange={(e) => setS({ ...s, longEdge: e.target.value ? Math.max(16, Number(e.target.value)) : null })}
          />
        </div>
        <div>
          <label>Output sharpening</label>
          <select value={s.sharpen} onChange={(e) => setS({ ...s, sharpen: e.target.value as ExportSettings["sharpen"] })}>
            <option value="none">None</option>
            <option value="screen">Screen</option>
            <option value="print">Print</option>
          </select>
        </div>
      </div>
      <div className="field">
        <label>Folder</label>
        <div className="row">
          <input value={s.folder} onChange={(e) => setS({ ...s, folder: e.target.value })} />
          <button
            onClick={async () => {
              const d = await pickFolder("Export to…");
              if (d) setS({ ...s, folder: d });
            }}
          >
            Browse…
          </button>
        </div>
      </div>
      <div className="field">
        <label>File name</label>
        <input value={s.naming} onChange={(e) => setS({ ...s, naming: e.target.value })} />
        <p className="hint">
          Tokens: {"{name}"} {"{seq}"} {"{date}"} {"{time}"} {"{camera}"} {"{rating}"}. Existing files are never overwritten (a -1, -2… suffix is added).
        </p>
      </div>
      <div className="actions">
        <button onClick={onClose}>Cancel</button>
        <button
          className="primary"
          disabled={!s.folder || counts[scope] === 0}
          onClick={() => {
            onExport(scope, s);
            onClose();
          }}
        >
          Export {counts[scope]} photo{counts[scope] === 1 ? "" : "s"}
        </button>
      </div>
    </Modal>
  );
}

export function HelpOverlay({ onClose }: { onClose: () => void }) {
  const rows: [string, string][] = [
    ["P / X / U", "Pick / Reject / Unflag"],
    ["0 – 5", "Star rating"],
    ["← → ↑ ↓", "Move (Shift extends selection)"],
    ["Space", "Toggle grid ↔ loupe"],
    ["Enter / E", "Loupe"],
    ["G / Esc", "Grid"],
    ["C", "Compare selected (2–4) or this + next"],
    ["V", "Explore: looks & compositions"],
    ["[ / ]", "Explore: previous / next look · Enter applies"],
    ["D", "Develop"],
    ["Z", "Zoom 100% / fit"],
    ["I", "Info overlay"],
    ["B", "Choose this frame as burst best"],
    ["Shift+B", "Accept burst: pick best, reject the rest"],
    ["S", "Stack / unstack bursts"],
    ["A", "Toggle auto-advance after a decision"],
    [`${modLabel}Z`, "Undo last cull decision"],
    [`${modLabel}A`, "Select all shown"],
    [`${modLabel}Shift+C / V`, "Copy / paste develop settings"],
    [`${modLabel}E`, "Export"],
    [`${modLabel}I`, "Import"],
    ["R / J / \\", "Develop: crop · clipping · before/after"],
    ["?", "This help"],
  ];
  return (
    <Modal title="Keyboard shortcuts" onClose={onClose}>
      <table className="keys">
        <tbody>
          {rows.map(([k, v]) => (
            <tr key={k}>
              <td>
                <kbd>{k}</kbd>
              </td>
              <td>{v}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </Modal>
  );
}

function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: React.ReactNode }) {
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [onClose]);
  return (
    <div className="modal-bg" onMouseDown={onClose}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
        <h2>{title}</h2>
        {children}
      </div>
    </div>
  );
}
