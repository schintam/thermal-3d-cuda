import { useEffect, useRef } from "react";

interface Props {
  data: [Uint32Array, Uint32Array, Uint32Array] | null;
  clip: boolean;
  onToggleClip: () => void;
}

const COLORS = ["rgba(255,80,80,0.55)", "rgba(80,220,100,0.55)", "rgba(90,140,255,0.6)"];

export function Histogram({ data, clip, onToggleClip }: Props) {
  const ref = useRef<HTMLCanvasElement>(null);
  const hi = data ? data.some((c) => c[255] > 0) : false;
  const lo = data ? data.every((c) => c[0] > 0) : false;

  useEffect(() => {
    const cv = ref.current;
    if (!cv) return;
    const dpr = window.devicePixelRatio || 1;
    const w = cv.clientWidth;
    const h = cv.clientHeight;
    cv.width = w * dpr;
    cv.height = h * dpr;
    const g = cv.getContext("2d")!;
    g.scale(dpr, dpr);
    g.clearRect(0, 0, w, h);
    if (!data) return;
    // Scale to the 99.5th percentile bin so a spike doesn't flatten the rest.
    const all = Array.from(data[0]).concat(Array.from(data[1]), Array.from(data[2])).sort((a, b) => a - b);
    const max = Math.max(1, all[Math.floor(all.length * 0.995)]);
    g.globalCompositeOperation = "lighter";
    data.forEach((ch, c) => {
      g.fillStyle = COLORS[c];
      g.beginPath();
      g.moveTo(0, h);
      for (let i = 0; i < 256; i++) g.lineTo((i / 255) * w, h - Math.min(1, ch[i] / max) * h);
      g.lineTo(w, h);
      g.closePath();
      g.fill();
    });
  }, [data]);

  return (
    <div className="histogram">
      <canvas ref={ref} />
      <button className={"clipind lo" + (lo ? " on" : "")} title="Shadow clipping (J toggles overlay)" onClick={onToggleClip}>
        ◤
      </button>
      <button className={"clipind hi" + (hi ? " on" : "") + (clip ? " active" : "")} title="Highlight clipping (J toggles overlay)" onClick={onToggleClip}>
        ◥
      </button>
    </div>
  );
}
