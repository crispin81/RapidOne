import { useEffect, useRef, useState } from "react";
import { Brush, Eraser, Loader2, RefreshCw, SquareDashed } from "lucide-react";
import type { Outline, RemovalMode, RemovalModes } from "../api";
import { BarGrip, useFloatingBar } from "../floatingBar";
import HSlider from "./HSlider";

type Area = [number, number, number, number];

interface Props {
  /** The displayed photo (uncropped, upright, no keystone while removing). */
  image: HTMLImageElement | null;
  /** A removal or search is running. */
  busy: boolean;
  /** Removals on this photo so far. */
  count: number;
  /** SAM 3 is set up: Ctrl+E finds objects by words and shows them first. */
  canFind: boolean;
  /** Remove what's in `area` (fractions of the photo); `what` names it, or ""; `outline` is SAM 3's shape. */
  onRemove: (area: Area, what: string, outline?: string) => void;
  /** Remove several found objects, one after another. */
  onRemoveAll: (found: Outline[], what: string) => void;
  /** Brush: remove the painted shape (a white-on-black PNG data URL of the whole photo). */
  onRemovePainted: (painted: string, what: string) => void;
  /** SAM 3: the objects matching `words`, or (with `area`, no words) the object in that box. */
  onFind: (words: string, area?: Area) => Promise<Outline[]>;
  onClearAll: () => void;
  /** The removers installed and which is in use (the switch shows when more than one is). */
  modes?: RemovalModes | null;
  onMode?: (mode: RemovalMode) => void;
  /** Try again: the last removal, the model's next version in its place. */
  onRetry?: () => void;
  onDone: () => void;
  /** Start with the whole photo selected (Ctrl+E): say what to remove, Enter. */
  startWhole?: boolean;
}

const MIN_PX = 6; // a smaller drag is a click, not a box

/** A brush stroke: points as fractions of the photo, radius as a fraction of its width. */
interface Stroke {
  r: number;
  pts: { x: number; y: number }[];
}

/** Draws strokes onto a canvas the photo's size (w x h), in `colour`. */
function paint(ctx: CanvasRenderingContext2D, strokes: Stroke[], w: number, h: number, colour: string) {
  ctx.strokeStyle = colour;
  ctx.fillStyle = colour;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const s of strokes) {
    const r = s.r * w;
    ctx.lineWidth = r * 2;
    if (s.pts.length === 1) {
      ctx.beginPath();
      ctx.arc(s.pts[0].x * w, s.pts[0].y * h, r, 0, Math.PI * 2);
      ctx.fill();
      continue;
    }
    ctx.beginPath();
    s.pts.forEach((p, i) => (i === 0 ? ctx.moveTo(p.x * w, p.y * h) : ctx.lineTo(p.x * w, p.y * h)));
    ctx.stroke();
  }
}

/**
 * The Remove tool.
 * - Drag a box round an object (loosely is fine) and press Enter; a word
 *   ("sign") is optional. With SAM 3, the object in the box is outlined as
 *   soon as the box is drawn (Enter removes it; Esc to type a word or
 *   redraw); if SAM 3 finds nothing the whole box is removed.
 * - Ctrl+E: type what to remove and press Enter. With SAM 3 every match is
 *   outlined first: Tab picks the next, Enter removes the picked one,
 *   Shift+Enter removes them all. Without SAM 3 (or if it finds nothing),
 *   Klein looks for it in the whole photo.
 * The photo shows uncropped while this is open, like Crop. Esc: back / done.
 */
export default function RemoveOverlay({ image, busy, count, canFind, onRemove, onRemoveAll, onRemovePainted, onFind, onClearAll, onRetry, onDone, modes, onMode, startWhole = false }: Props) {
  const bar = useFloatingBar("remove");
  const layer = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const [box, setBox] = useState<Area | null>(startWhole ? [0, 0, 1, 1] : null);
  const whole = box !== null && box[0] <= 0 && box[1] <= 0 && box[2] >= 1 && box[3] >= 1;
  const [what, setWhat] = useState("");
  const [finding, setFinding] = useState(false);
  const [matches, setMatches] = useState<Outline[] | null>(null);
  const [picked, setPicked] = useState(0);
  const [note, setNote] = useState<string | null>(null);
  const drag = useRef<{ x: number; y: number } | null>(null);
  // Brush: paint over the object instead of a box. [ and ] change its size.
  const [tool, setTool] = useState<"box" | "brush">("box");
  const [strokes, setStrokes] = useState<Stroke[]>([]);
  const [brushPx, setBrushPx] = useState(24);
  // Where the pointer is over the layer (px), for the brush's outline: its size as it paints.
  const [cursor, setCursor] = useState<{ x: number; y: number } | null>(null);
  const painting = useRef(false);
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (startWhole) input.current?.focus();
  }, [startWhole]);

  // The photo's rectangle inside this layer, in pixels.
  const photoRect = () => {
    if (!image || !layer.current) return null;
    const r = image.getBoundingClientRect();
    const l = layer.current.getBoundingClientRect();
    return { left: r.left - l.left, top: r.top - l.top, width: r.width, height: r.height };
  };

  const toFraction = (clientX: number, clientY: number) => {
    const p = photoRect();
    const l = layer.current?.getBoundingClientRect();
    if (!p || !l) return null;
    return {
      x: Math.min(1, Math.max(0, (clientX - l.left - p.left) / p.width)),
      y: Math.min(1, Math.max(0, (clientY - l.top - p.top) / p.height)),
    };
  };

  const reset = () => {
    setBox(null);
    setWhat("");
    setMatches(null);
    setNote(null);
    setStrokes([]);
  };

  // The brush strokes on screen.
  useEffect(() => {
    const c = canvas.current;
    const r = photoRect();
    if (!c || !r) return;
    c.width = Math.round(r.width);
    c.height = Math.round(r.height);
    const ctx = c.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, c.width, c.height);
    paint(ctx, strokes, c.width, c.height, "rgba(62, 230, 255, 0.45)");
  });

  const submitPainted = () => {
    if (!strokes.length || busy || !image) return;
    // The mask at the preview's own resolution: white where painted.
    const w = image.naturalWidth || 2400;
    const h = image.naturalHeight || 1600;
    const off = document.createElement("canvas");
    off.width = w;
    off.height = h;
    const ctx = off.getContext("2d");
    if (!ctx) return;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, w, h);
    paint(ctx, strokes, w, h, "#fff");
    onRemovePainted(off.toDataURL("image/png"), what.trim());
    reset();
  };

  const removePicked = (all: boolean) => {
    if (!matches?.length || busy) return;
    const words = what.trim();
    if (all) {
      onRemoveAll(matches, words);
      reset();
      return;
    }
    const m = matches[picked];
    onRemove(m.area, words, m.outline);
    const rest = matches.filter((_, i) => i !== picked);
    if (rest.length) {
      setMatches(rest);
      setPicked(0);
    } else {
      reset();
    }
  };

  // SAM 3's outline of what's in a newly drawn box (the newest box wins).
  const outlineSeq = useRef(0);
  const outlineBox = async (b: Area) => {
    const seq = ++outlineSeq.current;
    setFinding(true);
    setNote(null);
    try {
      const found = (await onFind("", b)).slice(0, 1);
      if (seq !== outlineSeq.current) return;
      if (found.length) {
        setMatches(found);
        setPicked(0);
      } else {
        setNote("Nothing outlined in the box: Enter removes the whole box");
      }
    } catch (e) {
      if (seq === outlineSeq.current) setNote(`Couldn't outline: ${e}`);
    } finally {
      if (seq === outlineSeq.current) setFinding(false);
    }
  };

  const submit = async (all = false) => {
    if (busy || finding) return;
    if (tool === "brush") {
      submitPainted();
      return;
    }
    if (matches) {
      removePicked(all);
      return;
    }
    if (!box) return;
    const words = what.trim();
    // The whole photo needs words to find the object by.
    if (whole && !words) {
      input.current?.focus();
      return;
    }
    // Box, no words: show SAM 3's outline of the object in it first.
    const inBox = !whole && !words;
    if ((whole || inBox) && canFind) {
      setFinding(true);
      setNote(null);
      try {
        const found = inBox ? (await onFind("", box)).slice(0, 1) : await onFind(words);
        if (found.length) {
          setMatches(found);
          setPicked(0);
          return;
        }
        setNote(inBox ? "Nothing outlined in the box; removing the whole box…" : `No "${words}" outlined; looking for it in the whole photo instead…`);
      } catch (e) {
        setNote(`Couldn't search: ${e}`);
        return;
      } finally {
        setFinding(false);
      }
    }
    onRemove(box, words);
    reset();
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Typing in a box (its words box, or any other): it handles its own keys.
      const tgt = e.target as HTMLElement | null;
      if (e.target === input.current || (tgt && (tgt.tagName === "INPUT" || tgt.tagName === "TEXTAREA" || tgt.tagName === "SELECT" || tgt.isContentEditable))) return;
      if ((e.key === "[" || e.key === "]") && tool === "brush") {
        e.preventDefault();
        setBrushPx((s) => Math.min(300, Math.max(4, Math.round(s * (e.key === "]" ? 1.25 : 0.8)))));
        return;
      }
      // B (or Ctrl+B): box or brush. Erase takes B first while it's open, so it doesn't reach the
      // Blacks slider's key.
      if (e.code === "KeyB" && !e.altKey && !e.shiftKey) {
        e.preventDefault();
        e.stopPropagation();
        setTool((t) => (t === "box" ? "brush" : "box"));
        reset();
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        if (strokes.length) setStrokes([]);
        else if (matches) setMatches(null);
        else if (box) setBox(null);
        else onDone();
      } else if (e.key === "Enter" && (box || matches || strokes.length)) {
        e.preventDefault();
        void submit(e.shiftKey);
      } else if (e.key === "Tab" && matches?.length) {
        e.preventDefault();
        e.stopPropagation();
        setPicked((i) => (i + (e.shiftKey ? matches.length - 1 : 1)) % matches.length);
      }
    };
    // Capture: ahead of the app's own keys (Tab here steps through what was found).
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  const p = photoRect();
  const at = (a: Area) =>
    p ? { left: p.left + a[0] * p.width, top: p.top + a[1] * p.height, width: (a[2] - a[0]) * p.width, height: (a[3] - a[1]) * p.height } : undefined;
  const shown = box && !matches ? at(box) : undefined;
  const working = busy || finding;

  return (
    <div
      ref={layer}
      className="remove-layer"
      // The brush shows its own ring instead of the crosshair.
      style={tool === "brush" ? { cursor: "none" } : undefined}
      onPointerDown={(e) => {
        if (working || e.button !== 0) return;
        const f = toFraction(e.clientX, e.clientY);
        if (!f) return;
        if (tool === "brush") {
          const r = photoRect();
          if (!r) return;
          (e.target as Element).setPointerCapture(e.pointerId);
          painting.current = true;
          setStrokes((s) => [...s, { r: brushPx / 2 / r.width, pts: [f] }]);
          return;
        }
        // A click on a found object picks it.
        if (matches) {
          const hit = matches.findIndex((m) => f.x >= m.area[0] && f.x <= m.area[2] && f.y >= m.area[1] && f.y <= m.area[3]);
          if (hit >= 0) {
            setPicked(hit);
            return;
          }
          setMatches(null);
        }
        (e.target as Element).setPointerCapture(e.pointerId);
        drag.current = { x: f.x, y: f.y };
        setBox(null);
      }}
      onPointerMove={(e) => {
        if (tool === "brush") {
          const l = layer.current?.getBoundingClientRect();
          if (l) setCursor({ x: e.clientX - l.left, y: e.clientY - l.top });
        }
        if (painting.current) {
          const f = toFraction(e.clientX, e.clientY);
          if (f) setStrokes((s) => [...s.slice(0, -1), { ...s[s.length - 1], pts: [...s[s.length - 1].pts, f] }]);
          return;
        }
        const start = drag.current;
        if (!start) return;
        const f = toFraction(e.clientX, e.clientY);
        if (!f) return;
        setBox([Math.min(start.x, f.x), Math.min(start.y, f.y), Math.max(start.x, f.x), Math.max(start.y, f.y)]);
      }}
      onPointerLeave={() => setCursor(null)}
      // The wheel resizes the brush (up: bigger), in proportion, so a touchpad's small steps are smooth.
      onWheel={(e) => {
        if (tool !== "brush" || working) return;
        const dy = e.deltaMode === 1 ? e.deltaY * 33 : e.deltaY;
        if (dy === 0) return;
        // About 8% a mouse notch, and at least a pixel, so a small brush still changes.
        setBrushPx((s) => {
          let next = Math.round(s * Math.exp(-dy * 0.0008));
          if (next === s) next = s + (dy < 0 ? 1 : -1);
          return Math.min(300, Math.max(4, next));
        });
      }}
      onPointerUp={() => {
        if (painting.current) {
          painting.current = false;
          setTimeout(() => input.current?.focus(), 0);
          return;
        }
        if (!drag.current) return;
        drag.current = null;
        const r = photoRect();
        const b = box;
        const big = Boolean(b && r && (b[2] - b[0]) * r.width >= MIN_PX && (b[3] - b[1]) * r.height >= MIN_PX);
        setBox(big ? b : null);
        // Outlined at once (no words needed); Esc drops it to type a word.
        if (big && b && canFind && !busy) void outlineBox(b);
        // Ready to type what it is, or press Enter.
        setTimeout(() => input.current?.focus(), 0);
      }}
    >
      {matches && p &&
        matches.map((m, i) => (
          <div key={m.outline}>
            <img
              className={`remove-outline${i === picked ? " remove-outline--picked" : ""}`}
              src={m.overlay}
              alt=""
              draggable={false}
              style={{ left: p.left, top: p.top, width: p.width, height: p.height }}
            />
            {i === picked && <div className={`remove-box remove-box--found${busy ? " remove-box--busy" : ""}`} style={at(m.area)} />}
          </div>
        ))}
      {shown && <div className={`remove-box${whole ? " remove-box--whole" : ""}${working ? " remove-box--busy" : ""}`} style={shown} />}
      {tool === "brush" && cursor && !working && (
        // As the mask brush's (Layers): a thin cyan ring the brush's size.
        <svg className="remove-brush-cursor gradient-cursor" width="100%" height="100%">
          <circle cx={cursor.x} cy={cursor.y} r={brushPx / 2} />
        </svg>
      )}
      {tool === "brush" && p && (
        <canvas
          ref={canvas}
          className={`remove-paint${working ? " remove-box--busy" : ""}`}
          style={{ left: p.left, top: p.top, width: p.width, height: p.height }}
        />
      )}
      <div ref={bar.ref} style={bar.style} className="cropbar removebar" onPointerDown={(e) => e.stopPropagation()}>
        <BarGrip bar={bar} />
        <span className="cropbar__what">
          <Eraser size={13} /> Remove
        </span>
        {modes && onMode && [modes.large, modes.medium, modes.lite].filter(Boolean).length > 1 && (
          <span className="removebar__tools removebar__mode">
            {(
              [
                ["large", modes.large, "Large", "FLUX.2 Klein + SAM 3 (graphics card): paints big areas believably; can invent things"],
                ["medium", modes.medium, "Medium", "RORem + SAM 3 (graphics card): trained only to remove what's masked"],
                ["lite", modes.lite, "Lite", "LaMa + MobileSAM + Grounding DINO (any computer, fast): continues the surroundings, never invents objects"],
              ] as const
            )
              .filter(([, ok]) => ok)
              .map(([id, , label, tip]) => (
                <button
                  key={id}
                  type="button"
                  className={modes.mode === id ? "is-on" : undefined}
                  title={tip}
                  disabled={working}
                  onClick={() => {
                    onMode(id);
                    reset();
                  }}
                >
                  {label}
                </button>
              ))}
          </span>
        )}
        {modes && [modes.large, modes.medium, modes.lite].filter(Boolean).length === 1 && (
          <span className="removebar__keys" title="The remover installed on this computer">
            {modes.mode === "lite" ? "Lite" : modes.mode === "medium" ? "Medium" : "Large"}
          </span>
        )}
        <span className="removebar__tools" title="B: switch between box and brush">
          <button type="button" className={tool === "box" ? "is-on" : undefined} onClick={() => { setTool("box"); reset(); }} title="Box: drag round an object">
            <SquareDashed size={13} />
          </button>
          <button type="button" className={tool === "brush" ? "is-on" : undefined} onClick={() => { setTool("brush"); reset(); }} title="Brush: paint over the object (scroll wheel, or [ and ], for size)">
            <Brush size={13} />
          </button>
        </span>
        {tool === "brush" && (
          <label className="patchbar__feather" title="Brush size, in screen pixels (the scroll wheel and [ ] change it too)">
            Size
            <HSlider min={4} max={300} step={1} value={brushPx} reset={24} width={110} aria-label="Brush size" onChange={(v) => setBrushPx(Math.round(v))} />
            <span className="patchbar__value">{brushPx}</span>
          </label>
        )}
        {working ? (
          <span className="removebar__busy">
            <Loader2 size={13} className="spin" /> {finding ? "Finding it…" : whole ? "Finding and removing it…" : "Removing…"} (the first one loads the models)
          </span>
        ) : matches ? (
          <>
            <span className="removebar__found">
              {!what.trim()
                ? "Outlined: Enter removes it, Esc to redraw"
                : matches.length === 1
                  ? `1 "${what.trim()}" found`
                  : `${picked + 1} of ${matches.length} "${what.trim()}" found`}
            </span>
            {matches.length > 1 && <span className="removebar__keys">Tab: next</span>}
            <button type="button" className="is-primary" onClick={() => removePicked(false)} title="Remove the outlined one (Enter)">
              Remove
            </button>
            {matches.length > 1 && (
              <button type="button" onClick={() => removePicked(true)} title="Remove every one found (Shift+Enter)">
                Remove all {matches.length}
              </button>
            )}
          </>
        ) : (
          <>
            <input
              ref={input}
              type="text"
              value={what}
              placeholder={
                tool === "brush"
                  ? strokes.length
                    ? "Enter to remove (words optional)"
                    : `Paint over what to remove ([ ] size: ${brushPx} px)`
                  : whole
                    ? "What to remove? e.g. sign, lamp post"
                    : box
                      ? canFind
                        ? "What is it? (optional)"
                        : "What is it? One word helps, e.g. sign"
                      : "Type what to remove, or drag a box round it"
              }
              disabled={tool === "brush" && !strokes.length}
              // Typing with no box drawn: look for it in the whole photo (as Ctrl+E).
              onFocus={() => {
                if (tool === "box" && !box) setBox([0, 0, 1, 1]);
              }}
              onChange={(e) => setWhat(e.currentTarget.value)}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") void submit(e.shiftKey);
                // Esc: out of the box too, so the tool's keys (B, [ ]) work again.
                if (e.key === "Escape") {
                  setBox(null);
                  e.currentTarget.blur();
                }
              }}
            />
            <button
              type="button"
              className="is-primary"
              disabled={tool === "brush" ? !strokes.length : !box || (whole && !what.trim())}
              onClick={() => void submit()}
              title={whole && canFind ? "Find it (Enter)" : "Remove it (Enter)"}
            >
              {whole && canFind ? "Find" : "Remove"}
            </button>
          </>
        )}
        {note && !working && <span className="removebar__note">{note}</span>}
        {onRetry && !working && !matches && (
          <button type="button" onClick={onRetry} title="Didn't come out right? Replace the last removal with a different version">
            <RefreshCw size={13} /> Try again
          </button>
        )}
        {count > 0 && !working && (
          <button type="button" onClick={onClearAll} title="Put back everything removed from this photo">
            Undo all ({count})
          </button>
        )}
        <button type="button" onClick={onDone} title="Done (Esc)">
          Done
        </button>
      </div>
    </div>
  );
}
