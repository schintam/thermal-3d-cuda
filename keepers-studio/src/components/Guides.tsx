import { Guide } from "../api";

export type OverlayKind = Guide | "spiral" | "diagonal" | "none";

const PHI = 0.381966;

/**
 * Composition guides drawn over an image: thirds, golden section, golden
 * spiral, the diagonal method, and centre lines. Fills its positioned parent.
 */
export function GuideOverlay({
  guide,
  subject,
  subtle = false,
  flip = 0,
}: {
  guide: OverlayKind;
  subject?: [number, number];
  subtle?: boolean;
  /** Spiral orientation 0..3. */
  flip?: number;
}) {
  if (guide === "none" || guide === "level") return null;
  const lines: [number, number, number, number][] = [];
  const v = (x: number) => lines.push([x, 0, x, 1]);
  const h = (y: number) => lines.push([0, y, 1, y]);
  if (guide === "thirds") [1 / 3, 2 / 3].forEach((t) => (v(t), h(t)));
  if (guide === "golden") [PHI, 1 - PHI].forEach((t) => (v(t), h(t)));
  if (guide === "centre") (v(0.5), h(0.5));
  if (guide === "diagonal") {
    lines.push([0, 0, 1, 1], [1, 0, 0, 1]);
  }
  const sx = flip & 1 ? -1 : 1;
  const sy = flip & 2 ? -1 : 1;
  return (
    <svg className={"guides" + (subtle ? " subtle" : "")} viewBox="0 0 1 1" preserveAspectRatio="none">
      {lines.map(([x1, y1, x2, y2], i) => (
        <line key={i} x1={x1} y1={y1} x2={x2} y2={y2} vectorEffect="non-scaling-stroke" />
      ))}
      {guide === "spiral" && (
        <g transform={`translate(${sx < 0 ? 1 : 0} ${sy < 0 ? 1 : 0}) scale(${sx} ${sy})`}>
          <path d={spiralPath()} vectorEffect="non-scaling-stroke" fill="none" />
        </g>
      )}
      {subject && <circle cx={subject[0]} cy={subject[1]} r={0.012} className="subject" vectorEffect="non-scaling-stroke" />}
    </svg>
  );
}

/** Golden spiral approximated by quarter arcs in successively smaller golden rectangles (unit square, stretched to the frame). */
function spiralPath(): string {
  let [x, y, w, hgt] = [0, 0, 1, 1];
  let d = "";
  for (let i = 0; i < 9; i++) {
    const dir = i % 4;
    if (dir === 0) {
      const s = w * (1 - PHI);
      d += `${i === 0 ? "M" : "L"}${x},${y + hgt} A${s},${hgt} 0 0 1 ${x + s},${y} `;
      x += s;
      w -= s;
    } else if (dir === 1) {
      const s = hgt * (1 - PHI);
      d += `A${w},${s} 0 0 1 ${x + w},${y + s} `;
      y += s;
      hgt -= s;
    } else if (dir === 2) {
      const s = w * (1 - PHI);
      d += `A${s},${hgt} 0 0 1 ${x + w - s},${y + hgt} `;
      w -= s;
    } else {
      const s = hgt * (1 - PHI);
      d += `A${w},${s} 0 0 1 ${x},${y + hgt - s} `;
      hgt -= s;
    }
  }
  return d;
}
