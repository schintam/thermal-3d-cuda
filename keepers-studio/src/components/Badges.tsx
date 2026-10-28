import { Photo } from "../api";

/** Small overlays: cull state, rating and the local auto-scores (suggestions only). */
export function Badges({ photo: p, large = false }: { photo: Photo; large?: boolean }) {
  const warnings: string[] = [];
  if (p.sharpness != null && p.sharpness < 22) warnings.push("soft");
  if (p.clipHi != null && p.clipHi > 0.03) warnings.push("highlights");
  if (p.exposure != null && p.exposure < 0.12) warnings.push("dark");
  if (p.clipLo != null && p.clipLo > 0.15) warnings.push("shadows");
  return (
    <div className={"badges" + (large ? " large" : "")}>
      <div className="tl">
        {p.pick === 1 && <span className="flag pick" title="Pick (P)">⚑</span>}
        {p.pick === -1 && <span className="flag reject" title="Rejected (X)">✕</span>}
        {p.burstBest && <span className="best" title="Suggested best of burst (B to choose another)">★ best</span>}
        {p.dupOf != null && <span className="dup" title="Near-duplicate of an earlier photo">≈ dup</span>}
      </div>
      <div className="tr">
        {p.edited && <span className="edited" title="Has develop edits">◐</span>}
        {p.kind === "raw" && <span className="kind">RAW</span>}
      </div>
      <div className="bl">
        {p.rating > 0 && <span className="stars">{"★".repeat(p.rating)}</span>}
      </div>
      <div className="br">
        {warnings.map((w) => (
          <span key={w} className="warn" title={`Auto-score: ${w}`}>
            {w}
          </span>
        ))}
        {p.sharpness != null && (
          <span className="score" title={`Sharpness score ${p.sharpness.toFixed(0)} / 100 (local estimate)`}>
            <i style={{ width: `${Math.min(100, p.sharpness)}%` }} />
          </span>
        )}
      </div>
    </div>
  );
}
