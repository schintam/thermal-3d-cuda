import { useEffect } from "react";
import { Photo, formatShot, formatTime, previewUrl } from "../api";
import { Badges } from "./Badges";
import { ZoomState, ZoomView } from "./ZoomView";

interface Props {
  photo: Photo;
  neighbours: Photo[];
  view: ZoomState;
  onView: (v: ZoomState) => void;
  showInfo: boolean;
  burstSize: number;
  /** Why Auto-cull suggested what it did. */
  note?: string;
}

export function Loupe({ photo, neighbours, view, onView, showInfo, burstSize, note }: Props) {
  // Prefetch the next/previous previews so flipping through is instant.
  useEffect(() => {
    const imgs = neighbours.map((n) => {
      const i = new Image();
      i.decoding = "async";
      i.src = previewUrl(n.id);
      return i;
    });
    return () => imgs.forEach((i) => (i.src = ""));
  }, [neighbours]);

  return (
    <div className="loupe">
      <ZoomView photo={photo} view={view} onView={onView} />
      <Badges photo={photo} large />
      {showInfo && (
        <div className="info">
          <div className="name">{photo.fileName}</div>
          <div>{formatTime(photo.captureTime)}</div>
          <div>{[photo.camera, photo.lens].filter(Boolean).join(" · ")}</div>
          <div>{formatShot(photo)}</div>
          {photo.width && (
            <div>
              {photo.width} × {photo.height}
            </div>
          )}
          {photo.sharpness != null && (
            <div className="scores">
              Sharpness {photo.sharpness.toFixed(0)} · Exposure {((photo.exposure ?? 0) * 100).toFixed(0)}% · Clipped{" "}
              {((photo.clipHi ?? 0) * 100).toFixed(1)}% / {((photo.clipLo ?? 0) * 100).toFixed(1)}%
            </div>
          )}
          {burstSize > 1 && <div>Burst of {burstSize}{photo.burstBest ? " — suggested best" : ""}</div>}
          {note && <div className="cull-note">{note}</div>}
          {photo.pairPath && <div>RAW + JPEG pair</div>}
          {photo.error && <div className="err">⚠ {photo.error}</div>}
        </div>
      )}
    </div>
  );
}

