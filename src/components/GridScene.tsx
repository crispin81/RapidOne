import { useEffect, useRef, useState, type CSSProperties } from "react";
import storr from "../assets/backdrop/storr.jpg";

/**
 * Backdrop for the empty viewer: a grid corridor (floor, ceiling and walls)
 * whose lines run in perspective to a picture in the middle, which fills half
 * the width and height of the viewer, behind the terminal prompt. The picture
 * is Chris's photo of the Old Man of Storr, shown like an old monitor: fine
 * scanlines and a slow bright band rolling down.
 *
 * The grid is drawn in a 0-100 box stretched to the viewer, so the picture's
 * frame is always the middle half however the window is shaped.
 */

/** The picture's frame, in the 0-100 box: the middle half. */
const INNER = 25;
/** Lines running towards the picture, per side. */
const RAYS = 10;
/** Cross lines (frames) between the window edge and the picture. */
const RINGS = 7;

function ringFraction(k: number): number {
  // Spaced in perspective: crowding together as they near the picture.
  const f = (n: number) => 1 - 1 / (1 + n * 0.55);
  return f(k) / f(RINGS);
}

/**
 * The editor's photo sits in the corridor's outermost frame (the closest to
 * you), inset this far (0-100 box) from the viewer's edges.
 */
export const PHOTO_RING = INNER * ringFraction(1);

/** A rectangle in the 0-100 box: left, top, right, bottom. */
export interface Box {
  l: number;
  t: number;
  r: number;
  b: number;
}

const MIDDLE_HALF: Box = { l: INNER, t: INNER, r: 100 - INNER, b: 100 - INNER };

/**
 * The corridor's lines (SVG path, 0-100 box): floor, ceiling, walls and the
 * frames between, running from the box's edges to `end` (the picture at its
 * far end): the middle half on the home screen, the photo's own rectangle in
 * the editor, so the corridor takes each photo's shape.
 */
export function corridorPath(end: Box = MIDDLE_HALF): string {
  const { l, t, r, b } = end;
  const lines: string[] = [];
  for (let i = 0; i <= RAYS; i++) {
    const f = i / RAYS;
    const x = l + f * (r - l);
    const y = t + f * (b - t);
    lines.push(`M${f * 100} 100L${x} ${b}`); // floor
    lines.push(`M${f * 100} 0L${x} ${t}`); // ceiling
    lines.push(`M0 ${f * 100}L${l} ${y}`); // left wall
    lines.push(`M100 ${f * 100}L${r} ${y}`); // right wall
  }
  for (let k = 1; k < RINGS; k++) {
    const g = ringFraction(k);
    const [L, T, R, B] = [l * g, t * g, 100 - (100 - r) * g, 100 - (100 - b) * g];
    lines.push(`M${L} ${T}H${R}V${B}H${L}Z`);
  }
  return lines.join("");
}

/**
 * Light fall-off on the corridor lines: full near the picture, fading towards
 * the edges, following its rectangle (a fade across and a fade down, kept
 * where both are).
 */
export function corridorFalloff(p: Box): CSSProperties {
  const edge = "rgba(0,0,0,0.35)";
  const across = `linear-gradient(to right, ${edge}, black ${p.l}%, black ${p.r}%, ${edge})`;
  const down = `linear-gradient(to bottom, ${edge}, black ${p.t}%, black ${p.b}%, ${edge})`;
  const mask = `${across}, ${down}`;
  return { WebkitMaskImage: mask, maskImage: mask, WebkitMaskComposite: "source-in", maskComposite: "intersect" };
}

/**
 * `image`: the picture to show (the photo being edited on the home screen);
 * Storr by default. `bottomInset`: the slider panel's height docked over the
 * bottom; the corridor runs on under it, the picture stays above it.
 */
export default function GridScene({ image, bottomInset = 0 }: { image?: string; bottomInset?: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 1200, h: 800 });
  // The picture on screen and its shape (width / height). A new picture goes up only once it's
  // decoded, its shape with it: the previous one stays until then, so neither the frame jumps nor
  // (in WebKitGTK's GPU drawing) the picture shows empty for a frame.
  const [shown, setShown] = useState<string | null>(null);
  const [aspect, setAspect] = useState<number | null>(null);
  const wanted = image ?? storr;
  useEffect(() => {
    let live = true;
    const im = new Image();
    im.src = wanted;
    im.decode()
      .catch(() => {})
      .then(() => {
        if (!live) return;
        setShown(wanted);
        if (im.naturalWidth > 0 && im.naturalHeight > 0) setAspect(im.naturalWidth / im.naturalHeight);
      });
    return () => {
      live = false;
    };
  }, [wanted]);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth || 1, h: el.clientHeight || 1 }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  // The middle half of the area above the panel, in the 0-100 box...
  const above = Math.max(0.2, 1 - bottomInset / size.h);
  const middle: Box = { l: INNER, r: 100 - INNER, t: INNER * above, b: (100 - INNER) * above };
  // ...and the picture's own shape fitted inside it, so a portrait or a
  // panorama isn't cropped, and the corridor runs to its edges (as in the editor).
  const end = aspect ? fitBox(middle, aspect, size.w, size.h) : middle;
  return (
    <div className="scene" aria-hidden="true" ref={ref}>
      <svg className="scene__grid" viewBox="0 0 100 100" preserveAspectRatio="none" style={corridorFalloff(end)}>
        <path d={corridorPath(end)} vectorEffect="non-scaling-stroke" />
      </svg>
      <div
        className="scene__picture"
        style={{ left: `${end.l}%`, top: `${end.t}%`, width: `${end.r - end.l}%`, height: `${end.b - end.t}%` }}
      >
        {shown && <img src={shown} alt="" draggable={false} style={shown !== storr ? { objectPosition: "center" } : undefined} />}
      </div>
    </div>
  );
}

/** `area` (0-100 box) with a rectangle of this width / height fitted inside it, centred. */
function fitBox(area: Box, aspect: number, width: number, height: number): Box {
  const bw = ((area.r - area.l) / 100) * width;
  const bh = ((area.b - area.t) / 100) * height;
  const [w, h] = bw / bh > aspect ? [bh * aspect, bh] : [bw, bw / aspect];
  const [halfW, halfH] = [(w / width) * 50, (h / height) * 50];
  const [cx, cy] = [(area.l + area.r) / 2, (area.t + area.b) / 2];
  return { l: cx - halfW, r: cx + halfW, t: cy - halfH, b: cy + halfH };
}
