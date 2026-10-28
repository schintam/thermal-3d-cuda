import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Photo, thumbUrl } from "../api";
import { Badges } from "./Badges";

interface Props {
  items: Photo[];
  focus: number | null;
  selected: Set<number>;
  size: number;
  burstCounts: Map<number, number>;
  expanded: Set<number>;
  collapse: boolean;
  onClick: (id: number, e: React.MouseEvent) => void;
  onOpen: (id: number) => void;
  onColumns: (n: number) => void;
  onToggleBurst: (burstId: number) => void;
  emptyText: string;
}

const GAP = 8;
const CAPTION = 22;
const OVERSCAN_ROWS = 3;

/** Virtualised grid: only rows near the viewport are in the DOM, so 50k photos scroll like 50. */
export function Grid({ items, focus, selected, size, burstCounts, expanded, collapse, onClick, onOpen, onColumns, onToggleBurst, emptyText }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [box, setBox] = useState({ w: 800, h: 600 });

  useLayoutEffect(() => {
    const el = ref.current!;
    const ro = new ResizeObserver(() => setBox({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setBox({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, []);

  const cellW = size + GAP;
  const cellH = size + CAPTION + GAP;
  const cols = Math.max(1, Math.floor((box.w - GAP) / cellW));
  const rows = Math.ceil(items.length / cols);
  useEffect(() => {
    onColumns(cols);
  }, [cols, onColumns]);

  // Keep the focused cell in view during keyboard navigation.
  useEffect(() => {
    if (focus == null || !ref.current) return;
    const idx = items.findIndex((p) => p.id === focus);
    if (idx < 0) return;
    const top = Math.floor(idx / cols) * cellH;
    const el = ref.current;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + cellH > el.scrollTop + el.clientHeight) el.scrollTop = top + cellH - el.clientHeight + GAP;
  }, [focus, items, cols, cellH]);

  const first = Math.max(0, Math.floor(scrollTop / cellH) - OVERSCAN_ROWS);
  const last = Math.min(rows, Math.ceil((scrollTop + box.h) / cellH) + OVERSCAN_ROWS);
  const leftPad = Math.max(GAP, (box.w - cols * cellW + GAP) / 2);

  const cells = [];
  for (let r = first; r < last; r++) {
    for (let c = 0; c < cols; c++) {
      const i = r * cols + c;
      if (i >= items.length) break;
      const p = items[i];
      cells.push(
        <Cell
          key={p.id}
          photo={p}
          x={leftPad + c * cellW}
          y={GAP + r * cellH}
          size={size}
          focused={p.id === focus}
          selected={selected.has(p.id)}
          stack={collapse && p.burstId != null ? burstCounts.get(p.burstId) ?? 0 : 0}
          expanded={p.burstId != null && expanded.has(p.burstId)}
          onClick={onClick}
          onOpen={onOpen}
          onToggleBurst={onToggleBurst}
        />,
      );
    }
  }

  return (
    <div className="grid" ref={ref} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}>
      <div style={{ height: rows * cellH + GAP, position: "relative" }}>{cells}</div>
      {items.length === 0 && <div className="empty">{emptyText}</div>}
    </div>
  );
}

interface CellProps {
  photo: Photo;
  x: number;
  y: number;
  size: number;
  focused: boolean;
  selected: boolean;
  stack: number;
  expanded: boolean;
  onClick: Props["onClick"];
  onOpen: Props["onOpen"];
  onToggleBurst: Props["onToggleBurst"];
}

const Cell = memo(function Cell({ photo: p, x, y, size, focused, selected, stack, expanded, onClick, onOpen, onToggleBurst }: CellProps) {
  const [failed, setFailed] = useState(false);
  useEffect(() => setFailed(false), [p.processed]);
  const cls = ["cell", focused && "focused", selected && "selected", p.pick === -1 && "rejected", p.pick === 1 && "picked"]
    .filter(Boolean)
    .join(" ");
  return (
    <div
      className={cls}
      style={{ transform: `translate(${x}px, ${y}px)`, width: size, height: size + CAPTION }}
      onMouseDown={(e) => onClick(p.id, e)}
      onDoubleClick={() => onOpen(p.id)}
    >
      <div className="thumb" style={{ width: size, height: size }}>
        {failed || p.error ? (
          <div className="placeholder">
            <span>{p.kind.toUpperCase()}</span>
            {p.error && <small title={p.error}>⚠ unreadable</small>}
            {!p.error && p.kind === "heic" && <small>preview unavailable</small>}
          </div>
        ) : (
          <img src={thumbUrl(p)} loading="lazy" decoding="async" draggable={false} onError={() => setFailed(true)} alt="" />
        )}
        <Badges photo={p} />
        {stack > 1 && (
          <button
            className={"stack" + (expanded ? " open" : "")}
            title={expanded ? "Collapse burst (S)" : "Expand burst (S)"}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.stopPropagation();
              onToggleBurst(p.burstId!);
            }}
          >
            ❐ {stack}
          </button>
        )}
      </div>
      <div className="caption" title={p.path}>
        {p.fileName}
        {p.pairPath && <span className="pair">+JPG</span>}
      </div>
    </div>
  );
});
