import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Edits, Explore as ExploreData, ExploreComposition, Guide, Photo, Rendered, api } from "../api";
import { GuideOverlay } from "./Guides";

interface Props {
  photo: Photo;
  /** Photos "Apply look to all" works on (the picks). */
  keepers: Photo[];
  onSaved: (photos: Photo[]) => void;
  onError: (msg: string) => void;
  onOpenDevelop: () => void;
  onNotice: (msg: string) => void;
}

interface Overrides {
  grain?: number;
  glow?: number;
  edgeBurn?: number;
}

const ORIGINAL = "original";

/**
 * Explore: the photo rendered in many photographic traditions, plus
 * composition choices computed from the photo itself. Pick a look and a
 * framing, adjust strength and character, apply.
 */
export function Explore({ photo, keepers, onSaved, onError, onOpenDevelop, onNotice }: Props) {
  const [base, setBase] = useState<Edits | null>(null);
  const [data, setData] = useState<ExploreData | null>(null);
  const [look, setLook] = useState<string>(ORIGINAL);
  const [comp, setComp] = useState<number>(-1);
  const [strength, setStrength] = useState(100);
  const [over, setOver] = useState<Overrides>({});
  const [lookEdits, setLookEdits] = useState<Edits | null>(null);
  const [cards, setCards] = useState<Record<string, string>>({});
  const [stage, setStage] = useState<Rendered | null>(null);
  const [busy, setBusy] = useState(false);
  const photoId = useRef(photo.id);
  photoId.current = photo.id;

  // Load this photo's edits and analyse it.
  useEffect(() => {
    let live = true;
    setBase(null);
    setData(null);
    setLook(ORIGINAL);
    setComp(-1);
    setStrength(100);
    setOver({});
    setCards((old) => {
      Object.values(old).forEach(URL.revokeObjectURL);
      return {};
    });
    (async () => {
      try {
        const e = await api.getEdits(photo.id);
        if (!live) return;
        setBase(e);
        const d = await api.explore(photo.id, e);
        if (live) setData(d);
      } catch (err) {
        onError(`Explore: ${err}`);
      }
    })();
    return () => {
      live = false;
    };
  }, [photo.id, onError]);

  // Render the carousel cards one after another (small, fast renders).
  useEffect(() => {
    if (!data || !base) return;
    let live = true;
    const jobs: [string, Edits][] = [
      [`look:${ORIGINAL}`, base],
      ...data.looks.map((l) => [`look:${l.id}`, l.edits] as [string, Edits]),
      ...data.compositions.map((c, i) => [`comp:${i}`, c.edits] as [string, Edits]),
    ];
    (async () => {
      for (const [key, edits] of jobs) {
        if (!live) return;
        try {
          const r = await api.render(photo.id, edits, { small: true });
          if (!live) return URL.revokeObjectURL(r.url);
          setCards((c) => ({ ...c, [key]: r.url }));
        } catch {
          /* a failed card just stays blank */
        }
      }
    })();
    return () => {
      live = false;
    };
  }, [data, base, photo.id]);

  const harmony = useMemo(
    () => (data?.colour.harmonies[0] ? ([data.colour.harmonies[0].template, data.colour.harmonies[0].angle] as [typeof data.colour.harmonies[0]["template"], number]) : null),
    [data],
  );

  // The selected look at the chosen strength.
  useEffect(() => {
    if (!base) return;
    if (look === ORIGINAL) return setLookEdits(base);
    let live = true;
    api
      .lookEdits(look, base, strength, harmony)
      .then((e) => live && setLookEdits(e))
      .catch((err) => onError(String(err)));
    return () => {
      live = false;
    };
  }, [look, strength, base, harmony, onError]);

  const composition: ExploreComposition | null = comp >= 0 && data ? data.compositions[comp] : null;
  const combined = useMemo<Edits | null>(() => {
    if (!lookEdits) return null;
    const e: Edits = { ...lookEdits };
    if (over.grain != null) e.grain = over.grain;
    if (over.glow != null) e.glow = over.glow;
    if (over.edgeBurn != null) e.edgeBurn = over.edgeBurn;
    if (composition) {
      e.crop = composition.edits.crop;
      e.angle = composition.edits.angle;
    }
    return e;
  }, [lookEdits, over, composition]);

  // Main stage render: newest request wins.
  const inflight = useRef(false);
  const pending = useRef<Edits | null>(null);
  const pump = useCallback(async () => {
    if (inflight.current || !pending.current) return;
    const e = pending.current;
    pending.current = null;
    inflight.current = true;
    setBusy(true);
    const id = photoId.current;
    try {
      const r = await api.render(id, e);
      if (id === photoId.current) {
        setStage((old) => {
          if (old) URL.revokeObjectURL(old.url);
          return r;
        });
      } else URL.revokeObjectURL(r.url);
    } catch (err) {
      onError(String(err));
    } finally {
      inflight.current = false;
      setBusy(false);
      pump();
    }
  }, [onError]);
  useEffect(() => {
    if (!combined) return;
    pending.current = combined;
    pump();
  }, [combined, pump]);
  useEffect(
    () => () =>
      setStage((old) => {
        if (old) URL.revokeObjectURL(old.url);
        return null;
      }),
    [photo.id],
  );

  // [ and ] step through looks; Enter applies.
  const lookIds = useMemo(() => [ORIGINAL, ...(data?.looks.map((l) => l.id) ?? [])], [data]);
  const apply = useCallback(async () => {
    if (!combined) return;
    try {
      onSaved(await api.saveEdits([photo.id], combined));
      onNotice(`Applied to ${photo.fileName}`);
    } catch (err) {
      onError(String(err));
    }
  }, [combined, photo, onSaved, onError, onNotice]);
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (t.tagName === "INPUT" && (t as HTMLInputElement).type !== "range") return;
      if (e.key === "]" || e.key === "[") {
        e.preventDefault();
        const i = lookIds.indexOf(look);
        const next = lookIds[(i + (e.key === "]" ? 1 : -1) + lookIds.length) % lookIds.length];
        setLook(next);
        setOver({});
      } else if (e.key === "Enter") {
        e.preventDefault();
        apply();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [lookIds, look, apply]);

  const applyToPicks = async () => {
    if (look === ORIGINAL) return;
    const targets = keepers;
    try {
      for (const p of targets) {
        const own = await api.getEdits(p.id);
        let h = harmony;
        if (look === "harmony" && p.id !== photo.id) {
          const c = await api.analyzeColour(p.id, own);
          h = c.harmonies[0] ? [c.harmonies[0].template, c.harmonies[0].angle] : null;
        }
        const e = await api.lookEdits(look, own, strength, h);
        if (over.grain != null) e.grain = over.grain;
        if (over.glow != null) e.glow = over.glow;
        if (over.edgeBurn != null) e.edgeBurn = over.edgeBurn;
        if (p.id === photo.id && composition) {
          e.crop = composition.edits.crop;
          e.angle = composition.edits.angle;
        }
        onSaved(await api.saveEdits([p.id], e));
      }
      onNotice(`Applied “${data?.looks.find((l) => l.id === look)?.name}” to ${targets.length} photo${targets.length === 1 ? "" : "s"}`);
    } catch (err) {
      onError(String(err));
    }
  };

  const current = data?.looks.find((l) => l.id === look);
  const groups = useMemo(() => {
    const g: Record<string, ExploreData["looks"]> = {};
    for (const l of data?.looks ?? []) (g[l.group] ??= []).push(l);
    return g;
  }, [data]);

  return (
    <div className="explore">
      <div className="ex-main">
        <Stage img={stage} busy={busy} guide={composition?.guide ?? null} subject={composition?.subject ?? null} />
        <aside className="ex-side">
          <h3>{current ? current.group : "Your photo"}</h3>
          <div className="ex-title">{current ? current.name : "As edited"}</div>
          <p className="hint">{current ? current.description : "Pick a look and a framing below. Nothing changes until you apply."}</p>
          {current && (
            <>
              <Slider label="Strength" value={strength} min={0} max={150} onChange={setStrength} />
              <Slider label="Grain" value={over.grain ?? combined?.grain ?? 0} min={0} max={100} onChange={(v) => setOver({ ...over, grain: v })} />
              <Slider label="Glow" value={over.glow ?? combined?.glow ?? 0} min={0} max={100} onChange={(v) => setOver({ ...over, glow: v })} />
              <Slider label="Print edges" value={over.edgeBurn ?? combined?.edgeBurn ?? 0} min={0} max={100} onChange={(v) => setOver({ ...over, edgeBurn: v })} />
            </>
          )}
          {composition && (
            <p className="ex-comp">
              <b>{composition.label}</b>
              <br />
              <span className="hint">{guideText(composition.guide)}</span>
            </p>
          )}
          <div className="ex-actions">
            <button className="primary wide" onClick={apply} disabled={!combined}>
              Apply (Enter)
            </button>
            {current && keepers.length > 1 && (
              <button className="wide" onClick={applyToPicks}>
                Apply look to all {keepers.length} picks
              </button>
            )}
            <button className="wide" onClick={onOpenDevelop}>
              Fine-tune in Develop
            </button>
          </div>
          {data && (
            <div className="ex-palette">
              <h3>Colour</h3>
              <div className="swatches">
                {data.colour.palette.map((p, i) => (
                  <span key={i} title={`${Math.round(p.share * 100)}%`} style={{ background: `rgb(${p.rgb.join(",")})`, flex: Math.max(0.15, p.share) }} />
                ))}
              </div>
              {data.colour.harmonies[0] && (
                <p className="hint">
                  Closest harmony: <b>{data.colour.harmonies[0].label}</b> ({Math.round(data.colour.harmonies[0].score)}/100)
                </p>
              )}
            </div>
          )}
        </aside>
      </div>

      <div className="ex-rows">
        <div className="ex-row">
          <div className="ex-row-label">Looks</div>
          <div className="ex-cards">
            <Card name="As edited" url={cards[`look:${ORIGINAL}`]} on={look === ORIGINAL} onClick={() => (setLook(ORIGINAL), setOver({}))} />
            {Object.entries(groups).map(([group, list]) => (
              <div key={group} className="ex-group">
                <span className="ex-group-label">{group}</span>
                {list.map((l) => (
                  <Card key={l.id} name={l.name} title={l.description} url={cards[`look:${l.id}`]} on={look === l.id} onClick={() => (setLook(l.id), setOver({}))} />
                ))}
              </div>
            ))}
            {!data && <div className="hint ex-loading">Studying the photo…</div>}
          </div>
        </div>
        <div className="ex-row">
          <div className="ex-row-label">Composition</div>
          <div className="ex-cards">
            <Card name="Current framing" url={cards[`look:${ORIGINAL}`]} on={comp === -1} onClick={() => setComp(-1)} />
            {data?.compositions.map((c, i) => (
              <Card key={i} name={c.label} url={cards[`comp:${i}`]} on={comp === i} onClick={() => setComp(i)} guide={c.guide} subject={c.subject} />
            ))}
            {data && data.compositions.length === 0 && <div className="hint ex-loading">No stronger framing found — the photo is already well composed.</div>}
          </div>
        </div>
      </div>
    </div>
  );
}

function guideText(g: Guide): string {
  switch (g) {
    case "thirds":
      return "The subject sits on a third — the classic way to give a frame tension and room.";
    case "golden":
      return "The subject sits on the golden section (0.382), a slightly tighter, more classical balance than thirds.";
    case "centre":
      return "Centred and symmetrical: calm, formal, direct.";
    case "level":
      return "Levels the horizon the photo is built on.";
  }
}

function Card({
  name,
  url,
  on,
  onClick,
  title,
  guide,
  subject,
}: {
  name: string;
  url?: string;
  on: boolean;
  onClick: () => void;
  title?: string;
  guide?: Guide;
  subject?: [number, number];
}) {
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (on) ref.current?.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  }, [on]);
  return (
    <button ref={ref} className={"ex-card" + (on ? " on" : "")} onClick={onClick} title={title ?? name}>
      <div className="ex-thumb">
        {url ? <img src={url} alt="" draggable={false} /> : <div className="ex-skeleton" />}
        {url && guide && guide !== "level" && <GuideOverlay guide={guide} subject={subject} subtle />}
      </div>
      <span>{name}</span>
    </button>
  );
}

function Stage({ img, busy, guide, subject }: { img: Rendered | null; busy: boolean; guide: Guide | null; subject: [number, number] | null }) {
  const ref = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 1, h: 1 });
  useLayoutEffect(() => {
    const el = ref.current!;
    const ro = new ResizeObserver(() => setBox({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const pad = 20;
  const iw = img?.width ?? 3;
  const ih = img?.height ?? 2;
  const s = Math.min((box.w - pad * 2) / iw, (box.h - pad * 2) / ih);
  const dw = iw * s;
  const dh = ih * s;
  const style = { left: (box.w - dw) / 2, top: (box.h - dh) / 2, width: dw, height: dh };
  return (
    <div className="ex-stage" ref={ref}>
      {img && <img src={img.url} style={style} alt="" draggable={false} />}
      {img && guide && guide !== "level" && (
        <div className="ex-guide" style={style}>
          <GuideOverlay guide={guide} subject={subject ?? undefined} />
        </div>
      )}
      {busy && <div className="spinner" />}
      {!img && <div className="empty">Developing…</div>}
    </div>
  );
}

function Slider({ label, value, min, max, onChange }: { label: string; value: number; min: number; max: number; onChange: (v: number) => void }) {
  return (
    <label className="slider">
      <span className="sl-label">{label}</span>
      <input type="range" min={min} max={max} value={value} onChange={(e) => onChange(Number(e.target.value))} />
      <span className="sl-value">{Math.round(value)}</span>
    </label>
  );
}
