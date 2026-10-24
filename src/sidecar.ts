import type { Adjustments } from "./adjustments";
import type { ProfileId, Spot } from "./api";
import type { Curves } from "./curves";
import type { Grade } from "./grade";
import type { Snapshot } from "./history";
import type { Mixer } from "./mixer";
import type { Crop } from "./crop";
import type { Gradient } from "./gradients";

/**
 * RapidOne's own sidecar (`.rpd`): plain JSON in a `sidecars` folder next to
 * the photos, one per photo (see `sidecar.rs`). Written as soon as a photo's
 * Lightroom XMP has been read, and on every edit after, so RapidOne takes
 * over; the XMP stays where it is for Lightroom (and, for now, RapidCulling).
 *
 * Only what differs from the photo's defaults is stored: unchanged sliders
 * are left out, and so are untouched curves, Mixer, Grade and profile.
 */

export const SIDECAR_FORMAT = "rapidone-sidecar";
export const SIDECAR_VERSION = 1;

export interface SidecarEdit {
  /** Changed sliders only, by id, in Lightroom's numbers. */
  sliders: Partial<Adjustments>;
  profile?: ProfileId;
  mixer?: Mixer;
  curves?: Curves;
  grade?: Grade;
  /** Crop, straighten and quarter turns (not part of presets). */
  crop?: Crop;
  /** Objects removed (their pictures in sidecars/removals), and patches (the Patch tool). */
  removals?: Spot[];
  /** Linear and radial gradients (local adjustments). */
  gradients?: Gradient[];
}

/** Where a photo's look came from: a style (Teach / apply look), a preset, or a Lightroom XMP (that one
 *  is known from `origin`, so never written as a look). */
export interface PhotoLook {
  from: "style" | "preset" | "xmp";
  name: string;
}

export interface SidecarFile {
  format: typeof SIDECAR_FORMAT;
  version: number;
  /** Where the edit first came from: the photo's Lightroom XMP, or made here. */
  origin: "lightroom-xmp" | "rapidone";
  /** When it was last written (ISO 8601). */
  updated: string;
  /** The look last put on it, a style (Teach / apply look) or a preset, for the thumbnail's icon. */
  look?: PhotoLook;
  edit: SidecarEdit;
  /** Stars (0-5, -1 = rejected) and pick, as in the XMP, for RapidCulling to share. */
  rating?: number;
  pick?: boolean;
}

export function editFromSnapshot(s: Snapshot): SidecarEdit {
  return {
    sliders: s.edits ?? {},
    ...(s.profile ? { profile: s.profile } : {}),
    ...(s.mixer ? { mixer: s.mixer } : {}),
    ...(s.curves ? { curves: s.curves } : {}),
    ...(s.grade ? { grade: s.grade } : {}),
    ...(s.crop ? { crop: s.crop } : {}),
    ...(s.removals?.length ? { removals: s.removals } : {}),
    ...(s.gradients?.length ? { gradients: s.gradients } : {}),
  };
}

export function snapshotFromEdit(e: SidecarEdit): Snapshot {
  return {
    edits: Object.keys(e.sliders ?? {}).length > 0 ? e.sliders : undefined,
    profile: e.profile,
    mixer: e.mixer,
    curves: e.curves,
    grade: e.grade,
    crop: e.crop,
    removals: e.removals?.length ? e.removals : undefined,
    gradients: e.gradients?.length ? e.gradients : undefined,
  };
}

/** The sidecar file's text. */
export function sidecarText(file: Omit<SidecarFile, "format" | "version" | "updated">): string {
  const full: SidecarFile = { format: SIDECAR_FORMAT, version: SIDECAR_VERSION, updated: new Date().toISOString(), ...file };
  return JSON.stringify(full, null, 2) + "\n";
}

/** Reads a sidecar's text; null if it isn't one we understand. */
/** A mask's adjustments (gradients.ts `Gradient`): one with all of them at 0, or hidden, changes nothing. */
const MASK_ADJUSTMENTS = ["exposure", "temperature", "tint", "contrast", "highlights", "shadows", "whites", "blacks", "texture", "clarity", "dehaze", "saturation"];

/** Whether an .rpd's edit changes anything, as `sidecar::has_edit`: a photo opened or reset keeps an empty
 *  one, and sliders at 0, straight-line curves or masks with no adjustment change nothing. */
export function editHasContent(edit: SidecarFile["edit"]): boolean {
  const straight = (c: unknown) =>
    !Array.isArray(c) || c.every((p: { x?: number; y?: number }) => typeof p?.x === "number" && typeof p?.y === "number" && Math.abs(p.x - p.y) < 1e-6);
  return Object.entries(edit ?? {}).some(([key, x]) => {
    if (x === null || x === undefined) return false;
    if (key === "sliders" && typeof x === "object") return Object.values(x).some((v) => typeof v !== "number" || v !== 0);
    if (key === "curves" && typeof x === "object") return Object.values(x).some((c) => !straight(c));
    if (key === "gradients" && Array.isArray(x))
      return x.some((g: Record<string, unknown>) => !g?.hidden && MASK_ADJUSTMENTS.some((k) => typeof g[k] === "number" && g[k] !== 0));
    return Array.isArray(x) ? x.length > 0 : typeof x === "object" ? Object.keys(x).length > 0 : true;
  });
}

export function parseSidecar(text: string | null): SidecarFile | null {
  if (!text) return null;
  try {
    const f = JSON.parse(text) as SidecarFile;
    if (f.format !== SIDECAR_FORMAT || typeof f.version !== "number" || f.version > SIDECAR_VERSION || !f.edit) return null;
    return { ...f, edit: { ...f.edit, sliders: f.edit.sliders ?? {} } };
  } catch {
    return null;
  }
}
