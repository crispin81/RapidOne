import type { BokehLens } from "./api";

/**
 * The Creative tab's edit data, stored in the photo's `.rpd` beside the sliders, curves and masks.
 * Everything is off or at a harmless default until the user touches it, and the engine's
 * `Bokeh` (`src-tauri/src/creative.rs`) mirrors these names and defaults exactly.
 */

export type BokehMode = "open" | "simulate" | "manual";
export type BokehShape = "smooth" | "blades" | "bubble" | "swirl" | "mirror";

export interface Bokeh {
  enabled: boolean;
  mode: BokehMode;
  /** The target f-number (Open up and Simulate); 0 is the shot's own, until it's moved. */
  aperture: number;
  /** The target focal length in mm (Simulate). */
  focal: number;
  /** 0–100: Manual's blur strength. */
  amount: number;
  /** The picked focus point, fractions of the upright, uncropped frame. */
  focusU: number;
  focusV: number;
  /** Focus on the subject (its matte's median distance) instead of the point. */
  focusAuto: boolean;
  /** Extra depth kept sharp beyond the f-number's own, in control-depth units 0–100 (log distance, as the masks' band). */
  focusRange: number;
  shape: BokehShape;
  /** 5–11 (Blades only). */
  blades: number;
  /** 0–100: a sharp polygon at 0, round at 100 (Blades only). */
  bladeRounding: number;
  catsEye: number;
  boost: number;
  fringe: number;
}

export interface Creative {
  bokeh: Bokeh;
  mist: Mist;
  relight: Relight;
}

/** A lamp in the room (point, or spot for a grid/snoot), or the sky's own sun or moon: parallel
 *  light from a direction (Azimuth and Elevation), no falloff. */
export type LightKind = "point" | "spot" | "sun" | "moon";

/** Whether a light is the sun or the moon (placed by direction, not in the room). */
export const isCelestial = (l: { kind: LightKind }) => l.kind === "sun" || l.kind === "moon";

/** One virtual light (`src-tauri/src/relight.rs`). Point and spot use u/v and Depth; the sun uses
 *  Azimuth and Elevation; the spot also Cone and Feather. */
export interface Light {
  id: number;
  kind: LightKind;
  on: boolean;
  /** Point and spot: its place on the image plane, fractions (it may sit off frame, −0.5..1.5). */
  u: number;
  v: number;
  /** Point and spot: 0 just in front of the scene, 100 behind it (a backlight). */
  depth: number;
  /** Spot only: where its cone points, fractions of the picture (the dot on the photo). */
  aimU: number;
  aimV: number;
  /** Point and spot: the real 3D facing, degrees — yaw around the vertical (0 = into the scene,
   *  90 = right, 360 wraps) and pitch above/below the horizon (+ up, − down). Absent on older
   *  files, where the aim point above still sets the direction. */
  yaw?: number;
  pitch?: number;
  /** 0–100: 0–4 stops over the scene's own light. */
  intensity: number;
  /** 2000–12000 K: a blackbody's colour, relative to the photo's white balance. */
  kelvin: number;
  /** −100..100: green to magenta. */
  tint: number;
  /** 0–100: how far it wraps round edges, and how broad its sheen. */
  softness: number;
  /** 0–100: its reach in metres and the penumbra of its shadows. */
  size: number;
  shadows: number;
  shadowSoft: number;
  /** Sun: degrees around the vertical (0 = from the right, 90 = behind the subject), and above the
   *  horizon. */
  azimuth: number;
  elevation: number;
  /** Spot: half angle 5–85°, and the edge's softness. */
  cone: number;
  feather: number;
  /** The modifier picked in the Type list (a `LIGHT_TYPES` id), so it stays picked even when its
   *  numbers no longer identify it (Rim and Fill are both a dish's size). Absent on older files. */
  modifier?: string;
}

/** Relight: virtual lights shaped by the scene's depth and normals. */
export interface Relight {
  enabled: boolean;
  /** −100..100: how the scene's own light is kept, in stops (33.3 = one stop); 0 leaves it. */
  ambient: number;
  /** 0–100: the photo's own micro-surface tilted into the normals. */
  relief: number;
  lights: Light[];
}

/** Mist: depth fog, height fog, sun glow, noise banks and a subject protection. */
export interface Mist {
  enabled: boolean;
  /** 0–100: extinction; 50 is a visibility a few times the scene's far anchor. */
  density: number;
  /** How near the fog comes (the slider's End), control-depth units 0–100 (log distance, as the
   *  masks' band); 100 reaches the camera, so it covers the subject. */
  start: number;
  /** How far it goes (the slider's Start), the same units; 0 has no limit. */
  far: number;
  /** 0–100: how softly the fog eases in, in the same units. */
  softness: number;
  /** −100..100: cool to warm fog colour. */
  temperature: number;
  /** −100..100: green to magenta. */
  tint: number;
  /** 0–100: how much the fog pools near the ground (0 uniform). */
  height: number;
  /** 0–100: 0 a deep layer, 100 a shallow one. */
  heightFalloff: number;
  /** 0–100: tilts the estimated ground plane (50 = as estimated). */
  horizon: number;
  /** 0–100: sun in-scattering. */
  glow: number;
  /** The glow point on the image, 0..1 inside the frame, −0.5..1.5 off it. */
  glowU: number;
  glowV: number;
  /** 0–100: 3D noise banks, only ever adding fog. */
  texture: number;
  /** 0 or 1: deterministic noise seed. */
  seed: number;
  /** 0–100: how much the subject stays clear of fog. Its slider has gone (the fog's End does that): 0. */
  protectSubject: number;
}

/** The sharp band on screen: the f-number's own zone (`bokehZone`; just the focus plane without
 *  one), and half of Focus range more either side. Control depth, 1 = nearest. */
export function focusBand(focus: number, zone: { near: number; far: number } | null, focusRange: number): { near: number; far: number } {
  const half = focusRange / 200;
  return { near: Math.min(1, (zone?.near ?? focus) + half), far: Math.max(0, (zone?.far ?? focus) - half) };
}

export const DEFAULT_BOKEH: Bokeh = {
  enabled: false,
  mode: "open",
  aperture: 0,
  focal: 0,
  amount: 0,
  focusU: 0.5,
  focusV: 0.5,
  focusAuto: true,
  focusRange: 0,
  shape: "smooth",
  blades: 7,
  bladeRounding: 50,
  catsEye: 0,
  boost: 0,
  fringe: 0,
};

export const DEFAULT_MIST: Mist = {
  enabled: false,
  density: 0,
  start: 100,
  far: 0,
  softness: 15,
  temperature: -20,
  tint: 0,
  height: 0,
  heightFalloff: 50,
  horizon: 50,
  glow: 0,
  glowU: 0.5,
  glowV: 0.2,
  texture: 0,
  seed: 1,
  protectSubject: 0,
};

export const DEFAULT_LIGHT: Light = {
  id: 1,
  kind: "point",
  on: true,
  u: 0.36,
  v: 0.34,
  depth: 30,
  aimU: 0.5,
  aimV: 0.5,
  intensity: 45,
  kelvin: 5600,
  tint: 0,
  softness: 35,
  size: 30,
  shadows: 0,
  shadowSoft: 40,
  azimuth: 45,
  elevation: 35,
  cone: 85,
  feather: 45,
};

export const DEFAULT_RELIGHT: Relight = { enabled: false, ambient: 0, relief: 0, lights: [] };

export const DEFAULT_CREATIVE: Creative = { bokeh: DEFAULT_BOKEH, mist: DEFAULT_MIST, relight: DEFAULT_RELIGHT };

const sameBokeh = (a: Partial<Bokeh> | undefined, b: Bokeh) => (Object.keys(b) as (keyof Bokeh)[]).every((k) => (a?.[k] ?? b[k]) === b[k]);
const sameMist = (a: Partial<Mist> | undefined, b: Mist) => (Object.keys(b) as (keyof Mist)[]).every((k) => (a?.[k] ?? b[k]) === b[k]);
const sameLight = (a: Partial<Light> | undefined, b: Light = DEFAULT_LIGHT) => (Object.keys(b) as (keyof Light)[]).every((k) => (a?.[k] ?? b[k]) === b[k]);
/** Whether Relight changes anything: it has a light on, or the ambient is moved. */
function relightActive(r: Relight): boolean {
  return r.ambient !== 0 || r.lights.some((l) => l.on && l.intensity > 0);
}
const sameRelight = (a: Partial<Relight> | undefined) => {
  if (!a) return true;
  const r = { ...DEFAULT_RELIGHT, ...a, lights: a.lights ?? [] };
  return !relightActive(r) && r.relief === 0 && r.lights.every((l) => sameLight(l));
};

/** Whether the Creative edit changes anything (so an untouched photo writes no `creative`). */
export function hasCreative(c: Partial<Creative> | undefined | null): boolean {
  return Boolean(c && (!sameBokeh(c.bokeh, DEFAULT_BOKEH) || !sameMist(c.mist, DEFAULT_MIST) || !sameRelight(c.relight)));
}

/** Whether Bokeh has any strength: what `enabled` follows, as there's no switch (an untouched photo
 *  then costs nothing). */
function bokehActive(b: Bokeh): boolean {
  return b.mode === "manual" ? b.amount > 0 : b.aperture > 0;
}

/** A Creative edit from a `.rpd` (perhaps from before Mist): every field it has, the rest default.
 *  One left switched off when there were switches stays invisible: its strength goes back to none.
 *  Simulate and Mist's Protect have gone: the first becomes Lens based, the second 0. */
export function normalizeCreative(c: Partial<Creative> | undefined | null): Creative {
  const saved = { ...DEFAULT_BOKEH, ...(c?.bokeh ?? {}) };
  const bokeh: Bokeh = saved.mode === "simulate" ? { ...saved, mode: "open", focal: 0, enabled: saved.enabled && saved.aperture > 0 } : saved;
  const mist = { ...DEFAULT_MIST, ...(c?.mist ?? {}), protectSubject: 0 };
  const savedRelight = c?.relight;
  const relight: Relight = {
    ...DEFAULT_RELIGHT,
    ...(savedRelight ?? {}),
    // A light's modifier is fixed when it is first read, so moving Size only resizes it: without
    // this, the Type list and the 3D shape were re-guessed from the size on every render.
    lights: (savedRelight?.lights ?? []).map((l, i) => {
      const light = { ...DEFAULT_LIGHT, ...l, id: l.id ?? i + 1 };
      if (light.kind !== "point" && light.kind !== "spot" && light.kind !== "sun" && light.kind !== "moon") light.kind = "point";
      return light.modifier ? light : { ...light, modifier: typeOf(light) };
    }),
  };
  return {
    bokeh: !bokeh.enabled && bokehActive(bokeh) ? { ...bokeh, aperture: 0, focal: 0, amount: 0 } : bokeh,
    mist: !mist.enabled && mist.density > 0 ? { ...mist, density: 0 } : mist,
    // A Relight left switched off when there were switches keeps its lights off (invisible).
    relight: !relight.enabled && relightActive(relight) ? { ...relight, ambient: 0, lights: relight.lights.map((l) => ({ ...l, on: false })) } : relight,
  };
}

/** One Bokeh value changed (a copy, as the edit is immutable state). */
export function withBokeh(c: Creative | undefined, patch: Partial<Bokeh>): Creative {
  const base = normalizeCreative(c);
  const bokeh = { ...base.bokeh, ...patch };
  return { ...base, bokeh: { ...bokeh, enabled: bokehActive(bokeh) } };
}

/** One Relight value changed (not its lights). */
export function withRelight(c: Creative | undefined, patch: Partial<Relight>): Creative {
  const base = normalizeCreative(c);
  const relight = { ...base.relight, ...patch };
  return { ...base, relight: { ...relight, enabled: relightActive(relight) } };
}

/** One light changed, by id. */
export function withLight(c: Creative | undefined, id: number, patch: Partial<Light>): Creative {
  const base = normalizeCreative(c);
  const lights = base.relight.lights.map((l) => (l.id === id ? { ...l, ...patch } : l));
  const relight = { ...base.relight, lights };
  return { ...base, relight: { ...relight, enabled: relightActive(relight) } };
}

/** The ambient the first light sets: one stop down (33.3 is a stop). */
export const FIRST_LIGHT_AMBIENT = -33;

/** A light added, selected by its new id; `typeId` (a `LIGHT_TYPES` id) gives it that type's
 *  character. Each new light stands a step left or right of the last, so they never stack. */
export function addLight(c: Creative | undefined, kind: LightKind, typeId?: string, place?: Partial<Light>): Creative {
  const base = normalizeCreative(c);
  const id = Math.max(0, ...base.relight.lights.map((l) => l.id)) + 1;
  const n = base.relight.lights.length;
  const side = n === 0 ? 0 : (n % 2 ? -1 : 1) * Math.ceil(n / 2) * 0.2;
  const fresh: Light = {
    ...DEFAULT_LIGHT,
    id,
    kind,
    u: Math.min(1.5, Math.max(-0.5, DEFAULT_LIGHT.u + side)),
  };
  const typed: Light = typeId ? withLightType(fresh, typeId) : { ...fresh, modifier: typeOf(fresh) };
  // Placed by the 3D space when it knows the scene (round the subject); else the default spot.
  const light: Light = place ? { ...typed, ...place, id } : typed;
  // The first light into an untouched Relight brings the ambient a stop down, as a photographer
  // drops the shutter speed so the flash reads; after that the ambient is left as it's set.
  const first = base.relight.lights.length === 0 && base.relight.ambient === 0;
  const sunUp = place?.elevation ?? typed.elevation;
  const ambient = first
    ? typed.kind === "moon"
      ? moonAmbient(sunUp)
      : typed.kind === "sun"
        ? sunAmbient(sunUp)
        : FIRST_LIGHT_AMBIENT
    : skyAmbient(base.relight, typed.kind, sunUp);
  const relight = { ...base.relight, ambient, lights: [...base.relight.lights, light] };
  return { ...base, relight: { ...relight, enabled: true } };
}

/** A light removed. */
export function removeLight(c: Creative | undefined, id: number): Creative {
  const base = normalizeCreative(c);
  const relight = { ...base.relight, lights: base.relight.lights.filter((l) => l.id !== id) };
  return { ...base, relight: { ...relight, enabled: relightActive(relight) } };
}

/** One Mist value changed. */
export function withMist(c: Creative | undefined, patch: Partial<Mist>): Creative {
  const base = normalizeCreative(c);
  const mist = { ...base.mist, ...patch };
  return { ...base, mist: { ...mist, enabled: mist.density > 0 } };
}

export const BOKEH_MODES: { id: BokehMode; label: string; title: string }[] = [
  { id: "open", label: "Real lens", title: "Real lens: your lens, opened wider than you shot it (only the extra blur is added); Blur is its f-number" },
  { id: "manual", label: "Simple", title: "Simple: no lens maths, just how much blur (no EXIF needed)" },
];

export const BOKEH_SHAPES: { id: BokehShape; label: string; title: string }[] = [
  { id: "smooth", label: "Smooth", title: "Smooth: a soft round disc (an apodised portrait prime)" },
  { id: "blades", label: "Blades", title: "Blades: a stopped-down aperture's polygon" },
  { id: "bubble", label: "Bubble", title: "Soap bubble: a bright rim" },
  { id: "swirl", label: "Swirl", title: "Swirl: streaked tangentially toward the corners (Helios style)" },
  { id: "mirror", label: "Mirror", title: "Mirror: a ring, hollow in the middle (a mirror lens)" },
];

/** What each letter adjusts while held in the selected Creative panel: a slider's first letter. */
const CREATIVE_LETTERS: Record<string, Record<string, string>> = {
  // A slider's letter is its name's first (Glow in Mist: L, as G is Ground; Sides: I).
  Bokeh: { B: "aperture", S: "focusRange", I: "blades", E: "bladeRounding", V: "catsEye", G: "boost" },
  Mist: { A: "density", F: "far", N: "start", W: "temperature", G: "height", L: "glow", P: "texture" },
  Relight: { I: "intensity", K: "kelvin", T: "tint", O: "softness", Z: "size", P: "intensity", J: "aim", D: "throw", X: "u", Y: "v", A: "ambient", R: "relief", C: "cone", S: "cone", F: "feather", M: "azimuth", V: "elevation" },
};

/** The Creative panels, whose first letters select them with Shift. */
const CREATIVE_PANELS = ["Depth", "Bokeh", "Mist", "Relight"];

/** The Creative panel Shift + `code` (e.g. "KeyB") selects. */
export function creativePanelForKey(code: string): string | undefined {
  return CREATIVE_PANELS.find((name) => `Key${name[0]}` === code);
}

/** What holding `code` adjusts in a Creative panel (a held id, "creative:aperture"), if it's showing. */
export function creativeHold(panel: string, code: string, bokeh: Bokeh, relight?: Relight, selectedLight?: number | null): string | undefined {
  let key = CREATIVE_LETTERS[panel]?.[code.replace(/^Key/, "")];
  if (panel === "Bokeh") {
    if (key === "aperture" && bokeh.mode === "manual") key = "amount";
    if ((key === "blades" || key === "bladeRounding") && bokeh.shape !== "blades") return undefined;
  }
  if (panel === "Relight") {
    // Ambient and Relief always show; a light's sliders only with a light selected, and Depth
    // (not a sun), Cone and Feather (spot only) only where the engine uses them.
    if (key === "ambient" || key === "relief") return `creative:relight:${key}`;
    const light = relight?.lights.find((l) => l.id === selectedLight);
    if (!light) return undefined;
    return `creative:light:${light.id}:${key}`;
  }
  return key && `creative:${key}`;
}

/** A light's type: what kind of lamp it is, and its character. The look is in the type; the place
 *  is only ever the point in the 3D space. */
export interface LightType {
  id: string;
  label: string;
  kind: LightKind;
  intensity: number;
  kelvin: number;
  softness: number;
  size: number;
  cone: number;
  feather: number;
  shadows: number;
  shadowSoft: number;
}

export const LIGHT_TYPES: LightType[] = [
  // Each as the real modifier lights a face: Soft is how far it wraps (the source's size seen from
  // the subject), Cone / Feather its beam (half angle, and how gradually it spills past it),
  // Shadow / Soft-ish its cast shadows (every real light casts them; a big source blurs them).
  // Intensity: every key type exposes a face about the same at its typical power from about
  // 1.5 m (Power shows it in Ws), so switching type changes the light's quality, not the exposure;
  // the octabox fill sits about a stop under.
  { id: "softbox", label: "Softbox", kind: "point", intensity: 50, kelvin: 5500, softness: 50, size: 62, cone: 50, feather: 50, shadows: 70, shadowSoft: 60 },
  { id: "umbrella", label: "Umbrella", kind: "point", intensity: 50, kelvin: 5600, softness: 55, size: 50, cone: 75, feather: 70, shadows: 65, shadowSoft: 70 },
  { id: "bare", label: "Bare flash", kind: "point", intensity: 50, kelvin: 5600, softness: 5, size: 6, cone: 40, feather: 35, shadows: 85, shadowSoft: 10 },
  { id: "dish", label: "Beauty dish", kind: "point", intensity: 50, kelvin: 5500, softness: 25, size: 28, cone: 32, feather: 35, shadows: 80, shadowSoft: 30 },
  { id: "grid", label: "Grid / snoot", kind: "spot", intensity: 55, kelvin: 5600, softness: 8, size: 12, cone: 10, feather: 25, shadows: 85, shadowSoft: 15 },
  { id: "bounce", label: "Bounce flash", kind: "point", intensity: 50, kelvin: 5600, softness: 80, size: 85, cone: 85, feather: 80, shadows: 40, shadowSoft: 90 },
  { id: "rim", label: "Strip box (rim)", kind: "point", intensity: 55, kelvin: 5600, softness: 35, size: 30, cone: 40, feather: 45, shadows: 70, shadowSoft: 45 },
  // The sky: parallel light from its direction, shadows as hard as the sun's tiny disc makes them,
  // Spread (Cone) how wide a patch it falls in (100: the whole scene). The moon is a stop and a half
  // under a low sun, cool against the photo's white balance.
  { id: "sun", label: "Sun", kind: "sun", intensity: 45, kelvin: 5200, softness: 8, size: 2, cone: 50, feather: 30, shadows: 90, shadowSoft: 18 },
  { id: "moon", label: "Moon", kind: "moon", intensity: 15, kelvin: 6800, softness: 12, size: 2, cone: 50, feather: 40, shadows: 80, shadowSoft: 24 },
  { id: "fill", label: "Octabox (fill)", kind: "point", intensity: 30, kelvin: 5600, softness: 60, size: 55, cone: 55, feather: 55, shadows: 40, shadowSoft: 70 },
];

/** The sun's colour at a height in the sky, as Kelvin: warm gold on the horizon, softer through
 *  golden hour, daylight from about 40° up. The engine takes it relative to the photo's own white
 *  balance, so it isn't doubled on a photo that's already warm. */
export function sunKelvin(elevation: number): number {
  const t = Math.min(1, Math.max(0, elevation / 40));
  const s = t * t * (3 - 2 * t);
  return Math.round((3200 + 2300 * s) / 50) * 50;
}

/** The scene's own light under a sun at this height, as the Ambient slider: the sky stays bright
 *  while the sun is high (a little under a stop down), and falls away through golden hour to
 *  about two stops down with the sun on the horizon. */
export function sunAmbient(elevation: number): number {
  const t = Math.min(1, Math.max(0, elevation / 35));
  const s = t * t * (3 - 2 * t);
  return Math.round(-66 + 46 * s);
}

/** The night's own light under a moon at this height, as Ambient: about two and a half stops
 *  down with the moon low, a little under two with it high. */
export function moonAmbient(elevation: number): number {
  const t = Math.min(1, Math.max(0, elevation / 50));
  const s = t * t * (3 - 2 * t);
  return Math.round(-80 + 20 * s);
}

/** The ambient when the sky changes (a sun or moon added, or a light turned into one), with no
 *  other sun or moon switched on: the sky sets it, for its height — a sun the day's (bright high
 *  up, falling at sunset), a moon the night's. You can move it after; it then stays as you set it
 *  (dragging the sun or moon only carries it along while it's still at the sky's own value). */
export function skyAmbient(r: Relight, kind: LightKind, elevation: number): number {
  if (kind !== "moon" && kind !== "sun") return r.ambient;
  if (r.lights.some((l) => l.on && (l.kind === "sun" || l.kind === "moon"))) return r.ambient;
  return kind === "moon" ? moonAmbient(elevation) : sunAmbient(elevation);
}

/** A typical flash's power for each type, in watt-seconds: what its Power slider reads at the
 *  type's own Intensity. Studio heads in modifiers 300–400 Ws, a gridded/snooted head 200, a
 *  speedlight bounced 60. */
export const TYPE_WATTS: Record<string, number> = { softbox: 400, umbrella: 300, bare: 200, dish: 400, grid: 200, bounce: 60, rim: 300, fill: 200 };

/** A light's Intensity as flash power: the type's typical watt-seconds at its own Intensity, and
 *  doubling per stop (Intensity 25 is a stop). */
export function lightWatts(l: Light, intensity = l.intensity): number {
  const t = LIGHT_TYPES.find((x) => x.id === typeOf(l)) ?? LIGHT_TYPES[0];
  return (TYPE_WATTS[t.id] ?? 300) * Math.pow(2, (intensity - t.intensity) / 25);
}

/** The Intensity for a power in watt-seconds (the inverse of `lightWatts`), 0–100. */
export function intensityForWatts(l: Light, watts: number): number {
  const t = LIGHT_TYPES.find((x) => x.id === typeOf(l)) ?? LIGHT_TYPES[0];
  const i = t.intensity + 25 * Math.log2(Math.max(1e-3, watts) / (TYPE_WATTS[t.id] ?? 300));
  return Math.min(100, Math.max(0, i));
}

/** Watt-seconds as a flash's display would show them: 5s below 100, 10s up to 1000, then 50s. */
export function wattsText(w: number): string {
  const step = w < 100 ? 5 : w < 1000 ? 10 : 50;
  return String(Math.max(step === 5 ? 1 : step, Math.round(w / step) * step));
}

/** The type a light is closest to, for the dropdown. */
export function typeOf(l: Light): string {
  if (l.modifier && LIGHT_TYPES.some((t) => t.id === l.modifier)) return l.modifier;
  if (l.kind === "sun") return "sun";
  if (l.kind === "moon") return "moon";
  if (l.kind === "spot") return "grid";
  if (l.size <= 8) return "bare";
  if (l.size >= 70) return "bounce";
  if (l.size >= 55) return "softbox";
  if (l.size >= 40) return "umbrella";
  return "dish";
}

/** A light with a type's character (and its kind); its place stays where it is. */
export function withLightType(l: Light, typeId: string): Light {
  const t = LIGHT_TYPES.find((x) => x.id === typeId) ?? LIGHT_TYPES[0];
  return { ...l, modifier: t.id, kind: t.kind, intensity: t.intensity, kelvin: t.kelvin, softness: t.softness, size: t.size, cone: t.cone, feather: t.feather, shadows: t.shadows, shadowSoft: t.shadowSoft };
}

/** A one-click flash look: the ambient and the light(s) together, so the look is exactly as named
 *  (it replaces the photo's lights; every value is still editable afterwards). */
export interface FlashPreset {
  id: string;
  label: string;
  title: string;
  ambient: number;
  relief: number;
  lights: Omit<Light, "id">[];
}

/** The flash modifiers, from a bare bulb to a bounced flash: shown as a row in the Relight panel.
 *  Each is a starting point to Place and adjust, not a law. */
export const FLASH_PRESETS: FlashPreset[] = [
  {
    id: "bare",
    label: "Bare",
    title: "Bare on-camera flash: hard, direct, little shadow (Size 3, Soft 5)",
    ambient: -50,
    relief: 25,
    lights: [{ kind: "point", on: true, u: 0.5, v: 0.42, depth: 5, intensity: 78, kelvin: 5600, tint: 0, softness: 5, size: 3, shadows: 10, shadowSoft: 30, azimuth: 45, elevation: 35, cone: 85, feather: 45, aimU: 0.5, aimV: 0.5 }],
  },
  {
    id: "umbrella",
    label: "Umbr",
    title: "Shoot-through umbrella: broad and soft, high and to the left (Size 50, Soft 45)",
    ambient: -45,
    relief: 10,
    lights: [{ kind: "point", on: true, u: 0.33, v: 0.22, depth: 25, intensity: 50, kelvin: 5600, tint: 0, softness: 45, size: 50, shadows: 50, shadowSoft: 60, azimuth: 45, elevation: 35, cone: 85, feather: 45, aimU: 0.5, aimV: 0.5 }],
  },
  {
    id: "softbox",
    label: "Soft",
    title: "Large softbox: the portrait key, high left, soft shadows (Size 62, Soft 55)",
    ambient: -40,
    relief: 0,
    lights: [{ kind: "point", on: true, u: 0.3, v: 0.25, depth: 20, intensity: 45, kelvin: 5500, tint: 0, softness: 55, size: 62, shadows: 70, shadowSoft: 55, azimuth: 45, elevation: 35, cone: 85, feather: 45, aimU: 0.5, aimV: 0.5 }],
  },
  {
    id: "dish",
    label: "Dish",
    title: "Beauty dish: crisp above the face, a short throw (Size 28, Soft 28)",
    ambient: -45,
    relief: 20,
    lights: [{ kind: "point", on: true, u: 0.5, v: 0.18, depth: 15, intensity: 60, kelvin: 5500, tint: 0, softness: 28, size: 28, shadows: 60, shadowSoft: 30, azimuth: 45, elevation: 35, cone: 85, feather: 45, aimU: 0.5, aimV: 0.5 }],
  },
  {
    id: "grid",
    label: "Grid",
    title: "Snoot or grid: a narrow beam (Spot, Cone 10, Feather 20)",
    ambient: -55,
    relief: 25,
    lights: [{ kind: "spot", on: true, u: 0.5, v: 0.3, depth: 15, intensity: 68, kelvin: 5600, tint: 0, softness: 12, size: 15, shadows: 85, shadowSoft: 15, azimuth: 45, elevation: 35, cone: 10, feather: 20, aimU: 0.5, aimV: 0.5 }],
  },
  {
    id: "bounce",
    label: "Bounce",
    title: "Flash bounced off the ceiling: wide, soft, from above (Size 78, Soft 70)",
    ambient: -35,
    relief: 0,
    lights: [{ kind: "point", on: true, u: 0.5, v: 0.04, depth: 42, intensity: 35, kelvin: 5600, tint: 0, softness: 70, size: 78, shadows: 35, shadowSoft: 80, azimuth: 45, elevation: 35, cone: 85, feather: 45, aimU: 0.5, aimV: 0.5 }],
  },
  {
    id: "direct",
    label: "Direct",
    title: "A hard direct flash from the camera's side (Size 20, Soft 20)",
    ambient: -45,
    relief: 15,
    lights: [{ kind: "point", on: true, u: 0.5, v: 0.45, depth: 5, intensity: 50, kelvin: 5600, tint: 0, softness: 20, size: 20, shadows: 30, shadowSoft: 40, azimuth: 45, elevation: 35, cone: 85, feather: 40, aimU: 0.5, aimV: 0.5 }],
  },
  {
    id: "rim",
    label: "Rim",
    title: "A backlight: the flash behind the subject, ambient well down",
    ambient: -60,
    relief: 10,
    lights: [{ kind: "point", on: true, u: 0.62, v: 0.35, depth: 85, intensity: 62, kelvin: 5600, tint: 0, softness: 20, size: 28, shadows: 25, shadowSoft: 40, azimuth: 45, elevation: 35, cone: 85, feather: 45, aimU: 0.5, aimV: 0.5 }],
  },
  {
    id: "fill",
    label: "Fill",
    title: "Daylight fill flash: gentle, matching the daylight (Intensity 26)",
    ambient: -18,
    relief: 0,
    lights: [{ kind: "point", on: true, u: 0.42, v: 0.3, depth: 10, intensity: 26, kelvin: 5600, tint: 0, softness: 25, size: 20, shadows: 0, shadowSoft: 40, azimuth: 45, elevation: 35, cone: 85, feather: 45, aimU: 0.5, aimV: 0.5 }],
  },
];

/** A flash preset applied: its ambient, relief and lights (the photo's own lights are replaced). */
export function withFlashPreset(c: Creative | undefined, preset: FlashPreset): Creative {
  const base = normalizeCreative(c);
  const lights: Light[] = preset.lights.map((l, i) => ({ ...DEFAULT_LIGHT, ...l, id: i + 1 }));
  const relight: Relight = { ...base.relight, ambient: preset.ambient, relief: preset.relief, lights, enabled: true };
  return { ...base, relight };
}

/** A point or spot's aim as an angle and a throw, for its sliders (the dot stays the same data):
 *  the angle in degrees from the lamp (0 = right, 90 = up), the throw as a share of half the frame
 *  (0 = aimed at the lamp itself, 100 = half a frame away). */
export function aimAngle(l: Light): number {
  const dx = l.aimU - l.u;
  const dy = l.v - l.aimV;
  return Math.round(((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360);
}

export function aimThrow(l: Light): number {
  return Math.round(Math.min(1, Math.hypot(l.aimU - l.u, l.aimV - l.v) * 2) * 100);
}

/** The aim point for an angle and a throw: the inverse of `aimAngle` / `aimThrow`. */
export function aimFromAngle(l: Light, angle: number, throwPct: number): { aimU: number; aimV: number } {
  const r = Math.max(0, Math.min(1, throwPct / 100)) / 2;
  const a = (angle * Math.PI) / 180;
  return { aimU: l.u + Math.cos(a) * r, aimV: l.v - Math.sin(a) * r };
}

/** A sun's direction from a point dragged around the picture's centre: the angle round it is the
 *  azimuth (right 0°, up 90°, left 180°, down 270°), the distance out is the elevation (89° at the
 *  frame's edge). */
export function sunFromAim(x: number, y: number): { azimuth: number; elevation: number } {
  const dx = (x - 0.5) * 2;
  const dy = (0.5 - y) * 2;
  const r = Math.min(1, Math.hypot(dx, dy));
  const az = ((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360;
  return { azimuth: Math.round(az), elevation: Math.round(r * 89) };
}

/** Where a sun's handle sits for its azimuth and elevation (the inverse of `sunFromAim`). */
export function aimFromSun(azimuth: number, elevation: number): { x: number; y: number } {
  const r = Math.max(0, Math.min(1, elevation / 89));
  const a = (azimuth * Math.PI) / 180;
  return { x: 0.5 + (Math.cos(a) * r) / 2, y: 0.5 - (Math.sin(a) * r) / 2 };
}

/** Relight's lights for the Depth Scope: each with its place and a swatch of its colour. */
export function lightScope(r: Relight, selected: number | null): { id: number; kind: "point" | "spot"; u: number; v: number; depth: number; azimuth: number; elevation: number; color: [number, number, number]; selected: boolean }[] {
  return r.lights
    .filter((l) => l.on && !isCelestial(l))
    .map((l) => {
      const c = kelvinSwatch(l.kelvin);
      return { id: l.id, kind: l.kind as "point" | "spot", u: l.u, v: l.v, depth: l.depth, azimuth: l.azimuth, elevation: l.elevation, color: [c[0] / 255, c[1] / 255, c[2] / 255] as [number, number, number], selected: l.id === selected };
    });
}

/** A light's colour as an sRGB swatch, for the chips and the scope (an approximation of the
 *  engine's blackbody fit). */
export function kelvinSwatch(kelvin: number): [number, number, number] {
  const t = Math.min(40000, Math.max(1000, kelvin)) / 100;
  const r = t <= 66 ? 255 : 329.698727446 * Math.pow(t - 60, -0.1332047592);
  const g = t <= 66 ? 99.4708025861 * Math.log(t) - 161.1195681661 : 288.1221695283 * Math.pow(t - 60, -0.0755148492);
  const b = t >= 66 ? 255 : t <= 19 ? 0 : 138.5177312231 * Math.log(t - 10) - 305.0447927307;
  return [Math.round(r), Math.round(g), Math.round(b)];
}

/** The aperture's thirds (f/0.95 to f/16), as a camera's dial steps them. */
export const APERTURE_STEPS: number[] = (() => {
  const out: number[] = [0.95];
  let n = 1.0;
  while (n < 16) {
    out.push(Math.round(n * 100) / 100);
    n *= Math.pow(2, 1 / 6); // thirds of a stop (two per displayed third)
  }
  out.push(16);
  return [...new Set(out.map((v) => Math.round(v * 100) / 100))];
})();

/** The f-number shown as "f/1.8" / "f/11". */
export function apertureText(n: number): string {
  return `f/${n < 10 ? n.toFixed(1).replace(/\.0$/, "") : n.toFixed(0)}`;
}

/**
 * The lens line the panel shows: "102 mm f/2.8 · shot at f/4 · focus 2.4 m", with "focus: from
 * your click" when EXIF has no focus distance. Missing EXIF is said plainly ("no lens in EXIF").
 */
export function lensLine(lens: BokehLens | null | undefined): string {
  if (!lens || (!lens.focalMm && !lens.fNumber && !lens.lens)) return "No lens in this photo's EXIF";
  const parts: string[] = [];
  if (lens.focalMm) parts.push(`${Math.round(lens.focalMm)} mm`);
  if (lens.fNumber) parts.push(`shot at ${apertureText(lens.fNumber)}`);
  parts.push(lens.focusDistanceM ? `focus ${lens.focusDistanceM < 10 ? lens.focusDistanceM.toFixed(1) : Math.round(lens.focusDistanceM)} m` : "focus: from your click");
  if (lens.lens) parts.push(lens.lens);
  return parts.join(" · ");
}
