import { useEffect, useRef, useState, useLayoutEffect } from "react";
import { Contrast, Eye, EyeOff, Minus, Paintbrush, Plus, Trash2, UserRound, X } from "lucide-react";
import { LinearGradientIcon, RadialGradientIcon } from "./GradientIcons";
import HSlider from "./HSlider";
import { BarGrip, useFloatingBar } from "../floatingBar";
import { BRUSH_FEATHER, BRUSH_SIZE, hasSubject, newGradient, SUBJECT_FEATHER, type Gradient, type GradientKind, type MaskPart, type Stroke } from "../gradients";

interface Props {
  /** The displayed photo (uncropped, upright, no keystone while open). */
  image: HTMLImageElement | null;
  gradients: Gradient[];
  /** `key` names the change for undo (a drag or a slider counts once). */
  onChange: (gradients: Gradient[], key: string | null) => void;
  /** A mask (any kind, combined or not) as a red overlay over the photo, for the picked mask. */
  maskOverlay?: (mask: Gradient) => Promise<string | null>;
  /** Which mask is picked (-1: none), told to the app: the bottom panel's sliders act on it. */
  onPicked?: (index: number) => void;
  /** Show the picked mask in red (the bottom panel's tick box). */
  showMask: boolean;
  /** Show mask on / off (its switch in the layers panel's header). */
  onShowMask?: () => void;
}


type Pt = { x: number; y: number };
/** What a drag moves: a new gradient, a whole one, or one of its handles. */
type Drag =
  | { what: "new"; start: Pt }
  // A part of the picked mask: painted, or drawn (linear: to its far end; radial: its corner).
  | { what: "paint-part"; part: number; stroke: Stroke }
  | { what: "new-part"; start: Pt }
  | { what: "draw-part"; part: number; start: Pt }
  // A part's own pin and handles: moved, or reshaped as a gradient's are.
  | { what: "part-move"; part: number; start: Pt; orig: Gradient }
  | { what: "part-handle"; part: number; handle: "full" | "zero" | "rx" | "ry" | "rotate" | "feather" | "mid"; start: Pt; orig: Gradient }
  | { what: "paint"; index: number; stroke: Stroke }
  | { what: "move" | "mid"; start: Pt; orig: Gradient }
  | { what: "draw-radial"; start: Pt; orig: Gradient }
  | { what: "full" | "zero" | "rx" | "ry" | "rotate" | "feather"; orig: Gradient };

const MIN_PX = 6;
/** How far round the photo (px) the overlay's SVG reaches, for pins and lines off the photo. */
const SVG_MARGIN = 2000;

/** A shape's name and icon in the layers. */
function kindName(g: Gradient): string {
  if (g.kind === "subject") return g.invert ? "Background" : "Subject";
  return g.kind === "brush" ? "Brush" : g.kind === "linear" ? "Linear" : "Radial";
}
function kindIcon(kind: GradientKind) {
  if (kind === "subject") return <UserRound size={12} />;
  if (kind === "brush") return <Paintbrush size={12} />;
  return kind === "linear" ? <LinearGradientIcon size={12} /> : <RadialGradientIcon size={12} />;
}

/** A gradient's handle dragged to `f` (fractions of a photo `aspect` wide): its new shape. */
function reshaped(g: Gradient, handle: "full" | "zero" | "rx" | "ry" | "rotate" | "feather" | "mid", f: Pt, aspect: number, start: Pt = f): Gradient {
  if (handle === "full") return { ...g, full: [f.x, f.y] };
  if (handle === "zero") return { ...g, zero: [f.x, f.y] };
  if (handle === "mid") {
    // A linear's middle line dragged: the full and zero lines closer together or further
    // apart about it (towards the zero side: wider), the middle and the angle kept.
    const mx = (g.full[0] + g.zero[0]) / 2, my = (g.full[1] + g.zero[1]) / 2;
    const dx = (g.zero[0] - g.full[0]) * aspect, dy = g.zero[1] - g.full[1];
    const len = Math.hypot(dx, dy) || 1;
    const ux = dx / len, uy = dy / len;
    const s = (f.x - start.x) * aspect * ux + (f.y - start.y) * uy;
    const t = Math.max(0.005, len / 2 + s);
    return { ...g, full: [mx - (ux * t) / aspect, my - uy * t], zero: [mx + (ux * t) / aspect, my + uy * t] };
  }
  if (handle === "feather" && g.kind === "linear") {
    // Linear: both lines move together, closer or further from the middle (same angle).
    const mx = (g.full[0] + g.zero[0]) / 2, my = (g.full[1] + g.zero[1]) / 2;
    const dx = (g.zero[0] - g.full[0]) * aspect, dy = g.zero[1] - g.full[1];
    const len = Math.hypot(dx, dy) || 1;
    const ux = dx / len, uy = dy / len;
    const t = Math.max(0.005, Math.abs((f.x - mx) * aspect * ux + (f.y - my) * uy));
    return { ...g, full: [mx - (ux * t) / aspect, my - uy * t], zero: [mx + (ux * t) / aspect, my + uy * t] };
  }
  // Radii and rotation: the pointer in the oval's own (rotated) frame.
  const a = (g.angle * Math.PI) / 180;
  const px = (f.x - g.centre[0]) * aspect, py = f.y - g.centre[1];
  const u = px * Math.cos(a) + py * Math.sin(a);
  const v = -px * Math.sin(a) + py * Math.cos(a);
  if (handle === "rx") return { ...g, rx: Math.max(0.01, Math.abs(u) / aspect) };
  if (handle === "ry") return { ...g, ry: Math.max(0.01, Math.abs(v)) };
  // Radial feather: where the pointer is between the centre (100) and the edge (0).
  if (handle === "feather") return { ...g, feather: Math.round(Math.min(100, Math.max(0, 100 * (1 - Math.hypot(u / (g.rx * aspect), v / g.ry))))) };
  return { ...g, angle: Math.round((Math.atan2(py, px) * 180) / Math.PI) };
}

/** A gradient moved by (dx, dy). */
function moved(g: Gradient, dx: number, dy: number): Gradient {
  return g.kind === "linear"
    ? { ...g, full: [g.full[0] + dx, g.full[1] + dy], zero: [g.zero[0] + dx, g.zero[1] + dy] }
    : { ...g, centre: [g.centre[0] + dx, g.centre[1] + dy] };
}

/**
 * Linear and radial gradients and subject masks (top bar): drag on the photo
 * to draw a gradient (linear: from full effect to none; radial: from the
 * centre out), then set its sliders. Drag the pin to move it, the handles to
 * reshape it. Click a pin to pick it. A subject mask needs no drawing: the
 * photo's subject is found for it (shown in red while it's picked), and each
 * photo it's pasted onto finds its own. Delete removes the picked one, Esc /
 * Done closes.
 */
export default function GradientOverlay({ image, gradients, onChange, maskOverlay, onPicked, showMask, onShowMask }: Props) {
  const layersBox = useFloatingBar("masks");
  const maskBar = useFloatingBar("masks-bar");
  const layer = useRef<HTMLDivElement>(null);
  const [picked, setPicked] = useState<number>(gradients.length - 1);
  // The type picked in the layers panel for a new layer (null: none; the photo then only picks).
  // Subject starts its layer straight away; the others when drawn (or painted).
  const [tool, setTool] = useState<GradientKind | null>(null);
  const drag = useRef<Drag | null>(null);
  const [, redraw] = useState(0);
  const sel = picked >= 0 && picked < gradients.length ? gradients[picked] : null;
  // The app knows which mask is picked: the bottom panel's sliders and tick boxes act on it.
  useEffect(() => {
    onPicked?.(sel ? picked : -1);
  }, [picked, sel, onPicked]);
  useEffect(() => () => onPicked?.(-1), [onPicked]);

  // Re-measure when the window changes size.
  useEffect(() => {
    const on = () => redraw((n) => n + 1);
    window.addEventListener("resize", on);
    return () => window.removeEventListener("resize", on);
  }, []);

  // Zoomed or panned (mouse wheel while drawing): the picture moves under
  // the overlay, so it's drawn again wherever the picture now sits.
  const lastPlace = useRef("");
  useLayoutEffect(() => {
    const r = image?.getBoundingClientRect();
    const key = r ? `${r.left},${r.top},${r.width},${r.height}` : "";
    if (key !== lastPlace.current) {
      lastPlace.current = key;
      redraw((n) => n + 1);
    }
  });
  const rect = () => {
    if (!image || !layer.current) return null;
    const r = image.getBoundingClientRect();
    const l = layer.current.getBoundingClientRect();
    return { left: r.left - l.left, top: r.top - l.top, width: r.width, height: r.height, ox: r.left, oy: r.top };
  };
  const p = rect();
  const toFraction = (e: { clientX: number; clientY: number }): Pt | null => {
    const r = rect();
    return r ? { x: (e.clientX - r.ox) / r.width, y: (e.clientY - r.oy) / r.height } : null;
  };

  const update = (i: number, g: Gradient, key: string | null) => onChange(gradients.map((o, j) => (j === i ? g : o)), key);
  // A subject mask (Invert puts it on the background, as Lightroom's Select Background).
  const addSubject = () => {
    onChange([...gradients, newGradient("subject", { x: 0.5, y: 0.5 }, { x: 0.5, y: 0.5 })], null);
    setPicked(gradients.length);
  };
  /** Done (or Esc): nothing picked, no new layer waiting; the masks stay open (the Masks button closes them). */
  const finish = () => {
    setTool(null);
    setPartMode(null);
    setPartMenu(null);
    setSelPart(null);
    setPicked(-1);
  };
  /** A type picked for a new layer (picked again: dropped). */
  const startLayer = (kind: GradientKind) => {
    setPartMode(null);
    setPartMenu(null);
    if (kind === "subject") {
      setTool(null);
      addSubject();
      return;
    }
    setTool((t) => (t === kind ? null : kind));
  };

  // The brush: its size (radius, a fraction of the photo's width), feather, and Erase (or Alt held).
  const [brushSize, setBrushSize] = useState(BRUSH_SIZE);
  const [brushFeather, setBrushFeather] = useState(BRUSH_FEATHER);
  const [brushErase, setBrushErase] = useState(false);
  const [cursor, setCursor] = useState<Pt | null>(null);
  const [altHeld, setAltHeld] = useState(false);
  useEffect(() => {
    const on = (e: KeyboardEvent) => setAltHeld(e.altKey);
    window.addEventListener("keydown", on);
    window.addEventListener("keyup", on);
    return () => {
      window.removeEventListener("keydown", on);
      window.removeEventListener("keyup", on);
    };
  }, []);
  const erasing = brushErase !== altHeld;

  // The picked mask's overlay: where it acts, in red, worked out by the engine for the whole
  // mask (its parts added and subtracted, Feather and Invert), so it's exactly what's applied.
  const [overlay, setOverlay] = useState<string | null>(null);
  const shapeKey = sel
    ? JSON.stringify([sel.kind, sel.full, sel.zero, sel.centre, sel.rx, sel.ry, sel.angle, sel.feather, sel.invert, sel.strokes, sel.parts, sel.frozen?.length])
    : null;
  const selRef = useRef(sel);
  selRef.current = sel;
  useEffect(() => {
    if (!shapeKey || !showMask || !maskOverlay || !selRef.current) {
      setOverlay(null);
      return;
    }
    let stale = false;
    const t = window.setTimeout(() => {
      if (!selRef.current) return;
      maskOverlay(selRef.current)
        .then((src) => {
          if (!stale) setOverlay(src);
        })
        .catch(() => {
          if (!stale) setOverlay(null);
        });
    }, 150);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [shapeKey, showMask, maskOverlay]);

  // Add / Subtract: the next shape drawn (or the subject) becomes a part of the picked mask.
  const [partMenu, setPartMenu] = useState<"add" | "subtract" | null>(null);
  const [partMode, setPartMode] = useState<{ subtract: boolean; kind: GradientKind } | null>(null);
  // The brush part being painted in this Add / Subtract (one stroke: done on release).
  const brushPart = useRef<number | null>(null);
  // The part picked (its pin clicked, or just drawn): its handles show.
  const [selPart, setSelPart] = useState<number | null>(null);
  // Painting: a new brush layer, the picked brush layer, or a brush + / − shape.
  const brushing = tool === "brush" || (tool === null && !partMode && sel?.kind === "brush") || partMode?.kind === "brush";
  const removePart = (k: number) => {
    if (!sel) return;
    setSelPart(null);
    setParts((sel.parts ?? []).filter((_, j) => j !== k), null);
  };
  // A part (or the + / − menu) asked for on a mask not yet picked: set once it is.
  const pending = useRef<{ part: number | null; menu: "add" | "subtract" | null }>({ part: null, menu: null });
  useEffect(() => {
    setPartMode(null);
    setPartMenu(pending.current.menu);
    setSelPart(pending.current.part);
    pending.current = { part: null, menu: null };
    brushPart.current = null;
  }, [picked]);
  /** From the layers: mask `i` picked, with its part `part` (null: the mask's own shape) or menu. */
  const pickLayer = (i: number, part: number | null, menu: "add" | "subtract" | null = null) => {
    setTool(null);
    if (i === picked) {
      setSelPart(part);
      setPartMode(null);
      setPartMenu(menu);
      return;
    }
    pending.current = { part, menu };
    setPicked(i);
  };
  const setParts = (parts: MaskPart[], key: string | null) => {
    if (sel) update(picked, { ...sel, parts }, key);
  };
  const choosePart = (kind: GradientKind) => {
    const subtract = partMenu === "subtract";
    setPartMenu(null);
    brushPart.current = null;
    if (!sel) return;
    if (kind === "subject") {
      // The subject: added (or taken away) straight away.
      setParts([...(sel.parts ?? []), { subtract, shape: newGradient("subject", { x: 0.5, y: 0.5 }, { x: 0.5, y: 0.5 }) }], null);
      setPartMode(null);
      return;
    }
    setTool(null);
    setPartMode({ subtract, kind });
  };
  const remove = (i: number) => {
    onChange(gradients.filter((_, j) => j !== i), null);
    setPicked(-1);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Typing in a box: its keys are the box's.
      const tgt = e.target as HTMLElement | null;
      if (tgt && (tgt.tagName === "INPUT" || tgt.tagName === "TEXTAREA" || tgt.tagName === "SELECT" || tgt.isContentEditable)) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        if (partMode || partMenu) {
          setPartMode(null);
          setPartMenu(null);
          return;
        }
        finish();
      } else if ((e.key === "Delete" || e.key === "Backspace") && sel) {
        e.preventDefault();
        e.stopImmediatePropagation();
        if (selPart !== null && sel.parts?.[selPart]) removePart(selPart);
        else remove(picked);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    // A part of the picked mask (Add / Subtract): painted or drawn.
    if (partMode && sel) {
      const f = toFraction(e);
      if (!f) return;
      if (partMode.kind === "brush") {
        const stroke: Stroke = { points: [[f.x, f.y]], size: brushSize, feather: brushFeather, erase: erasing };
        const parts = [...(sel.parts ?? [])];
        const at = brushPart.current;
        if (at !== null && parts[at]?.shape.kind === "brush") {
          parts[at] = { ...parts[at], shape: { ...parts[at].shape, strokes: [...(parts[at].shape.strokes ?? []), stroke] } };
          setParts(parts, null);
          drag.current = { what: "paint-part", part: at, stroke };
        } else {
          if (stroke.erase) return;
          parts.push({ subtract: partMode.subtract, shape: { ...newGradient("brush", f, f), strokes: [stroke] } });
          brushPart.current = parts.length - 1;
          setParts(parts, null);
          drag.current = { what: "paint-part", part: parts.length - 1, stroke };
        }
      } else {
        drag.current = { what: "new-part", start: f };
      }
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      return;
    }
    const f = toFraction(e);
    if (!f) return;
    if (tool === "brush" || (tool === null && sel?.kind === "brush")) {
      // Brush picked for a new layer: the stroke starts it. Else a stroke on the picked brush layer.
      const stroke: Stroke = { points: [[f.x, f.y]], size: brushSize, feather: brushFeather, erase: erasing };
      const onBrush = tool === null && sel?.kind === "brush";
      if (onBrush) {
        update(picked, { ...sel, strokes: [...(sel.strokes ?? []), stroke] }, null);
        drag.current = { what: "paint", index: picked, stroke };
      } else {
        if (stroke.erase) return; // nothing to erase on a new mask
        const g: Gradient = { ...newGradient("brush", f, f), strokes: [stroke] };
        onChange([...gradients, g], null);
        setPicked(gradients.length);
        setTool(null);
        drag.current = { what: "paint", index: gradients.length, stroke };
      }
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      return;
    }
    drag.current = { what: "new", start: f };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  };
  const startHandle = (e: React.PointerEvent, i: number, what: "move" | "full" | "zero" | "rx" | "ry" | "rotate" | "feather" | "mid") => {
    e.stopPropagation();
    if (e.button !== 0) return;
    setPicked(i);
    const f = toFraction(e);
    if (!f) return;
    drag.current = what === "move" || what === "mid" ? { what, start: f, orig: gradients[i] } : { what, orig: gradients[i] };
    layer.current?.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const d = drag.current;
    const r = rect();
    const f = toFraction(e);
    if (brushing) setCursor(f);
    if (!d || !f || !r) return;
    if (d.what === "part-move" || d.what === "part-handle") {
      if (!sel) return;
      const parts = [...(sel.parts ?? [])];
      const part = parts[d.part];
      if (!part) return;
      const shape = d.what === "part-move" ? moved(d.orig, f.x - d.start.x, f.y - d.start.y) : reshaped(d.orig, d.handle, f, r.width / r.height, d.start);
      parts[d.part] = { ...part, shape };
      setParts(parts, `gradient:${picked}:part:${d.part}:${d.what === "part-move" ? "move" : d.handle}`);
      return;
    }
    if (d.what === "paint-part" || d.what === "new-part" || d.what === "draw-part") {
      if (!sel || !partMode) return;
      const parts = [...(sel.parts ?? [])];
      if (d.what === "paint-part") {
        const last = d.stroke.points[d.stroke.points.length - 1];
        if (Math.hypot((f.x - last[0]) * r.width, (f.y - last[1]) * r.height) < 2) return;
        d.stroke = { ...d.stroke, points: [...d.stroke.points, [f.x, f.y]] };
        const part = parts[d.part];
        if (!part) return;
        const strokes = [...(part.shape.strokes ?? [])];
        strokes[strokes.length - 1] = d.stroke;
        parts[d.part] = { ...part, shape: { ...part.shape, strokes } };
        setParts(parts, `gradient:${picked}:part-paint`);
        return;
      }
      if (d.what === "new-part") {
        if (Math.hypot((f.x - d.start.x) * r.width, (f.y - d.start.y) * r.height) < MIN_PX) return;
        parts.push({ subtract: partMode.subtract, shape: newGradient(partMode.kind, d.start, f) });
        setParts(parts, null);
        setSelPart(parts.length - 1);
        drag.current = { what: "draw-part", part: parts.length - 1, start: d.start };
        return;
      }
      const part = parts[d.part];
      if (!part) return;
      const s = part.shape;
      parts[d.part] = {
        ...part,
        shape:
          s.kind === "linear"
            ? { ...s, zero: [f.x, f.y] }
            : { ...s, rx: Math.max(0.01, Math.abs(f.x - d.start.x)), ry: Math.max(0.01, Math.abs(f.y - d.start.y)) },
      };
      setParts(parts, `gradient:${picked}:part-draw`);
      return;
    }
    if (d.what === "paint") {
      // The stroke grows as the pointer moves (points at least ~2 px apart), one undo step per stroke.
      const last = d.stroke.points[d.stroke.points.length - 1];
      if (Math.hypot((f.x - last[0]) * r.width, (f.y - last[1]) * r.height) < 2) return;
      d.stroke = { ...d.stroke, points: [...d.stroke.points, [f.x, f.y]] };
      const g = gradients[d.index];
      if (!g) return;
      const strokes = [...(g.strokes ?? [])];
      strokes[strokes.length - 1] = d.stroke;
      update(d.index, { ...g, strokes }, `gradient:${d.index}:paint`);
      return;
    }
    const aspect = r.width / r.height;
    if (d.what === "new") {
      const px = Math.hypot((f.x - d.start.x) * r.width, (f.y - d.start.y) * r.height);
      if (px < MIN_PX) return;
      if (!tool) return;
      const g = newGradient(tool, d.start, f);
      setTool(null);
      onChange([...gradients, g], null);
      const i = gradients.length;
      setPicked(i);
      // Carry on dragging the new one's far handle.
      drag.current = tool === "linear" ? { what: "zero", orig: g } : { what: "draw-radial", start: d.start, orig: g };
      return;
    }
    const i = picked;
    const g = d.orig;
    const key = `gradient:${i}:${d.what}`;
    if (d.what === "move") {
      const dx = f.x - d.start.x, dy = f.y - d.start.y;
      update(i, g.kind === "linear"
        ? { ...g, full: [g.full[0] + dx, g.full[1] + dy], zero: [g.zero[0] + dx, g.zero[1] + dy] }
        : { ...g, centre: [g.centre[0] + dx, g.centre[1] + dy] }, key);
    } else if (d.what === "full") {
      update(i, { ...g, full: [f.x, f.y] }, key);
    } else if (d.what === "zero") {
      update(i, { ...g, zero: [f.x, f.y] }, key);
    } else if (d.what === "feather") {
      update(i, reshaped(g, "feather", f, aspect), key);
    } else if (d.what === "mid") {
      update(i, reshaped(g, "mid", f, aspect, d.start), key);
    } else if (d.what === "draw-radial") {
      // Drawing a radial: the drag's corner sets both radii (an upright oval).
      update(i, { ...g, rx: Math.max(0.01, Math.abs(f.x - d.start.x)), ry: Math.max(0.01, Math.abs(f.y - d.start.y)) }, key);
    } else {
      // Radii and rotation: the pointer in the oval's own (rotated) frame.
      const a = (g.angle * Math.PI) / 180;
      const px = (f.x - g.centre[0]) * aspect, py = f.y - g.centre[1];
      const u = px * Math.cos(a) + py * Math.sin(a);
      const v = -px * Math.sin(a) + py * Math.cos(a);
      if (d.what === "rx") update(i, { ...g, rx: Math.max(0.01, Math.abs(u) / aspect) }, key);
      else if (d.what === "ry") update(i, { ...g, ry: Math.max(0.01, Math.abs(v)) }, key);
      else update(i, { ...g, angle: Math.round((Math.atan2(py, px) * 180) / Math.PI) }, key);
    }
  };
  const onPointerUp = () => {
    // A part is done when the mouse is let go (a brush part after its stroke), so + / − can be
    // clicked straight away for the next.
    const w = drag.current?.what;
    if (w === "new") {
      // A click on the photo, not a drag: the picked part let go, else the picked mask (the
      // other pins come back).
      if (selPart !== null) setSelPart(null);
      else setPicked(-1);
    }
    if (w === "draw-part" || w === "paint-part") {
      setPartMode(null);
      brushPart.current = null;
    }
    drag.current = null;
  };

  // `side` tells a pair apart (an oval's two rx and two ry handles), so each keeps its own element.
  const handle = (x: number, y: number, i: number, what: "move" | "full" | "zero" | "rx" | "ry" | "rotate" | "feather" | "mid", cls = "", side = "") =>
    what === "feather" ? (
      <circle key={`${i}-${what}`} className={`gradient-handle gradient-handle--feather ${cls}`} cx={x} cy={y} r={5} onPointerDown={(e) => startHandle(e, i, what)} />
    ) : (
    <rect
      key={`${i}-${what}${side}`}
      className={`gradient-handle ${cls}`}
      x={x - 5}
      y={y - 5}
      width={10}
      height={10}
      onPointerDown={(e) => startHandle(e, i, what)}
    />
  );
  /** A linear's middle handle (a diamond on the middle line, beside the pin): drags the full and
   *  zero lines apart or together. */
  const midHandle = (mx: number, my: number, fx: number, fy: number, zx: number, zy: number, down: (e: React.PointerEvent) => void) => {
    const len = Math.hypot(zx - fx, zy - fy) || 1;
    const x = mx - ((zy - fy) / len) * 34, y = my + ((zx - fx) / len) * 34;
    return (
      <rect key="mid" className="gradient-handle gradient-handle--mid" x={x - 5} y={y - 5} width={10} height={10} transform={`rotate(45 ${x} ${y})`} onPointerDown={down} />
    );
  };
  /** Where a radial's feather handle sits (pixels): on its inner oval, at a diagonal. */
  const featherAt = (g: Gradient, W: number, H: number): [number, number] => {
    const cx = g.centre[0] * W, cy = g.centre[1] * H;
    const inner = Math.max(0.02, 1 - g.feather / 100);
    const a = (g.angle * Math.PI) / 180;
    const u = g.rx * W * inner * Math.SQRT1_2, v = -g.ry * H * inner * Math.SQRT1_2;
    return [cx + u * Math.cos(a) - v * Math.sin(a), cy + u * Math.sin(a) + v * Math.cos(a)];
  };

  /** The gradient's lines (picked: with its handles), in the photo's pixels. */
  const shape = (g: Gradient, i: number) => {
    if (!p) return null;
    const on = i === picked;
    const W = p.width, H = p.height;
    if (g.kind === "brush") {
      const first = g.strokes?.[0]?.points[0] ?? [0.5, 0.5];
      // The picked brush mask shows where it's painted (red), as Lightroom's overlay; erasing in grey.
      return (
        <g key={i} className={on ? "gradient-shape gradient-shape--on" : "gradient-shape"}>
          <circle
            className="gradient-pin"
            cx={first[0] * W}
            cy={first[1] * H}
            r={6}
            onPointerDown={(e) => {
              e.stopPropagation();
              setPicked(i);
            }}
          />
        </g>
      );
    }
    if (g.kind === "subject") {
      // No shape: a pin along the bottom right of the photo for each subject mask, to pick it
      // (S: on the subject, B: inverted, on the background). The zoom buttons are top left, the layers top right.
      const n = gradients.slice(0, i).filter((o) => o.kind === "subject").length;
      const x = W - 18 - n * 26;
      const y = H - 18;
      return (
        <g key={i} className={on ? "gradient-shape gradient-shape--on" : "gradient-shape"}>
          <circle
            className="gradient-pin gradient-pin--subject"
            cx={x}
            cy={y}
            r={9}
            onPointerDown={(e) => {
              e.stopPropagation();
              setPicked(i);
            }}
          />
          <text className="gradient-pin__label" x={x} y={y + 4} textAnchor="middle">
            {g.invert ? "B" : "S"}
          </text>
        </g>
      );
    }
    if (g.kind === "linear") {
      const [fx, fy, zx, zy] = [g.full[0] * W, g.full[1] * H, g.zero[0] * W, g.zero[1] * H];
      const len = Math.hypot(zx - fx, zy - fy) || 1;
      // Perpendicular, long enough to cross the photo.
      const nx = (-(zy - fy) / len) * (W + H), ny = ((zx - fx) / len) * (W + H);
      const line = (x: number, y: number, cls: string) => <line className={cls} x1={x - nx} y1={y - ny} x2={x + nx} y2={y + ny} />;
      // Dragging the middle line spreads the full and zero lines apart, or closer.
      const mx = (fx + zx) / 2, my = (fy + zy) / 2;
      return (
        <g key={i} className={on ? "gradient-shape gradient-shape--on" : "gradient-shape"}>
          {on && line(fx, fy, "gradient-line")}
          {on && line(mx, my, "gradient-line gradient-line--mid")}
          {on && (
            <line className="gradient-midhit" x1={mx - nx} y1={my - ny} x2={mx + nx} y2={my + ny} onPointerDown={(e) => startHandle(e, i, "mid")} />
          )}
          {on && line(zx, zy, "gradient-line")}
          {on && handle(fx, fy, i, "full")}
          {on && handle(zx, zy, i, "zero")}
          {on && midHandle(mx, my, fx, fy, zx, zy, (e) => startHandle(e, i, "mid"))}
          <circle className="gradient-pin" cx={mx} cy={my} r={6} onPointerDown={(e) => startHandle(e, i, "move")} />
        </g>
      );
    }
    const cx = g.centre[0] * W, cy = g.centre[1] * H, rx = g.rx * W, ry = g.ry * H;
    const inner = 1 - g.feather / 100;
    const a = (g.angle * Math.PI) / 180;
    const at = (u: number, v: number): [number, number] => [cx + u * Math.cos(a) - v * Math.sin(a), cy + u * Math.sin(a) + v * Math.cos(a)];
    return (
      <g key={i} className={on ? "gradient-shape gradient-shape--on" : "gradient-shape"}>
        {on && <ellipse className="gradient-line" cx={cx} cy={cy} rx={rx} ry={ry} transform={`rotate(${g.angle} ${cx} ${cy})`} />}
        {on && inner > 0.02 && (
          <ellipse className="gradient-line gradient-line--mid" cx={cx} cy={cy} rx={rx * inner} ry={ry * inner} transform={`rotate(${g.angle} ${cx} ${cy})`} />
        )}
        {on && handle(...at(rx, 0), i, "rx", "", "+")}
        {on && handle(...at(-rx, 0), i, "rx", "", "-")}
        {on && handle(...at(0, ry), i, "ry", "", "+")}
        {on && handle(...at(0, -ry), i, "ry", "", "-")}
        {on && handle(...at(rx + 18, 0), i, "rotate", "gradient-handle--rotate")}
        {on && handle(...featherAt(g, W, H), i, "feather")}
        <circle className="gradient-pin" cx={cx} cy={cy} r={6} onPointerDown={(e) => startHandle(e, i, "move")} />
      </g>
    );
  };

  return (
    <div
      className={`remove-layer gradient-layer${brushing ? " gradient-layer--brush" : ""}`}
      ref={layer}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerLeave={() => setCursor(null)}
    >
      {p && overlay && (
        <img className="gradient-subject" src={overlay} alt="" draggable={false} style={{ left: p.left, top: p.top, width: p.width, height: p.height }} />
      )}
      {p && (
        // A margin round the photo (same coordinates inside): pins and lines off the photo are in the
        // SVG's own box, as WebKitGTK only repaints that box and left copies behind when they moved.
        <svg
          className="gradient-svg"
          style={{ left: p.left - SVG_MARGIN, top: p.top - SVG_MARGIN, width: p.width + 2 * SVG_MARGIN, height: p.height + 2 * SVG_MARGIN }}
          viewBox={`${-SVG_MARGIN} ${-SVG_MARGIN} ${p.width + 2 * SVG_MARGIN} ${p.height + 2 * SVG_MARGIN}`}
        >
          {/* A mask picked: only its pins (the others come back when it's let go). */}
          {gradients.map((g, i) => (sel && i !== picked ? null : shape(g, i)))}
          {/* The picked mask's linear and radial parts, dashed (cyan added, orange taken away). */}
          {/* The stroke being painted, in red (grey erasing) at the brush's size, until it's let go: the
              engine's exact overlay is worked out only once the brush stops, so it showed nothing meanwhile. */}
          {(() => {
            const d = drag.current;
            if (!d || (d.what !== "paint" && d.what !== "paint-part")) return null;
            const pts = d.stroke.points;
            const width = d.stroke.size * 2 * p.width;
            return (
              <path
                className={`gradient-live-stroke${d.stroke.erase ? " gradient-live-stroke--erase" : ""}`}
                d={pts.map(([x, y], k) => `${k ? "L" : "M"}${(x * p.width).toFixed(1)},${(y * p.height).toFixed(1)}`).join(" ") + (pts.length === 1 ? " l0.01,0" : "")}
                strokeWidth={width}
              />
            );
          })()}
          {sel?.parts?.map((pt, k) => {
            const s = pt.shape;
            const W = p.width, H = p.height;
            const cls = `gradient-part${pt.subtract ? " gradient-part--subtract" : ""}`;
            const on = selPart === k;
            // A part picked: only its pins.
            if (selPart !== null && !on) return null;
            // A part's pin picks it (and drags it); its handles, once picked, reshape it.
            const startPart = (e: React.PointerEvent, what: "move" | "full" | "zero" | "rx" | "ry" | "rotate" | "feather" | "mid") => {
              e.stopPropagation();
              if (e.button !== 0) return;
              const f = toFraction(e);
              if (!f) return;
              setSelPart(k);
              drag.current = what === "move" ? { what: "part-move", part: k, start: f, orig: s } : { what: "part-handle", part: k, handle: what, start: f, orig: s };
              layer.current?.setPointerCapture(e.pointerId);
            };
            const partHandle = (x: number, y: number, what: "full" | "zero" | "rx" | "ry" | "rotate" | "feather" | "mid", extra = "", side = "") =>
              what === "feather" ? (
                <circle key={what} className="gradient-handle gradient-handle--part gradient-handle--feather" cx={x} cy={y} r={5} onPointerDown={(e) => startPart(e, what)} />
              ) : (
              // Keyed by which handle it is (and its side), not where: keyed by position, every move swapped
              // it for a new square and WebKitGTK left the old ones painted (a trail on a part's handles).
              <rect key={what + side} className={`gradient-handle gradient-handle--part ${extra}`} x={x - 5} y={y - 5} width={10} height={10} onPointerDown={(e) => startPart(e, what)} />
            );
            if (s.kind === "linear") {
              const [fx, fy, zx, zy] = [s.full[0] * W, s.full[1] * H, s.zero[0] * W, s.zero[1] * H];
              const len = Math.hypot(zx - fx, zy - fy) || 1;
              const nx = (-(zy - fy) / len) * (W + H), ny = ((zx - fx) / len) * (W + H);
              const mx = (fx + zx) / 2, my = (fy + zy) / 2;
              return (
                <g key={k}>
                  {on && <line className={cls} x1={fx - nx} y1={fy - ny} x2={fx + nx} y2={fy + ny} />}
                  <line className={cls} x1={mx - nx} y1={my - ny} x2={mx + nx} y2={my + ny} />
                  {on && <line className="gradient-midhit" x1={mx - nx} y1={my - ny} x2={mx + nx} y2={my + ny} onPointerDown={(e) => startPart(e, "mid")} />}
                  {on && <line className={cls} x1={zx - nx} y1={zy - ny} x2={zx + nx} y2={zy + ny} />}
                  {on && partHandle(fx, fy, "full")}
                  {on && partHandle(zx, zy, "zero")}
                  {on && midHandle(mx, my, fx, fy, zx, zy, (e) => startPart(e, "mid"))}
                  <circle className={`gradient-pin gradient-pin--part${pt.subtract ? " gradient-pin--subtract" : ""}${on ? " is-on" : ""}`} cx={mx} cy={my} r={5} onPointerDown={(e) => startPart(e, "move")} />
                </g>
              );
            }
            if (s.kind === "radial") {
              const cx = s.centre[0] * W, cy = s.centre[1] * H, rx = s.rx * W, ry = s.ry * H;
              const a = (s.angle * Math.PI) / 180;
              const at = (u: number, v: number): [number, number] => [cx + u * Math.cos(a) - v * Math.sin(a), cy + u * Math.sin(a) + v * Math.cos(a)];
              return (
                <g key={k}>
                  <ellipse className={cls} cx={cx} cy={cy} rx={rx} ry={ry} transform={`rotate(${s.angle} ${cx} ${cy})`} />
                  {on && partHandle(...at(rx, 0), "rx", "", "+")}
                  {on && partHandle(...at(-rx, 0), "rx", "", "-")}
                  {on && partHandle(...at(0, ry), "ry", "", "+")}
                  {on && partHandle(...at(0, -ry), "ry", "", "-")}
                  {on && 1 - s.feather / 100 > 0.02 && (
                    <ellipse className={cls} cx={cx} cy={cy} rx={rx * (1 - s.feather / 100)} ry={ry * (1 - s.feather / 100)} transform={`rotate(${s.angle} ${cx} ${cy})`} />
                  )}
                  {on && partHandle(...at(rx + 18, 0), "rotate", "gradient-handle--rotate")}
                  {on && partHandle(...featherAt(s, W, H), "feather")}
                  <circle className={`gradient-pin gradient-pin--part${pt.subtract ? " gradient-pin--subtract" : ""}${on ? " is-on" : ""}`} cx={cx} cy={cy} r={5} onPointerDown={(e) => startPart(e, "move")} />
                </g>
              );
            }
            return null;
          })}
          {brushing && cursor && (
            <g className={`gradient-cursor${erasing ? " gradient-cursor--erase" : ""}`}>
              <circle cx={cursor.x * p.width} cy={cursor.y * p.height} r={brushSize * p.width} />
              <circle cx={cursor.x * p.width} cy={cursor.y * p.height} r={brushSize * p.width * (1 - brushFeather / 100)} className="gradient-cursor__inner" />
            </g>
          )}
        </svg>
      )}
      {/* The layers: each mask, its own shape and the shapes added (+) or taken away (−). */}
      {(
        <div ref={layersBox.ref} style={layersBox.style} className="masklayers" onPointerDown={(e) => e.stopPropagation()}>
          <div className="masklayers__title">
            <span className="masklayers__name">
              <BarGrip bar={layersBox} />
              Masks
            </span>
            {onShowMask && (
              <label className="masklayers__show" title="Show the picked mask in red over the photo">
                <input type="checkbox" checked={showMask} onChange={onShowMask} disabled={!sel} />
                Show mask
              </label>
            )}
          </div>
          {/* A new layer: its type (Subject at once; the others drawn on the photo). */}
          <div className="masklayers__new">
            <button type="button" onClick={() => startLayer("subject")} title="New subject layer: the photo's main subject, found for you (pasted onto other photos, each finds its own, until it has + / − shapes)">
              <UserRound size={14} />
            </button>
            <button type="button" className={tool === "brush" ? "is-on" : undefined} onClick={() => startLayer("brush")} title="New brush layer: paint on the photo (Alt: erase)">
              <Paintbrush size={14} />
            </button>
            <button type="button" className={tool === "linear" ? "is-on" : undefined} onClick={() => startLayer("linear")} title="New linear layer: drag on the photo from full effect to none">
              <LinearGradientIcon size={14} />
            </button>
            <button type="button" className={tool === "radial" ? "is-on" : undefined} onClick={() => startLayer("radial")} title="New radial layer: drag on the photo from the centre out">
              <RadialGradientIcon size={14} />
            </button>
          </div>
          {gradients.map((g, i) => {
            const on = i === picked;
            return (
              <div key={i} className={`masklayers__mask${on ? " is-on" : ""}`}>
                <div className="masklayers__row masklayers__row--mask" onClick={() => pickLayer(i, null)}>
                  <button
                    type="button"
                    className={g.hidden ? "masklayers__eye is-off" : "masklayers__eye"}
                    title={g.hidden ? "Show this mask's effect" : "Hide this mask's effect"}
                    onClick={(e) => {
                      e.stopPropagation();
                      update(i, { ...g, hidden: !g.hidden }, null);
                    }}
                  >
                    {g.hidden ? <EyeOff size={12} /> : <Eye size={12} />}
                  </button>
                  <span className={g.hidden ? "masklayers__name is-off" : "masklayers__name"}>Mask {i + 1}</span>
                  <button
                    type="button"
                    className={on && (partMenu === "add" || (partMode && !partMode.subtract)) ? "is-on" : undefined}
                    title="Add another shape to this mask"
                    onClick={(e) => {
                      e.stopPropagation();
                      pickLayer(i, null, on && partMenu === "add" ? null : "add");
                    }}
                  >
                    <Plus size={12} />
                  </button>
                  <button
                    type="button"
                    className={on && (partMenu === "subtract" || partMode?.subtract) ? "is-on" : undefined}
                    title="Take a shape away from this mask"
                    onClick={(e) => {
                      e.stopPropagation();
                      pickLayer(i, null, on && partMenu === "subtract" ? null : "subtract");
                    }}
                  >
                    <Minus size={12} />
                  </button>
                  <button
                    type="button"
                    className={g.invert ? "is-on" : undefined}
                    title={g.invert ? "Inverted: the effect everywhere but the mask (click: back)" : "Invert the mask: the effect everywhere but where it is"}
                    onClick={(e) => {
                      e.stopPropagation();
                      update(i, { ...g, invert: !g.invert }, null);
                    }}
                  >
                    <Contrast size={12} />
                  </button>
                  <button
                    type="button"
                    className="masklayers__x"
                    title="Delete this mask"
                    onClick={(e) => {
                      e.stopPropagation();
                      remove(i);
                    }}
                  >
                    <X size={12} />
                  </button>
                </div>
                {on && partMenu && (
                  <div className="masklayers__kinds">
                    {!hasSubject(g) && (
                      <button type="button" onClick={() => choosePart("subject")}>
                        <UserRound size={12} /> Subject
                      </button>
                    )}
                    <button type="button" onClick={() => choosePart("brush")}>
                      <Paintbrush size={12} /> Brush
                    </button>
                    <button type="button" onClick={() => choosePart("linear")}>
                      <LinearGradientIcon size={12} /> Linear
                    </button>
                    <button type="button" onClick={() => choosePart("radial")}>
                      <RadialGradientIcon size={12} /> Radial
                    </button>
                  </div>
                )}
                <div
                  className={`masklayers__row masklayers__row--part${on && selPart === null ? " is-on" : ""}`}
                  onClick={() => pickLayer(i, null)}
                >
                  {kindIcon(g.kind)} {kindName(g)}
                </div>
                {g.parts?.map((pt, k) => (
                  <div
                    key={k}
                    className={`masklayers__row masklayers__row--part${pt.subtract ? " masklayers__row--subtract" : ""}${on && selPart === k ? " is-on" : ""}`}
                    onClick={() => pickLayer(i, k)}
                    title="Pick this shape (Delete: take it off)"
                  >
                    <span className="masklayers__sign">{pt.subtract ? "−" : "+"}</span>
                    {kindIcon(pt.shape.kind)} {kindName(pt.shape)}
                    {on && (
                      <button
                        type="button"
                        className={`masklayers__inv${pt.shape.invert ? " is-on" : ""}`}
                        title={pt.shape.invert ? "This shape inverted (click: back)" : "Invert this shape"}
                        onClick={(e) => {
                          e.stopPropagation();
                          setParts((g.parts ?? []).map((q, j) => (j === k ? { ...q, shape: { ...q.shape, invert: !q.shape.invert } } : q)), null);
                        }}
                      >
                        <Contrast size={11} />
                      </button>
                    )}
                    {on && (
                      <button
                        type="button"
                        className="masklayers__x"
                        title="Take this shape off the mask"
                        onClick={(e) => {
                          e.stopPropagation();
                          removePart(k);
                        }}
                      >
                        <X size={11} />
                      </button>
                    )}
                  </div>
                ))}
              </div>
            );
          })}
        </div>
      )}
      {/* The bar: only with something in it (brush settings, a new layer's hint, the picked layer's). */}
      {(brushing || tool || sel) && (
      <div ref={maskBar.ref} style={maskBar.style} className="cropbar removebar gradientbar" onPointerDown={(e) => e.stopPropagation()}>
        <BarGrip bar={maskBar} />
        {brushing && (
          <>
            <label className="gradientbar__slider" title="Brush size">
              <span>Size</span>
              <HSlider min={0.003} max={0.2} step={0.001} value={brushSize} reset={BRUSH_SIZE} onChange={setBrushSize} aria-label="Brush size" />
              <span className="gradientbar__value">{Math.round(brushSize * 1000) / 10}</span>
            </label>
            <label className="gradientbar__slider" title="Brush feather: how soft its edge is">
              <span>Feather</span>
              <HSlider min={0} max={100} step={1} value={brushFeather} reset={BRUSH_FEATHER} onChange={setBrushFeather} aria-label="Brush feather" />
              <span className="gradientbar__value">{brushFeather}</span>
            </label>
            <label className="check" title="Erase from the brush mask (or hold Alt while painting)">
              <input type="checkbox" checked={brushErase} onChange={() => setBrushErase((v) => !v)} />
              Erase
            </label>
          </>
        )}
        {tool ? (
          <span className="removebar__keys">
            {tool === "brush" ? "New brush layer: paint on the photo (Alt: erase)" : tool === "linear" ? "New linear layer: drag from full effect to none" : "New radial layer: drag from the centre out"}
          </span>
        ) : sel ? (
          <>
            {sel.kind === "subject" && (
              <>
                <label className="gradientbar__slider" title="Feather: how soft the mask's edge is (0: the model's own edge; double-click: back to the start)">
                  <span>Feather</span>
                  <HSlider
                    min={0}
                    max={100}
                    step={1}
                    value={sel.feather}
                    reset={SUBJECT_FEATHER}
                    onChange={(v) => update(picked, { ...sel, feather: v }, `gradient:${picked}:feather`)}
                    aria-label="Feather"
                  />
                  <span className="gradientbar__value">{Math.round(sel.feather)}</span>
                </label>
              </>
            )}
            {sel.kind === "radial" && (
              <>
                <label className="gradientbar__slider" title="Feather: how soft its edge is">
                  <span>Feather</span>
                  <HSlider min={0} max={100} step={1} value={sel.feather} reset={50}
                    onChange={(v) => update(picked, { ...sel, feather: v }, `gradient:${picked}:feather`)} aria-label="Feather" />
                  <span className="gradientbar__value">{Math.round(sel.feather)}</span>
                </label>
              </>
            )}
            {partMode && (
              <span className="removebar__keys">
                {partMode.subtract ? "Subtracting" : "Adding"}:{" "}
                {partMode.kind === "brush" ? "paint on the photo" : partMode.kind === "linear" ? "drag from full to none" : "drag from the centre out"}
              </span>
            )}
            <button
              type="button"
              onClick={() => (selPart !== null && sel.parts?.[selPart] ? removePart(selPart) : remove(picked))}
              title={selPart !== null && sel.parts?.[selPart] ? "Take the picked part off this mask (Delete)" : "Delete this mask (Delete)"}
            >
              <Trash2 size={13} />
            </button>
          </>
        ) : null}
      </div>
      )}
    </div>
  );
}
