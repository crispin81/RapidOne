// Applying Lightroom develop settings (a preset or a photo's own Lightroom
// edit) to a photo's edit state. Our sliders take Lightroom's numbers as they
// are; only the wheels (hue/saturation -> puck position) and curves (0..255 ->
// 0..1) need converting. A preset changes only what it contains.

import { ALL_SLIDERS, clampToSlider, type Adjustments } from "./adjustments";
import { PROFILES, profileOrFallback, type LightroomSettings, type ProfileId } from "./api";
import type { CurveChannel, Curves } from "./curves";
import { GRADE_RANGES, type Grade, type RangeGrade } from "./grade";
import { MIXER_COLOURS, type Mixer } from "./mixer";

const SLIDERS_BY_ID = Object.fromEntries(ALL_SLIDERS.map((d) => [d.id, d]));

export interface PhotoEditState {
  /** Changed sliders only (the rest stay at the photo's neutrals). */
  edits: Partial<Adjustments>;
  profile: ProfileId;
  mixer: Mixer;
  curves: Curves;
  grade: Grade;
}

/** Lightroom's hue (0..360) and saturation (0..100) as a wheel puck (right = red, anticlockwise). */
export function hueSatToWheel(hue: number, sat: number): { x: number; y: number } {
  const r = Math.min(100, Math.max(0, sat)) / 100;
  const a = (hue * Math.PI) / 180;
  return { x: r * Math.cos(a), y: -r * Math.sin(a) };
}

const CURVE_KEYS: [keyof LightroomSettings["curves"], CurveChannel][] = [
  ["luma", "luma"],
  ["rgb", "rgb"],
  ["r", "r"],
  ["g", "g"],
  ["b", "b"],
];

/**
 * The photo's edit state with Lightroom's settings applied on top.
 * `adobeAvailable`: whether Adobe Color can be offered; if not, Adobe Color
 * falls back to Shinobi Colour (its closest match).
 */
export function applyLightroom(state: PhotoEditState, lr: LightroomSettings, adobeAvailable: boolean): PhotoEditState {
  const edits: Partial<Adjustments> = { ...state.edits };
  for (const [id, value] of Object.entries(lr.sliders)) {
    const def = SLIDERS_BY_ID[id];
    if (def) edits[id] = clampToSlider(def, value);
  }
  if (lr.whiteBalance === "asShot") {
    delete edits.temperature;
    delete edits.tint;
  } else if (lr.whiteBalance === "custom" && lr.temperature != null && lr.tint != null) {
    edits.temperature = clampToSlider(SLIDERS_BY_ID.temperature, lr.temperature);
    edits.tint = clampToSlider(SLIDERS_BY_ID.tint, lr.tint);
  }

  let profile = state.profile;
  // The Adobe profile the edit used, else its Shinobi equivalent.
  if (lr.profile) profile = profileOrFallback(lr.profile, adobeAvailable);

  const mixer: Mixer = { ...state.mixer };
  for (const c of MIXER_COLOURS) {
    const band = lr.mixer[c.id];
    if (band) mixer[c.id] = { ...mixer[c.id], ...band };
  }

  const curves: Curves = { ...state.curves };
  for (const [key, channel] of CURVE_KEYS) {
    const points = lr.curves[key];
    if (points && points.length >= 2) {
      curves[channel] = [...points].sort((p, q) => p[0] - q[0]).map(([x, y]) => ({ x: x / 255, y: y / 255 }));
    }
  }

  const grade: Grade = { ...state.grade };
  for (const range of GRADE_RANGES) {
    const g = lr.grade[range];
    if (!g) continue;
    const current = grade[range];
    // The wheel holds hue and saturation together, so a preset giving only
    // one of them keeps the other from the current puck.
    const curHue = ((Math.atan2(-current.y, current.x) * 180) / Math.PI + 360) % 360;
    const curSat = Math.min(1, Math.hypot(current.x, current.y)) * 100;
    const puck = hueSatToWheel(g.hue ?? curHue, g.sat ?? curSat);
    const next: RangeGrade = { ...puck, lum: g.lum ?? current.lum };
    grade[range] = next;
  }
  const extra = lr.grade[""];
  if (extra?.blending != null) grade.blending = extra.blending;
  if (extra?.balance != null) grade.balance = extra.balance;

  return { edits, profile, mixer, curves, grade };
}

/** A one-line summary for the notice banner. */
export function describeApplied(lr: LightroomSettings, what: string, adobeAvailable: boolean): string {
  const parts = [`${what}${lr.name ? ` "${lr.name}"` : ""} applied.`];
  if (lr.profile && !adobeAvailable && profileOrFallback(lr.profile, false) !== lr.profile) {
    const label = (id: ProfileId) => PROFILES.find((p) => p.id === id)?.label ?? id.replace(/^user:/, "");
    parts.push(`${label(lr.profile)} isn't in your profiles, so ${label(profileOrFallback(lr.profile, false))} is used (Profile menu → Add your own profile…).`);
  }
  if (lr.skipped.length > 0) parts.push(`Not in RapidOne yet: ${lr.skipped.join(", ")}.`);
  return parts.join(" ");
}

// ---- RapidOne's own presets ------------------------------------------------
//
// Saved in the app's presets folder (presets.rs), one JSON file each. The
// settings use the same shape as a Lightroom preset read from an .xmp (only
// what the preset sets), so a Lightroom preset is converted once on import
// and every preset, imported or saved here, applies the same way.

export const PRESET_FORMAT = "rapidone-preset";

export interface RapidPreset {
  format: typeof PRESET_FORMAT;
  version: 1;
  name: string;
  /** Where it came from: imported from a Lightroom .xmp, or saved here. */
  origin: "lightroom" | "rapidone";
  settings: LightroomSettings;
}

export function presetText(p: RapidPreset): string {
  return JSON.stringify(p, null, 2) + "\n";
}

export function parsePreset(text: string): RapidPreset | null {
  try {
    const p = JSON.parse(text) as RapidPreset;
    return p.format === PRESET_FORMAT && typeof p.name === "string" && p.settings ? p : null;
  } catch {
    return null;
  }
}

/** A Lightroom preset converted to RapidOne's (named as in Lightroom, else after its file). */
export function presetFromLightroom(lr: LightroomSettings, fileName: string): RapidPreset {
  const name = (lr.name ?? fileName.replace(/\.xmp$/i, "")).trim() || "Lightroom preset";
  return { format: PRESET_FORMAT, version: 1, name, origin: "lightroom", settings: { ...lr, name } };
}

/** A photo's whole edit as a preset (everything it changes). */
export function presetFromState(name: string, state: PhotoEditState): RapidPreset {
  // autoTransform marks a photo's auto straighten; it is not a setting to copy.
  const { temperature, tint, autoTransform: _auto, ...sliders } = state.edits as Record<string, number>;
  const custom = temperature != null && tint != null;
  const curves: LightroomSettings["curves"] = {};
  for (const [key, channel] of CURVE_KEYS) curves[key] = state.curves[channel].map((p) => [p.x * 255, p.y * 255] as [number, number]);
  const grade: LightroomSettings["grade"] = { "": { blending: state.grade.blending, balance: state.grade.balance } };
  for (const range of GRADE_RANGES) {
    const r = state.grade[range];
    grade[range] = {
      hue: ((Math.atan2(-r.y, r.x) * 180) / Math.PI + 360) % 360,
      sat: Math.min(1, Math.hypot(r.x, r.y)) * 100,
      lum: r.lum,
    };
  }
  return {
    format: PRESET_FORMAT,
    version: 1,
    name,
    origin: "rapidone",
    settings: {
      name,
      sliders,
      whiteBalance: custom ? "custom" : "asShot",
      temperature: custom ? temperature : null,
      tint: custom ? tint : null,
      profile: state.profile,
      mixer: Object.fromEntries(MIXER_COLOURS.map((c) => [c.id, { ...state.mixer[c.id] }])),
      curves,
      grade,
      skipped: [],
    },
  };
}
