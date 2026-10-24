import { useEffect, useRef } from "react";
import { cropToRatio, fitCrop, towards, type Crop, type Rect } from "../crop";

/**
 * The crop frame while cropping (after RapidRetouch's): covers the viewer,
 * draws the whole photo turned by the straighten angle under a straight frame,
 * with rule-of-thirds lines and handles. Drag an edge or corner to resize,
 * inside to slide the photo under the frame (as Lightroom's: the frame stays put),
 * outside to straighten (a finer grid shows while turning).
 * The engine renders the photo uncropped while this is open.
 */

type Handle = "move" | "rotate" | "n" | "s" | "e" | "w" | "nw" | "ne" | "sw" | "se";

const HANDLE_PX = 12; // how close the pointer must be to grab an edge or corner
const MIN_FRACTION = 0.03; // smallest crop, as a fraction of the photo
const MAX_ANGLE = 45;
const CYAN = "#3ee6ff";
// A quarter-circle double arrow with solid heads, white edged in black (as Lightroom's), for turning the photo
// from outside the frame. Drawn bulging to the top right; turned to bulge away from whichever corner the pointer is nearest.
const ROTATE_ARROW =
  '<path d="M7.5 4A13 13 0 0 1 20 16.5" fill="none"/><path d="M2.5 4L8 0.5V7.5Z"/><path d="M20 21.5L16.5 16H23.5Z"/>';
const rotateCursor = (turn: number) =>
  `url("data:image/svg+xml,${encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" width="17" height="17" viewBox="0 0 24 24" stroke-linejoin="round">' +
      `<g transform="rotate(${turn} 12 12) translate(12 12) scale(0.8) translate(-12 -12)">` +
      `<g stroke="black" stroke-width="4" fill="black">${ROTATE_ARROW}</g>` +
      `<g stroke="white" stroke-width="1.6" fill="white">${ROTATE_ARROW}</g></g></svg>`,
  )}") 8 8, crosshair`;
// Top right, bottom right, bottom left, top left.
const ROTATE_CURSORS = [0, 90, 180, 270].map(rotateCursor);

const CURSORS: Record<Handle, string> = {
  move: "move",
  rotate: ROTATE_CURSORS[0],
  n: "ns-resize",
  s: "ns-resize",
  e: "ew-resize",
  w: "ew-resize",
  nw: "nwse-resize",
  se: "nwse-resize",
  ne: "nesw-resize",
  sw: "nesw-resize",
};

/** A small flat-topped hexagon handle, centred on (x, y). */
function hexHandle(ctx: CanvasRenderingContext2D, x: number, y: number) {
  const r = 5.5;
  ctx.beginPath();
  for (let k = 0; k < 6; k++) {
    const a = (k * Math.PI) / 3;
    const [px, py] = [x + r * Math.cos(a), y + r * Math.sin(a)];
    if (k === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.closePath();
  ctx.fill();
  ctx.stroke();
}

interface Props {
  /** The displayed photo (its on-screen rectangle and pixels), uncropped. */
  image: HTMLImageElement | null;
  /** The photo's size in pixels (after any quarter turns), for its shape. */
  width: number;
  height: number;
  crop: Crop;
  /** Width / height to keep while dragging a corner, or null for free. */
  ratio: number | null;
  onChange: (c: Crop) => void;
}

export default function CropOverlay({ image, width, height, crop, ratio, onChange }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const drag = useRef<{ handle: Handle; start: [number, number]; from: Crop; startTurn: number; shift: [number, number]; downAt: [number, number] } | null>(null);
  // How far the photo is drawn from its place in the viewer, as fractions of its size: sliding the photo
  // moves the crop the other way over it, so the photo is drawn shifted by as much and the frame stays still.
  const shift = useRef<[number, number]>([0, 0]);
  // The picture drawn while a handle is held (see onPointerDown).
  const held = useRef<HTMLCanvasElement | null>(null);
  // While dragging, the frame is redrawn on every move and only handed to the
  // app on release, so it keeps up with the pointer.
  const cropRef = useRef(crop);
  if (!drag.current) cropRef.current = crop;
  const live = (c: Crop) => {
    const d = drag.current;
    if (d?.handle === "move")
      shift.current = [d.shift[0] + d.from.x0 - c.x0, d.shift[1] + d.from.y0 - c.y0];
    cropRef.current = c;
    redraw();
  };

  const redraw = () => {
    const canvas = canvasRef.current;
    if (!canvas || !image || !image.complete || image.naturalWidth === 0) return;
    const c = canvas.getBoundingClientRect();
    const img = image.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const bw = Math.round(c.width * dpr);
    const bh = Math.round(c.height * dpr);
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, c.width, c.height);
    // On whole screen pixels: the photo's place can come out a fraction of a pixel different from one
    // redraw to the next (D65 Glow's frame sits at fractional places), and drawn there as it is the
    // photo shimmered sideways by a pixel while a handle moved (24 Oct, the blonde girl).
    const snap = (v: number) => Math.round(v * dpr) / dpr;
    const iw = snap(img.width);
    const ih = snap(img.height);
    const ox = snap(img.left - c.left + shift.current[0] * iw);
    const oy = snap(img.top - c.top + shift.current[1] * ih);
    const { angle, x0, y0, x1, y1 } = cropRef.current;

    ctx.fillStyle = "#101010"; // neutral, as the viewer
    if (angle === 0 && shift.current[0] === 0 && shift.current[1] === 0) {
      // Straight and in its place: the loupe's own picture underneath shows through a hole, never
      // redrawn, so it can't move. (Drawn on this canvas, redrawn on every pointer move, it shimmered
      // a pixel sideways while a handle moved, both with and without D65 Glow, 24 Oct.)
      ctx.beginPath();
      ctx.rect(0, 0, c.width, c.height);
      ctx.rect(img.left - c.left, img.top - c.top, img.width, img.height);
      ctx.fill("evenodd");
    } else {
      // Turned (straightening) or slid: cover the viewer and draw the photo, under a straight frame.
      ctx.fillRect(0, 0, c.width, c.height);
      ctx.save();
      ctx.translate(ox + iw / 2, oy + ih / 2);
      ctx.rotate((angle * Math.PI) / 180);
      ctx.drawImage(held.current ?? image, -iw / 2, -ih / 2, iw, ih);
      ctx.restore();
    }
    const L = ox + x0 * iw;
    const T = oy + y0 * ih;
    const R = ox + x1 * iw;
    const B = oy + y1 * ih;
    ctx.beginPath();
    ctx.rect(0, 0, c.width, c.height);
    ctx.rect(L, T, R - L, B - T);
    ctx.fillStyle = "rgba(8, 8, 8, 0.65)";
    ctx.fill("evenodd");
    // Rule of thirds, and a finer grid while straightening.
    const turning = drag.current?.handle === "rotate";
    const grid = (n: number, colour: string) => {
      ctx.strokeStyle = colour;
      ctx.lineWidth = 1;
      ctx.beginPath();
      for (let f = 1; f < n; f++) {
        ctx.moveTo(L + ((R - L) * f) / n, T);
        ctx.lineTo(L + ((R - L) * f) / n, B);
        ctx.moveTo(L, T + ((B - T) * f) / n);
        ctx.lineTo(R, T + ((B - T) * f) / n);
      }
      ctx.stroke();
    };
    if (turning) grid(9, "rgba(190, 246, 255, 0.18)");
    grid(3, "rgba(190, 246, 255, 0.4)");
    // The frame: cyan with a soft glow.
    ctx.save();
    ctx.shadowColor = "rgba(62, 230, 255, 0.8)";
    ctx.shadowBlur = 6;
    ctx.strokeStyle = CYAN;
    ctx.lineWidth = 1.5;
    ctx.strokeRect(L, T, R - L, B - T);
    ctx.restore();
    // Hexagon handles: corners, and edge middles when the shape is free.
    ctx.fillStyle = "#ffffff";
    ctx.strokeStyle = "#080808";
    ctx.lineWidth = 1.5;
    const mx = (L + R) / 2;
    const my = (T + B) / 2;
    const marks: [number, number][] = [
      [L, T],
      [R, T],
      [L, B],
      [R, B],
    ];
    if (ratio === null) marks.push([mx, T], [mx, B], [L, my], [R, my]);
    for (const [x, y] of marks) hexHandle(ctx, x, y);
    if (turning) {
      const text = `${angle > 0 ? "+" : ""}${angle.toFixed(1)}°`;
      ctx.font = '600 13px "Exo 2", system-ui, sans-serif';
      const tw = ctx.measureText(text).width;
      ctx.fillStyle = "rgba(8, 8, 8, 0.85)";
      ctx.fillRect(mx - tw / 2 - 8, my - 13, tw + 16, 26);
      ctx.fillStyle = CYAN;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(text, mx, my);
    }
  };

  useEffect(() => {
    redraw();
  });
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ro = new ResizeObserver(() => redraw());
    ro.observe(canvas);
    image?.addEventListener("load", redraw);
    return () => {
      ro.disconnect();
      image?.removeEventListener("load", redraw);
    };
  });

  /** Pointer position as fractions of the photo's on-screen rectangle, and the handle under it. */
  const locate = (e: React.PointerEvent): { at: [number, number]; handle: Handle } => {
    const img = image!.getBoundingClientRect();
    const px = e.clientX - img.left - shift.current[0] * img.width;
    const py = e.clientY - img.top - shift.current[1] * img.height;
    const { x0, y0, x1, y1 } = cropRef.current;
    const L = x0 * img.width;
    const R = x1 * img.width;
    const T = y0 * img.height;
    const B = y1 * img.height;
    const near = (a: number, b: number) => Math.abs(a - b) <= HANDLE_PX;
    const withinX = px > L - HANDLE_PX && px < R + HANDLE_PX;
    const withinY = py > T - HANDLE_PX && py < B + HANDLE_PX;
    let v = withinX && near(py, T) ? "n" : withinX && near(py, B) ? "s" : "";
    let hz = withinY && near(px, L) ? "w" : withinY && near(px, R) ? "e" : "";
    if (ratio !== null && !(v && hz)) {
      v = "";
      hz = "";
    }
    const handle = (v + hz || (px > L && px < R && py > T && py < B ? "move" : "rotate")) as Handle;
    return { at: [px / img.width, py / img.height], handle };
  };

  /** The pointer's direction from the frame's centre, in degrees on screen. */
  const turnAt = (e: React.PointerEvent, c: Crop) => {
    const img = image!.getBoundingClientRect();
    const cx = img.left + (shift.current[0] + (c.x0 + c.x1) / 2) * img.width;
    const cy = img.top + (shift.current[1] + (c.y0 + c.y1) / 2) * img.height;
    return (Math.atan2(e.clientY - cy, e.clientX - cx) * 180) / Math.PI;
  };

  /** The turning cursor for the corner of the frame the pointer is nearest, bulging away from it. */
  const rotateCursorAt = (e: React.PointerEvent) => {
    const img = image!.getBoundingClientRect();
    const c = cropRef.current;
    const right = e.clientX > img.left + (shift.current[0] + (c.x0 + c.x1) / 2) * img.width;
    const below = e.clientY > img.top + (shift.current[1] + (c.y0 + c.y1) / 2) * img.height;
    return ROTATE_CURSORS[below ? (right ? 1 : 2) : right ? 0 : 3];
  };

  const onPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!image || e.button !== 0) return;
    const { at, handle } = locate(e);
    e.currentTarget.setPointerCapture(e.pointerId);
    const from = cropRef.current;
    drag.current = { handle, start: at, from, startTurn: turnAt(e, from), shift: shift.current, downAt: [e.clientX, e.clientY] };
    // The picture as it is now, held for the drag: a sharper render of the photo landing meanwhile
    // (the quick one is replaced a moment after the tool opens) would shift its detail by a fraction
    // of a pixel mid-drag, a wobble. The new picture shows on release.
    if (image && image.complete && image.naturalWidth > 0) {
      const copy = document.createElement("canvas");
      copy.width = image.naturalWidth;
      copy.height = image.naturalHeight;
      copy.getContext("2d")?.drawImage(image, 0, 0);
      held.current = copy;
    }
  };

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!image) return;
    const d = drag.current;
    if (!d) {
      const { handle } = locate(e);
      e.currentTarget.style.cursor = handle === "rotate" ? rotateCursorAt(e) : CURSORS[handle];
      return;
    }
    const { from } = d;
    // Measured against where the photo was drawn when the drag began: while sliding, the photo moves with the pointer.
    const at = [d.start[0] + (e.clientX - d.downAt[0]) / image.getBoundingClientRect().width, d.start[1] + (e.clientY - d.downAt[1]) / image.getBoundingClientRect().height];
    if (d.handle === "rotate") {
      e.currentTarget.style.cursor = rotateCursorAt(e);
      let turn = turnAt(e, from) - d.startTurn;
      if (turn > 180) turn -= 360;
      if (turn < -180) turn += 360;
      const angle = Math.max(-MAX_ANGLE, Math.min(MAX_ANGLE, from.angle + turn));
      // Fitted from the frame the drag started with, so it grows back as the photo is turned back.
      const turned = fitCrop({ ...from, angle: Math.round(angle * 10) / 10 }, width, height);
      live(ratio === null ? turned : cropToRatio(turned, ratio, width, height));
      return;
    }
    const dx = at[0] - d.start[0];
    const dy = at[1] - d.start[1];
    const r: Rect = { x0: from.x0, y0: from.y0, x1: from.x1, y1: from.y1 };
    const want = { ...r };
    if (d.handle === "move") {
      // The photo follows the pointer, so the crop moves the other way over it.
      want.x0 -= dx;
      want.x1 -= dx;
      want.y0 -= dy;
      want.y1 -= dy;
    } else {
      if (d.handle.includes("w")) want.x0 = Math.min(r.x0 + dx, r.x1 - MIN_FRACTION);
      if (d.handle.includes("e")) want.x1 = Math.max(r.x1 + dx, r.x0 + MIN_FRACTION);
      if (d.handle.includes("n")) want.y0 = Math.min(r.y0 + dy, r.y1 - MIN_FRACTION);
      if (d.handle.includes("s")) want.y1 = Math.max(r.y1 + dy, r.y0 + MIN_FRACTION);
    }
    if (ratio !== null && d.handle !== "move") {
      // Keep the shape: the height follows the width, from the fixed corner.
      const fw = want.x1 - want.x0;
      const fh = (fw * width) / ratio / height;
      if (d.handle.includes("n")) want.y0 = want.y1 - fh;
      else want.y1 = want.y0 + fh;
      live({ ...from, ...towards(r, want, from.angle, width, height) });
      return;
    }
    // Each axis on its own, so at the photo's edge the frame slides along it rather than sticking.
    const xOnly = towards(r, { ...r, x0: want.x0, x1: want.x1 }, from.angle, width, height);
    const both = towards(xOnly, { ...xOnly, y0: want.y0, y1: want.y1 }, from.angle, width, height);
    live({ ...from, ...both });
  };

  const onPointerUp = () => {
    if (!drag.current) return;
    drag.current = null;
    held.current = null;
    onChange(cropRef.current);
    redraw();
  };

  return (
    <canvas
      ref={canvasRef}
      className="crop-overlay"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      // A drag that ends without its release reaching the canvas (let go outside the window): finished
      // anyway, so the frame follows the crop again (undo, Reset) instead of staying on the drag's.
      onLostPointerCapture={onPointerUp}
    />
  );
}
