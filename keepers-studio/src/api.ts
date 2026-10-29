import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Kind = "raw" | "jpeg" | "tiff" | "png" | "heic";

export interface Photo {
  id: number;
  folderId: number;
  path: string;
  fileName: string;
  kind: Kind;
  pairPath: string | null;
  processed: boolean;
  error: string | null;
  captureTime: number | null;
  camera: string | null;
  lens: string | null;
  iso: number | null;
  shutter: string | null;
  aperture: number | null;
  focal: number | null;
  orientation: number;
  width: number | null;
  height: number | null;
  sharpness: number | null;
  exposure: number | null;
  clipHi: number | null;
  clipLo: number | null;
  burstId: number | null;
  burstBest: boolean;
  dupOf: number | null;
  pick: -1 | 0 | 1;
  rating: number;
  edited: boolean;
}

export interface Folder {
  id: number;
  path: string;
  count: number;
  /** Set for folders imported from another app's library. */
  label: string | null;
}

export type ExternalSource =
  | { kind: "lightroom"; path: string }
  | { kind: "applePhotos"; path: string }
  | { kind: "takeout"; paths: string[]; destination: string | null };

export interface SuggestedSources {
  lightroom: string[];
  applePhotos: string | null;
  icloudPhotos: string | null;
}

export interface Hsl {
  hue: number;
  sat: number;
  lum: number;
}

export interface Crop {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Edits {
  exposure: number;
  contrast: number;
  highlights: number;
  shadows: number;
  whites: number;
  blacks: number;
  temp: number;
  tint: number;
  vibrance: number;
  saturation: number;
  curve: [number, number][];
  hsl: Hsl[];
  rotate: number;
  flipH: boolean;
  angle: number;
  crop: Crop | null;
  sharpen: number;
  sharpenRadius: number;
  nrLuma: number;
  nrColor: number;
  mono: Mono;
  film: FilmLook;
  grain: number;
  grainSize: number;
  glow: number;
  edgeBurn: number;
  harmony: Harmony | null;
  zoneAnchor: number | null;
  zoneN: number;
}

export type MonoFilm = "panchromatic" | "classicPan" | "fineGrain" | "ortho" | "infrared";
export type LensFilter = "none" | "yellow" | "orange" | "red" | "green" | "blue";
export type Toner = "none" | "selenium" | "sepia" | "gold" | "platinum" | "cyanotype";
export type ColorStock = "none" | "portraitNegative" | "vividSlide" | "classicSlide" | "tungstenCine" | "fadedPrint" | "mutedEarth";
export type HarmonyTemplate = "monochromatic" | "analogous" | "complementary" | "splitAccent" | "square" | "dualAnalogous";

export interface Mono {
  enabled: boolean;
  amount: number;
  film: MonoFilm;
  filter: LensFilter;
  grade: number;
  soft: number;
  hard: number;
  toner: Toner;
  tonerAmount: number;
}

export interface FilmLook {
  stock: ColorStock;
  strength: number;
}

export interface Harmony {
  template: HarmonyTemplate;
  angle: number;
  amount: number;
}

export interface ColourAnalysis {
  palette: { rgb: [number, number, number]; share: number }[];
  harmonies: { template: HarmonyTemplate; label: string; angle: number; score: number }[];
  hues: number[];
}

export type Guide = "thirds" | "golden" | "centre" | "level";

export interface ExploreLook {
  id: string;
  name: string;
  group: string;
  description: string;
  edits: Edits;
}

export interface ExploreComposition {
  label: string;
  guide: Guide;
  crop: Crop | null;
  angle: number;
  subject: [number, number];
  score: number;
  edits: Edits;
}

export interface Explore {
  looks: ExploreLook[];
  compositions: ExploreComposition[];
  colour: ColourAnalysis;
  subject: [number, number];
}

export interface ViewOpts {
  ignoreCrop?: boolean;
  clipWarnings?: boolean;
  zones?: boolean;
  small?: boolean;
}

export const defaultEdits = (): Edits => ({
  exposure: 0,
  contrast: 0,
  highlights: 0,
  shadows: 0,
  whites: 0,
  blacks: 0,
  temp: 0,
  tint: 0,
  vibrance: 0,
  saturation: 0,
  curve: [
    [0, 0],
    [1, 1],
  ],
  hsl: Array.from({ length: 8 }, () => ({ hue: 0, sat: 0, lum: 0 })),
  rotate: 0,
  flipH: false,
  angle: 0,
  crop: null,
  sharpen: 0,
  sharpenRadius: 1,
  nrLuma: 0,
  nrColor: 0,
  mono: {
    enabled: false,
    amount: 100,
    film: "panchromatic",
    filter: "none",
    grade: 2,
    soft: 0,
    hard: 0,
    toner: "none",
    tonerAmount: 0,
  },
  film: { stock: "none", strength: 100 },
  grain: 0,
  grainSize: 1,
  glow: 0,
  edgeBurn: 0,
  harmony: null,
  zoneAnchor: null,
  zoneN: 0,
});

export interface Preset {
  name: string;
  edits: Edits;
  builtin: boolean;
}

export type ExportFormat = "jpeg" | "png" | "tiff";
export interface ExportSettings {
  format: ExportFormat;
  quality: number;
  longEdge: number | null;
  sixteenBit: boolean;
  sharpen: "none" | "screen" | "print";
  folder: string;
  naming: string;
}

export interface CopyOptions {
  destination: string;
  template: string;
  shootName: string;
}

export type EngineEvent =
  | { type: "importStatus"; message: string; done: number; total: number; finished: boolean }
  | { type: "progress"; done: number; total: number }
  | { type: "photosUpdated"; photos: Photo[] }
  | { type: "catalogChanged" }
  | { type: "exportStatus"; done: number; total: number; errors: string[]; last: string | null; finished: boolean }
  | { type: "error"; message: string };

export interface Rendered {
  url: string;
  width: number;
  height: number;
  histogram: [Uint32Array, Uint32Array, Uint32Array];
}

export const api = {
  folders: () => invoke<Folder[]>("folders"),
  photos: (folderId: number | null) => invoke<Photo[]>("photos", { folderId }),
  importFolder: (path: string, copy: CopyOptions | null) => invoke<void>("import_folder", { path, copy }),
  importExternal: (source: ExternalSource) => invoke<void>("import_external", { source }),
  suggestedSources: () => invoke<SuggestedSources>("suggested_sources"),
  removeFolder: (id: number) => invoke<void>("remove_folder", { id }),
  setPick: (ids: number[], pick: number) => invoke<Photo[]>("set_pick", { ids, pick }),
  setRating: (ids: number[], rating: number) => invoke<Photo[]>("set_rating", { ids, rating }),
  setBurstBest: (id: number) => invoke<Photo[]>("set_burst_best", { id }),
  getEdits: (id: number) => invoke<Edits>("get_edits", { id }),
  saveEdits: (ids: number[], edits: Edits) => invoke<Photo[]>("save_edits", { ids, edits }),
  warm: (id: number) => invoke<void>("warm", { id }),
  presets: () => invoke<Preset[]>("presets"),
  savePreset: (name: string, edits: Edits) => invoke<Preset[]>("save_preset", { name, edits }),
  deletePreset: (name: string) => invoke<Preset[]>("delete_preset", { name }),
  exportPresets: () => invoke<{ name: string; settings: ExportSettings }[]>("export_presets"),
  exportPhotos: (ids: number[], settings: ExportSettings) => invoke<void>("export_photos", { ids, settings }),
  exportHistory: (id: number) => invoke<{ path: string; settings: string; at: number }[]>("export_history", { id }),
  settings: () => invoke<{ writeSidecars: boolean; platform: string; picturesDir: string | null }>("settings"),
  setWriteSidecars: (on: boolean) => invoke<void>("set_write_sidecars", { on }),

  explore: (id: number, edits: Edits) => invoke<Explore>("explore", { id, edits }),
  lookEdits: (look: string, base: Edits, strength: number, harmony: [HarmonyTemplate, number] | null) =>
    invoke<Edits>("look_edits", { look, base, strength, harmony }),
  placeZone: (id: number, edits: Edits, x: number, y: number, zone: number) => invoke<Edits>("place_zone", { id, edits, x, y, zone }),
  analyzeColour: (id: number, edits: Edits) => invoke<ColourAnalysis>("analyze_colour", { id, edits }),

  async render(id: number, edits: Edits, opts: ViewOpts = {}): Promise<Rendered> {
    const buf = await invoke<ArrayBuffer>("render", { id, edits, opts });
    const view = new DataView(buf);
    const width = view.getUint32(0, true);
    const height = view.getUint32(4, true);
    const hist = new Uint32Array(buf.slice(8, 8 + 3 * 256 * 4));
    const jpeg = buf.slice(8 + 3 * 256 * 4);
    const url = URL.createObjectURL(new Blob([jpeg], { type: "image/jpeg" }));
    return {
      url,
      width,
      height,
      histogram: [hist.subarray(0, 256), hist.subarray(256, 512), hist.subarray(512, 768)],
    };
  },

  onEvent: (cb: (e: EngineEvent) => void) => listen<EngineEvent>("keepers", (e) => cb(e.payload)),
};

/** Image URLs served by the engine's custom protocols. `v` busts the cache when a photo is reprocessed. */
export const thumbUrl = (p: Photo) => `${convertFileSrc(String(p.id), "kthumb")}?v=${p.processed ? 1 : 0}`;
export const previewUrl = (id: number) => convertFileSrc(String(id), "kprev");
export const fullUrl = (id: number) => convertFileSrc(String(id), "kfull");

export const isMac = navigator.userAgent.includes("Mac");
/** Cmd on macOS, Ctrl elsewhere. */
export const modKey = (e: { metaKey: boolean; ctrlKey: boolean }) => (isMac ? e.metaKey : e.ctrlKey);
export const modLabel = isMac ? "⌘" : "Ctrl+";

export function formatShot(p: Photo): string {
  const bits: string[] = [];
  if (p.focal) bits.push(`${Math.round(p.focal)}mm`);
  if (p.aperture) bits.push(`f/${p.aperture.toFixed(1).replace(/\.0$/, "")}`);
  if (p.shutter) bits.push(p.shutter);
  if (p.iso) bits.push(`ISO ${p.iso}`);
  return bits.join("  ·  ");
}

export function formatTime(ms: number | null): string {
  if (ms == null) return "No capture date";
  // Capture times are camera-local clock values stored as if UTC.
  const d = new Date(ms);
  return d.toLocaleString(undefined, { timeZone: "UTC", dateStyle: "medium", timeStyle: "medium" });
}
