/**
 * Crop and straighten (after RapidRetouch's): the photo is turned by
 * `quarter` quarter turns, then by `angle` degrees (positive clockwise) about
 * its centre on a canvas the same size, then x0..x1, y0..y1 (fractions of
 * that canvas) are kept. The engine applies it (crop.rs), so the preview and
 * the export are cut the same way.
 */
export interface Crop {
  quarter: number;
  angle: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

export const NO_CROP: Crop = { quarter: 0, angle: 0, x0: 0, y0: 0, x1: 1, y1: 1 };

export const isNoCrop = (c: Crop) => c.quarter % 4 === 0 && c.angle === 0 && c.x0 === 0 && c.y0 === 0 && c.x1 === 1 && c.y1 === 1;

export type Rect = Pick<Crop, "x0" | "y0" | "x1" | "y1">;

/** A crop that only cuts (no straighten, no quarter turn): a plain window onto the photo. */
export const straightCrop = (c: Crop) => c.angle === 0 && ((c.quarter % 4) + 4) % 4 === 0;

/** Crop shapes; "Original" is the photo's own. */
export const CROP_ASPECTS = ["Free", "Original", "1:1", "4:5", "5:4", "2:3", "3:2", "16:9"];

/** Width / height for a crop shape, or null for free. */
export function aspectRatio(aspect: string, width: number, height: number): number | null {
  if (aspect === "Free") return null;
  if (aspect === "Original") return width / height;
  const [w, h] = aspect.split(":").map(Number);
  return w / h;
}

/** Where a point of the turned canvas (fractions) came from in the photo
 * (fractions), for a photo of w x h pixels. */
function source(x: number, y: number, angle: number, w: number, h: number): [number, number] {
  const t = (-angle * Math.PI) / 180;
  const dx = (x - 0.5) * w;
  const dy = (y - 0.5) * h;
  return [0.5 + (dx * Math.cos(t) - dy * Math.sin(t)) / w, 0.5 + (dx * Math.sin(t) + dy * Math.cos(t)) / h];
}

const EPS = 1e-6;
/** True if every corner of the rectangle lies on the photo once it is turned. */
export function inside(r: Rect, angle: number, w: number, h: number): boolean {
  for (const [x, y] of [
    [r.x0, r.y0],
    [r.x1, r.y0],
    [r.x0, r.y1],
    [r.x1, r.y1],
  ]) {
    const [sx, sy] = source(x, y, angle, w, h);
    if (sx < -EPS || sx > 1 + EPS || sy < -EPS || sy > 1 + EPS) return false;
  }
  return true;
}

const lerp = (a: Rect, b: Rect, t: number): Rect => ({
  x0: a.x0 + (b.x0 - a.x0) * t,
  y0: a.y0 + (b.y0 - a.y0) * t,
  x1: a.x1 + (b.x1 - a.x1) * t,
  y1: a.y1 + (b.y1 - a.y1) * t,
});

/** The furthest point from `from` toward `to` that stays on the photo (`from` must be on it). */
export function towards(from: Rect, to: Rect, angle: number, w: number, h: number): Rect {
  if (inside(to, angle, w, h)) return to;
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 20; i++) {
    const mid = (lo + hi) / 2;
    if (inside(lerp(from, to, mid), angle, w, h)) lo = mid;
    else hi = mid;
  }
  return lerp(from, to, lo);
}

/** Shrink the crop toward the photo's centre, keeping its shape, until it fits the turned photo. */
export function fitCrop(c: Crop, w: number, h: number): Crop {
  const r: Rect = { x0: c.x0, y0: c.y0, x1: c.x1, y1: c.y1 };
  if (inside(r, c.angle, w, h)) return c;
  const hw = (r.x1 - r.x0) * 0.001;
  const hh = (r.y1 - r.y0) * 0.001;
  const tiny: Rect = { x0: 0.5 - hw, y0: 0.5 - hh, x1: 0.5 + hw, y1: 0.5 + hh };
  return { ...c, ...towards(tiny, r, c.angle, w, h) };
}

/** The largest crop of `ratio` (width / height, in pixels) centred on the current one that fits the turned photo. */
export function cropToRatio(c: Crop, ratio: number, w: number, h: number): Crop {
  const cx = (c.x0 + c.x1) / 2;
  const cy = (c.y0 + c.y1) / 2;
  let pw = w;
  let ph = w / ratio;
  if (ph > h) {
    ph = h;
    pw = h * ratio;
  }
  const fw = pw / w / 2;
  const fh = ph / h / 2;
  return fitCrop({ ...c, x0: cx - fw, y0: cy - fh, x1: cx + fw, y1: cy + fh }, w, h);
}

/**
 * The corners of the part `c` keeps, on the whole unturned photo (fractions, clockwise from top
 * left), for a photo `aspect` wide to 1 high: what Masks, Patch and Erase, which show the whole
 * photo, leave clear of their veil. The engine's geometry (crop.rs) run backwards: canvas corner,
 * back through the straighten, then back through the quarter turns. Keystone isn't followed (those
 * tools show the photo without it).
 */
export function cropOutline(c: Crop, aspect: number): [number, number][] {
  const q = ((c.quarter % 4) + 4) % 4;
  // The turned photo's sides.
  const [w, h] = q % 2 === 1 ? [1, aspect] : [aspect, 1];
  const t = (-c.angle * Math.PI) / 180;
  const [sn, cs] = [Math.sin(t), Math.cos(t)];
  const corners: [number, number][] = [
    [c.x0, c.y0],
    [c.x1, c.y0],
    [c.x1, c.y1],
    [c.x0, c.y1],
  ];
  return corners.map(([fx, fy]) => {
    const x = (fx - 0.5) * w;
    const y = (fy - 0.5) * h;
    // On the turned photo (fractions).
    const u = (x * cs - y * sn + w / 2) / w;
    const v = (x * sn + y * cs + h / 2) / h;
    // Back through the quarter turns (clockwise, as image::imageops::rotate90).
    if (q === 1) return [v, 1 - u];
    if (q === 2) return [1 - u, 1 - v];
    if (q === 3) return [1 - v, u];
    return [u, v];
  });
}
