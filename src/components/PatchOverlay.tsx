import { useEffect, useRef, useState } from "react";
import { Lasso, Trash2 } from "lucide-react";
import type { Patch } from "../api";
import { BarGrip, useFloatingBar } from "../floatingBar";
import HSlider from "./HSlider";

type Pt = [number, number];

interface Props {
  /** The displayed photo (uncropped, upright, no keystone while patching). */
  image: HTMLImageElement | null;
  /** The photo's patches, each with its place in the photo's removals list. */
  patches: { index: number; patch: Patch }[];
  /** Heal or Clone: for new patches, and the switch shows it. */
  mode: Patch["mode"];
  onMode: (mode: Patch["mode"]) => void;
  onAdd: (patch: Patch) => void;
  onChange: (index: number, patch: Patch) => void;
  onDelete: (index: number) => void;
  onDone: () => void;
}

const MIN_POINTS = 3;
/** Clone's edge softness, as a share of the area's size: the engine's own default (patch.rs). */
const CLONE_FEATHER = 0.15;

/**
 * The Patch tool (Photoshop's way): draw round what to fix, then drag it onto clean texture; on
 * letting go it takes what's there. Heal | Clone: Heal takes the texture and keeps the tone and
 * colour from round the area; Clone copies exactly. Every patch stays editable: click its pin to
 * pick it, drag its orange outline (where it copies from) somewhere else, change a Clone's feather,
 * or delete it (Delete); its mode stays as drawn. Esc: drops a picked patch or a drawn outline, else done.
 */
export default function PatchOverlay({ image, patches, mode, onMode, onAdd, onChange, onDelete, onDone }: Props) {
  const layer = useRef<HTMLDivElement>(null);
  const bar = useFloatingBar("patch");
  // The outline being drawn, then (drawn) waiting to be dragged onto its source.
  const [drawn, setDrawn] = useState<Pt[] | null>(null);
  const drawing = useRef(false);
  // A drag: the new outline onto its source, or a picked patch's source moved.
  const [drag, setDrag] = useState<{ from: Pt; now: Pt; what: "new" | number } | null>(null);
  const [picked, setPicked] = useState<number | null>(null);
  // The cursor says what a drag or click does there (as Photoshop's): crosshair to draw, move inside
  // an outline that can be dragged, a hand over a patch to pick, a closed hand while dragging.
  const [cursor, setCursor] = useState<string>("crosshair");
  const [, setTick] = useState(0);
  const pickedPatch = patches.find((p) => p.index === picked)?.patch ?? null;
  // Clone's feather: for new patches, or (moved while one is picked) that patch's; it's sent when
  // the slider is let go, so the photo renders once, not on every step.
  const [newFeather, setNewFeather] = useState(CLONE_FEATHER);
  const [featherDraft, setFeatherDraft] = useState<number | null>(null);
  const shownMode = pickedPatch?.mode ?? mode;
  const feather = featherDraft ?? (pickedPatch ? (pickedPatch.feather || CLONE_FEATHER) : newFeather);
  const commitFeather = (v: number) => {
    if (pickedPatch && picked !== null) onChange(picked, { ...pickedPatch, feather: v });
    else setNewFeather(v);
    setFeatherDraft(null);
  };

  // Redrawn when the photo moves or resizes (zoom, window).
  useEffect(() => {
    const again = () => setTick((t) => t + 1);
    window.addEventListener("resize", again);
    const ro = image ? new ResizeObserver(again) : null;
    if (image && ro) ro.observe(image);
    return () => {
      window.removeEventListener("resize", again);
      ro?.disconnect();
    };
  }, [image]);
  // A picked patch that's gone (undone, deleted) is no longer picked.
  useEffect(() => {
    if (picked !== null && !patches.some((p) => p.index === picked)) setPicked(null);
  }, [patches, picked]);

  const photoRect = () => {
    if (!image || !layer.current) return null;
    const r = image.getBoundingClientRect();
    const l = layer.current.getBoundingClientRect();
    return { left: r.left - l.left, top: r.top - l.top, width: r.width, height: r.height };
  };
  const toFraction = (clientX: number, clientY: number): Pt | null => {
    const p = photoRect();
    const l = layer.current?.getBoundingClientRect();
    if (!p || !l) return null;
    return [(clientX - l.left - p.left) / p.width, (clientY - l.top - p.top) / p.height];
  };
  const inside = (poly: Pt[], [x, y]: Pt) => {
    let hit = false;
    for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
      const [xi, yi] = poly[i];
      const [xj, yj] = poly[j];
      if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) hit = !hit;
    }
    return hit;
  };
  const moved = (poly: Pt[], [dx, dy]: Pt): Pt[] => poly.map(([x, y]) => [x + dx, y + dy]);
  const centre = (poly: Pt[]): Pt => [poly.reduce((s, p) => s + p[0], 0) / poly.length, poly.reduce((s, p) => s + p[1], 0) / poly.length];

  // Keys: Esc drops what's in hand (then closes), Delete removes the picked patch.
  const live = useRef({ drawn, picked, onDone, onDelete });
  live.current = { drawn, picked, onDone, onDelete };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Typing in a box: its keys are the box's.
      const tgt = e.target as HTMLElement | null;
      if (tgt && (tgt.tagName === "INPUT" || tgt.tagName === "TEXTAREA" || tgt.tagName === "SELECT" || tgt.isContentEditable)) return;
      const l = live.current;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        if (l.drawn) setDrawn(null);
        else if (l.picked !== null) setPicked(null);
        else l.onDone();
      } else if ((e.key === "Delete" || e.key === "Backspace") && l.picked !== null) {
        e.preventDefault();
        e.stopImmediatePropagation();
        l.onDelete(l.picked);
        setPicked(null);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  const p = photoRect();
  const toPath = (poly: Pt[]) =>
    p ? poly.map(([x, y], i) => `${i ? "L" : "M"}${(p.left + x * p.width).toFixed(1)},${(p.top + y * p.height).toFixed(1)}`).join(" ") + " Z" : "";
  const dragOffset = (): Pt => (drag ? [drag.now[0] - drag.from[0], drag.now[1] - drag.from[1]] : [0, 0]);
  // While dragging: the area shows what it will take (the picture on screen, shifted by where it
  // copies from, clipped to the area), as Photoshop's Patch does. Exactly the result for Clone; for
  // Heal, its tone match follows when the drag ends and the engine renders it.
  const preview: { area: Pt[]; offset: Pt } | null = !drag
    ? null
    : drag.what === "new"
      ? drawn && { area: drawn, offset: dragOffset() }
      : pickedPatch && { area: pickedPatch.polygon, offset: [pickedPatch.offset[0] + dragOffset()[0], pickedPatch.offset[1] + dragOffset()[1]] };

  return (
    <div
      ref={layer}
      className="remove-layer patch-layer"
      style={{ cursor: drag ? "grabbing" : cursor }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        const f = toFraction(e.clientX, e.clientY);
        if (!f) return;
        (e.target as Element).setPointerCapture?.(e.pointerId);
        // Inside the drawn outline: drag it onto its source.
        if (drawn && inside(drawn, f)) {
          setDrag({ from: f, now: f, what: "new" });
          return;
        }
        // Inside a picked patch's source outline: move where it copies from.
        if (pickedPatch && picked !== null && inside(moved(pickedPatch.polygon, pickedPatch.offset), f)) {
          setDrag({ from: f, now: f, what: picked });
          return;
        }
        // On a patch (its area): pick it.
        const hit = [...patches].reverse().find((q) => inside(q.patch.polygon, f));
        if (hit && !drawn) {
          setPicked(hit.index);
          return;
        }
        // Else: draw a new outline.
        setPicked(null);
        drawing.current = true;
        setDrawn([f]);
      }}
      onPointerMove={(e) => {
        const f = toFraction(e.clientX, e.clientY);
        if (!f) return;
        if (!drag && !drawing.current) {
          const draggable =
            (drawn && inside(drawn, f)) || (pickedPatch && inside(moved(pickedPatch.polygon, pickedPatch.offset), f));
          const onPatch = !drawn && patches.some((q) => inside(q.patch.polygon, f));
          setCursor(draggable ? "move" : onPatch ? "pointer" : "crosshair");
        }
        if (drag) setDrag({ ...drag, now: f });
        else if (drawing.current) setDrawn((d) => (d ? [...d, f] : [f]));
      }}
      onPointerUp={() => {
        if (drawing.current) {
          drawing.current = false;
          setDrawn((d) => (d && d.length >= MIN_POINTS ? d : null));
          return;
        }
        if (!drag) return;
        const off = dragOffset();
        const far = Math.hypot(off[0], off[1]) > 0.002;
        if (drag.what === "new" && drawn && far) {
          onAdd({ kind: "patch", polygon: drawn, offset: off, mode, ...(mode === "clone" ? { feather: newFeather } : {}) });
          setDrawn(null);
        } else if (typeof drag.what === "number" && pickedPatch && far) {
          onChange(drag.what, { ...pickedPatch, offset: [pickedPatch.offset[0] + off[0], pickedPatch.offset[1] + off[1]] });
        }
        setDrag(null);
      }}
    >
      {p && (
        <svg className="patch-svg" width="100%" height="100%">
          {preview && image && (
            <>
              <defs>
                <clipPath id="patch-preview-clip">
                  <path d={toPath(preview.area)} />
                </clipPath>
              </defs>
              {(() => {
                // The picture to preview from: the image itself, or, handed the loupe's stand-in for the
                // whole photo (a cropped photo, Viewer), the cropped picture it carries, placed over the
                // crop's part of the whole photo (outside the crop there's nothing to show).
                const src = image.dataset.picture ?? (image.currentSrc || image.src);
                const [x0, y0, x1, y1] = (image.dataset.window ?? "0,0,1,1").split(",").map(Number);
                return (
                  <image
                    href={src}
                    x={p.left + (x0 - preview.offset[0]) * p.width}
                    y={p.top + (y0 - preview.offset[1]) * p.height}
                    width={(x1 - x0) * p.width}
                    height={(y1 - y0) * p.height}
                    preserveAspectRatio="none"
                    clipPath="url(#patch-preview-clip)"
                  />
                );
              })()}
            </>
          )}
          {/* Each patch: its pin; picked, also its area and its source (orange). Unpicked, the outline is
              hidden, so the photo stays clear and only the pins show. */}
          {patches.map(({ index, patch }) => {
            const on = index === picked;
            const offset: Pt = on && drag && drag.what === index ? [patch.offset[0] + dragOffset()[0], patch.offset[1] + dragOffset()[1]] : patch.offset;
            const [cx, cy] = centre(patch.polygon);
            return (
              <g key={index}>
                {on && <path d={toPath(patch.polygon)} className="patch-area patch-area--on" />}
                {on && <path d={toPath(moved(patch.polygon, offset))} className="patch-source" />}
                <circle cx={p.left + cx * p.width} cy={p.top + cy * p.height} r={on ? 6 : 4.5} className={`patch-pin${on ? " patch-pin--on" : ""}`} />
              </g>
            );
          })}
          {drawn && <path d={toPath(drawn)} className="patch-area patch-area--on" />}
          {drawn && drag?.what === "new" && <path d={toPath(moved(drawn, dragOffset()))} className="patch-source" />}
        </svg>
      )}
      <div ref={bar.ref} style={bar.style} className="cropbar removebar" onPointerDown={(e) => e.stopPropagation()}>
        <BarGrip bar={bar} />
        <span className="cropbar__what">
          <Lasso size={13} /> Patch
        </span>
        <span className="removebar__tools removebar__mode" title="Heal: the texture, with the tone and colour from round the area. Clone: an exact copy">
          {(["heal", "clone"] as const).map((m) => (
            <button
              key={m}
              type="button"
              className={(pickedPatch?.mode ?? mode) === m ? "is-on" : undefined}
              // A patch keeps the mode it was drawn with: picked, the switch shows it and can't change it.
              disabled={pickedPatch !== null}
              title={pickedPatch ? "A patch keeps the mode it was drawn with: draw a new one for the other" : undefined}
              onClick={() => onMode(m)}
            >
              {m === "heal" ? "Heal" : "Clone"}
            </button>
          ))}
        </span>
        {shownMode === "clone" && (
          <label className="patchbar__feather" title="How soft the clone's edge is (a share of the patch's size)">
            Feather
            <HSlider
              min={0.005}
              max={0.3}
              step={0.005}
              value={feather}
              reset={CLONE_FEATHER}
              width={90}
              aria-label="Feather"
              onChange={setFeatherDraft}
              onCommit={commitFeather}
            />
            <span className="patchbar__value">{Math.round(feather * 100)}%</span>
          </label>
        )}
        <span className="removebar__note">
          {drawn
            ? "Drag the outline onto clean texture"
            : pickedPatch
              ? "Drag the orange outline to copy from somewhere else · Delete removes it"
              : "Draw round what to fix"}
        </span>
        {pickedPatch && picked !== null && (
          <button
            type="button"
            onClick={() => {
              onDelete(picked);
              setPicked(null);
            }}
            title="Delete this patch (Delete)"
          >
            <Trash2 size={13} />
          </button>
        )}
        <button type="button" onClick={onDone} title="Done (Esc)">
          Done
        </button>
      </div>
    </div>
  );
}
