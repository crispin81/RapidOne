// Colour mixer (HSL): hue, saturation and luminance shifts per colour band.
// The panel shows one band at a time - pick a colour dot, then use the three
// sliders - so 24 controls fit in the width of three.

import type { SliderDef } from "./adjustments";

export type MixerColour = "red" | "orange" | "yellow" | "green" | "aqua" | "blue" | "purple" | "magenta";

export interface MixerColourInfo {
  id: MixerColour;
  label: string;
  /** Dot colour in the panel. */
  swatch: string;
  /** Centre of the band on the hue circle, in degrees - used by the real engine. */
  centreDeg: number;
}

// Same eight bands, in the same order, as Lightroom's colour mixer.
export const MIXER_COLOURS: MixerColourInfo[] = [
  { id: "red", label: "Red", swatch: "#e0474a", centreDeg: 0 },
  { id: "orange", label: "Orange", swatch: "#ec8a3a", centreDeg: 30 },
  { id: "yellow", label: "Yellow", swatch: "#e6cf3e", centreDeg: 60 },
  { id: "green", label: "Green", swatch: "#4fbf5a", centreDeg: 120 },
  { id: "aqua", label: "Aqua", swatch: "#3fc4c4", centreDeg: 180 },
  { id: "blue", label: "Blue", swatch: "#3d6fe0", centreDeg: 225 },
  { id: "purple", label: "Purple", swatch: "#8a52d6", centreDeg: 270 },
  { id: "magenta", label: "Magenta", swatch: "#d94bb3", centreDeg: 315 },
];

export interface BandShift {
  hue: number;
  sat: number;
  lum: number;
}

export type Mixer = Record<MixerColour, BandShift>;

const ZERO: BandShift = { hue: 0, sat: 0, lum: 0 };

export const DEFAULT_MIXER: Mixer = Object.fromEntries(MIXER_COLOURS.map((c) => [c.id, ZERO])) as Mixer;

/**
 * A band's Hue track, as the white balance sliders show theirs: the colour it
 * turns towards at the bottom (lowered: the band before it round the hue
 * circle, e.g. red to magenta), its own colour in the middle, and the colour it
 * turns towards at the top (raised: the next band, e.g. red to orange).
 */
export function hueTrack(band: MixerColour): string {
  const i = MIXER_COLOURS.findIndex((c) => c.id === band);
  const n = MIXER_COLOURS.length;
  const [down, own, up] = [MIXER_COLOURS[(i + n - 1) % n], MIXER_COLOURS[i], MIXER_COLOURS[(i + 1) % n]];
  return `linear-gradient(to top, ${down.swatch}, ${own.swatch}, ${up.swatch})`;
}

export function isBandChanged(b: BandShift): boolean {
  return b.hue !== 0 || b.sat !== 0 || b.lum !== 0;
}

function mixerSlider(id: keyof BandShift, label: string): SliderDef {
  // No keyboard shortcut yet (empty code); same steps as the other sliders.
  return { id, label, code: "", modifier: "none", min: -100, max: 100, step: 1, coarseStep: 5, decimals: 0, neutral: 0 };
}

export const MIXER_SLIDERS: SliderDef[] = [
  mixerSlider("hue", "Hue"),
  mixerSlider("sat", "Saturation"),
  mixerSlider("lum", "Luminance"),
];
