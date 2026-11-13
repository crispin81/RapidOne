// Every bottom-panel slider and its hold-key shortcut, in one place so the
// key scheme (still being decided - see RAPIDONE_BRIEF.md section 4) can be
// changed without touching any component.
//
// Shortcut rule so far: the first slider on a letter gets the plain letter,
// the second gets Shift+letter. A third uses Alt+letter as a placeholder
// until the scheme is settled.

export type Modifier = "none" | "shift" | "alt";

export interface SliderDef {
  id: string;
  label: string;
  /** `KeyboardEvent.code`, e.g. "KeyE" - layout-independent and unaffected by Shift. */
  code: string;
  modifier: Modifier;
  min: number;
  max: number;
  /**
   * The values it can take, when narrower than the track (`min`..`max`): the track then runs past
   * them, blocked, so it lines up with its neighbours (Noise beside Sharpen, which goes below 0).
   */
  lowest?: number;
  highest?: number;
  /** Track positions per unit of value (default 1): Noise's 0..100 spread over Sharpen's 0..150 half. */
  stretch?: number;
  /** Change per held-key + ↑/↓ press and per scroll-wheel notch: the small, everyday step. */
  step: number;
  /** Change per held-key + Ctrl+↑/↓ press: the big jump. Values snap to multiples of it. */
  coarseStep: number;
  /** Decimal places shown in the value readout. */
  decimals: number;
  /** Colour gradient drawn on the track, bottom to top (white balance sliders). */
  trackGradient?: string;
  /**
   * How values are spread along the track. "mired" (for Kelvin) spaces them by
   * 1/K, as Lightroom does, so equal movements look like equal changes and
   * 3000-8000 K gets most of the track.
   */
  scale?: "linear" | "mired";
  /** Hover text, when it should say more than the label ("Kelvin (Temperature)"). */
  tooltip?: string;
  /** Value meaning "unchanged" (0 for most; 100 for Scale). */
  neutral: number;
}

export interface SliderGroup {
  name: string;
  sliders: SliderDef[];
}

function s(
  id: string,
  label: string,
  letter: string,
  modifier: Modifier = "none",
  range: Partial<
    Pick<
      SliderDef,
      "min" | "max" | "lowest" | "highest" | "stretch" | "step" | "coarseStep" | "decimals" | "neutral" | "trackGradient" | "scale" | "tooltip"
    >
  > = {},
): SliderDef {
  return {
    id,
    label,
    // An empty letter means no keyboard shortcut (mouse/touch only).
    code: letter ? `Key${letter}` : "",
    modifier,
    min: -100,
    max: 100,
    step: 1,
    coarseStep: 5,
    decimals: 0,
    neutral: 0,
    ...range,
  };
}

// The on-screen order (Lightroom's, mostly) is set by PANEL_ORDER in
// BottomPanel, which also places the Curves, Mixer and Grade panels.
export const SLIDER_GROUPS: SliderGroup[] = [
  {
    name: "W/B",
    sliders: [
      // Same colour scales as RapidRetouch, turned vertical: blue (cooler) at the bottom to amber (warmer) at the top.
      // Kelvin, Lightroom's RAW range. Higher = warmer picture (the light is
      // assumed bluer). The neutral is per photo: its as-shot value.
      // Named Kelvin (what it shows) so it doesn't share a letter with Tint.
      s("temperature", "Kelvin", "K", "none", {
        tooltip: "Kelvin (Temperature)",
        min: 2000,
        max: 50000,
        step: 50,
        coarseStep: 500,
        scale: "mired",
        trackGradient: "linear-gradient(to top, #4a86d8, #b9c3cf, #e0a74a)",
      }),
      // Green at the bottom to magenta at the top.
      // Lightroom's tint scale; positive = magenta. Neutral is the as-shot tint.
      s("tint", "Tint", "T", "none", {
        min: -150,
        max: 150,
        trackGradient: "linear-gradient(to top, #5fae58, #c0c0c0, #c45cb8)",
      }),
    ],
  },
  {
    name: "Tone",
    sliders: [
      s("exposure", "Exposure", "E", "none", { min: -5, max: 5, step: 0.05, coarseStep: 1 / 3, decimals: 2 }),
      s("contrast", "Contrast", "C"),
      s("highlights", "Highlights", "H"),
      s("shadows", "Shadows", "S"),
      s("whites", "Whites", "W"),
      s("blacks", "Blacks", "B"),
    ],
  },
  {
    // The photo's subject (people) mask's Exposure and Contrast (Shadows until 13 Nov), the most used of its sliders, in the
    // main panel (Chris, 13 Nov; named People Mask, Shift+P, as Shift+S and Shift+M were taken): the very
    // values the Masks panel shows for that mask (read and written there, never kept in the photo's
    // own sliders). Shown once the photo has one ("People mask: all photos" adds one to every photo).
    name: "People Mask",
    sliders: [
      s("subjectExposure", "Exposure", "E", "none", { min: -4, max: 4, step: 0.05, coarseStep: 1 / 3, decimals: 2, tooltip: "People Mask Exposure (the subject mask's)" }),
      s("subjectContrast", "Contrast", "C", "none", { tooltip: "People Mask Contrast (the subject mask's)" }),
    ],
  },
  {
    // Lightroom's Presence (Texture, Clarity, Dehaze, Vibrance, Saturation),
    // named Definition so its group key (D) doesn't clash with Pick (P).
    name: "Definition",
    sliders: [
      s("texture", "Texture", "T", "shift"),
      s("clarity", "Clarity", "C", "shift"),
      s("dehaze", "Dehaze", "D"),
      s("vibrance", "Vibrance", "V"),
      s("saturation", "Saturation", "S", "shift"),
    ],
  },
  // No Levels group: the Curves panel (after Definition) covers it - its end
  // points move sideways for black/white input points and up/down for output
  // levels.
  {
    // Lightroom's Detail, named Sharpen so its group key (S) doesn't clash
    // with Definition (D).
    name: "Sharpen",
    sliders: [
      // Below 0 it softens (Lightroom's mask Sharpness does; its global Amount stops at 0).
      // -150..150 so 0 sits in the middle (Chris, 1 Nov); Lightroom's 0..150 unchanged.
      s("sharpen", "Sharpen", "S", "alt", { min: -150, max: 150 }),
      // On Sharpen's track so the two look even, 0 centred and blocked below it; its 0..100 spread over
      // the whole right half (100 at the top, no stop there) (Chris, 1 Nov).
      s("noise", "Noise", "N", "none", { min: -150, max: 150, lowest: 0, highest: 100, stretch: 1.5 }),
    ],
  },
  {
    // Auto level / Auto keystone buttons sit beside these (BottomPanel).
    // Lightroom's Transform panel, named Keystone so its group key (K)
    // doesn't clash with Tone (T).
    name: "Keystone",
    sliders: [
      s("rotate", "Rotate", "R", "alt", { min: -45, max: 45, step: 0.1, coarseStep: 1, decimals: 1 }),
      s("vertical", "Vertical", "V", "alt"),
      s("horizontal", "Horizontal", "H", "shift"),
      s("scale", "Scale", "S", "none", { min: 50, max: 150, neutral: 100 }),
    ],
  },
  {
    name: "Effects",
    // Adaptive vignette: RapidOne's subject-aware one, its own setting. The
    // legacy (Lightroom) vignette is in the panel's pop-up ("Legacy vignette").
    sliders: [s("adaptiveVignette", "Adaptive vignette", "V", "shift", { tooltip: "Adaptive vignette: darkens around the subject" }), s("grain", "Grain", "G", "none", { min: 0, max: 100 })],
  },
  {
    // Lightroom's Color noise reduction. Set in the Sharpen panel's switch ("Legacy noise"), not as a
    // group of its own (not in BottomPanel's PANEL_ORDER). 25 on a RAW, 0 on anything else (`defaultsFor`).
    name: "Legacy noise",
    sliders: [s("colourNoise", "Colour", "", "none", { min: 0, max: 100, neutral: 25, tooltip: "Colour noise reduction: smooths colour speckle, brightness untouched" })],
  },
  {
    // The legacy (Lightroom) vignette: its amount and shape. Set in the
    // Effects panel's pop-up (or from a Lightroom edit), not as a group of
    // its own (not in BottomPanel's PANEL_ORDER).
    name: "Legacy vignette",
    sliders: [
      // Lightroom's post-crop vignette, under Lightroom's names (PostCropVignette*).
      s("vignette", "Amount", "", "none"),
      s("vignetteMidpoint", "Midpoint", "", "none", { min: 0, max: 100, neutral: 50 }),
      s("vignetteRoundness", "Roundness", "", "none", { min: -100, max: 100 }),
      s("vignetteFeather", "Feather", "", "none", { min: 0, max: 100, neutral: 50 }),
      s("vignetteHighlights", "Highlights", "", "none", { min: 0, max: 100 }),
    ],
  },
  {
    // Lens Corrections' Defringe (Lightroom's numbers): Amount 0..20 per colour and its hue
    // range (0..100 along that colour's band). Set in the Lens menu, not as a group of its own
    // (not in BottomPanel's PANEL_ORDER).
    name: "Defringe",
    sliders: [
      s("defringePurple", "Purple amount", "", "none", { min: 0, max: 20 }),
      s("defringePurpleHueLo", "Purple hue from", "", "none", { min: 0, max: 100, neutral: 30 }),
      s("defringePurpleHueHi", "Purple hue to", "", "none", { min: 0, max: 100, neutral: 70 }),
      s("defringeGreen", "Green amount", "", "none", { min: 0, max: 20 }),
      s("defringeGreenHueLo", "Green hue from", "", "none", { min: 0, max: 100, neutral: 40 }),
      s("defringeGreenHueHi", "Green hue to", "", "none", { min: 0, max: 100, neutral: 60 }),
    ],
  },
  {
    // Lightroom's parametric (region) tone curve. Shown in the Curves panel's
    // Region tab, not as a group of its own (not in BottomPanel's PANEL_ORDER).
    name: "Parametric",
    sliders: [
      s("paramHighlights", "Highlights", ""),
      s("paramLights", "Lights", ""),
      s("paramDarks", "Darks", ""),
      s("paramShadows", "Shadows", ""),
      // Where the regions meet, as a percentage of the tone range (Lightroom's defaults).
      s("paramShadowSplit", "Shadow split", "", "none", { min: 5, max: 95, neutral: 25 }),
      s("paramMidSplit", "Midtone split", "", "none", { min: 5, max: 95, neutral: 50 }),
      s("paramHighSplit", "Highlight split", "", "none", { min: 5, max: 95, neutral: 75 }),
    ],
  },
  {
    // Lightroom's Calibration, named Fine tuning so its group key (F)
    // doesn't clash with Curves (C).
    name: "Fine tuning",
    sliders: [
      // Green at the bottom to magenta at the top, as Tint.
      s("calShadowTint", "Shadow tint", "T", "alt", { trackGradient: "linear-gradient(to top, #5fae58, #c0c0c0, #c45cb8)" }),
      // The three primaries are shown one at a time by CalibrationPanel (pick
      // a dot, then Hue / Saturation), so they have no shortcuts of their own.
      s("calRedHue", "Red hue", ""),
      s("calRedSat", "Red sat", ""),
      s("calGreenHue", "Green hue", ""),
      s("calGreenSat", "Green sat", ""),
      s("calBlueHue", "Blue hue", ""),
      s("calBlueSat", "Blue sat", ""),
    ],
  },
];

/**
 * Left-to-right panel order: Lightroom Classic's Develop order, except
 * Transform (here Keystone) moved up to follow Curves and Calibration (here
 * Fine tuning) to follow Grade, so the colour panels sit together (user,
 * 2026-10-06). Names are slider
 * groups from SLIDER_GROUPS, or the Curves / Mixer / Grade panels.
 */
export const PANEL_ORDER = [
  "W/B",
  "Tone",
  "People Mask",
  "Definition",
  "Curves",
  "Keystone",
  "Mixer",
  "Grade",
  "Fine tuning",
  "Sharpen",
  "Effects",
];

/**
 * Keyboard: Shift + a panel's first letter selects it (it lights up), then
 * hold a slider's letter to adjust it - only that panel's sliders answer, so
 * the same letter can mean different sliders in different panels (T: Tint in
 * W/B, Texture in Definition). Esc, or Shift + its letter again, deselects.
 */
export function panelForKey(code: string): string | undefined {
  return PANEL_ORDER.find((name) => `Key${panelKey(name)}` === code);
}

/** The letter that selects a panel with Shift (upper case): its first. */
export function panelKey(name: string): string {
  return name[0].toUpperCase();
}

/** The photo's own sliders that are really its subject mask's (the "People Mask" group). */
export const SUBJECT_SLIDERS: Record<string, "exposure" | "contrast"> = { subjectExposure: "exposure", subjectContrast: "contrast" };

/** The slider in `panel` whose letter is `code` (e.g. "KeyT"), if any. */
export function sliderInPanel(panel: string, code: string): SliderDef | undefined {
  return SLIDER_GROUPS.find((g) => g.name === panel)?.sliders.find((d) => d.code !== "" && d.code === code);
}

export const ALL_SLIDERS: SliderDef[] = SLIDER_GROUPS.flatMap((g) => g.sliders);

/** Neutral value for each slider. */
export const DEFAULTS: Record<string, number> = Object.fromEntries(ALL_SLIDERS.map((d) => [d.id, d.neutral]));

/** A photo's starting values: Colour noise reduction is on for a RAW, off for what's already been developed (Lightroom's defaults). */
export function defaultsFor(path: string | null | undefined): Record<string, number> {
  return /\.(jpe?g|tiff?|png|webp)$/i.test(path ?? "") ? { ...DEFAULTS, colourNoise: 0 } : DEFAULTS;
}

export type Adjustments = Record<string, number>;

export function clampToSlider(def: SliderDef, value: number): number {
  const clamped = Math.min(def.highest ?? def.max, Math.max(def.lowest ?? def.min, value));
  // Round to the slider's precision so repeated nudges never drift (0.1 + 0.2 ...).
  const factor = 10 ** Math.max(def.decimals, 2);
  return Math.round(clamped * factor) / factor;
}

export function formatValue(def: SliderDef, value: number): string {
  const text = value.toFixed(def.decimals);
  return value > 0 && (def.lowest ?? def.min) < 0 ? `+${text}` : text;
}

/**
 * Short names shown horizontally under each slider (4-5 letters fit the
 * column). Each starts with its shortcut letter where it has one, so the
 * bold first letter is still the key. Labels of 5 letters or fewer are used
 * as they are.
 */
const SHORT_NAMES: Record<string, string> = {
  Kelvin: "Kelv",
  Exposure: "Expo",
  Contrast: "Cont",
  Highlights: "High",
  Shadows: "Shad",
  Whites: "White",
  Blacks: "Black",
  Texture: "Tex",
  Clarity: "Clar",
  Dehaze: "Dehz",
  Vibrance: "Vibr",
  Saturation: "Sat",
  Sharpen: "Sharp",
  Vignette: "Vign",
  "Adaptive vignette": "Vign",
  Rotate: "Rot",
  Vertical: "Vert",
  Horizontal: "Horiz",
  "Shadow tint": "Tint",
  Luminance: "Lum",
  Blending: "Blend",
};

export function shortName(label: string): string {
  return SHORT_NAMES[label] ?? (label.length <= 5 ? label : label.slice(0, 4));
}

export function modifierSymbol(modifier: Modifier): string {
  return modifier === "shift" ? "⇧" : modifier === "alt" ? "Alt" : "";
}

/**
 * One keyboard (or scroll) nudge. Moves to the next multiple of the step in
 * that direction rather than adding the step blindly, so exposure lands on
 * camera-style thirds (+0.33, +0.67, +1.00...) even after a fine adjustment
 * left it in between. Values within a few percent of a multiple count as on it,
 * which absorbs the two-decimal rounding of stored values (0.33 vs 1/3).
 */
export function nudge(def: SliderDef, value: number, direction: 1 | -1, coarse: boolean): number {
  const step = coarse ? def.coarseStep : def.step;
  const grid = value / step;
  const nearest = Math.round(grid);
  const base = Math.abs(grid - nearest) < 0.06 ? nearest : direction > 0 ? Math.floor(grid) : Math.ceil(grid);
  return clampToSlider(def, (base + direction) * step);
}

/** Where a value sits along the track, 0 (bottom) to 1 (top). */
export function toFraction(def: SliderDef, value: number): number {
  if (def.scale === "mired") {
    const f = (1 / def.min - 1 / value) / (1 / def.min - 1 / def.max);
    return Math.min(1, Math.max(0, f));
  }
  return Math.min(1, Math.max(0, (value * (def.stretch ?? 1) - def.min) / (def.max - def.min)));
}

/** The value at a position along the track (0..1). */
export function fromFraction(def: SliderDef, f: number): number {
  const t = Math.min(1, Math.max(0, f));
  if (def.scale === "mired") {
    return 1 / (1 / def.min - t * (1 / def.min - 1 / def.max));
  }
  return (def.min + t * (def.max - def.min)) / (def.stretch ?? 1);
}
