import type { SliderDef } from "./adjustments";

/**
 * Linear and radial gradients and subject masks (local adjustments), as the
 * engine takes them (`gradient.rs`). Positions are fractions of the upright,
 * uncropped photo. A subject mask has no position: each photo's own subject
 * is found for it, so it pastes onto other photos as a recipe.
 */
export type GradientKind = "linear" | "radial" | "subject" | "brush";

/** One stroke of a brush mask: its path (fractions of the photo), radius (a fraction of the
 *  photo's width), 0..100 feather, and whether it erases. */
export interface Stroke {
  points: [number, number][];
  size: number;
  feather: number;
  erase: boolean;
}

/** A part of a combined mask: a shape added to the mask, or subtracted from it. */
export interface MaskPart {
  subtract: boolean;
  shape: Gradient;
}

/** Whether a mask has a subject in it (its own shape, or one of its parts). */
export function hasSubject(g: Gradient): boolean {
  return g.kind === "subject" || Boolean(g.parts?.some((p) => p.shape.kind === "subject"));
}

/** A new brush's size (radius, a fraction of the photo's width) and feather. */
export const BRUSH_SIZE = 0.03;
export const BRUSH_FEATHER = 50;

/** A subject mask's Feather to start with: a soft edge like Lightroom's Select Subject. */
export const SUBJECT_FEATHER = 25;

export interface Gradient {
  kind: GradientKind;
  /** Linear: full effect at `full`, none at `zero`. */
  full: [number, number];
  zero: [number, number];
  /** Its effect switched off (the layer's eye), the mask kept. */
  hidden?: boolean;
  /** Radial: centre, radii (fractions of the width / height), degrees clockwise, 0..100 feather, effect outside.
   *  Subject: `feather` softens the mask's edge, `invert` puts the effect on the background. */
  centre: [number, number];
  rx: number;
  ry: number;
  angle: number;
  feather: number;
  invert: boolean;
  /** Brush: the strokes painted, in order. */
  strokes?: Stroke[];
  /** Other shapes added to or subtracted from this one (a combined mask, as Lightroom's Add / Subtract). */
  parts?: MaskPart[];
  /** Subject: the subject mask as found on the photo it was copied from (PNG, base64), used as it is:
   *  a combined mask with a subject in it is pasted as a fixed shape, not found again per photo. */
  frozen?: string;
  /** Stops. */
  exposure: number;
  /** -100..100 each. */
  temperature: number;
  tint: number;
  contrast: number;
  highlights: number;
  shadows: number;
  /** -100..100 each, as the global sliders. */
  whites: number;
  blacks: number;
  texture: number;
  clarity: number;
  dehaze: number;
  saturation: number;
}

/** A new gradient drawn from `a` to `b` (fractions of the photo), no adjustment yet. */
export function newGradient(kind: GradientKind, a: { x: number; y: number }, b: { x: number; y: number }): Gradient {
  const base: Gradient = {
    kind,
    full: [a.x, a.y],
    zero: [b.x, b.y],
    centre: [a.x, a.y],
    rx: 0.01,
    ry: 0.01,
    angle: 0,
    feather: 50,
    invert: false,
    exposure: 0,
    temperature: 0,
    tint: 0,
    contrast: 0,
    highlights: 0,
    shadows: 0,
    whites: 0,
    blacks: 0,
    texture: 0,
    clarity: 0,
    dehaze: 0,
    saturation: 0,
  };
  if (kind === "subject") base.feather = SUBJECT_FEATHER;
  if (kind === "radial") {
    base.rx = Math.max(0.01, Math.abs(b.x - a.x));
    base.ry = Math.max(0.01, Math.abs(b.y - a.y));
  }
  return base;
}

/**
 * The bottom panel's sliders a mask uses while a mask tool is open: the same sliders, set
 * on the picked mask (from 0) instead of the photo. Temperature and Tint are -100..100 there
 * (warmer / cooler, magenta / green), not Kelvin. The others are greyed out.
 */
export const MASK_SLIDER_IDS = ["exposure", "temperature", "tint", "contrast", "highlights", "shadows", "whites", "blacks", "texture", "clarity", "dehaze", "saturation"] as const;
export type MaskSliderId = (typeof MASK_SLIDER_IDS)[number];

export function isMaskSlider(id: string): id is MaskSliderId {
  return (MASK_SLIDER_IDS as readonly string[]).includes(id);
}

/** A bottom-panel slider as a mask uses it, or null if masks don't have it. */
export function maskSliderDef(def: SliderDef): SliderDef | null {
  if (!isMaskSlider(def.id)) return null;
  if (def.id === "exposure") return { ...def, min: -4, max: 4, neutral: 0 };
  if (def.id === "temperature" || def.id === "tint") {
    return { ...def, label: def.id === "temperature" ? "Temp" : def.label, tooltip: undefined, min: -100, max: 100, step: 1, coarseStep: 5, decimals: 0, neutral: 0, scale: undefined };
  }
  return { ...def, neutral: 0 };
}
