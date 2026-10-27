import type { SidecarEdit } from "./sidecar";
import type { Gradient } from "./gradients";
import type { Crop } from "./crop";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { CurvePoint, Curves } from "./curves";
import type { Grade, RangeGrade } from "./grade";
import { MIXER_COLOURS, type Mixer } from "./mixer";
import type { AutoTransform } from "./transform";
import type { FullPreview, PhotoInfo, ScanProgress } from "./types";

/** Resolves with the paths that failed to decode once the whole scan is done. */
export function scanFolder(folder: string): Promise<string[]> {
  return invoke("scan_folder", { folder });
}

export function getFullPreview(path: string): Promise<FullPreview> {
  return invoke("get_full_preview", { path });
}

export function setRatingAndPick(path: string, rating: number, pick: boolean): Promise<void> {
  return invoke("set_rating_and_pick", { path, rating, pick });
}

export function setRating(path: string, rating: number): Promise<void> {
  return invoke("set_rating", { path, rating });
}

export function onPhotoReady(handler: (photo: PhotoInfo) => void): Promise<UnlistenFn> {
  return listen<PhotoInfo>("photo-ready", (event) => handler(event.payload));
}

export function onScanProgress(handler: (progress: ScanProgress) => void): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan-progress", (event) => handler(event.payload));
}

export function autoTransform(path: string): Promise<AutoTransform> {
  return invoke("auto_transform", { path });
}

export interface DevelopedPreview extends FullPreview {
  camera: string;
  decodeMs: number;
  developMs: number;
  /** The exposure (stops) this image was rendered at. */
  exposure: number;
  /** The photo's as-shot white balance in Lightroom's numbers (null without camera matrices). */
  asShotTemperature: number | null;
  asShotTint: number | null;
}

/** RapidOne's own development of the RAW (step 1: superpixel, as-shot WB, camera matrix). */
export function developPreview(path: string): Promise<DevelopedPreview> {
  return invoke("develop_preview", { path });
}

/** Settings the engine renders with. Grows as sliders move into the engine. */
/** Sliders the RAW engine renders from real data (the rest are still quick previews). */
export const ENGINE_SLIDERS = [
  "exposure",
  "contrast",
  "highlights",
  "shadows",
  "whites",
  "blacks",
  "vibrance",
  "saturation",
  "calShadowTint",
  "calRedHue",
  "calRedSat",
  "calGreenHue",
  "calGreenSat",
  "calBlueHue",
  "calBlueSat",
  "paramShadows",
  "paramDarks",
  "paramLights",
  "paramHighlights",
  "paramShadowSplit",
  "paramMidSplit",
  "paramHighSplit",
  "vignette",
  "adaptiveVignette",
  // Texture and Clarity: the Mac's matched versions (definition-port, tools.rs).
  "texture",
  "clarity",
  // Dehaze: the Mac's model refitted on the PC's Dehaze kit (60 photos, +-100);
  // re-check through our own render once the colour engine is finished.
  "dehaze",
  // Sharpen: Lightroom-matched Richardson-Lucy (detail.rs). Noise: AI noise
  // reduction (denoise.rs), a blend towards the cleaned photo.
  "sharpen",
  "noise",
  "grain",
  // The legacy (Lightroom) vignette's shape (Lightroom's names; its amount is "vignette").
  "vignetteMidpoint",
  "vignetteRoundness",
  "vignetteFeather",
  "vignetteHighlights",
] as const;

/**
 * Profiles (tone curve + colour character), as the engine names them:
 * RapidOne's own Shinobi profiles, and Adobe Color read from Adobe's profiles
 * when they're installed on this computer.
 */
/**
 * A photo's profile: RapidOne's built-in Shinobi ones, a profile the user
 * added (`user:<name>`, from Documents/RapidOne/Profiles), or Adobe Color /
 * Adobe Standard, which use the user's added profiles of those names (a
 * Lightroom edit asks for them by these ids).
 */
export type ProfileId = "shinobiStandard" | "shinobiColour" | "adobeColor" | "adobeStandard" | `user:${string}`;

/** A profile the user added, by name, and the cameras it covers. */
export interface UserProfile {
  name: string;
  kind: "camera" | "look";
  cameras: string[];
}

/**
 * Zoomed-in detail: the part of the photo on screen (`rect`: left, top,
 * right, bottom as fractions of the photo as shown), rendered at full
 * resolution with these settings, at most `width` x `height` (a JPEG data URL).
 */
export function renderDetail(
  path: string,
  settings: RenderSettings,
  rect: [number, number, number, number],
  width: number,
  height: number,
  interactive = false,
): Promise<string> {
  return invoke("render_detail", { path, settings, rect, width, height, interactive });
}

/** The profiles the user has added (Documents/RapidOne/Profiles). */
export function userProfiles(): Promise<UserProfile[]> {
  return invoke("user_profiles");
}

/** Adds .dcp / .xmp profile files (copied into their profiles folder): [names added, files skipped with why]. */
export function addUserProfiles(paths: string[]): Promise<[string[], string[]]> {
  return invoke("add_user_profiles", { paths });
}

/** The id a user profile is chosen by: Adobe's two keep the ids Lightroom edits use. */
export function userProfileId(name: string): ProfileId {
  return name === "Adobe Color" ? "adobeColor" : name === "Adobe Standard" ? "adobeStandard" : `user:${name}`;
}

/** The name of a user profile id, or null for a built-in one. */
export function userProfileName(id: ProfileId): string | null {
  if (id === "adobeColor") return "Adobe Color";
  if (id === "adobeStandard") return "Adobe Standard";
  return id.startsWith("user:") ? id.slice(5) : null;
}

/** RapidOne's built-in profiles (and Adobe's two ids, for labels; those come from the user's own profiles). */
export const PROFILES: { id: ProfileId; label: string; hint: string; needsAdobe?: boolean }[] = [
  { id: "shinobiColour", label: "Shinobi Colour", hint: "RapidOne's default: a touch more contrast and colour (close to Adobe Color)" },
  { id: "shinobiStandard", label: "Shinobi Standard", hint: "Flatter and more neutral (close to Adobe Standard)" },
  {
    id: "adobeColor",
    label: "Adobe Color",
    hint: "Adobe Color from your own profiles (cameras you haven't added it for show Shinobi Colour)",
    needsAdobe: true,
  },
  {
    id: "adobeStandard",
    label: "Adobe Standard",
    hint: "Adobe Standard from your own profiles (cameras you haven't added it for show Shinobi Standard)",
    needsAdobe: true,
  },
];

/** The Shinobi profile an Adobe one falls back to when Adobe's aren't installed. */
export function profileOrFallback(profile: ProfileId, adobeAvailable: boolean): ProfileId {
  if (adobeAvailable) return profile;
  return profile === "adobeColor" ? "shinobiColour" : profile === "adobeStandard" ? "shinobiStandard" : profile;
}

/** Whether Adobe's profiles are installed, so Adobe Color can be offered. */
export function adobeColorAvailable(): Promise<boolean> {
  return invoke<boolean>("adobe_color_available");
}

export const DEFAULT_PROFILE: ProfileId = "shinobiColour";

/** A tone range for the engine, in Lightroom's numbers: hue 0..360, sat 0..100, lum -100..100. */
interface EngineGradeRange {
  hue: number;
  sat: number;
  lum: number;
}

export type RenderSettings = Record<(typeof ENGINE_SLIDERS)[number], number> & {
  temperature?: number;
  tint?: number;
  profile: ProfileId;
  /** A profile the user added, by name (Documents/RapidOne/Profiles). */
  userProfile?: string;
  /** Per band, red .. magenta: [hue, sat, lum]. */
  mixer: [number, number, number][];
  /** Crop, straighten and quarter turns, applied by the engine last. */
  crop: Crop;
  /** Objects removed, blended into the photo before everything else. */
  removals: Spot[];
  /** Linear and radial gradients (local adjustments). */
  gradients: Gradient[];
  /** Keystone (Transform sliders), warped by the engine before the crop. */
  keystone: { vertical: number; horizontal: number; rotate: number; scale: number };
  /** Point curves in Lightroom's 0..255 units. */
  curveLuma: [number, number][];
  curveRgb: [number, number][];
  curveRed: [number, number][];
  curveGreen: [number, number][];
  curveBlue: [number, number][];
  grade: {
    shadows: EngineGradeRange;
    mids: EngineGradeRange;
    highs: EngineGradeRange;
    global: EngineGradeRange;
    blending: number;
    balance: number;
  };
};

/** A wheel's dot (x, y within the unit circle; right = red, round anticlockwise) as Lightroom's hue and saturation. */
function wheelToHueSat(r: RangeGrade): EngineGradeRange {
  const hue = ((Math.atan2(-r.y, r.x) * 180) / Math.PI + 360) % 360;
  return { hue, sat: Math.min(1, Math.hypot(r.x, r.y)) * 100, lum: r.lum };
}

const curveForEngine = (points: CurvePoint[]): [number, number][] => points.map((p) => [p.x * 255, p.y * 255]);

/**
 * Temperature (Kelvin) and Tint (Lightroom units) are sent only once the
 * photo's as-shot values are known (`withWhiteBalance`); before that the
 * engine renders as shot.
 */
export function engineSettingsFrom(
  values: Record<string, number>,
  withWhiteBalance: boolean,
  profile: ProfileId,
  mixer: Mixer,
  curves: Curves,
  grade: Grade,
  crop: Crop,
  removals: Spot[] = [],
  gradients: Gradient[] = [],
): RenderSettings {
  const settings = {
    ...Object.fromEntries(ENGINE_SLIDERS.map((k) => [k, values[k] ?? 0])),
    // A user's own profile: by name, falling back to Shinobi Colour where it
    // doesn't cover the camera. Adobe's two ids go as they are (the engine
    // asks for the user's profiles of those names).
    profile: profile.startsWith("user:") ? "shinobiColour" : profile,
    ...(profile.startsWith("user:") ? { userProfile: profile.slice(5) } : {}),
    crop,
    removals,
    gradients,
    keystone: { vertical: values.vertical ?? 0, horizontal: values.horizontal ?? 0, rotate: values.rotate ?? 0, scale: values.scale ?? 100 },
    mixer: MIXER_COLOURS.map((c) => [mixer[c.id].hue, mixer[c.id].sat, mixer[c.id].lum]),
    curveLuma: curveForEngine(curves.luma),
    curveRgb: curveForEngine(curves.rgb),
    curveRed: curveForEngine(curves.r),
    curveGreen: curveForEngine(curves.g),
    curveBlue: curveForEngine(curves.b),
    grade: {
      shadows: wheelToHueSat(grade.shadows),
      mids: wheelToHueSat(grade.mids),
      highs: wheelToHueSat(grade.highs),
      global: wheelToHueSat(grade.global),
      blending: grade.blending,
      balance: grade.balance,
    },
  } as RenderSettings;
  // Lens corrections (Lensfun): this photo's own switch, else "Correct lenses automatically";
  // `lensRev` changes the settings (and the cache key) when a lens choice changes.
  const lensOn = values.lensCorrect ?? (lensPrefs.auto ? 100 : 0);
  (settings as Record<string, unknown>).lensCorrect = lensOn;
  if (lensOn > 0) (settings as Record<string, unknown>).lensRev = lensPrefs.rev;
  if (withWhiteBalance) {
    settings.temperature = values.temperature;
    settings.tint = values.tint;
  }
  return settings;
}

/**
 * An object removed from a photo (non-destructive): the model's patch, the
 * part of the photo it covers and the object's area, as fractions of the
 * upright, uncropped photo [left, top, right, bottom].
 */
export interface Removal {
  file: string;
  crop: [number, number, number, number];
  area: [number, number, number, number];
  ev: number;
}

/** A patch (the Patch tool, patch.rs): an area given another's texture (Heal) or an exact copy
 *  (Clone). Kept in the same list as the removals ("kind" tells it apart); fractions of the
 *  upright, uncropped photo; the source is the outline moved by `offset`. */
export interface Patch {
  kind: "patch";
  polygon: [number, number][];
  offset: [number, number];
  mode: "heal" | "clone";
  /** Edge softness, a share of the area's size (left out: the mode's own). */
  feather?: number;
}

/** One entry of a photo's removals list: a removal or a patch. */
export type Spot = Removal | Patch;

export const isPatch = (s: Spot): s is Patch => (s as Patch).kind === "patch";

/** Whether object removal is set up on this computer. */
export function removalAvailable(): Promise<boolean> {
  return invoke("removal_available");
}

/** Where renders run (the Render setting): the GPU if usable, else the CPU (Auto); the GPU, saying
 *  when it falls back (GPU); always the CPU (CPU). */
export type RenderMode = "auto" | "gpu" | "cpu";

/** The Render setting, the GPU found, whether the last render ran on it, and why not if it didn't. */
export interface RenderStatus {
  mode: RenderMode;
  adapter: string | null;
  onGpu: boolean;
  note: string | null;
}

export function renderStatus(): Promise<RenderStatus> {
  return invoke("render_status");
}

/** Sets the Render setting; the next render uses it (no restart). */
export function setRenderMode(mode: RenderMode): Promise<RenderStatus> {
  return invoke("set_render_mode", { mode });
}

export type RemovalMode = "large" | "medium" | "lite";

/** The removers installed - Large (FLUX.2 Klein + SAM 3), Medium (RORem +
 *  SAM 3), Lite (LaMa + MobileSAM + Grounding DINO, any computer) - and the
 *  one in use. */
export interface RemovalModes {
  large: boolean;
  medium: boolean;
  lite: boolean;
  mode: RemovalMode;
}

export function removalModes(): Promise<RemovalModes> {
  return invoke("removal_modes");
}

/** The Remove bar's Large / Medium / Lite switch. */
export function setRemovalMode(mode: RemovalMode): Promise<void> {
  return invoke("set_removal_mode", { mode });
}

/** Removes what's in `area` from the photo (`what` optionally names it, `outline` is SAM 3's shape); the removal to add to its edit. */
export function removeObject(path: string, area: [number, number, number, number], what: string, outline?: string, seed = 0): Promise<Removal> {
  return invoke("remove_object", { path, area, what, outline: outline ?? null, seed });
}

/** An object SAM 3 found: its area, outline file (for removeObject), confidence and a picture of the outline to lay over the photo. */
export interface Outline {
  area: [number, number, number, number];
  outline: string;
  score: number;
  overlay: string;
}

/** Brush: removes the painted shape (a white-on-black PNG data URL covering the whole uncropped photo). */
export function removePainted(path: string, painted: string, what: string, seed = 0): Promise<Removal> {
  return invoke("remove_painted", { path, painted, what, seed });
}

/** Cleans a neighbouring photo's noise ahead of time (it has Noise on), so stepping to it is instant. */
export function prepareNoise(path: string): Promise<void> {
  return invoke("prepare_noise", { path });
}

/** A shoot organised by Cull: its `rated` folder (Edit opens that instead), or null. */
/** The shoot's views once Organise has sorted it (All = its own folder, Rated, Unrated), or null. */
export function folderViews(folder: string): Promise<import("./components/ViewSwitch").FolderViews | null> {
  return invoke("folder_views", { folder });
}

export function ratedSubfolder(folder: string): Promise<string | null> {
  return invoke("rated_subfolder", { folder });
}

/** The stars and pick of these photos as their sidecars now say (after rating in Cull), as [path, rating, pick]. */
export function readFlags(paths: string[]): Promise<[string, number, boolean][]> {
  return invoke("read_flags", { paths });
}

/** Loads the noise model in the background (Edit opening), so the first photo with Noise on doesn't wait. */
export function warmDenoise(): Promise<void> {
  return invoke("warm_denoise");
}

/** Loads Lite's Remove models in the background (nothing happens unless Lite is the remover in use). */
export function warmLite(): Promise<void> {
  return invoke("warm_lite");
}

/** Whether objects can be found by words (SAM 3 is set up). */
export function canFindObjects(): Promise<boolean> {
  return invoke("can_find_objects");
}

/** The Remove tool opened or closed: its AI models load with it and leave the GPU shortly after it closes. */
export function removalHold(open: boolean): Promise<void> {
  return invoke("removal_hold", { open });
}

/** Remove opened on this photo: SAM 3 analyses it in the background, so boxes outline quickly. */
export function prepareFind(path: string): Promise<void> {
  return invoke("prepare_find", { path });
}

/** SAM 3: every object in the photo matching `words`, most confident first;
 *  or, with `area` and no words, the object in that box. */
export function findObjects(path: string, words: string, area?: [number, number, number, number]): Promise<Outline[]> {
  return invoke("find_objects", { path, words, area: area ?? null });
}

/** Develops a photo into the engine's cache ahead of time (a neighbour of the one on screen). */
export function prefetchScene(path: string): Promise<void> {
  return invoke("prefetch_scene", { path });
}

/** The photo's RapidOne sidecar (.rpd) text, or null if it has none yet. */
export function readSidecar(path: string): Promise<string | null> {
  return invoke("read_sidecar", { path });
}

/** A photo's lens: the name in its EXIF, and the database's match for it (id, name). */
export interface LensInfo {
  exif: string | null;
  detected: [number, string] | null;
  /** The lens chosen by hand for this EXIF name, if one was. */
  chosen: [number, string] | null;
}

/** The photo's lens: its EXIF name, the database's match and any lens chosen for that name. */
export function lensInfo(path: string): Promise<[LensInfo, string | null]> {
  return invoke("lens_info", { path, lensId: 0 });
}

/** App-wide lens settings, set by the app: "Correct lenses automatically", and a number
 *  bumped when a lens choice or the profiles change (so renders aren't taken from the cache). */
export const lensPrefs = { auto: false, rev: 0 };

/** Chooses a lens (0: none) for every photo whose EXIF names `exif`. */
export function lensChoose(exif: string, lensId: number): Promise<void> {
  return invoke("lens_choose", { exif, lensId });
}

/** Each photo's EXIF lens name ("" if none), from its metadata. */
export function lensNames(paths: string[]): Promise<string[]> {
  return invoke("lens_names", { paths });
}

/** The user's own lens profiles (Lensfun .xml): [lenses added, problems]. */
export function lensAddProfiles(paths: string[]): Promise<[string[], string[]]> {
  return invoke("lens_add_profiles", { paths });
}

/** The lens database in use: [Lensfun commit, its date (ISO), lenses]. */
export function lensDbVersion(): Promise<[string, string, number]> {
  return invoke("lens_db_version");
}

/** Saves a newer lens database (Lensfun's files: [name, text]); returns the lenses it has. */
export function lensDbSave(files: [string, string][], sha: string, date: string): Promise<number> {
  return invoke("lens_db_save", { files, sha, date });
}

/** Lenses in the database matching every word typed: [id, name]. */
export function lensSearch(query: string): Promise<[number, string][]> {
  return invoke("lens_search", { query });
}

/** Writes the photo's RapidOne sidecar (.rpd). */
export function writeSidecar(path: string, contents: string): Promise<void> {
  return invoke("write_sidecar", { path, contents });
}

/** Finds the photo's subject mask in the background (for the subject-aware vignette). */
/** A mask (any kind, combined or not) as a red overlay over the whole frame (PNG data URL); null without what it needs. */
export function maskOverlay(path: string, mask: import("./gradients").Gradient): Promise<string | null> {
  return invoke("mask_overlay", { path, mask });
}

/** The photo's subject mask as a PNG (base64), to freeze into a combined mask when it's copied. */
export function subjectSnapshot(path: string): Promise<string | null> {
  return invoke("subject_snapshot", { path });
}

/** A photo rendered with these settings: its fit-to-screen picture and thumbnail (data URL), without disturbing the photos being edited. */
export function refreshPhoto(path: string, settings: RenderSettings): Promise<{ preview: DevelopedPreview; thumb: string }> {
  return invoke("refresh_photo", { path, settings });
}

/** Keeps a picture in the folder's cache under the app's settings key for it. */
export function cachePreview(path: string, key: string, preview: DevelopedPreview): Promise<void> {
  return invoke("cache_preview", { path, key, preview });
}

/** Finds a photo's subject mask ahead of time (kept in the folder's cache), without disturbing the photos being edited. */
export function findSubject(path: string): Promise<boolean> {
  return invoke("find_subject", { path });
}

export function prepareSubject(path: string): Promise<boolean> {
  return invoke("prepare_subject", { path });
}

/** Re-renders a developed photo from the engine's cached scene data. */
export function renderPreview(path: string, settings: RenderSettings, cacheKey?: string, interactive = false, shownLong?: number, background = false): Promise<DevelopedPreview> {
  return invoke("render_preview", { path, settings, cacheKey: cacheKey ?? null, interactive, shownLong: shownLong ?? null, background });
}

/**
 * The photo as graded in Edit, full size and 16-bit, for Retouch: a TIFF in the
 * folder's sidecars/handoff/ (its path). Rendered again only when `key` (the
 * grade) has changed.
 */
export function renderHandoff(path: string, settings: RenderSettings, key: string): Promise<string> {
  return invoke("render_handoff", { path, settings, key });
}

/** Adds one photo (such as the TIFF Retouch handed back) to the library: its thumbnail, stars and capture time. */
export function addPhoto(path: string): Promise<PhotoInfo> {
  return invoke("add_photo", { path });
}

/** Clears the hand-off TIFFs (a cache for Retouch) of these folders' shoots; returns the bytes freed. */
export function clearHandoffs(folders: string[]): Promise<number> {
  return invoke("clear_handoffs", { folders });
}

/** Photos in an organised shoot's `rated` folder moved to its `unrated` folder (with .xmp, JPEG, .rpd). Returns those moved. */
export function moveToUnrated(paths: string[]): Promise<string[]> {
  return invoke("move_to_unrated", { paths });
}

/** Moves photos (with their .xmp and .rpd) to `sidecars/trash` in their folder; nothing is deleted. Returns the photos moved. */
/** A photo sent to the trash folder: where it was, and where it is now (for Undo). */
export interface Trashed {
  path: string;
  trashed: string;
}

export function trashPhotos(paths: string[]): Promise<Trashed[]> {
  return invoke("trash_photos", { paths });
}

/** Puts photos back from the trash folder (Undo after a delete); the photos put back. */
export function restorePhotos(items: Trashed[]): Promise<string[]> {
  return invoke("restore_photos", { items });
}

/** How exported files are named: the photo's own name, or a name and a number in capture-time order. */
export interface ExportNamingArgs {
  mode: "original" | "sequence";
  base?: string;
  start?: number;
  digits?: number;
}

/** One planned export file, in export (capture-time) order. */
export interface PlannedExport {
  path: string;
  out: string;
  /** The output is already there (an earlier export, or something else). */
  exists: boolean;
}

/** Where each photo's export goes, and which of those files already exist. */
export function exportPlan(
  sources: { path: string; timestampMs: number }[],
  naming: ExportNamingArgs,
  folder: string | null,
  format: "jpeg" | "tiff",
): Promise<PlannedExport[]> {
  return invoke("export_plan", { sources, naming, folder, format });
}

/**
 * Renders one photo at full size with these settings (RapidOne's own engine,
 * so the colours are the viewer's) and writes it to `out`. `longEdge` scales
 * it down (never up). Returns the file written.
 */
export function exportPhoto(
  path: string,
  settings: RenderSettings,
  out: string,
  format: "jpeg" | "tiff",
  longEdge: number | null,
): Promise<string> {
  return invoke("export_photo", { path, settings, out, format, longEdge });
}

/** An Auto Style style (a model learned from the photographer's edits). */
export interface AutoStyle {
  name: string;
  path: string;
  /** Edited photos and shoots it learned from (0: an older style that can't be added to). */
  photos: number;
  shoots: number;
}

/** The installed Auto Style styles. */
export function autoStyleList(): Promise<AutoStyle[]> {
  return invoke("auto_style_list");
}

/** A style's look (every setting its edits share): from an .xmp as preset settings, or a RapidOne edit
 *  (.rpd form, from a style trained mostly on RapidOne edits); both null without one. */
export function autoStyleLook(style: string): Promise<{ lightroom: LightroomSettings | null; rapidone: SidecarEdit | null }> {
  return invoke("auto_style_look", { style });
}

/** Auto Style for one photo: its white balance (Kelvin, tint) and exposure as the style would set them. */
/** A photo's ISO and aperture (f-number); either can be missing. */
export function shotDetails(path: string): Promise<{ iso: number | null; aperture: number | null }> {
  return invoke("shot_details", { path });
}

/** The computer's core count (the webview's navigator.hardwareConcurrency can be capped). */
/** How many photos Export does at once on this computer (its cores and free memory, `export::workers_for`). */
export function exportWorkers(): Promise<number> {
  return invoke("export_workers");
}

export function coreCount(): Promise<number> {
  return invoke("core_count");
}

/** A style's settings for one photo; contrast null for a style trained before it learned Contrast. */
export function autoStylePhoto(style: string, path: string): Promise<{ temperature: number; tint: number; exposure: number; contrast: number | null }> {
  return invoke("auto_style_photo", { style, path });
}

/**
 * Train Style: a new style called `name` from folders of edited RAWs (only
 * RAWs with a Lightroom .xmp edit count), or with `style` (Increase training)
 * the folders added to that style and it retrained on everything.
 */
export function autoStyleTrain(name: string, folders: string[], style?: string): Promise<AutoStyle> {
  return invoke("auto_style_train", { name, folders, style: style ?? null });
}

export interface StyleProgress {
  stage: "reading" | "checking" | "training";
  done: number;
  total: number;
}

export function onStyleProgress(handler: (p: StyleProgress) => void): Promise<UnlistenFn> {
  return listen<StyleProgress>("style-progress", (event) => handler(event.payload));
}

/** A photo's last engine picture from the folder's cache (sidecars/cache), with the settings key it was rendered with. */
/** Photos whose edits were reset: their thumbnails back to the camera's picture ([path, thumbnail]). */
export function cameraThumbs(paths: string[]): Promise<[string, string][]> {
  return invoke("camera_thumbs", { paths });
}

/** Just the kept picture's settings key and the as-shot white balance it read (no picture, nothing touched). */
export function cachedPreviewKey(path: string): Promise<[string, number | null, number | null] | null> {
  return invoke("cached_preview_key", { path });
}

export function cachedPreview(path: string): Promise<{ key: string; preview: DevelopedPreview } | null> {
  return invoke("cached_preview", { path });
}

/**
 * Lightroom develop settings (from a preset or a photo's sidecar), in
 * RapidOne's terms - only what the XMP sets. Our sliders take Lightroom's
 * numbers as they are.
 */
export interface LightroomSettings {
  name: string | null;
  /** Slider id -> value. */
  sliders: Record<string, number>;
  whiteBalance: "asShot" | "custom" | null;
  temperature: number | null;
  tint: number | null;
  profile: ProfileId | null;
  /** Band -> hue / sat / lum. */
  mixer: Partial<Record<string, Partial<Record<"hue" | "sat" | "lum", number>>>>;
  /** Point curves, Lightroom's 0..255: rgb, r, g, b. */
  /** Lightroom's are rgb / r / g / b; RapidOne's own presets add luma. */
  curves: Partial<Record<"luma" | "rgb" | "r" | "g" | "b", [number, number][]>>;
  /** shadows / mids / highs / global -> hue (0..360), sat (0..100), lum; "" -> blending, balance. */
  grade: Partial<Record<string, Partial<Record<string, number>>>>;
  /** Lightroom settings present that RapidOne doesn't apply yet. */
  skipped: string[];
  /** The photo's own crop (a photo's Lightroom edit only), in RapidOne's terms. */
  crop?: Crop | null;
  /** Its Linear and Radial gradients (a photo's Lightroom edit only). */
  gradients?: Gradient[];
}

/** RapidOne's own presets (JSON each), from the app's presets folder. */
export function listPresets(): Promise<string[]> {
  return invoke("list_presets");
}

/** Saves a RapidOne preset under its name (the same name replaces it). */
export function savePreset(name: string, contents: string): Promise<void> {
  return invoke("save_preset", { name, contents });
}

/** Copies every preset into a folder (a backup); how many. */
export function exportPresets(folder: string): Promise<number> {
  return invoke("export_presets", { folder });
}

/** A text file's contents (a RapidOne preset being imported). */
export function readTextFile(path: string): Promise<string> {
  return invoke("read_text_file", { path });
}

export function deletePreset(name: string): Promise<void> {
  return invoke("delete_preset", { name });
}

/** Reads a Lightroom preset (.xmp). */
export function readLightroomPreset(path: string): Promise<LightroomSettings> {
  return invoke("read_lightroom_preset", { path });
}

/** A photo's own Lightroom edit from its XMP sidecar, or null if it has none. */
export function readLightroomEdit(path: string): Promise<LightroomSettings | null> {
  return invoke("read_lightroom_edit", { path });
}

/** The parametric (region) curve as the engine applies it, sampled evenly over 0..1, for drawing. */
export function parametricCurvePoints(amounts: [number, number, number, number], splits: [number, number, number]): Promise<number[]> {
  return invoke("parametric_curve_points", { amounts, splits });
}
