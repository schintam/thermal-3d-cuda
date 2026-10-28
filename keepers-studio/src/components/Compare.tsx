import { Photo } from "../api";
import { Badges } from "./Badges";
import { ZoomState, ZoomView } from "./ZoomView";

interface Props {
  photos: Photo[];
  active: number;
  onActive: (id: number) => void;
  view: ZoomState;
  onView: (v: ZoomState) => void;
}

/** 2–4 photos side by side; zoom and pan are synced across all panes. */
export function Compare({ photos, active, onActive, view, onView }: Props) {
  const cols = photos.length <= 2 ? photos.length : 2;
  return (
    <div className="compare" style={{ gridTemplateColumns: `repeat(${cols}, 1fr)` }}>
      {photos.map((p) => (
        <div key={p.id} className={"pane" + (p.id === active ? " active" : "")} onMouseDownCapture={() => onActive(p.id)}>
          <ZoomView photo={p} view={view} onView={onView} />
          <Badges photo={p} large />
          <div className="panelabel">
            {p.fileName}
            {p.sharpness != null && <span> · sharpness {p.sharpness.toFixed(0)}</span>}
          </div>
        </div>
      ))}
    </div>
  );
}
