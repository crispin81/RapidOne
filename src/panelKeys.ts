import { keyLabel } from "./macKeys";
import { SLIDER_GROUPS, sliderInPanel } from "./adjustments";
import type { CurveChannel } from "./curves";
import type { GradeRange } from "./grade";
import type { MixerColour } from "./mixer";

/**
 * Keyboard shortcuts inside each panel, one rule throughout: with a panel
 * selected (its letter tapped), **tap** a letter to pick something inside it
 * (a colour band, a grading range, a curve channel), **hold** a letter and
 * use the arrows or scroll wheel to adjust something. A quick tap of a letter
 * that's only a hold here goes to the panel with that letter instead.
 */

export type Primary = "Red" | "Green" | "Blue";
export type CurvesTab = CurveChannel | "region";

/** What's picked inside the panels (lifted to App so the keys can change it). */
export interface PanelPicks {
  mixerBand: MixerColour;
  gradeRange: GradeRange;
  calPrimary: Primary;
  curvesTab: CurvesTab;
  /** Effects shows the legacy (Lightroom) vignette's sliders in place of the adaptive one. */
  legacyVignette?: boolean;
}

export const DEFAULT_PICKS: PanelPicks = { mixerBand: "orange", gradeRange: "mids", calPrimary: "Red", curvesTab: "luma" };

/**
 * Adjustable things that aren't plain sliders, held by these ids (VSliders
 * show them active): the Mixer's selected band, and Grade's selected range
 * (hue, saturation and luminance) plus Blending and Pivot.
 */
export const MIXER_HOLD: Record<string, "hue" | "sat" | "lum"> = { mixerHue: "hue", mixerSat: "sat", mixerLum: "lum" };
export const GRADE_HOLDS = ["gradeHue", "gradeSat", "gradeLum", "gradeBlending", "gradeBalance"] as const;
export type GradeHold = (typeof GRADE_HOLDS)[number];

const key = (letter: string) => `Key${letter}`;

/** What holding `code` adjusts in `panel` (a slider id or one of the ids above), if anything. */
export function holdTarget(panel: string, code: string, picks: PanelPicks): string | undefined {
  const letter = code.replace(/^Key/, "");
  switch (panel) {
    case "Mixer":
      return { H: "mixerHue", S: "mixerSat", L: "mixerLum" }[letter];
    case "Grade":
      return { H: "gradeHue", S: "gradeSat", L: "gradeLum", B: "gradeBlending", P: "gradeBalance" }[letter];
    case "Fine tuning":
      return { T: "calShadowTint", H: `cal${picks.calPrimary}Hue`, S: `cal${picks.calPrimary}Sat` }[letter];
    case "Curves":
      return picks.curvesTab === "region"
        ? { H: "paramHighlights", L: "paramLights", D: "paramDarks", S: "paramShadows" }[letter]
        : undefined;
    default:
      return sliderInPanel(panel, code)?.id;
  }
}

const MIXER_TAPS: Record<string, MixerColour> = {
  R: "red", O: "orange", Y: "yellow", G: "green", A: "aqua", B: "blue", P: "purple", M: "magenta",
};
const GRADE_TAPS: Record<string, GradeRange> = { G: "global", H: "highs", M: "mids", S: "shadows" };
const CAL_TAPS: Record<string, Primary> = { R: "Red", G: "Green", B: "Blue" };
const CURVE_TAPS: Record<string, CurvesTab> = { L: "luma", A: "rgb", R: "r", G: "g", B: "b", P: "region" };

/** What tapping `code` picks in `panel`, if anything. */
export function tapPick(panel: string, code: string): Partial<PanelPicks> | undefined {
  const letter = code.replace(/^Key/, "");
  switch (panel) {
    case "Mixer":
      return MIXER_TAPS[letter] && { mixerBand: MIXER_TAPS[letter] };
    case "Grade":
      return GRADE_TAPS[letter] && { gradeRange: GRADE_TAPS[letter] };
    case "Fine tuning":
      return CAL_TAPS[letter] && { calPrimary: CAL_TAPS[letter] };
    case "Curves":
      return CURVE_TAPS[letter] && { curvesTab: CURVE_TAPS[letter] };
    default:
      return undefined;
  }
}

/** The letter shown for a pick (titles and hints), e.g. the Mixer's "Aqua (A)". */
export const TAP_LETTERS = { mixer: MIXER_TAPS, grade: GRADE_TAPS, cal: CAL_TAPS, curves: CURVE_TAPS };
export { key as keyCode };

// ---- Hover text: every control says its shortcut.

/** "Shift+M (Mixer)": selects a panel. */
export function panelHint(panel: string): string {
  return `Shift+${panel[0]} (${panel})`;
}

/** How to adjust a control from the keyboard: select its panel, then hold its letter. */
export function adjustHint(panel: string, letter: string, via?: string): string {
  return keyLabel(`Keys: ${panelHint(panel)}${via ? `, ${via}` : ""}, then hold ${letter} with ↑/↓ or scroll · Ctrl: bigger steps · Backspace: reset`);
}

/** How to pick something (a band, range, channel) from the keyboard. */
export function pickHint(panel: string, letter: string): string {
  return `Keys: ${panelHint(panel)}, then tap ${letter}`;
}

/** The panel a slider's letter works in (its group), for its hover text. */
export function sliderPanel(id: string): string | undefined {
  return SLIDER_GROUPS.find((g) => g.sliders.some((d) => d.id === id))?.name;
}
