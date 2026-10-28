import { useLayoutEffect, useRef, useState } from "react";
import { Photo, fullUrl, previewUrl, thumbUrl } from "../api";

/** Zoom state shared between views: `zoom` null = fit, else 1 = 100%. Centre is normalised 0..1. */
export interface ZoomState {
  zoom: number | null;
  cx: number;
  cy: number;
}

export const FIT: ZoomState = { zoom: null, cx: 0.5, cy: 0.5 };

interface Props {
  photo: Photo;
  view: ZoomState;
  onView: (v: ZoomState) => void;
}

/** Image viewer with fit / 100% zoom, drag to pan, wheel to zoom about the cursor. */
export function ZoomView({ photo, view, onView }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 1, h: 1 });
  const [natural, setNatural] = useState<{ w: number; h: number } | null>(null);
  const [loadedSrc, setLoadedSrc] = useState<string | null>(null);
  const drag = useRef<{ x: number; y: number; cx: number; cy: number; moved: boolean } | null>(null);

  useLayoutEffect(() => {
    const el = ref.current!;
    const ro = new ResizeObserver(() => setBox({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setBox({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, []);

  // Full-resolution size; fall back to whatever has loaded.
  const W = photo.width ?? natural?.w ?? 1600;
  const H = photo.height ?? natural?.h ?? 1067;
  const fit = Math.min(box.w / W, box.h / H, 1);
  const scale = view.zoom ?? fit;
  const dw = W * scale;
  const dh = H * scale;
  const zoomed = view.zoom != null && view.zoom > fit * 1.01;
  // Clamp the centre so the image never leaves the viewport while zoomed.
  const clamp = (c: number, d: number, b: number) => (d <= b ? 0.5 : Math.min(1 - b / 2 / d, Math.max(b / 2 / d, c)));
  const cx = clamp(view.cx, dw, box.w);
  const cy = clamp(view.cy, dh, box.h);
  const left = box.w / 2 - cx * dw;
  const top = box.h / 2 - cy * dh;
  const src = zoomed ? fullUrl(photo.id) : previewUrl(photo.id);

  const onDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    drag.current = { x: e.clientX, y: e.clientY, cx, cy, moved: false };
  };
  const onMove = (e: React.MouseEvent) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.x;
    const dy = e.clientY - d.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) d.moved = true;
    if (d.moved && zoomed) onView({ zoom: view.zoom, cx: d.cx - dx / dw, cy: d.cy - dy / dh });
  };
  const onUp = (e: React.MouseEvent) => {
    const d = drag.current;
    drag.current = null;
    if (!d || d.moved) return;
    if (zoomed) onView(FIT);
    else {
      // Zoom to 100% centred on the clicked point.
      const rect = ref.current!.getBoundingClientRect();
      onView({ zoom: 1, cx: (e.clientX - rect.left - left) / dw, cy: (e.clientY - rect.top - top) / dh });
    }
  };
  const onWheel = (e: React.WheelEvent) => {
    const rect = ref.current!.getBoundingClientRect();
    const px = (e.clientX - rect.left - left) / dw;
    const py = (e.clientY - rect.top - top) / dh;
    const next = Math.min(4, Math.max(fit, scale * Math.pow(1.0015, -e.deltaY)));
    if (next <= fit * 1.001) return onView(FIT);
    // Keep the point under the cursor fixed.
    const ndw = W * next;
    const ndh = H * next;
    onView({ zoom: next, cx: px - (e.clientX - rect.left - box.w / 2) / ndw, cy: py - (e.clientY - rect.top - box.h / 2) / ndh });
  };

  return (
    <div
      className={"zoomview" + (zoomed ? " zoomed" : "")}
      ref={ref}
      onMouseDown={onDown}
      onMouseMove={onMove}
      onMouseUp={onUp}
      onMouseLeave={() => (drag.current = null)}
      onWheel={onWheel}
    >
      {/* Low-res thumbnail underneath while the preview loads. */}
      {loadedSrc !== src && (
        <img className="under" src={thumbUrl(photo)} style={{ left, top, width: dw, height: dh }} draggable={false} alt="" />
      )}
      <img
        key={src}
        src={src}
        style={{ left, top, width: dw, height: dh, opacity: loadedSrc === src ? 1 : 0 }}
        draggable={false}
        onLoad={(e) => {
          setNatural({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight });
          setLoadedSrc(src);
        }}
        alt=""
      />
      {zoomed && <div className="zoomlabel">{Math.round(scale * 100)}%</div>}
    </div>
  );
}
