import { useRef, useState } from "react";

interface Props {
  points: [number, number][];
  onChange: (p: [number, number][]) => void;
}

const S = 220;

/** Monotone curve editor: drag points, click empty space to add, double-click a point to remove. */
export function ToneCurve({ points, onChange }: Props) {
  const ref = useRef<SVGSVGElement>(null);
  const [drag, setDrag] = useState<number | null>(null);
  const sorted = [...points].sort((a, b) => a[0] - b[0]);

  const toXY = (e: React.PointerEvent): [number, number] => {
    const r = ref.current!.getBoundingClientRect();
    return [clamp((e.clientX - r.left) / r.width), clamp(1 - (e.clientY - r.top) / r.height)];
  };

  const down = (e: React.PointerEvent) => {
    const [x, y] = toXY(e);
    let i = sorted.findIndex((p) => Math.hypot(p[0] - x, p[1] - y) < 0.05);
    if (i < 0) {
      const next = [...sorted, [x, y] as [number, number]].sort((a, b) => a[0] - b[0]);
      i = next.findIndex((p) => p[0] === x && p[1] === y);
      onChange(next);
    }
    setDrag(i);
    (e.target as Element).setPointerCapture?.(e.pointerId);
  };

  const move = (e: React.PointerEvent) => {
    if (drag == null) return;
    let [x, y] = toXY(e);
    const pts = [...sorted];
    // Endpoints keep their x; inner points stay between neighbours.
    if (drag === 0) x = 0;
    else if (drag === pts.length - 1) x = 1;
    else x = Math.min(pts[drag + 1][0] - 0.01, Math.max(pts[drag - 1][0] + 0.01, x));
    pts[drag] = [x, y];
    onChange(pts);
  };

  const remove = (i: number) => {
    if (i === 0 || i === sorted.length - 1) return;
    onChange(sorted.filter((_, j) => j !== i));
  };

  const path = curvePath(sorted);
  return (
    <svg
      ref={ref}
      className="tonecurve"
      viewBox={`0 0 ${S} ${S}`}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={() => setDrag(null)}
    >
      {[0.25, 0.5, 0.75].map((g) => (
        <g key={g} className="gridline">
          <line x1={g * S} y1={0} x2={g * S} y2={S} />
          <line x1={0} y1={g * S} x2={S} y2={g * S} />
        </g>
      ))}
      <line className="diag" x1={0} y1={S} x2={S} y2={0} />
      <path d={path} />
      {sorted.map((p, i) => (
        <circle
          key={i}
          cx={p[0] * S}
          cy={(1 - p[1]) * S}
          r={5}
          onDoubleClick={(e) => {
            e.stopPropagation();
            remove(i);
          }}
        />
      ))}
    </svg>
  );
}

function clamp(v: number) {
  return Math.min(1, Math.max(0, v));
}

/** Same Fritsch–Carlson monotone cubic the engine uses, sampled for display. */
function curvePath(p: [number, number][]): string {
  const n = p.length;
  if (n < 2) return "";
  const d = p.slice(0, -1).map((q, i) => (p[i + 1][1] - q[1]) / (p[i + 1][0] - q[0] || 1e-6));
  const m = p.map((_, i) => (i === 0 ? d[0] : i === n - 1 ? d[n - 2] : d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) / 2));
  for (let i = 0; i < n - 1; i++) {
    if (d[i] === 0) {
      m[i] = 0;
      m[i + 1] = 0;
    } else {
      const a = m[i] / d[i];
      const b = m[i + 1] / d[i];
      const s = a * a + b * b;
      if (s > 9) {
        const t = 3 / Math.sqrt(s);
        m[i] = t * a * d[i];
        m[i + 1] = t * b * d[i];
      }
    }
  }
  const pts: string[] = [];
  for (let k = 0; k <= 100; k++) {
    const x = k / 100;
    let y: number;
    if (x <= p[0][0]) y = p[0][1];
    else if (x >= p[n - 1][0]) y = p[n - 1][1];
    else {
      let i = 0;
      while (i < n - 2 && x > p[i + 1][0]) i++;
      const h = p[i + 1][0] - p[i][0];
      const t = (x - p[i][0]) / h;
      const t2 = t * t;
      const t3 = t2 * t;
      y = (2 * t3 - 3 * t2 + 1) * p[i][1] + (t3 - 2 * t2 + t) * h * m[i] + (-2 * t3 + 3 * t2) * p[i + 1][1] + (t3 - t2) * h * m[i + 1];
    }
    pts.push(`${(x * S).toFixed(1)},${((1 - clamp(y)) * S).toFixed(1)}`);
  }
  return "M" + pts.join(" L");
}
