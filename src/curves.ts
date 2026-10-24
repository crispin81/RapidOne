// Tone curve data and maths, shared by the Curves panel and the preview.
// Points are in 0..1 on both axes. Every curve keeps exactly two end points,
// and they move freely, which is what lets curves replace Levels: dragging
// the black end point right is "input black", the white end point left is
// "input white", and moving them vertically sets the output levels. Inputs
// beyond an end point clamp to that end point's output.

export interface CurvePoint {
  x: number;
  y: number;
}

/** "luma" changes brightness only (colour preserved); "rgb" applies one curve to all three channels. */
export type CurveChannel = "luma" | "rgb" | "r" | "g" | "b";

export type Curves = Record<CurveChannel, CurvePoint[]>;

export const CURVE_CHANNELS: CurveChannel[] = ["luma", "rgb", "r", "g", "b"];

/** A straight curve with ready-made points for shadows, mids and highlights,
 * each at the centre of its square in the panel's 3x3 grid (the 1, 5 and 9
 * keys, laid out like the number pad). */
const IDENTITY: CurvePoint[] = [
  { x: 0, y: 0 },
  { x: 1 / 6, y: 1 / 6 },
  { x: 1 / 2, y: 1 / 2 },
  { x: 5 / 6, y: 5 / 6 },
  { x: 1, y: 1 },
];

export const DEFAULT_CURVES: Curves = { luma: IDENTITY, rgb: IDENTITY, r: IDENTITY, g: IDENTITY, b: IDENTITY };

/** Smallest sideways gap between neighbouring points, so a curve never folds back. */
export const MIN_GAP = 0.01;

/**
 * The keyboard shortcut's move: for square 0 (shadows, key 1), 1 (mids, 5) or
 * 2 (highlights, 9), the interior point in that third nearest the square's
 * default point is nudged by `step` in the arrow's direction (kept between its
 * neighbours and inside 0..1). If that third has no point, one is added on
 * the curve first.
 */
export function nudgePoint(points: CurvePoint[], square: 0 | 1 | 2, arrow: string, step: number): CurvePoint[] {
  const centre = [1 / 6, 1 / 2, 5 / 6][square];
  let pts = [...points];
  let index = -1;
  let best = Infinity;
  pts.forEach((p, i) => {
    if (i === 0 || i === pts.length - 1 || p.x < square / 3 || p.x > (square + 1) / 3) return;
    const d = Math.abs(p.x - centre);
    if (d < best) {
      best = d;
      index = i;
    }
  });
  if (index === -1) {
    const added = { x: centre, y: curveEvaluator(pts)(centre) };
    pts = [...pts, added].sort((a, b) => a.x - b.x);
    index = pts.indexOf(added);
  }
  let { x, y } = pts[index];
  if (arrow === "ArrowUp") y += step;
  else if (arrow === "ArrowDown") y -= step;
  else if (arrow === "ArrowLeft") x -= step;
  else if (arrow === "ArrowRight") x += step;
  const lo = pts[index - 1].x + MIN_GAP;
  const hi = pts[index + 1].x - MIN_GAP;
  pts[index] = { x: Math.min(hi, Math.max(lo, x)), y: Math.min(1, Math.max(0, y)) };
  return pts;
}

/** Straight: every point on the diagonal, end points at the corners. */
export function isIdentity(points: CurvePoint[]): boolean {
  const first = points[0];
  const last = points[points.length - 1];
  return (
    first.x === 0 && first.y === 0 && last.x === 1 && last.y === 1 && points.every((p) => Math.abs(p.x - p.y) < 1e-6)
  );
}

/**
 * Builds an evaluator for a monotone cubic (Fritsch-Carlson) through the
 * points. Monotone, unlike a plain spline, means the curve never overshoots
 * between points - no surprise tone reversals or clipping bumps.
 */
export function curveEvaluator(points: CurvePoint[]): (x: number) => number {
  const pts = [...points].sort((a, b) => a.x - b.x);
  const n = pts.length;
  const dx: number[] = [];
  const slope: number[] = [];
  for (let i = 0; i < n - 1; i++) {
    dx.push(pts[i + 1].x - pts[i].x);
    slope.push((pts[i + 1].y - pts[i].y) / (dx[i] || 1e-9));
  }

  const tangent: number[] = new Array(n);
  tangent[0] = slope[0];
  tangent[n - 1] = slope[n - 2];
  for (let i = 1; i < n - 1; i++) {
    tangent[i] = slope[i - 1] * slope[i] <= 0 ? 0 : (slope[i - 1] + slope[i]) / 2;
  }
  for (let i = 0; i < n - 1; i++) {
    if (slope[i] === 0) {
      tangent[i] = 0;
      tangent[i + 1] = 0;
      continue;
    }
    const a = tangent[i] / slope[i];
    const b = tangent[i + 1] / slope[i];
    const h = a * a + b * b;
    if (h > 9) {
      const t = 3 / Math.sqrt(h);
      tangent[i] = t * a * slope[i];
      tangent[i + 1] = t * b * slope[i];
    }
  }

  return (x: number) => {
    if (x <= pts[0].x) return pts[0].y;
    if (x >= pts[n - 1].x) return pts[n - 1].y;
    let i = 0;
    while (x > pts[i + 1].x) i++;
    const h = dx[i];
    const t = (x - pts[i].x) / h;
    const t2 = t * t;
    const t3 = t2 * t;
    const y =
      (2 * t3 - 3 * t2 + 1) * pts[i].y +
      (t3 - 2 * t2 + t) * h * tangent[i] +
      (-2 * t3 + 3 * t2) * pts[i + 1].y +
      (t3 - t2) * h * tangent[i + 1];
    return Math.min(1, Math.max(0, y));
  };
}

/** Samples a curve into `size` evenly spaced output values. */
export function sampleCurve(points: CurvePoint[], size = 64): number[] {
  const f = curveEvaluator(points);
  return Array.from({ length: size }, (_, i) => f(i / (size - 1)));
}

/**
 * Per-channel lookup for the rough preview: luminosity, then RGB, then the
 * channel's own curve. A per-channel table can't keep colour fixed the way a
 * true luminosity curve does, so the preview applies the luminosity curve as
 * if it were RGB. The real engine will scale colour by the luminance change.
 */
export function channelTable(curves: Curves, channel: "r" | "g" | "b", size = 64): number[] {
  const luma = curveEvaluator(curves.luma);
  const master = curveEvaluator(curves.rgb);
  const own = curveEvaluator(curves[channel]);
  return Array.from({ length: size }, (_, i) => own(master(luma(i / (size - 1)))));
}

/** The per-channel curve as a function (luminosity, then RGB, then the channel's own), for composing with other steps. */
export function channelFunction(curves: Curves, channel: "r" | "g" | "b"): (x: number) => number {
  const luma = curveEvaluator(curves.luma);
  const master = curveEvaluator(curves.rgb);
  const own = curveEvaluator(curves[channel]);
  return (x: number) => own(master(luma(x)));
}

/**
 * The parametric (region) curve's shape, for drawing: amounts are Shadows,
 * Darks, Lights, Highlights (-100..100), splits in 0..100. PROVISIONAL and
 * kept identical to the engine's `tools::parametric_shape` until Lightroom's
 * measured curve replaces both.
 */
export function parametricShape(amounts: [number, number, number, number], splits: [number, number, number]): (x: number) => number {
  const [s1, m, s3] = splits.map((v) => Math.min(0.95, Math.max(0.05, v / 100)));
  const bump = (x: number, a: number, peak: number, b: number) => {
    if (x <= a || x >= b) return 0;
    const t = x < peak ? (0.5 * (x - a)) / Math.max(1e-4, peak - a) : 0.5 + (0.5 * (x - peak)) / Math.max(1e-4, b - peak);
    return Math.sin(Math.PI * t) ** 2;
  };
  const regions: [number, number, number, number][] = [
    [0, s1, m, 0.15],
    [0, 0.5 * (s1 + m), s3, 0.2],
    [s1, 0.5 * (m + s3), 1, 0.2],
    [m, s3, 1, 0.15],
  ];
  return (x) => {
    let y = x;
    regions.forEach(([a, peak, b, gain], k) => {
      y += (amounts[k] / 100) * gain * bump(x, a, peak, b);
    });
    return Math.min(1, Math.max(0, y));
  };
}
