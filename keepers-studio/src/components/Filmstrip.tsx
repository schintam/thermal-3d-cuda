import { useEffect, useRef } from "react";
import { Photo, thumbUrl } from "../api";

interface Props {
  items: Photo[];
  focus: number | null;
  onPick: (id: number) => void;
}

const W = 96;

/** Horizontal strip; virtualised by simple windowing around the scroll position. */
export function Filmstrip({ items, focus, onPick }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const idx = items.findIndex((p) => p.id === focus);

  useEffect(() => {
    const el = ref.current;
    if (!el || idx < 0) return;
    const x = idx * W;
    if (x < el.scrollLeft || x + W > el.scrollLeft + el.clientWidth) el.scrollLeft = x - el.clientWidth / 2 + W / 2;
  }, [idx]);

  // Render a window of ~200 around the focus; enough for any screen width.
  const start = Math.max(0, idx - 100);
  const end = Math.min(items.length, start + 200);
  return (
    <div className="filmstrip" ref={ref}>
      <div style={{ width: items.length * W, height: "100%", position: "relative" }}>
        {items.slice(start, end).map((p, i) => (
          <div
            key={p.id}
            className={"fs-cell" + (p.id === focus ? " focused" : "") + (p.pick === -1 ? " rejected" : "")}
            style={{ left: (start + i) * W }}
            onClick={() => onPick(p.id)}
            title={p.fileName}
          >
            {p.error ? <span className="fs-err" title={p.error}>⚠</span> : <img src={thumbUrl(p)} loading="lazy" draggable={false} alt="" />}
            {p.pick === 1 && <span className="fs-flag">⚑</span>}
            {p.rating > 0 && <span className="fs-stars">{"★".repeat(p.rating)}</span>}
          </div>
        ))}
      </div>
    </div>
  );
}
