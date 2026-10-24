// Colour grading, laid out like Lightroom's Color Grading panel: one colour
// push each for shadows, midtones, highlights and the whole image (global),
// each with its own luminance, plus Blending (how much the three tonal
// ranges overlap) and Balance (where the split between them falls). A
// wheel's puck position is the push - direction = hue, distance from the
// centre = strength.

export type GradeRange = "shadows" | "mids" | "highs" | "global";

export const GRADE_RANGES: GradeRange[] = ["shadows", "mids", "highs", "global"];

/** Puck offset from the wheel centre, each axis -1..1, kept within the unit circle. */
export interface WheelValue {
  x: number;
  y: number;
}

export interface RangeGrade extends WheelValue {
  /** -100..100, Lightroom's per-range Luminance. */
  lum: number;
}

export type Grade = Record<GradeRange, RangeGrade> & {
  /** 0..100, Lightroom's default 50. */
  blending: number;
  /** -100..100; positive moves the split down so more of the image counts as highlights. */
  balance: number;
};

const NEUTRAL_RANGE: RangeGrade = { x: 0, y: 0, lum: 0 };

export const DEFAULT_GRADE: Grade = {
  shadows: NEUTRAL_RANGE,
  mids: NEUTRAL_RANGE,
  highs: NEUTRAL_RANGE,
  global: NEUTRAL_RANGE,
  blending: 50,
  balance: 0,
};

export function isNeutralGrade(g: Grade): boolean {
  // Blending and Balance only shape the ranges, so they change nothing on their own.
  return GRADE_RANGES.every((r) => g[r].x === 0 && g[r].y === 0 && g[r].lum === 0);
}

/** Strongest push at the wheel's rim, as a fraction of full range. */
const MAX_PUSH = 0.15;
/** Brightness shift at Luminance ±100, as a fraction of full range. */
const MAX_LUM = 0.15;

/**
 * Converts a puck position into an additive R/G/B push. Hue 0° (right of
 * the wheel) is red, then round anticlockwise through yellow, green, cyan,
 * blue and magenta - the usual colour-wheel order.
 */
export function wheelToRgb({ x, y }: WheelValue): [number, number, number] {
  const angle = Math.atan2(-y, x);
  const strength = Math.min(1, Math.hypot(x, y)) * MAX_PUSH;
  return [
    strength * Math.cos(angle),
    strength * Math.cos(angle - (2 * Math.PI) / 3),
    strength * Math.cos(angle + (2 * Math.PI) / 3),
  ];
}

/**
 * How much each tonal range applies at a given brightness (0..1); the three
 * always sum to 1. Balance bends brightness before splitting; Blending sets
 * how soft the transitions are (50 = the plain quadratic split, lower =
 * crisper ranges, higher = more overlap).
 */
function rangeWeights(v: number, blending: number, balance: number): Record<"shadows" | "mids" | "highs", number> {
  const b = Math.min(1, Math.max(0, v)) ** 2 ** (-balance / 100);
  const p = 2 * 4 ** ((50 - blending) / 50);
  const s = (1 - b) ** p;
  const h = b ** p;
  const m = 0.5 * (4 * b * (1 - b)) ** (p / 2);
  const sum = s + m + h || 1;
  return { shadows: s / sum, mids: m / sum, highs: h / sum };
}

/**
 * Returns a function adding the grade's colour push and luminance to one
 * channel's value. The preview uses the channel's own value as its
 * brightness, an approximation; the real engine will weight by true luminance.
 */
export function gradeChannel(grade: Grade, channel: 0 | 1 | 2): (v: number) => number {
  const push = (r: GradeRange) => wheelToRgb(grade[r])[channel] + (grade[r].lum / 100) * MAX_LUM;
  const [ps, pm, ph, pg] = GRADE_RANGES.map(push);
  return (v: number) => {
    const w = rangeWeights(v, grade.blending, grade.balance);
    const out = v + w.shadows * ps + w.mids * pm + w.highs * ph + pg;
    return Math.min(1, Math.max(0, out));
  };
}
