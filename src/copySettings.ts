import { SLIDER_GROUPS } from "./adjustments";
import type { Snapshot } from "./history";

/** Parts of a photo's edit stored outside its sliders. */
type Part = "curves" | "grade" | "mixer" | "profile" | "crop" | "gradients";

export interface CopyGroup {
  id: string;
  label: string;
  sliders: string[];
  parts: Part[];
  /** Ticked unless the user changes it: per-photo things (keystone, crop) aren't. */
  byDefault: boolean;
}

const slidersOf = (name: string) => SLIDER_GROUPS.find((g) => g.name === name)?.sliders.map((s) => s.id) ?? [];

/**
 * What Copy settings can copy, Lightroom-style. White balance and Exposure
 * are separate so a pasted look can leave Auto Style's per-photo values alone.
 */
export const COPY_GROUPS: CopyGroup[] = [
  { id: "wb", label: "White balance", sliders: ["temperature", "tint"], parts: [], byDefault: true },
  { id: "exposure", label: "Exposure", sliders: ["exposure"], parts: [], byDefault: true },
  { id: "tone", label: "Tone (contrast, highlights, shadows, whites, blacks)", sliders: slidersOf("Tone").filter((id) => id !== "exposure"), parts: [], byDefault: true },
  { id: "curves", label: "Curves", sliders: slidersOf("Parametric"), parts: ["curves"], byDefault: true },
  { id: "definition", label: "Definition (texture, clarity, dehaze, vibrance, saturation)", sliders: slidersOf("Definition"), parts: [], byDefault: true },
  { id: "colour", label: "Colour (profile, mixer, grade, fine tuning)", sliders: slidersOf("Fine tuning"), parts: ["profile", "mixer", "grade"], byDefault: true },
  { id: "detail", label: "Sharpen and noise", sliders: slidersOf("Sharpen"), parts: [], byDefault: true },
  { id: "lens", label: "Lens corrections", sliders: ["lensCorrect"], parts: [], byDefault: true },
  { id: "effects", label: "Effects (vignette, grain)", sliders: [...slidersOf("Effects"), ...slidersOf("Legacy vignette")], parts: [], byDefault: true },
  // autoTransform: which auto straighten the photo has (lights its button).
  { id: "keystone", label: "Keystone and rotation", sliders: [...slidersOf("Keystone"), "autoTransform"], parts: [], byDefault: false },
  { id: "crop", label: "Crop", sliders: [], parts: ["crop"], byDefault: false },
  { id: "gradients", label: "Masks (subject, brush, linear, radial)", sliders: [], parts: ["gradients"], byDefault: false },
];

export const DEFAULT_COPY_GROUPS = COPY_GROUPS.filter((g) => g.byDefault).map((g) => g.id);

/**
 * `target` with the chosen groups taken from `source`: a setting the source
 * leaves at its default goes back to the default on the target too, so the
 * pasted groups end up exactly as on the source.
 */
export function pasteSnapshot(source: Snapshot, target: Snapshot, groups: string[]): Snapshot {
  const out: Snapshot = { ...target };
  const edits: Record<string, number> = { ...(target.edits as Record<string, number> | undefined) };
  const from = (source.edits ?? {}) as Record<string, number>;
  for (const g of COPY_GROUPS) {
    if (!groups.includes(g.id)) continue;
    for (const id of g.sliders) {
      if (from[id] !== undefined) edits[id] = from[id];
      else delete edits[id];
    }
    for (const part of g.parts) {
      // Each part's type matches between snapshots; TypeScript can't see it through the key.
      (out as Record<Part, unknown>)[part] = source[part];
    }
  }
  out.edits = Object.keys(edits).length > 0 ? edits : undefined;
  return out;
}
