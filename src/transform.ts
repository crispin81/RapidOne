// Turning the Transform sliders into a preview transform, and the auto
// buttons' measurements into slider values.

import type { Adjustments } from "./adjustments";

/** Vertical/Horizontal slider units are tilt: ±100 = ±40 degrees. */
export const TILT_DEG_PER_UNIT = 0.4;

export interface AutoTransform {
  /** The turn from the verticals (keystone's). */
  rotateDeg: number;
  /** Level's turn: verticals and horizontals together, the horizontals counting double (Lightroom's Level). */
  rotateLevelDeg: number;
  verticalTiltDeg: number | null;
  lineCount: number;
  /** The Scale (percent) that crops out the white once levelled, or levelled and keystoned. */
  coverScaleLevel: number;
  coverScaleKeystone: number;
}

/**
 * CSS transform for the preview. Read right to left: scale, level the
 * rotation, then undo the camera tilt in that levelled frame, viewed with a
 * perspective equal to the image's long edge - the same focal-length
 * assumption the auto measurement makes, so auto results straighten lines
 * exactly.
 */
export function previewTransform(v: Adjustments, longEdgePx: number): string | undefined {
  const { rotate, vertical, horizontal, scale } = v;
  if (rotate === 0 && vertical === 0 && horizontal === 0 && scale === 100) return undefined;
  // Tilted up (positive) means the top needs bringing toward the viewer,
  // which is a negative rotateX in CSS.
  const tiltX = -vertical * TILT_DEG_PER_UNIT;
  const tiltY = -horizontal * TILT_DEG_PER_UNIT;
  return `perspective(${Math.max(1, longEdgePx)}px) rotateX(${tiltX}deg) rotateY(${tiltY}deg) rotate(${rotate}deg) scale(${scale / 100})`;
}

/** Slider value for a measured camera tilt, clamped to the slider's range. */
export function tiltToVertical(tiltDeg: number): number {
  return Math.round(Math.max(-100, Math.min(100, tiltDeg / TILT_DEG_PER_UNIT)));
}

export function roundRotate(deg: number): number {
  return Math.round(Math.max(-45, Math.min(45, deg)) * 10) / 10;
}
