import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Crop, Edits, Photo, Preset, Rendered, api, defaultEdits, modLabel } from "../api";
import { Histogram } from "./Histogram";
import { ToneCurve } from "./ToneCurve";

interface Props {
  photo: Photo;
  /** Bumped by the app when this photo's edits changed elsewhere (paste, preset batch). */
  editsVersion: number;
  editsRef: React.MutableRefObject<Edits | null>;
  onSaved: (photos: Photo[]) => void;
  onError: (msg: string) => void;
  keepers: Photo[];
  cropMode: boolean;
  setCropMode: (on: boolean) => void;
  clip: boolean;
  setClip: (on: boolean) => void;
  before: boolean;
  setBeforeLocked: (on: boolean) => void;
  beforeLocked: boolean;
}

const HSL_NAMES = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];
const HSL_COLORS = ["#e54848", "#e8913a", "#e3cf3b", "#4fc14f", "#3fc9c9", "#4a7fe8", "#9058e0", "#d64fb4"];
const ASPECTS: { label: string; r: number | null }[] = [
  { label: "Free", r: null },
  { label: "Original", r: -1 },
  { label: "1:1", r: 1 },
  { label: "4:5", r: 4 / 5 },
  { label: "3:2", r: 3 / 2 },
  { label: "16:9", r: 16 / 9 },
];

export function Develop(props: Props) {
  const { photo, editsVersion, editsRef, onSaved, onError, cropMode, setCropMode, clip, setClip, before } = props;
  const [edits, setEditsState] = useState<Edits | null>(null);
  const [img, setImg] = useState<Rendered | null>(null);
  const [beforeImg, setBeforeImg] = useState<Rendered | null>(null);
  const [busy, setBusy] = useState(false);
  const [presets, setPresets] = useState<Preset[]>([]);
  const [hslTab, setHslTab] = useState<"hue" | "sat" | "lum">("hue");
  const [presetName, setPresetName] = useState("");
  const photoId = useRef(photo.id);
  photoId.current = photo.id;

  // ---- load edits for the current photo
  useEffect(() => {
    let live = true;
    setEditsState(null);
    api
      .getEdits(photo.id)
      .then((e) => live && setEditsState(e))
      .catch((e) => onError(String(e)));
    return () => {
      live = false;
    };
  }, [photo.id, editsVersion, onError]);

  useEffect(() => {
    editsRef.current = edits;
  }, [edits, editsRef]);

  useEffect(() => {
    api.presets().then(setPresets).catch(() => {});
  }, []);

  // ---- render loop: at most one request in flight; newest settings win
  const inflight = useRef(false);
  const pending = useRef<{ id: number; edits: Edits; ignoreCrop: boolean; clip: boolean } | null>(null);
  const pump = useCallback(async () => {
    if (inflight.current || !pending.current) return;
    const req = pending.current;
    pending.current = null;
    inflight.current = true;
    setBusy(true);
    try {
      const r = await api.render(req.id, req.edits, req.ignoreCrop, req.clip);
      if (req.id === photoId.current) {
        setImg((prev) => {
          if (prev) URL.revokeObjectURL(prev.url);
          return r;
        });
      } else URL.revokeObjectURL(r.url);
    } catch (e) {
      onError(`Develop failed: ${e}`);
    } finally {
      inflight.current = false;
      setBusy(false);
      pump();
    }
  }, [onError]);

  useEffect(() => {
    if (!edits) return;
    pending.current = { id: photo.id, edits, ignoreCrop: cropMode, clip };
    pump();
  }, [edits, cropMode, clip, photo.id, pump]);

  // Clear stale images when switching photos.
  useEffect(() => {
    setImg((prev) => {
      if (prev) URL.revokeObjectURL(prev.url);
      return null;
    });
    setBeforeImg((prev) => {
      if (prev) URL.revokeObjectURL(prev.url);
      return null;
    });
  }, [photo.id]);

  // "Before" = no edits at all.
  useEffect(() => {
    if (!before || beforeImg) return;
    api.render(photo.id, defaultEdits(), false, false).then(setBeforeImg).catch(() => {});
  }, [before, beforeImg, photo.id]);

  // ---- saving (trailing throttle)
  const saveTimer = useRef<number | undefined>(undefined);
  const setEdits = useCallback(
    (next: Edits) => {
      setEditsState(next);
      window.clearTimeout(saveTimer.current);
      const id = photo.id;
      saveTimer.current = window.setTimeout(() => {
        api.saveEdits([id], next).then(onSaved).catch((e) => onError(String(e)));
      }, 300);
    },
    [photo.id, onSaved, onError],
  );
  useEffect(() => () => window.clearTimeout(saveTimer.current), [photo.id]);

  const set = <K extends keyof Edits>(k: K, v: Edits[K]) => edits && setEdits({ ...edits, [k]: v });

  // ---- crop editing state
  const [cropRect, setCropRect] = useState<Crop>({ x: 0, y: 0, w: 1, h: 1 });
  const [aspect, setAspect] = useState<number | null>(null);
  useEffect(() => {
    if (cropMode && edits) setCropRect(edits.crop ?? { x: 0, y: 0, w: 1, h: 1 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cropMode]);
  const commitCrop = useCallback(() => {
    if (!edits) return;
    const full = cropRect.x < 0.001 && cropRect.y < 0.001 && cropRect.w > 0.999 && cropRect.h > 0.999;
    setEdits({ ...edits, crop: full ? null : cropRect });
    setCropMode(false);
  }, [edits, cropRect, setEdits, setCropMode]);
  // Enter commits the crop.
  useEffect(() => {
    if (!cropMode) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === "Enter") {
        e.preventDefault();
        commitCrop();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [cropMode, commitCrop]);

  if (!edits) return <div className="develop loading">Loading…</div>;

  const shown = before && beforeImg ? beforeImg : img;
  const applyPreset = (p: Preset) =>
    setEdits({ ...p.edits, rotate: edits.rotate, flipH: edits.flipH, angle: edits.angle, crop: edits.crop });

  return (
    <div className="develop">
      <aside className="dev-left">
        <h3>Presets</h3>
        <ul className="presets">
          {presets.map((p) => (
            <li key={p.name}>
              <button onClick={() => applyPreset(p)}>{p.name}</button>
              {!p.builtin && (
                <button
                  className="x"
                  title="Delete preset"
                  onClick={() => api.deletePreset(p.name).then(setPresets).catch((e) => onError(String(e)))}
                >
                  ✕
                </button>
              )}
            </li>
          ))}
        </ul>
        <form
          className="preset-save"
          onSubmit={(e) => {
            e.preventDefault();
            const name = presetName.trim();
            if (!name) return;
            api
              .savePreset(name, edits)
              .then((p) => {
                setPresets(p);
                setPresetName("");
              })
              .catch((err) => onError(String(err)));
          }}
        >
          <input placeholder="New preset name" value={presetName} onChange={(e) => setPresetName(e.target.value)} />
          <button type="submit" disabled={!presetName.trim()}>
            Save
          </button>
        </form>
        <h3>Settings</h3>
        <button className="wide" onClick={() => setEdits({ ...defaultEdits() })}>
          Reset all
        </button>
        <p className="hint">
          Copy {modLabel}Shift+C · Paste {modLabel}Shift+V
          <br />
          Before/after: hold \ (click button to lock)
          <br />
          Crop: R · Clipping: J
        </p>
        <button className={"wide" + (props.beforeLocked ? " on" : "")} onClick={() => props.setBeforeLocked(!props.beforeLocked)}>
          {props.beforeLocked ? "Showing before — click for after" : "Before / After"}
        </button>
      </aside>

      <Canvas
        img={shown}
        busy={busy}
        cropMode={cropMode}
        cropRect={cropRect}
        setCropRect={setCropRect}
        aspect={aspect === -1 && img ? img.width / img.height : aspect}
        label={before ? "Before" : null}
      />

      <aside className="dev-right">
        <Histogram data={img?.histogram ?? null} clip={clip} onToggleClip={() => setClip(!clip)} />
        <div className="exif-line">{photo.fileName}</div>

        {cropMode ? (
          <Section title="Crop & Straighten" open>
            <div className="aspects">
              {ASPECTS.map((a) => (
                <button key={a.label} className={aspect === a.r ? "on" : ""} onClick={() => setAspect(a.r)}>
                  {a.label}
                </button>
              ))}
            </div>
            <Slider label="Straighten" min={-45} max={45} step={0.1} value={edits.angle} onChange={(v) => set("angle", v)} />
            <div className="row">
              <button onClick={() => set("rotate", (edits.rotate + 3) % 4)}>⟲ Rotate</button>
              <button onClick={() => set("rotate", (edits.rotate + 1) % 4)}>⟳ Rotate</button>
              <button onClick={() => set("flipH", !edits.flipH)}>⇋ Flip</button>
            </div>
            <div className="row">
              <button onClick={() => setCropRect({ x: 0, y: 0, w: 1, h: 1 })}>Reset crop</button>
              <button className="primary" onClick={commitCrop}>
                Done (Enter)
              </button>
            </div>
          </Section>
        ) : (
          <button className="wide" onClick={() => setCropMode(true)}>
            ✂ Crop & Straighten (R){edits.crop || edits.angle || edits.rotate ? " ●" : ""}
          </button>
        )}

        <Section title="Basic" open>
          <Slider label="Temp" min={-100} max={100} value={edits.temp} onChange={(v) => set("temp", v)} className="temp" />
          <Slider label="Tint" min={-100} max={100} value={edits.tint} onChange={(v) => set("tint", v)} className="tint" />
          <Slider label="Exposure" min={-5} max={5} step={0.01} value={edits.exposure} onChange={(v) => set("exposure", v)} format={(v) => (v > 0 ? "+" : "") + v.toFixed(2)} />
          <Slider label="Contrast" min={-100} max={100} value={edits.contrast} onChange={(v) => set("contrast", v)} />
          <Slider label="Highlights" min={-100} max={100} value={edits.highlights} onChange={(v) => set("highlights", v)} />
          <Slider label="Shadows" min={-100} max={100} value={edits.shadows} onChange={(v) => set("shadows", v)} />
          <Slider label="Whites" min={-100} max={100} value={edits.whites} onChange={(v) => set("whites", v)} />
          <Slider label="Blacks" min={-100} max={100} value={edits.blacks} onChange={(v) => set("blacks", v)} />
          <Slider label="Vibrance" min={-100} max={100} value={edits.vibrance} onChange={(v) => set("vibrance", v)} />
          <Slider label="Saturation" min={-100} max={100} value={edits.saturation} onChange={(v) => set("saturation", v)} />
        </Section>

        <Section title="Tone Curve">
          <ToneCurve points={edits.curve} onChange={(c) => set("curve", c)} />
          <button
            className="link"
            onClick={() =>
              set("curve", [
                [0, 0],
                [1, 1],
              ])
            }
          >
            Reset curve
          </button>
        </Section>

        <Section title="HSL / Color">
          <div className="tabs">
            {(["hue", "sat", "lum"] as const).map((t) => (
              <button key={t} className={hslTab === t ? "on" : ""} onClick={() => setHslTab(t)}>
                {t === "hue" ? "Hue" : t === "sat" ? "Saturation" : "Luminance"}
              </button>
            ))}
          </div>
          {HSL_NAMES.map((n, i) => (
            <Slider
              key={n}
              label={n}
              min={-100}
              max={100}
              value={edits.hsl[i][hslTab]}
              accent={HSL_COLORS[i]}
              onChange={(v) => {
                const hsl = edits.hsl.map((b, j) => (j === i ? { ...b, [hslTab]: v } : b));
                set("hsl", hsl);
              }}
            />
          ))}
        </Section>

        <Section title="Detail">
          <Slider label="Sharpening" min={0} max={150} value={edits.sharpen} onChange={(v) => set("sharpen", v)} />
          <Slider label="Radius" min={0.5} max={3} step={0.1} value={edits.sharpenRadius} onChange={(v) => set("sharpenRadius", v)} def={1} format={(v) => v.toFixed(1)} />
          <Slider label="Noise (luma)" min={0} max={100} value={edits.nrLuma} onChange={(v) => set("nrLuma", v)} />
          <Slider label="Noise (color)" min={0} max={100} value={edits.nrColor} onChange={(v) => set("nrColor", v)} />
        </Section>

        <Section title="Lens Corrections">
          <p className="hint">
            {photo.fileName.toLowerCase().endsWith(".dng")
              ? "The camera's built-in DNG lens profile (distortion and vignetting, e.g. Leica Q2) is applied automatically."
              : "No lens profile available for this file in this version."}
          </p>
        </Section>
      </aside>
    </div>
  );
}

function Section({ title, open = false, children }: { title: string; open?: boolean; children: React.ReactNode }) {
  return (
    <details className="section" open={open}>
      <summary>{title}</summary>
      {children}
    </details>
  );
}

interface SliderProps {
  label: string;
  min: number;
  max: number;
  step?: number;
  value: number;
  def?: number;
  onChange: (v: number) => void;
  format?: (v: number) => string;
  accent?: string;
  className?: string;
}

function Slider({ label, min, max, step = 1, value, def = 0, onChange, format, accent, className }: SliderProps) {
  return (
    <label className={"slider " + (className ?? "")} onDoubleClick={() => onChange(def)} title="Double-click to reset">
      <span className="sl-label">{label}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        style={accent ? ({ accentColor: accent } as React.CSSProperties) : undefined}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <span className="sl-value">{format ? format(value) : (value > 0 && min < 0 ? "+" : "") + Math.round(value)}</span>
    </label>
  );
}

interface CanvasProps {
  img: Rendered | null;
  busy: boolean;
  cropMode: boolean;
  cropRect: Crop;
  setCropRect: (c: Crop) => void;
  aspect: number | null;
  label: string | null;
}

/** The developed image, fitted, with the crop overlay when cropping. */
function Canvas({ img, busy, cropMode, cropRect, setCropRect, aspect, label }: CanvasProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 1, h: 1 });
  useLayoutEffect(() => {
    const el = ref.current!;
    const ro = new ResizeObserver(() => setBox({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const pad = 24;
  const iw = img?.width ?? 3;
  const ih = img?.height ?? 2;
  const s = Math.min((box.w - pad * 2) / iw, (box.h - pad * 2) / ih);
  const dw = iw * s;
  const dh = ih * s;
  const left = (box.w - dw) / 2;
  const top = (box.h - dh) / 2;

  // Enforce the chosen aspect ratio (in pixels) on the crop rect.
  useEffect(() => {
    if (!cropMode || aspect == null || !img) return;
    const target = aspect * (ih / iw); // normalised w/h ratio
    let { x, y, w, h } = cropRect;
    if (w / h > target) w = h * target;
    else h = w / target;
    if (w > 1) (w = 1), (h = w / target);
    if (h > 1) (h = 1), (w = h * target);
    x = Math.min(x, 1 - w);
    y = Math.min(y, 1 - h);
    if (Math.abs(w - cropRect.w) > 1e-4 || Math.abs(h - cropRect.h) > 1e-4) setCropRect({ x, y, w, h });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [aspect, cropMode, iw, ih]);

  const drag = useRef<{ mode: string; sx: number; sy: number; start: Crop } | null>(null);
  const onDown = (mode: string) => (e: React.PointerEvent) => {
    e.stopPropagation();
    drag.current = { mode, sx: e.clientX, sy: e.clientY, start: cropRect };
    (e.target as Element).setPointerCapture(e.pointerId);
  };
  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const dx = (e.clientX - d.sx) / dw;
    const dy = (e.clientY - d.sy) / dh;
    let { x, y, w, h } = d.start;
    const min = 0.05;
    if (d.mode === "move") {
      x = Math.min(1 - w, Math.max(0, x + dx));
      y = Math.min(1 - h, Math.max(0, y + dy));
    } else {
      if (d.mode.includes("l")) {
        const nx = Math.min(x + w - min, Math.max(0, x + dx));
        w += x - nx;
        x = nx;
      }
      if (d.mode.includes("r")) w = Math.min(1 - x, Math.max(min, w + dx));
      if (d.mode.includes("t")) {
        const ny = Math.min(y + h - min, Math.max(0, y + dy));
        h += y - ny;
        y = ny;
      }
      if (d.mode.includes("b")) h = Math.min(1 - y, Math.max(min, h + dy));
      if (aspect != null) {
        const target = aspect * (ih / iw);
        if (d.mode.length === 1 && (d.mode === "t" || d.mode === "b")) w = Math.min(h * target, 1 - x);
        else h = Math.min(w / target, 1 - y);
        w = h * target;
      }
    }
    setCropRect({ x, y, w, h });
  };

  return (
    <div className="dev-canvas" ref={ref} onPointerMove={onMove} onPointerUp={() => (drag.current = null)}>
      {img && <img src={img.url} style={{ left, top, width: dw, height: dh }} draggable={false} alt="" />}
      {!img && <div className="empty">Developing…</div>}
      {busy && <div className="spinner" />}
      {label && <div className="beforelabel">{label}</div>}
      {cropMode && img && (
        <div className="crop-layer" style={{ left, top, width: dw, height: dh }}>
          <div
            className="crop-rect"
            style={{ left: cropRect.x * dw, top: cropRect.y * dh, width: cropRect.w * dw, height: cropRect.h * dh }}
            onPointerDown={onDown("move")}
          >
            <div className="thirds" />
            {["tl", "tr", "bl", "br", "t", "b", "l", "r"].map((m) => (
              <div key={m} className={"handle h-" + m} onPointerDown={onDown(m)} />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
