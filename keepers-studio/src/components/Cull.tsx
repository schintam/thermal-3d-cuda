import { useEffect, useMemo, useState } from "react";
import { CullOptions, CullPlan, CullReason, Photo, Strictness, api, modLabel, thumbUrl } from "../api";
import { Modal } from "./Dialogs";

export const REASON_TEXT: Record<CullReason, string> = {
  topOfMoment: "Best frame of its moment",
  strongFrame: "Nearly as strong as the best of its moment",
  alternate: "A good alternate",
  overLimit: "Good, but over the pick limit",
  burstFrame: "Weaker frame of a burst",
  duplicate: "Near-identical copy of a better photo",
  outOfFocus: "Out of focus or shaken",
  blown: "Highlights blown out",
  tooDark: "Far too dark to rescue",
  decided: "You already decided on this one",
  unscored: "Not analysed yet",
};

const STRICTNESS: { key: Strictness; label: string; text: string }[] = [
  { key: "gentle", label: "Gentle", text: "Rejects only clear failures. Picks generously." },
  { key: "balanced", label: "Balanced", text: "Rejects failures, duplicates and weaker burst frames. Picks the stronger half of each moment." },
  { key: "strict", label: "Strict", text: "Rejects borderline focus and exposure too. Picks only the standouts." },
];

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

const REJECT_REASONS: CullReason[] = ["burstFrame", "duplicate", "outOfFocus", "blown", "tooDark"];

export function CullDialog({
  folderId,
  photos,
  onApply,
  onClose,
}: {
  folderId: number | null;
  photos: Photo[];
  onApply: (plan: CullPlan) => void;
  onClose: () => void;
}) {
  const [opts, setOpts] = useState<CullOptions>(() => {
    try {
      return { strictness: "balanced", maxPicks: null, reviseDecided: false, ...JSON.parse(localStorage.getItem("keepers.cull") ?? "{}") };
    } catch {
      return { strictness: "balanced", maxPicks: null, reviseDecided: false };
    }
  });
  const [limit, setLimit] = useState(opts.maxPicks ?? 50);
  const [plan, setPlan] = useState<CullPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [show, setShow] = useState<"pick" | "reject">("pick");

  useEffect(() => {
    try {
      localStorage.setItem("keepers.cull", JSON.stringify(opts));
    } catch {
      /* preferences are a convenience */
    }
    let live = true;
    api
      .planCull(folderId, opts)
      .then((p) => live && setPlan(p))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, [folderId, opts]);

  const byId = useMemo(() => new Map(photos.map((p) => [p.id, p])), [photos]);
  const summary = useMemo(() => {
    const count = { pick: 0, reject: 0, leave: 0 };
    const reasons = new Map<CullReason, number>();
    for (const s of plan?.suggestions ?? []) {
      count[s.verdict]++;
      reasons.set(s.reason, (reasons.get(s.reason) ?? 0) + 1);
    }
    return { count, reasons };
  }, [plan]);
  const preview = useMemo(
    () => (plan?.suggestions ?? []).filter((s) => s.verdict === show).slice(0, 48),
    [plan, show],
  );
  const unscored = summary.reasons.get("unscored") ?? 0;
  const changes = summary.count.pick + summary.count.reject;

  return (
    <Modal title="Auto-cull" onClose={onClose}>
      <div className="cull">
        <p className="hint">
          Goes through {photos.length} photo{photos.length === 1 ? "" : "s"}, groups them into moments, and suggests picks and rejects from focus, exposure,
          bursts and duplicates. Nothing is deleted. Photos you've already flagged, starred or edited are left alone, and {modLabel}Z undoes the whole pass.
        </p>

        <div className="field">
          <label>How strict</label>
          <div className="segmented">
            {STRICTNESS.map((s) => (
              <button key={s.key} className={opts.strictness === s.key ? "on" : ""} onClick={() => setOpts({ ...opts, strictness: s.key })}>
                {s.label}
              </button>
            ))}
          </div>
          <p className="hint">{STRICTNESS.find((s) => s.key === opts.strictness)!.text}</p>
        </div>

        <div className="field cull-options">
          <label className="toggle">
            <input
              type="checkbox"
              checked={opts.maxPicks != null}
              onChange={(e) => setOpts({ ...opts, maxPicks: e.target.checked ? limit : null })}
            />{" "}
            Keep at most
          </label>
          <input
            type="number"
            min={1}
            value={limit}
            onChange={(e) => {
              const n = Math.max(1, Number(e.target.value) || 1);
              setLimit(n);
              if (opts.maxPicks != null) setOpts({ ...opts, maxPicks: n });
            }}
          />
          <span>picks, spread across the moments</span>
        </div>
        <label className="toggle small">
          <input type="checkbox" checked={opts.reviseDecided} onChange={(e) => setOpts({ ...opts, reviseDecided: e.target.checked })} /> Also revise photos I've
          already flagged (starred and edited photos are always kept)
        </label>

        {error && <p className="err">{error}</p>}
        {plan && (
          <>
            <div className="cull-summary">
              <button className={"pick" + (show === "pick" ? " on" : "")} onClick={() => setShow("pick")}>
                <b>{summary.count.pick}</b> picks
              </button>
              <button className={"reject" + (show === "reject" ? " on" : "")} onClick={() => setShow("reject")}>
                <b>{summary.count.reject}</b> rejects
              </button>
              <div>
                <b>{summary.count.leave}</b> left for you
              </div>
              <div>
                <b>{plan.moments}</b> moment{plan.moments === 1 ? "" : "s"}
              </div>
            </div>
            <ul className="cull-reasons">
              {REJECT_REASONS.filter((r) => summary.reasons.get(r)).map((r) => (
                <li key={r}>
                  <span className="flag reject">✕</span> {summary.reasons.get(r)} · {REASON_TEXT[r].toLowerCase()}
                </li>
              ))}
              {(summary.reasons.get("overLimit") ?? 0) > 0 && <li>{summary.reasons.get("overLimit")} good photos held back by the pick limit</li>}
              {(summary.reasons.get("decided") ?? 0) > 0 && <li>{summary.reasons.get("decided")} you already decided on, left as they are</li>}
            </ul>
            {unscored > 0 && (
              <p className="hint warn-text">
                {unscored} photo{unscored === 1 ? " is" : "s are"} still being analysed and will be left unflagged. Run Auto-cull again once previews finish.
              </p>
            )}
            <div className="cull-strip">
              {preview.map((s) => {
                const p = byId.get(s.id);
                return p ? <img key={s.id} src={thumbUrl(p)} title={`${p.fileName}: ${REASON_TEXT[s.reason]}`} loading="lazy" /> : null;
              })}
              {preview.length === 0 && <span className="hint">None</span>}
            </div>
          </>
        )}

        <div className="actions">
          <button onClick={onClose}>Cancel</button>
          <button
            className="primary"
            disabled={!plan || changes === 0}
            onClick={() => {
              onApply(plan!);
              onClose();
            }}
          >
            Apply {plural(summary.count.pick, "pick")}, {plural(summary.count.reject, "reject")}
          </button>
        </div>
      </div>
    </Modal>
  );
}
