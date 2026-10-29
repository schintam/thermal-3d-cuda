import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Edits, EngineEvent, ExportSettings, Folder, Photo, api, modKey, modLabel } from "./api";
import { Compare } from "./components/Compare";
import { Develop } from "./components/Develop";
import { Explore } from "./components/Explore";
import { ExportDialog, ExportScope, HelpOverlay, ImportDialog } from "./components/Dialogs";
import { Filmstrip } from "./components/Filmstrip";
import { Grid } from "./components/Grid";
import { Loupe } from "./components/Loupe";
import { FIT, ZoomState } from "./components/ZoomView";

type View = "grid" | "loupe" | "compare" | "explore" | "develop";
type FlagFilter = "all" | "picks" | "rejects" | "unflagged";
type Modal = { kind: "import"; source?: string } | { kind: "export" } | { kind: "help" } | null;

interface UndoEntry {
  prev: { id: number; pick: number; rating: number }[];
}

export default function App() {
  const [folders, setFolders] = useState<Folder[]>([]);
  const [folderId, setFolderId] = useState<number | null>(null);
  const [photos, setPhotos] = useState<Photo[]>([]);
  const [view, setView] = useState<View>("grid");
  const [focus, setFocus] = useState<number | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [flagFilter, setFlagFilter] = useState<FlagFilter>("all");
  const [minRating, setMinRating] = useState(0);
  const [collapse, setCollapse] = useState(true);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [thumbSize, setThumbSize] = useState(() => Number(localStorage.getItem("keepers.thumb")) || 200);
  const [autoAdvance, setAutoAdvance] = useState(true);
  const [zoom, setZoom] = useState<ZoomState>(FIT);
  const [showInfo, setShowInfo] = useState(true);
  const [compareIds, setCompareIds] = useState<number[]>([]);
  const [modal, setModal] = useState<Modal>(null);
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [importMsg, setImportMsg] = useState<string | null>(null);
  const [exportMsg, setExportMsg] = useState<{ text: string; last: string | null } | null>(null);
  const [errors, setErrors] = useState<string[]>([]);
  const [editsVersion, setEditsVersion] = useState(0);
  const [cropMode, setCropMode] = useState(false);
  const [clip, setClip] = useState(false);
  const [beforeHeld, setBeforeHeld] = useState(false);
  const [beforeLocked, setBeforeLocked] = useState(false);
  const [sidecars, setSidecars] = useState(true);
  const columns = useRef(1);
  /** Until the user navigates, focus follows the first photo as an import fills in. */
  const userFocused = useRef(false);
  const undo = useRef<UndoEntry[]>([]);
  const clipboard = useRef<Edits | null>(null);
  const developEdits = useRef<Edits | null>(null);

  const addError = useCallback((m: string) => setErrors((e) => [...e.slice(-4), m]), []);

  // ------------------------------------------------------------ data --
  const reload = useCallback(async () => {
    try {
      const [f, p] = await Promise.all([api.folders(), api.photos(folderId)]);
      setFolders(f);
      setPhotos(p);
    } catch (e) {
      addError(String(e));
    }
  }, [folderId, addError]);

  useEffect(() => {
    reload();
  }, [reload]);

  useEffect(() => {
    api.settings().then((s) => setSidecars(s.writeSidecars)).catch(() => {});
  }, []);

  const merge = useCallback((updated: Photo[]) => {
    if (!updated.length) return;
    const byId = new Map(updated.map((p) => [p.id, p]));
    setPhotos((prev) => prev.map((p) => byId.get(p.id) ?? p));
  }, []);

  // Engine events. Catalog-wide changes trigger a (throttled) reload.
  const reloadTimer = useRef<number | undefined>(undefined);
  useEffect(() => {
    const un = api.onEvent((e: EngineEvent) => {
      switch (e.type) {
        case "photosUpdated":
          merge(e.photos);
          break;
        case "catalogChanged":
          window.clearTimeout(reloadTimer.current);
          reloadTimer.current = window.setTimeout(reload, 300);
          break;
        case "progress":
          setProgress({ done: e.done, total: e.total });
          break;
        case "importStatus":
          setImportMsg(e.finished ? null : e.message);
          if (e.finished) {
            window.clearTimeout(reloadTimer.current);
            reload();
          }
          break;
        case "exportStatus":
          setExportMsg({
            text: e.finished
              ? `Exported ${e.done - e.errors.length} of ${e.total}${e.errors.length ? ` — ${e.errors.length} failed` : ""}`
              : `Exporting ${e.done} / ${e.total}…`,
            last: e.last,
          });
          if (e.finished && e.errors.length) e.errors.forEach(addError);
          break;
        case "error":
          addError(e.message);
          break;
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [merge, reload, addError]);

  // Drop a folder on the window to import it.
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "drop" && e.payload.paths.length) setModal({ kind: "import", source: e.payload.paths[0] });
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // ------------------------------------------------------- derived --
  const burstCounts = useMemo(() => {
    const m = new Map<number, number>();
    for (const p of photos) if (p.burstId != null) m.set(p.burstId, (m.get(p.burstId) ?? 0) + 1);
    return m;
  }, [photos]);

  const filtered = useMemo(
    () =>
      photos.filter(
        (p) =>
          (flagFilter === "all" ||
            (flagFilter === "picks" && p.pick === 1) ||
            (flagFilter === "rejects" && p.pick === -1) ||
            (flagFilter === "unflagged" && p.pick === 0)) &&
          p.rating >= minRating,
      ),
    [photos, flagFilter, minRating],
  );

  /** Bursts collapse to one representative (the suggested best, else the first shown). */
  const visible = useMemo(() => {
    if (!collapse) return filtered;
    const rep = new Map<number, number>();
    for (const p of filtered) {
      if (p.burstId == null || expanded.has(p.burstId)) continue;
      const cur = rep.get(p.burstId);
      if (cur == null || p.burstBest) rep.set(p.burstId, p.id);
    }
    return filtered.filter((p) => p.burstId == null || expanded.has(p.burstId) || rep.get(p.burstId) === p.id);
  }, [filtered, collapse, expanded]);

  const byId = useMemo(() => new Map(photos.map((p) => [p.id, p])), [photos]);
  const focusPhoto = focus != null ? byId.get(focus) ?? null : null;
  const keepers = useMemo(() => {
    const k = filtered.filter((p) => p.pick === 1);
    return k.length ? k : visible;
  }, [filtered, visible]);
  const navList = view === "develop" || view === "explore" ? keepers : visible;

  // Keep focus valid as filters change.
  useEffect(() => {
    if (!navList.length) return;
    if (focus == null || !userFocused.current || !navList.some((p) => p.id === focus)) {
      if (focus === navList[0].id) return;
      setFocus(navList[0].id);
      setSelected(new Set([navList[0].id]));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [navList]);

  // Pre-decode the next keeper while developing.
  useEffect(() => {
    if (view !== "develop" || focus == null) return;
    const i = keepers.findIndex((p) => p.id === focus);
    if (i >= 0 && i + 1 < keepers.length) api.warm(keepers[i + 1].id).catch(() => {});
  }, [view, focus, keepers]);

  // -------------------------------------------------------- actions --
  const targets = useCallback((): number[] => {
    if (view === "compare") return focus != null ? [focus] : [];
    if (focus != null && selected.has(focus) && selected.size > 1) return [...selected];
    return focus != null ? [focus] : [];
  }, [focus, selected, view]);

  const move = useCallback(
    (delta: number, extend = false) => {
      if (!navList.length) return;
      userFocused.current = true;
      const i = Math.max(0, navList.findIndex((p) => p.id === focus));
      const j = Math.min(navList.length - 1, Math.max(0, i + delta));
      const id = navList[j].id;
      setFocus(id);
      setZoom(FIT);
      setSelected((s) => {
        if (!extend) return new Set([id]);
        const n = new Set(s);
        const [a, b] = [Math.min(i, j), Math.max(i, j)];
        for (let k = a; k <= b; k++) n.add(navList[k].id);
        return n;
      });
    },
    [navList, focus],
  );

  const remember = (ids: number[]) => {
    undo.current.push({
      prev: ids.map((id) => {
        const p = byId.get(id)!;
        return { id, pick: p.pick, rating: p.rating };
      }),
    });
    if (undo.current.length > 200) undo.current.shift();
  };

  const decide = useCallback(
    async (kind: "pick" | "rating", value: number) => {
      const ids = targets();
      if (!ids.length) return;
      remember(ids);
      // Optimistic: the keystroke takes effect immediately.
      setPhotos((prev) => prev.map((p) => (ids.includes(p.id) ? { ...p, [kind]: value } : p)));
      const willHide =
        kind === "pick" &&
        ((flagFilter === "picks" && value !== 1) || (flagFilter === "rejects" && value !== -1) || (flagFilter === "unflagged" && value !== 0));
      if (autoAdvance && ids.length === 1 && view !== "compare" && !willHide) move(1);
      try {
        merge(kind === "pick" ? await api.setPick(ids, value) : await api.setRating(ids, value));
      } catch (e) {
        addError(String(e));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [targets, autoAdvance, view, move, merge, flagFilter, byId, addError],
  );

  const doUndo = useCallback(async () => {
    const u = undo.current.pop();
    if (!u) return;
    try {
      for (const pick of [-1, 0, 1]) {
        const ids = u.prev.filter((p) => p.pick === pick).map((p) => p.id);
        if (ids.length) merge(await api.setPick(ids, pick));
      }
      for (let r = 0; r <= 5; r++) {
        const ids = u.prev.filter((p) => p.rating === r).map((p) => p.id);
        if (ids.length) merge(await api.setRating(ids, r));
      }
      setFocus(u.prev[0].id);
    } catch (e) {
      addError(String(e));
    }
  }, [merge, addError]);

  const acceptBurst = useCallback(async () => {
    const p = focusPhoto;
    if (!p || p.burstId == null) return;
    const members = photos.filter((q) => q.burstId === p.burstId);
    const best = members.find((q) => q.burstBest) ?? p;
    const others = members.filter((q) => q.id !== best.id).map((q) => q.id);
    remember(members.map((m) => m.id));
    try {
      merge(await api.setPick([best.id], 1));
      if (others.length) merge(await api.setPick(others, -1));
    } catch (e) {
      addError(String(e));
    }
    if (autoAdvance) {
      // Jump past the whole burst.
      const idx = navList.findIndex((q) => q.id === p.id);
      const next = navList.slice(idx + 1).find((q) => q.burstId !== p.burstId);
      if (next) {
        setFocus(next.id);
        setSelected(new Set([next.id]));
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusPhoto, photos, merge, autoAdvance, navList, addError]);

  const copySettings = useCallback(async () => {
    if (focus == null) return;
    try {
      clipboard.current = view === "develop" && developEdits.current ? developEdits.current : await api.getEdits(focus);
      setExportMsg({ text: "Develop settings copied", last: null });
    } catch (e) {
      addError(String(e));
    }
  }, [focus, view, addError]);

  const pasteSettings = useCallback(async () => {
    const c = clipboard.current;
    if (!c) return;
    const ids = targets();
    try {
      // Paste look, keep each photo's own framing.
      for (const id of ids) {
        const own = await api.getEdits(id);
        merge(await api.saveEdits([id], { ...c, crop: own.crop, angle: own.angle, rotate: own.rotate, flipH: own.flipH }));
      }
      setEditsVersion((v) => v + 1);
      setExportMsg({ text: `Pasted settings to ${ids.length} photo${ids.length === 1 ? "" : "s"}`, last: null });
    } catch (e) {
      addError(String(e));
    }
  }, [targets, merge, addError]);

  const startCompare = useCallback(() => {
    let ids = [...selected].filter((id) => visible.some((p) => p.id === id));
    if (ids.length < 2 || ids.length > 4) {
      const i = visible.findIndex((p) => p.id === focus);
      ids = visible.slice(Math.max(0, i), Math.max(0, i) + 2).map((p) => p.id);
    }
    if (ids.length < 2) return;
    setCompareIds(ids.slice(0, 4));
    setFocus(ids[0]);
    setZoom(FIT);
    setView("compare");
  }, [selected, visible, focus]);

  const onExport = useCallback(
    (scope: ExportScope, s: ExportSettings) => {
      const ids =
        scope === "selected" ? targets() : scope === "picks" ? filtered.filter((p) => p.pick === 1).map((p) => p.id) : visible.map((p) => p.id);
      api.exportPhotos(ids, s).catch((e) => addError(String(e)));
      setExportMsg({ text: `Exporting 0 / ${ids.length}…`, last: null });
    },
    [targets, filtered, visible, addError],
  );

  // ------------------------------------------------------- keyboard --
  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (modal) return;
      const t = e.target as HTMLElement;
      if (t.tagName === "TEXTAREA" || t.tagName === "SELECT" || (t.tagName === "INPUT" && (t as HTMLInputElement).type !== "range")) return;
      // Let range sliders keep their arrow keys.
      if (t.tagName === "INPUT" && e.key.startsWith("Arrow")) return;
      const mod = modKey(e);
      const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;

      if (mod) {
        if (k === "z") return e.preventDefault(), void doUndo();
        if (k === "a") return e.preventDefault(), setSelected(new Set(navList.map((p) => p.id)));
        if (k === "e") return e.preventDefault(), setModal({ kind: "export" });
        if (k === "i") return e.preventDefault(), setModal({ kind: "import" });
        if (k === "c" && e.shiftKey) return e.preventDefault(), void copySettings();
        if (k === "v" && e.shiftKey) return e.preventDefault(), void pasteSettings();
        return;
      }
      if (e.altKey) return;

      switch (k) {
        case "p":
          return void decide("pick", 1);
        case "x":
          return void decide("pick", -1);
        case "u":
          return void decide("pick", 0);
        case "0":
        case "1":
        case "2":
        case "3":
        case "4":
        case "5":
          return void decide("rating", Number(k));
        case "ArrowRight":
          e.preventDefault();
          if (view === "compare") return cycleCompare(1);
          return move(1, e.shiftKey);
        case "ArrowLeft":
          e.preventDefault();
          if (view === "compare") return cycleCompare(-1);
          return move(-1, e.shiftKey);
        case "ArrowDown":
          e.preventDefault();
          return move(view === "grid" ? columns.current : 1, e.shiftKey);
        case "ArrowUp":
          e.preventDefault();
          return move(view === "grid" ? -columns.current : -1, e.shiftKey);
        case " ":
          e.preventDefault();
          setZoom(FIT);
          return setView(view === "grid" ? "loupe" : "grid");
        case "Enter":
          if (view === "develop" || view === "explore") return;
          return setView("loupe");
        case "e":
          return setView("loupe");
        case "g":
          return setView("grid");
        case "Escape":
          if (cropMode) return setCropMode(false);
          return setView("grid");
        case "c":
          return startCompare();
        case "v":
          if (focus != null) setView("explore");
          return;
        case "d":
          if (focus != null) setView("develop");
          return;
        case "z":
          return setZoom((z) => (z.zoom == null ? { zoom: 1, cx: 0.5, cy: 0.5 } : FIT));
        case "i":
          return setShowInfo((s) => !s);
        case "b":
          if (e.shiftKey) return void acceptBurst();
          if (focus != null)
            api
              .setBurstBest(focus)
              .then(merge)
              .catch((err) => addError(String(err)));
          return;
        case "s":
          return setCollapse((c) => !c);
        case "a":
          return setAutoAdvance((a) => !a);
        case "r":
          if (view === "develop") setCropMode(!cropMode);
          return;
        case "j":
          if (view === "develop") setClip((c) => !c);
          return;
        case "\\":
          if (view === "develop") setBeforeHeld(true);
          return;
        case "?":
        case "/":
          return setModal({ kind: "help" });
      }
    };
    const up = (e: KeyboardEvent) => {
      if (e.key === "\\") setBeforeHeld(false);
    };
    const cycleCompare = (d: number) => {
      const i = compareIds.indexOf(focus ?? -1);
      setFocus(compareIds[(i + d + compareIds.length) % compareIds.length]);
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, [modal, view, focus, navList, compareIds, cropMode, decide, move, doUndo, copySettings, pasteSettings, startCompare, acceptBurst, merge, addError]);

  // ---------------------------------------------------------- mouse --
  const lastClicked = useRef<number | null>(null);
  const onCellClick = useCallback(
    (id: number, e: React.MouseEvent) => {
      userFocused.current = true;
      if (e.shiftKey && lastClicked.current != null) {
        const a = visible.findIndex((p) => p.id === lastClicked.current);
        const b = visible.findIndex((p) => p.id === id);
        const [lo, hi] = [Math.min(a, b), Math.max(a, b)];
        setSelected(new Set(visible.slice(lo, hi + 1).map((p) => p.id)));
      } else if (modKey(e)) {
        setSelected((s) => {
          const n = new Set(s);
          if (n.has(id)) n.delete(id);
          else n.add(id);
          return n;
        });
        lastClicked.current = id;
      } else {
        setSelected(new Set([id]));
        lastClicked.current = id;
      }
      setFocus(id);
    },
    [visible],
  );
  const onColumns = useCallback((n: number) => {
    columns.current = n;
  }, []);
  const toggleBurst = useCallback((b: number) => {
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(b)) n.delete(b);
      else n.add(b);
      return n;
    });
  }, []);

  // --------------------------------------------------------- render --
  const counts = useMemo(() => {
    let picks = 0;
    let rejects = 0;
    for (const p of photos) {
      if (p.pick === 1) picks++;
      else if (p.pick === -1) rejects++;
    }
    return { picks, rejects };
  }, [photos]);

  const navIndex = navList.findIndex((p) => p.id === focus);
  const neighbours = useMemo(
    () => [navList[navIndex + 1], navList[navIndex + 2], navList[navIndex - 1]].filter(Boolean) as Photo[],
    [navList, navIndex],
  );

  const currentFolder = folders.find((f) => f.id === folderId);

  return (
    <div className="app">
      <header className="toolbar">
        <div className="brand">Keepers Studio</div>
        <nav className="modes">
          {(["grid", "loupe", "explore", "develop"] as const).map((m) => (
            <button key={m} className={view === m || (m === "loupe" && view === "compare") ? "on" : ""} onClick={() => focus != null || m === "grid" ? setView(m) : null}>
              {m === "grid" ? "Grid  G" : m === "loupe" ? "Loupe  E" : m === "explore" ? "Explore  V" : "Develop  D"}
            </button>
          ))}
          <button onClick={startCompare} className={view === "compare" ? "on" : ""}>
            Compare C
          </button>
        </nav>
        <div className="filters">
          <select value={flagFilter} onChange={(e) => setFlagFilter(e.target.value as FlagFilter)} title="Filter by flag">
            <option value="all">All photos</option>
            <option value="picks">Picks</option>
            <option value="unflagged">Unflagged</option>
            <option value="rejects">Rejects</option>
          </select>
          <select value={minRating} onChange={(e) => setMinRating(Number(e.target.value))} title="Minimum rating">
            <option value={0}>Any rating</option>
            {[1, 2, 3, 4, 5].map((r) => (
              <option key={r} value={r}>
                {"★".repeat(r)}+
              </option>
            ))}
          </select>
          <label className="toggle" title="Stack bursts (S)">
            <input type="checkbox" checked={collapse} onChange={(e) => setCollapse(e.target.checked)} /> Stack bursts
          </label>
          <label className="toggle" title="Move to next photo after a decision (A)">
            <input type="checkbox" checked={autoAdvance} onChange={(e) => setAutoAdvance(e.target.checked)} /> Auto-advance
          </label>
          {view === "grid" && (
            <input
              type="range"
              min={100}
              max={400}
              value={thumbSize}
              title="Thumbnail size"
              onChange={(e) => {
                setThumbSize(Number(e.target.value));
                localStorage.setItem("keepers.thumb", e.target.value);
              }}
            />
          )}
        </div>
        <div className="actions">
          <button onClick={() => setModal({ kind: "import" })} title={`Import (${modLabel}I)`}>
            Import…
          </button>
          <button className="primary" onClick={() => setModal({ kind: "export" })} title={`Export (${modLabel}E)`}>
            Export…
          </button>
          <button onClick={() => setModal({ kind: "help" })} title="Keyboard shortcuts (?)">
            ?
          </button>
        </div>
      </header>

      <div className="body">
        {view !== "develop" && view !== "explore" && (
          <aside className="sidebar">
            <h3>Library</h3>
            <ul className="folders">
              <li className={folderId == null ? "on" : ""} onClick={() => setFolderId(null)}>
                All photos <span>{folders.reduce((a, f) => a + f.count, 0)}</span>
              </li>
              {folders.map((f) => (
                <li key={f.id} className={folderId === f.id ? "on" : ""} onClick={() => setFolderId(f.id)} title={f.path}>
                  <em>{f.label ?? f.path.split(/[\\/]/).filter(Boolean).pop() ?? f.path}</em> <span>{f.count}</span>
                </li>
              ))}
            </ul>
            {currentFolder && (
              <div className="folder-actions">
                <button className="link" onClick={() => api.importFolder(currentFolder.path, null)}>
                  Rescan folder
                </button>
                <button className="link" onClick={() => revealItemInDir(currentFolder.path).catch((e) => addError(String(e)))}>
                  Show in {navigator.userAgent.includes("Mac") ? "Finder" : "Explorer"}
                </button>
                <button
                  className="link danger"
                  onClick={async () => {
                    if (await confirm(`Remove "${currentFolder.path}" from the catalog?\n\nNo files are deleted.`, { title: "Remove folder", kind: "warning" }))
                      api
                        .removeFolder(currentFolder.id)
                        .then(() => {
                          setFolderId(null);
                          reload();
                        })
                        .catch((e) => addError(String(e)));
                  }}
                >
                  Remove from catalog
                </button>
              </div>
            )}
            {folders.length === 0 && (
              <div className="welcome">
                <p>Drop a folder here or</p>
                <button className="primary" onClick={() => setModal({ kind: "import" })}>
                  Import photos…
                </button>
                <p className="hint">Nothing is uploaded. Originals are never modified.</p>
              </div>
            )}
            <h3>Session</h3>
            <div className="stats">
              <div>
                <span className="flag pick">⚑</span> {counts.picks} picks
              </div>
              <div>
                <span className="flag reject">✕</span> {counts.rejects} rejects
              </div>
              <div>{photos.length - counts.picks - counts.rejects} unflagged</div>
            </div>
            <label className="toggle small" title="Write cull decisions and edits to .keepers.json files next to the photos so they survive moving folders.">
              <input
                type="checkbox"
                checked={sidecars}
                onChange={(e) => {
                  setSidecars(e.target.checked);
                  api.setWriteSidecars(e.target.checked).catch((err) => addError(String(err)));
                }}
              />{" "}
              Write sidecar files
            </label>
          </aside>
        )}

        <main className="main">
          {view === "grid" && (
            <Grid
              items={visible}
              focus={focus}
              selected={selected}
              size={thumbSize}
              burstCounts={burstCounts}
              expanded={expanded}
              collapse={collapse}
              onClick={onCellClick}
              onOpen={(id) => {
                setFocus(id);
                setView("loupe");
              }}
              onColumns={onColumns}
              onToggleBurst={toggleBurst}
              emptyText={photos.length ? "No photos match the current filter." : importMsg ?? "Import a folder to begin — or drop one on this window."}
            />
          )}
          {view === "loupe" && focusPhoto && (
            <Loupe
              photo={focusPhoto}
              neighbours={neighbours}
              view={zoom}
              onView={setZoom}
              showInfo={showInfo}
              burstSize={focusPhoto.burstId != null ? burstCounts.get(focusPhoto.burstId) ?? 1 : 1}
            />
          )}
          {view === "compare" && (
            <Compare
              photos={compareIds.map((id) => byId.get(id)).filter(Boolean) as Photo[]}
              active={focus ?? -1}
              onActive={setFocus}
              view={zoom}
              onView={setZoom}
            />
          )}
          {view === "explore" && focusPhoto && (
            <Explore
              photo={focusPhoto}
              keepers={filtered.filter((p) => p.pick === 1)}
              onSaved={merge}
              onError={addError}
              onOpenDevelop={() => {
                setEditsVersion((v) => v + 1);
                setView("develop");
              }}
              onNotice={(text) => setExportMsg({ text, last: null })}
            />
          )}
          {view === "develop" && focusPhoto && (
            <Develop
              photo={focusPhoto}
              editsVersion={editsVersion}
              editsRef={developEdits}
              onSaved={merge}
              onError={addError}
              keepers={keepers}
              cropMode={cropMode}
              setCropMode={setCropMode}
              clip={clip}
              setClip={setClip}
              before={beforeHeld || beforeLocked}
              beforeLocked={beforeLocked}
              setBeforeLocked={setBeforeLocked}
            />
          )}
          {(view === "loupe" || view === "develop" || view === "compare" || view === "explore") && (
            <Filmstrip
              items={navList}
              focus={focus}
              onPick={(id) => {
                userFocused.current = true;
                setFocus(id);
                setSelected(new Set([id]));
                setZoom(FIT);
                if (view === "compare") setView("loupe");
              }}
            />
          )}
          {(view === "develop" || view === "explore") && keepers === visible && filtered.every((p) => p.pick !== 1) && (
            <div className="banner">No picks yet — showing all photos. Pick keepers (P) to develop only those.</div>
          )}
        </main>
      </div>

      <footer className="statusbar">
        <span>
          {visible.length} shown{photos.length !== visible.length ? ` of ${photos.length}` : ""}
          {selected.size > 1 ? ` · ${selected.size} selected` : ""}
        </span>
        {focusPhoto && <span className="dim">{focusPhoto.fileName}</span>}
        {progress.total > progress.done && (
          <span className="progress">
            Building previews {progress.done} / {progress.total}
            <i style={{ width: `${(100 * progress.done) / Math.max(1, progress.total)}%` }} />
          </span>
        )}
        {importMsg && <span>{importMsg}</span>}
        {exportMsg && (
          <span>
            {exportMsg.text}
            {exportMsg.last && (
              <button className="link" onClick={() => revealItemInDir(exportMsg.last!).catch((e) => addError(String(e)))}>
                Show
              </button>
            )}
          </span>
        )}
        <span className="spacer" />
        {!autoAdvance && <span className="dim">Auto-advance off</span>}
        <span className="dim">Local only · no account · no upload</span>
      </footer>

      {errors.length > 0 && (
        <div className="toasts">
          {errors.map((m, i) => (
            <div key={i} className="toast" onClick={() => setErrors((e) => e.filter((_, j) => j !== i))}>
              {m}
            </div>
          ))}
        </div>
      )}

      {modal?.kind === "import" && <ImportDialog initialSource={modal.source} onClose={() => setModal(null)} />}
      {modal?.kind === "export" && (
        <ExportDialog
          counts={{
            selected: targets().length,
            picks: filtered.filter((p) => p.pick === 1).length,
            visible: visible.length,
          }}
          onClose={() => setModal(null)}
          onExport={onExport}
        />
      )}
      {modal?.kind === "help" && <HelpOverlay onClose={() => setModal(null)} />}
    </div>
  );
}
