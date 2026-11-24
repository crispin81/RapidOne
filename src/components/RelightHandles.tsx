import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import "../picker.css";

/** One light's handle: where it sits on the photo as shown (fractions of the shown picture), its
 *  colour, and whether it is the selected one. */
export interface LightHandle {
  id: number;
  x: number;
  y: number;
  colour: [number, number, number];
  selected: boolean;
  kind: "point" | "sun" | "spot";
  /** A spot's aim: the dot its cone points at (drag it to turn the beam). */
  aim?: { x: number; y: number };
  /** A sun's direction: the arrow's tip, drawn from the picture's centre. */
  arrow?: { x: number; y: number };
}

interface Props {
  /** The photo as shown (its box is the photo's, zoomed or not). */
  image: HTMLImageElement | null;
  handles: LightHandle[];
  onSelect: (id: number) => void;
  /** A drag in progress: the new place, as fractions of the shown picture. */
  onDrag: (id: number, x: number, y: number) => void;
  /** The drag released. */
  onDrop: (id: number, x: number, y: number) => void;
  /** The wheel over a handle: `steps` positive moves the light towards the camera. */
  onDepth: (id: number, steps: number) => void;
  /** A spot's aim dot dragged (or released), and a sun's arrow tip: shown fractions. */
  onAim: (id: number, x: number, y: number, last: boolean) => void;
  onSun: (id: number, x: number, y: number, last: boolean) => void;
}

/**
 * Relight's lights on the photo, as draggable markers (the idea from RapidRAW and CalibRaw, our own
 * code): drag a marker to move its light across the frame, scroll over it to move it nearer or
 * farther (Depth), click it to select it for the panel. Laid over the photo itself, above every
 * other layer, following its box every frame (zoom, pan, window size), as the pickers do.
 */
export default function RelightHandles({ image, handles, onSelect, onDrag, onDrop, onDepth, onAim, onSun }: Props) {
  const [box, setBox] = useState<{ l: number; t: number; w: number; h: number } | null>(null);
  // What is being dragged: a lamp, a spot's aim dot, or a sun's arrow tip.
  const [drag, setDrag] = useState<string | null>(null);
  // Over the photo itself, on the page's body (fixed): inside the viewer it could end up under
  // another layer and never get the press.
  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const r = image?.getBoundingClientRect();
      if (r && r.width > 0 && r.height > 0) {
        setBox((b) => (b && b.l === r.left && b.t === r.top && b.w === r.width && b.h === r.height ? b : { l: r.left, t: r.top, w: r.width, h: r.height }));
      }
      raf = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(raf);
  }, [image]);

  const at = (clientX: number, clientY: number): [number, number] | null => {
    if (!box) return null;
    const x = (clientX - box.l) / box.w;
    const y = (clientY - box.t) / box.h;
    return Number.isFinite(x) && Number.isFinite(y) ? [Math.min(1, Math.max(0, x)), Math.min(1, Math.max(0, y))] : null;
  };

  if (!box || handles.length === 0) return null;
  return createPortal(
    <div className="lighthandles" style={{ left: box.l, top: box.t, width: box.w, height: box.h }} onPointerDown={(e) => e.stopPropagation()}>
      {/* The lines first: a spot's lamp to its aim, a sun's centre to its arrow tip. */}
      <svg className="lighthandles__lines" width={box.w} height={box.h} aria-hidden>
        {handles.map((h) => {
          const c = `rgb(${h.colour.map((v) => Math.round(v * 255)).join(",")})`;
          if (h.aim) return <line key={`a${h.id}`} x1={h.x * box.w} y1={h.y * box.h} x2={h.aim.x * box.w} y2={h.aim.y * box.h} stroke={c} strokeWidth={1.5} strokeDasharray="5 4" opacity={h.selected ? 0.95 : 0.55} />;
          if (h.arrow) return <line key={`s${h.id}`} x1={0.5 * box.w} y1={0.5 * box.h} x2={h.arrow.x * box.w} y2={h.arrow.y * box.h} stroke={c} strokeWidth={2} opacity={h.selected ? 0.95 : 0.6} />;
          return null;
        })}
      </svg>
      {handles.map((h) => {
        const on = h.kind !== "sun" && h.x >= -0.05 && h.x <= 1.05 && h.y >= -0.05 && h.y <= 1.05;
        const c = `rgb(${h.colour.map((v) => Math.round(v * 255)).join(",")})`;
        // A sun: just its arrow tip (the line is drawn above).
        if (h.kind === "sun" && h.arrow) {
          const key = `sun:${h.id}`;
          const deg = Math.atan2((h.arrow.y - 0.5) * box.h, (h.arrow.x - 0.5) * box.w) * (180 / Math.PI);
          return (
            <div
              key={key}
              className={`lighthandle lighthandle--aim${h.selected ? " lighthandle--on" : ""}`}
              style={{ left: h.arrow.x * box.w, top: h.arrow.y * box.h, ["--light" as string]: c, ["--aim-rot" as string]: `${deg}deg` }}
              title={`Sun ${h.id}: drag round the centre to turn it, outwards to raise it`}
              onPointerDown={(e) => {
                e.stopPropagation();
                if (e.button !== 0) return;
                onSelect(h.id);
                setDrag(key);
                (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
              }}
              onPointerMove={(e) => {
                if (drag !== key) return;
                const p = at(e.clientX, e.clientY);
                if (p) onSun(h.id, p[0], p[1], false);
              }}
              onPointerUp={(e) => {
                if (drag !== key) return;
                const p = at(e.clientX, e.clientY);
                if (p) onSun(h.id, p[0], p[1], true);
                setDrag(null);
                (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
              }}
              onPointerCancel={() => setDrag(null)}
              onDoubleClick={(e) => e.stopPropagation()}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
              }}
            >
              <span className="lighthandle__arrowhead" />
            </div>
          );
        }
        if (!on) return null;
        const lampKey = `lamp:${h.id}`;
        return (
          <div
            key={lampKey}
            className={`lighthandle${h.selected ? " lighthandle--on" : ""}${drag === lampKey ? " lighthandle--drag" : ""}`}
            style={{ left: h.x * box.w, top: h.y * box.h, ["--light" as string]: c }}
            title={`Light ${h.id} (${h.kind}): drag to move, scroll to change its Depth, click to select`}
            onPointerDown={(e) => {
              e.stopPropagation();
              if (e.button !== 0) return;
              onSelect(h.id);
              setDrag(lampKey);
              (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
            }}
            onPointerMove={(e) => {
              if (drag !== lampKey) return;
              const p = at(e.clientX, e.clientY);
              if (p) onDrag(h.id, p[0], p[1]);
            }}
            onPointerUp={(e) => {
              if (drag !== lampKey) return;
              const p = at(e.clientX, e.clientY);
              if (p) onDrop(h.id, p[0], p[1]);
              setDrag(null);
              (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
            }}
            onPointerCancel={() => setDrag(null)}
            onWheel={(e) => {
              e.preventDefault();
              e.stopPropagation();
              onDepth(h.id, e.deltaY < 0 ? 2 : -2);
            }}
            onDoubleClick={(e) => e.stopPropagation()}
            onContextMenu={(e) => {
              e.preventDefault();
              e.stopPropagation();
            }}
          >
            <span className="lighthandle__ring" />
            <span className="lighthandle__dot" />
            <span className="lighthandle__label">{h.id}</span>
          </div>
        );
      })}
      {handles.map((h) => {
        if (!h.aim) return null;
        const key = `aim:${h.id}`;
        const c = `rgb(${h.colour.map((v) => Math.round(v * 255)).join(",")})`;
        // Pointing from the lamp to the aim point, in screen pixels (so it reads right on any shape).
        const deg = Math.atan2((h.aim.y - h.y) * box.h, (h.aim.x - h.x) * box.w) * (180 / Math.PI);
        return (
          <div
            key={key}
            className="lighthandle lighthandle--aim lighthandle--on"
            style={{ left: h.aim.x * box.w, top: h.aim.y * box.h, ["--light" as string]: c, ["--aim-rot" as string]: `${deg}deg` }}
            title={`Light ${h.id}: where it throws. Drag the arrow to turn the beam`}
            onPointerDown={(e) => {
              e.stopPropagation();
              if (e.button !== 0) return;
              onSelect(h.id);
              setDrag(key);
              (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
            }}
            onPointerMove={(e) => {
              if (drag !== key) return;
              const p = at(e.clientX, e.clientY);
              if (p) onAim(h.id, p[0], p[1], false);
            }}
            onPointerUp={(e) => {
              if (drag !== key) return;
              const p = at(e.clientX, e.clientY);
              if (p) onAim(h.id, p[0], p[1], true);
              setDrag(null);
              (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
            }}
            onPointerCancel={() => setDrag(null)}
            onDoubleClick={(e) => e.stopPropagation()}
            onContextMenu={(e) => {
              e.preventDefault();
              e.stopPropagation();
            }}
          >
            <span className="lighthandle__arrowhead" />
          </div>
        );
      })}
    </div>,
    document.body,
  );
}
