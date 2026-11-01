import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Pipette } from "lucide-react";
import "../picker.css";

export type PickerKind = "wb" | "fringe";

interface Props {
  kind: PickerKind;
  /** The photo as shown (its box is the photo's, zoomed or not). */
  image: HTMLImageElement | null;
  /** A click on the photo: where, as fractions of the photo as shown. */
  onPick: (x: number, y: number) => void;
  onCancel: () => void;
}

const HINTS: Record<PickerKind, string> = {
  wb: "White balance: click something that should be neutral grey or white",
  fringe: "Defringe: click a purple or green fringe along an edge",
};

/**
 * The pickers' click layer over the photo (Lightroom's eyedroppers): a crosshair over the
 * photo, a click reads the colour there (once, then the picker closes), Esc or a right-click
 * cancels. While it's open the photo isn't dragged or zoomed by a click.
 */
export default function PickOverlay({ kind, image, onPick, onCancel }: Props) {
  const layer = useRef<HTMLDivElement>(null);
  const [inside, setInside] = useState(false);
  // Over the photo itself, above everything (put on the page's body, fixed): inside the viewer it
  // could end up under another layer (the zoomed view's drag hand, 1 Nov) and never get the click.
  // Its box follows the photo's, read every frame while it's open (zoom, pan, window size).
  const [box, setBox] = useState<{ l: number; t: number; w: number; h: number } | null>(null);
  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const r = image?.getBoundingClientRect();
      const host = image?.closest(".viewer__main")?.getBoundingClientRect();
      if (r && host) {
        const l = Math.max(r.left, host.left);
        const t = Math.max(r.top, host.top);
        const w = Math.min(r.right, host.right) - l;
        const h = Math.min(r.bottom, host.bottom) - t;
        setBox((b) => (b && b.l === l && b.t === t && b.w === w && b.h === h ? b : w > 0 && h > 0 ? { l, t, w, h } : null));
      }
      raf = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(raf);
  }, [image]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      onCancel();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onCancel]);

  // White balance: the picture's R G B under the cursor (0-100, as Lightroom shows them), read from a
  // copy of the picture on screen (its edit included), the average of a 5 x 5 spot: three numbers close
  // together mark a neutral tone (Chris, 1 Nov). Copied once per picture, read at most once a frame.
  const pixels = useRef<{ src: string; w: number; h: number; data: Uint8ClampedArray } | null>(null);
  const [readout, setReadout] = useState<{ x: number; y: number; rgb: [number, number, number] } | null>(null);
  const pending = useRef<{ cx: number; cy: number } | null>(null);
  const frame = useRef(0);
  useEffect(() => () => cancelAnimationFrame(frame.current), []);
  const sample = (cx: number, cy: number) => {
    if (kind !== "wb" || !image || !image.complete || image.naturalWidth < 1) return setReadout(null);
    if (pixels.current?.src !== image.currentSrc) {
      const c = document.createElement("canvas");
      c.width = image.naturalWidth;
      c.height = image.naturalHeight;
      const g = c.getContext("2d", { willReadFrequently: true });
      if (!g) return;
      g.drawImage(image, 0, 0);
      pixels.current = { src: image.currentSrc, w: c.width, h: c.height, data: g.getImageData(0, 0, c.width, c.height).data };
    }
    const p = at(cx, cy);
    const px = pixels.current;
    if (!p || !px) return setReadout(null);
    const x0 = Math.min(px.w - 1, Math.floor(p[0] * px.w));
    const y0 = Math.min(px.h - 1, Math.floor(p[1] * px.h));
    const sum = [0, 0, 0];
    let n = 0;
    for (let y = Math.max(0, y0 - 2); y <= Math.min(px.h - 1, y0 + 2); y++) {
      for (let x = Math.max(0, x0 - 2); x <= Math.min(px.w - 1, x0 + 2); x++) {
        const i = (y * px.w + x) * 4;
        sum[0] += px.data[i];
        sum[1] += px.data[i + 1];
        sum[2] += px.data[i + 2];
        n++;
      }
    }
    const pct = (v: number) => Math.round((v / n / 255) * 1000) / 10;
    setReadout({ x: cx, y: cy, rgb: [pct(sum[0]), pct(sum[1]), pct(sum[2])] });
  };
  const track = (cx: number, cy: number) => {
    pending.current = { cx, cy };
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      const q = pending.current;
      if (q) sample(q.cx, q.cy);
    });
  };

  const at = (clientX: number, clientY: number): [number, number] | null => {
    if (!image) return null;
    const r = image.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return null;
    const x = (clientX - r.left) / r.width;
    const y = (clientY - r.top) / r.height;
    return x >= 0 && x <= 1 && y >= 0 && y <= 1 ? [x, y] : null;
  };

  if (!box) return null;
  return createPortal(
    <div
      ref={layer}
      className={`pickoverlay${inside ? " pickoverlay--on" : ""}`}
      style={{ left: box.l, top: box.t, width: box.w, height: box.h }}
      onPointerDown={(e) => {
        e.stopPropagation();
        if (e.button === 2) {
          onCancel();
          return;
        }
        if (e.button !== 0) return;
        const p = at(e.clientX, e.clientY);
        if (p) onPick(p[0], p[1]);
      }}
      onPointerMove={(e) => {
        setInside(Boolean(at(e.clientX, e.clientY)));
        track(e.clientX, e.clientY);
      }}
      onPointerLeave={() => {
        setInside(false);
        pending.current = null;
        setReadout(null);
      }}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
      }}
    >
      <div className="pickoverlay__hint">
        <Pipette size={13} /> {HINTS[kind]} <span className="pickoverlay__esc">Esc to cancel</span>
      </div>
      {readout && inside && (
        <div className="pickoverlay__rgb" style={{ left: readout.x - box.l + 18, top: readout.y - box.t + 18 }}>
          <span>R {readout.rgb[0].toFixed(1)}</span>
          <span>G {readout.rgb[1].toFixed(1)}</span>
          <span>B {readout.rgb[2].toFixed(1)}</span>
        </div>
      )}
    </div>,
    document.body,
  );
}
