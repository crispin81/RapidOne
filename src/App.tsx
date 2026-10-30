import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";
import "./moduleTweaks.css";
import { ALL_SLIDERS, DEFAULTS, formatValue, nudge, panelForKey, SLIDER_GROUPS, type Adjustments, type SliderDef } from "./adjustments";
import { DEFAULT_PICKS, MIXER_HOLD, holdTarget, tapPick, type PanelPicks } from "./panelKeys";
import {
  adobeColorAvailable,
  userProfiles as fetchUserProfiles,
  renderDetail,
  addUserProfiles,
  type UserProfile,
  autoTransform,
  developPreview,
  getFullPreview,
  onPhotoReady,
  onScanProgress,
  readLightroomEdit,
  readLightroomPreset,
  listPresets,
  exportPresets,
  readTextFile,
  savePreset,
  deletePreset,
  prefetchScene,
  cachedPreview,
  cameraThumbs,
  prepareSubject,
  maskOverlay,
  subjectSnapshot,
  lensChoose,
  lensAddProfiles,
  lensPrefs,
  findSubject,
  refreshPhoto,
  cachePreview,
  cachedPreviewKey,
  readSidecar,
  writeSidecar,
  trashPhotos,
  restorePhotos,
  clearHandoffs,
  moveToUnrated,
  renderPreview,
  renderHandoff,
  autoStyleList,
  autoStylePhoto,
  coreCount,
  shotDetails,
  autoStyleLook,
  autoStyleTrain,
  onStyleProgress,
  removalAvailable,
  removalModes,
  setRemovalMode,
  type RemovalMode,
  type RemovalModes,
  removeObject,
  canFindObjects,
  findObjects,
  prepareFind,
  addPhoto,
  removalHold,
  removePainted,
  warmDenoise,
  warmLite,
  ratedSubfolder,
  folderViews,
  readFlags,
  prepareNoise,
  type Outline,
  type Removal,
  type Spot,
  type Patch,
  isPatch,
  type AutoStyle,
  type Trashed,
  scanFolder,
  cancelScan,
  setRatingAndPick,
  engineSettingsFrom,
  DEFAULT_PROFILE,
  type LightroomSettings,
  type ProfileId,
  type RenderSettings,
} from "./api";
import { applyLightroom, describeApplied, parsePreset, presetFromLightroom, presetFromState, presetText, type RapidPreset } from "./preset";
import PresetMenu, { type PresetScope } from "./components/PresetMenu";
import ContextMenu, { type ContextMenuItem } from "./components/ContextMenu";
import { Check, RotateCcw, Star, Trash2 } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import AutoStyleMenu, { type AutoStyleScope } from "./components/AutoStyleMenu";
import DenoiseMenu from "./components/DenoiseMenu";
import LensMenu from "./components/LensMenu";
import { checkLensUpdates } from "./lensUpdates";
import { NO_STRIP_FILTER, passesStripFilter, type StripFilter } from "./components/Filmstrip";
import CopyPasteMenu from "./components/CopyPasteMenu";
import RemoveOverlay from "./components/RemoveOverlay";
import PatchOverlay from "./components/PatchOverlay";
// The R1 icon, for Retouch's engine setup screen.
import rapidOneIcon from "../src-tauri/icons/128x128@2x.png";
import { DEFAULT_COPY_GROUPS, pasteSnapshot } from "./copySettings";
import { roundRotate, tiltToVertical } from "./transform";
import BottomPanel from "./components/BottomPanel";
import Filmstrip, { StripFilterBar } from "./components/Filmstrip";
import TitleBar from "./components/TitleBar";
import TopBar from "./components/TopBar";
import type { Mode } from "./components/ModeSwitch";
import { RapidCulling } from "rapidculling/ui";
import { ExportDialog, RapidRetouch } from "rapidretouch/ui";
import Library from "./components/Library";
import { useExport } from "./useExport";
import Viewer, { DragScale } from "./components/Viewer";
import { DEFAULT_CURVES, type Curves } from "./curves";
import { DEFAULT_GRADE, type Grade } from "./grade";
import { DEFAULT_MIXER, type Mixer } from "./mixer";
import { computeHistogram, type Histogram } from "./histogram";
import { isNoCrop, NO_CROP, straightCrop, type Crop } from "./crop";
import { hasSubject, isMaskSlider, MASK_SLIDER_IDS, maskSliderDef, type Gradient } from "./gradients";
import GradientOverlay from "./components/GradientOverlay";
import ViewSwitch, { folderFor, viewOf, type FolderViews } from "./components/ViewSwitch";
import { editFromSnapshot, editHasContent, parseSidecar, sidecarText, snapshotFromEdit, type PhotoLook, type SidecarFile } from "./sidecar";
import { keyLabel } from "./macKeys";
import { newHistory, recordChange, redo, undo, withEntry, type History, type Snapshot } from "./history";
import type { FullPreview, PhotoInfo, ScanProgress, ViewerImage } from "./types";

const SHOW_NAMES_KEY = "rapidone.showSliderNames";
/** The corridor lines and D65 halo round the photo, on or off (per machine). */
const SURROUND_KEY = "rapidone.d65Surround";

function loadSurround(): boolean {
  try {
    return localStorage.getItem(SURROUND_KEY) !== "false";
  } catch {
    return true;
  }
}
/** The last photo worked on (its path), for the start screen's picture. */
const LAST_PHOTO_KEY = "rapidone.lastPhoto";
/** The last working folder, for "resume last session" on the home screen. */
const LAST_FOLDER_KEY = "rapidone.lastFolder";
/** "Correct lenses automatically" (the Lens button): "1" on. */
const LENS_AUTO_KEY = "rapidone.lensAuto";
/** No removals (a stable value for photos without any). */
const NO_REMOVALS: Spot[] = [];
/** No gradients (a stable value for photos without any). */
const NO_GRADIENTS: Gradient[] = [];
/** The setting groups ticked in Copy settings. */
const COPY_GROUPS_KEY = "rapidone.copyGroups";

/** A photo's file name without its folder or extension. */
const baseName = (path: string) => path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, "") ?? path;

/** The names toggle is a per-machine preference; storage may be unavailable, so default to on. */
function loadShowNames(): boolean {
  try {
    return localStorage.getItem(SHOW_NAMES_KEY) !== "false";
  } catch {
    return true;
  }
}

function saveShowNames(show: boolean) {
  try {
    localStorage.setItem(SHOW_NAMES_KEY, String(show));
  } catch {
    // Not fatal - the setting just won't be remembered next launch.
  }
}

const SLIDERS_BY_ID = Object.fromEntries(ALL_SLIDERS.map((d) => [d.id, d]));

/** A photo and the TIFFs Retouch handed back for it. */
interface Stack {
  /** The original (the RAW). */
  parent: string;
  /** The original first, then the handed-back TIFFs, oldest to newest. */
  members: string[];
  /** The newest TIFF: what a collapsed stack shows. */
  top: string;
}

/** `<name>_retouched.tif`, `<name>_retouched-2.tif` ...: the name without the ending, and the version. */
const RETOUCHED = /^(.*)_retouched(?:-(\d+))?$/i;
const stackKey = (path: string) => path.replace(/\\/g, "/").replace(/\.[^./]+$/, "").toLowerCase();

/** The stacks in a folder, by member path (every member, the original too, maps to its stack). */
function buildStacks(photos: PhotoInfo[]): Map<string, Stack> {
  const originals = new Map<string, PhotoInfo>();
  const children = new Map<string, { path: string; version: number }[]>();
  for (const p of photos) {
    const key = stackKey(p.path);
    if (!RETOUCHED.test(key) || !/\.tiff?$/i.test(p.path)) originals.set(key, p);
  }
  for (const p of photos) {
    const m = RETOUCHED.exec(stackKey(p.path));
    if (!m || !/\.tiff?$/i.test(p.path)) continue;
    const parent = originals.get(m[1]);
    if (!parent) continue;
    const list = children.get(parent.path) ?? [];
    list.push({ path: p.path, version: m[2] ? Number(m[2]) : 1 });
    children.set(parent.path, list);
  }
  const stacks = new Map<string, Stack>();
  for (const [parent, list] of children) {
    list.sort((a, b) => a.version - b.version);
    const members = [parent, ...list.map((c) => c.path)];
    const stack = { parent, members, top: members[members.length - 1] };
    for (const m of members) stacks.set(m, stack);
  }
  return stacks;
}

function byCaptureTime(a: PhotoInfo, b: PhotoInfo): number {
  return a.timestampMs - b.timestampMs || a.fileName.localeCompare(b.fileName);
}

/** How long a notice stays before it goes by itself. */
const NOTICE_MS = 3000;

/** What Before keeps of a photo's edit: its geometry (with the crop), so it fills After's frame. */
const BEFORE_KEEPS = new Set([
  ...(SLIDER_GROUPS.find((g) => g.name === "Keystone")?.sliders.map((d) => d.id) ?? []),
  "autoTransform",
  "lensCorrect",
]);

export default function App() {
  const [folder, setFolder] = useState<string | null>(null);
  const [photosByPath, setPhotosByPath] = useState<Record<string, PhotoInfo>>({});
  const [currentPath, setCurrentPath] = useState<string | null>(null);
  const [preview, setPreview] = useState<ViewerImage | null>(null);
  const [cameraPreview, setCameraPreview] = useState<FullPreview | null>(null);
  // Edits live in memory only for now - saving them is a later step.
  // Only the sliders the user has changed, per photo; everything else is the
  // photo's neutral (see `neutrals`).
  const [edits, setEdits] = useState<Record<string, Partial<Adjustments>>>({});
  // Each photo's as-shot white balance in Lightroom's numbers, from the engine.
  const [asShot, setAsShot] = useState<Record<string, { temperature: number; tint: number }>>({});
  const [held, setHeld] = useState<{ sliderId: string; code: string } | null>(null);
  const [curvesByPath, setCurvesByPath] = useState<Record<string, Curves>>({});
  const [gradeByPath, setGradeByPath] = useState<Record<string, Grade>>({});
  const [mixerByPath, setMixerByPath] = useState<Record<string, Mixer>>({});
  // Profile per photo; chosen first, as every slider works on top of it.
  const [profileByPath, setProfileByPath] = useState<Record<string, ProfileId>>({});
  // Crop, straighten and quarter turns per photo (crop.ts); not part of presets.
  const [cropByPath, setCropByPath] = useState<Record<string, Crop>>({});
  // Objects removed per photo (non-destructive patches, removal.rs).
  const [removalsByPath, setRemovalsByPath] = useState<Record<string, Spot[]>>({});
  // Linear and radial gradients per photo (local adjustments, gradient.rs).
  const [gradientsByPath, setGradientsByPath] = useState<Record<string, Gradient[]>>({});
  // The gradient tool is open (which kind new ones are drawn with), or null.
  // The masks are open (the layers panel on the photo).
  const [gradientTool, setGradientTool] = useState(false);
  // With a mask tool open: the picked mask (-1: none), whose numbers the bottom panel's
  // sliders show and set instead of the photo's, and whether it shows in red.
  const [maskPicked, setMaskPicked] = useState(-1);
  const [maskShow, setMaskShow] = useState(true);
  // A mask's red overlay on the open photo (any kind, combined or not; its subject found once and kept).
  const maskOverlayFor = useCallback(
    (mask: Gradient) => (currentPath ? maskOverlay(currentPath, mask) : Promise.resolve(null)),
    [currentPath],
  );
  // The Remove tool is open; a removal is running.
  const [removing, setRemoving] = useState(false);
  const [removeBusy, setRemoveBusy] = useState(false);
  // Ctrl+E: Remove with the whole photo selected; bumped to restart the tool that way.
  const [removeWhole, setRemoveWhole] = useState(0);
  // The crop tool is open: the engine renders the photo uncropped under the frame.
  const [cropping, setCropping] = useState(false);
  const [cropAspect, setCropAspect] = useState("Free");
  // The user has added Adobe Color (Lightroom edits asking for it use theirs).
  const [adobeAvailable, setAdobeAvailable] = useState(false);
  // Profiles the user added (Documents/RapidOne/Profiles).
  const [userProfilesList, setUserProfilesList] = useState<UserProfile[]>([]);
  const refreshUserProfiles = useCallback(() => {
    fetchUserProfiles()
      .then(setUserProfilesList)
      .catch(() => setUserProfilesList([]));
    adobeColorAvailable()
      .then(setAdobeAvailable)
      .catch(() => setAdobeAvailable(false));
  }, []);
  useEffect(() => refreshUserProfiles(), [refreshUserProfiles]);
  // Profile menu → Add your own profile…: .dcp / .xmp files, copied into their profiles folder.
  const addOwnProfiles = useCallback(async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const chosen = await open({ multiple: true, title: "Add your own profile", filters: [{ name: "Profiles", extensions: ["dcp", "DCP", "xmp", "XMP"] }] });
    const paths = chosen ? (Array.isArray(chosen) ? chosen : [chosen]) : [];
    if (!paths.length) return;
    try {
      const [added, skipped] = await addUserProfiles(paths);
      refreshUserProfiles();
      const parts = [];
      if (added.length) parts.push(`Added to your profiles: ${added.join(", ")}.`);
      if (skipped.length) parts.push(`Not added: ${skipped.join("; ")}.`);
      setNotice(parts.join(" ") || null);
    } catch (e) {
      setError(`Couldn't add the profiles: ${e}`);
    }
  }, [refreshUserProfiles]);
  const [histogram, setHistogram] = useState<Histogram | null>(null);
  const [panelVisible, setPanelVisible] = useState(true);
  // The home screen (start prompt) while a folder is open.
  const [home, setHome] = useState(false);
  // Cull | Edit | Retouch, as chosen: Edit at every launch, then whichever was
  // last chosen this session (the mode shown is `mode`, below: Edit's home
  // screen until a folder is open and past it). A module's screen is mounted
  // the first time it's shown and then kept (hidden), so it keeps its place;
  // RapidRetouch's engine only starts once Retouch has been shown.
  const [chosenMode, setMode] = useState<Mode>("edit");
  const [visited, setVisited] = useState<Record<Mode, boolean>>({ cull: false, edit: true, retouch: false });
  // The top bar's places for Retouch's toolbar (so every tab shares one bar).
  const [retouchLeft, setRetouchLeft] = useState<HTMLDivElement | null>(null);
  const [retouchStatus, setRetouchStatus] = useState<HTMLDivElement | null>(null);
  const [retouchRight, setRetouchRight] = useState<HTMLDivElement | null>(null);
  // The slider panel docks over the bottom of the viewer (the corridor runs on
  // under its see-through bar); the viewer keeps the photo above it.
  const dockRef = useRef<HTMLDivElement | null>(null);
  const [dockHeight, setDockHeight] = useState(0);
  useEffect(() => {
    const el = dockRef.current;
    if (!el) {
      setDockHeight(0);
      return;
    }
    const ro = new ResizeObserver(() => setDockHeight(el.offsetHeight));
    ro.observe(el);
    return () => ro.disconnect();
  });
  // The D65 corridor lines and halo round the photo (top bar tick box).
  const [surround, setSurround] = useState(loadSurround);
  const toggleSurround = useCallback(() => {
    setSurround((prev) => {
      try {
        localStorage.setItem(SURROUND_KEY, String(!prev));
      } catch {
        // Not fatal: just not remembered next launch.
      }
      return !prev;
    });
  }, []);
  // Full frame (F): the photo fills the viewer, out of the corridor.
  const [fullFrame, setFullFrame] = useState(false);
  // A pointer is down on the slider panel (dragging a slider, curve, wheel):
  // the photo's hover holds still meanwhile.
  const [adjusting, setAdjusting] = useState(false);
  // Which panel group it's in (the Keystone group shows the alignment grid over the photo).
  const [adjustingGroup, setAdjustingGroup] = useState<string | null>(null);
  useEffect(() => {
    const down = (e: PointerEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest?.(".panel")) {
        setAdjusting(true);
        setAdjustingGroup(target.closest<HTMLElement>("[data-group]")?.dataset.group ?? null);
      }
    };
    const up = () => {
      setAdjusting(false);
      setAdjustingGroup(null);
    };
    window.addEventListener("pointerdown", down);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    return () => {
      window.removeEventListener("pointerdown", down);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
  }, []);
  // The panel selected by its letter key (lit up); slider letters then work within it.
  const [selectedPanel, setSelectedPanel] = useState<string | null>(null);
  // What's picked inside the Mixer, Grade, Fine tuning and Curves panels.
  const [picks, setPicks] = useState<PanelPicks>(DEFAULT_PICKS);
  const onPicks = useCallback((p: Partial<PanelPicks>) => setPicks((prev) => ({ ...prev, ...p })), []);
  // The start screen's picture: the last photo worked on, from its folder's
  // cache; undefined (the Isle of Skye default) if there's none or it's gone.
  const [startImage, setStartImage] = useState<string | undefined>();
  useEffect(() => {
    let last: string | null = null;
    try {
      last = localStorage.getItem(LAST_PHOTO_KEY);
    } catch {
      // No storage: keep the default picture.
    }
    if (!last) return;
    cachedPreview(last)
      .then((c) => c && setStartImage(c.preview.imageBase64))
      .catch(() => {});
  }, []);
  // Resume last session (the home screen's second choice on a fresh start): the
  // folder and photo of last time, opened again.
  const [lastFolder] = useState<string | null>(() => {
    try {
      return localStorage.getItem(LAST_FOLDER_KEY);
    } catch {
      return null;
    }
  });
  const [resumePhoto, setResumePhoto] = useState<string | null>(null);
  const [showNames, setShowNames] = useState(loadShowNames);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Information (not a problem): e.g. what a Lightroom preset applied and skipped.
  const [notice, setNotice] = useState<string | null>(null);
  // Undo after a delete: its own bar (a notice could be replaced at once, e.g. by the next photo's
  // Lightroom edit being read, taking the button with it), up for a while whatever else is said.
  const [undoDelete, setUndoDelete] = useState<{ text: string; items: Trashed[] } | null>(null);
  useEffect(() => {
    if (!undoDelete) return;
    const t = window.setTimeout(() => setUndoDelete(null), 4 * NOTICE_MS);
    return () => window.clearTimeout(t);
  }, [undoDelete]);
  // Lens profiles: checked for an update once a week, a little after starting (quietly
  // unless one came).
  useEffect(() => {
    const t = window.setTimeout(() => {
      void checkLensUpdates().then((m) => m && setNotice(m));
    }, 20000);
    return () => window.clearTimeout(t);
  }, []);
  // A notice goes by itself after a few seconds (a new one starts the time again); errors stay until dismissed.
  useEffect(() => {
    if (!notice) return;
    const t = window.setTimeout(() => setNotice(null), NOTICE_MS);
    return () => window.clearTimeout(t);
  }, [notice]);

  // Stacks, as in Lightroom: a photo and the TIFFs Retouch handed back
  // (`<name>_retouched.tif`, `_retouched-2.tif` ...) form one stack, the newest
  // TIFF on top. A collapsed stack shows only its top; the badge opens it.
  const [expandedStacks, setExpandedStacks] = useState<ReadonlySet<string>>(new Set());
  const stackByMember = useMemo(() => buildStacks(Object.values(photosByPath)), [photosByPath]);
  const sortedPhotos = useMemo(() => {
    // A handed-back TIFF has no capture time of its own: it sits where its photo does.
    const all = Object.values(photosByPath).map((p) => {
      const st = stackByMember.get(p.path);
      return st && st.parent !== p.path ? { ...p, timestampMs: photosByPath[st.parent]?.timestampMs ?? p.timestampMs } : p;
    });
    return all.sort(byCaptureTime);
  }, [photosByPath, stackByMember]);
  const allPhotos = useMemo(() => {
    const out: PhotoInfo[] = [];
    const done = new Set<string>();
    const byPath = new Map(sortedPhotos.map((x) => [x.path, x]));
    for (const p of sortedPhotos) {
      const st = stackByMember.get(p.path);
      if (!st) {
        out.push(p);
        continue;
      }
      if (done.has(st.parent)) continue;
      done.add(st.parent);
      // Open when asked, or while the photo being edited is one of the hidden members.
      const open = expandedStacks.has(st.parent) || Boolean(currentPath && currentPath !== st.top && st.members.includes(currentPath));
      const members = open ? [...st.members].reverse() : [st.top];
      for (const m of members) {
        const photo = byPath.get(m);
        if (photo) out.push(photo);
      }
    }
    return out;
  }, [sortedPhotos, stackByMember, expandedStacks, currentPath]);
  /** For the badge on a stack's first shown photo: how many it holds, and whether it is open. */
  const stackBadges = useMemo(() => {
    const badges: Record<string, { count: number; open: boolean }> = {};
    const shown = new Set(allPhotos.map((p) => p.path));
    const seen = new Set<string>();
    for (const p of allPhotos) {
      const st = stackByMember.get(p.path);
      if (!st || seen.has(st.parent)) continue;
      seen.add(st.parent);
      badges[p.path] = { count: st.members.length, open: st.members.filter((m) => shown.has(m)).length > 1 };
    }
    return badges;
  }, [allPhotos, stackByMember]);
  const toggleStack = useCallback(
    (path: string) => {
      const parent = stackByMember.get(path)?.parent;
      if (!parent) return;
      setExpandedStacks((prev) => {
        const next = new Set(prev);
        if (!next.delete(parent)) next.add(parent);
        return next;
      });
    },
    [stackByMember],
  );
  // The filmstrip's filter (exact stars, picked): what's shown, stepped
  // through with the arrows, and what "All photos" actions apply to.
  const [stripFilter, setStripFilter] = useState<StripFilter>(NO_STRIP_FILTER);
  const photos = useMemo(() => allPhotos.filter((p) => passesStripFilter(p, stripFilter)), [allPhotos, stripFilter]);
  const currentIndex = currentPath ? photos.findIndex((p) => p.path === currentPath) : -1;
  // A filter that hides the current photo moves to the first one shown.
  useEffect(() => {
    if (currentPath && currentIndex === -1 && photos.length > 0 && photosByPath[currentPath]) setCurrentPath(photos[0].path);
    // Only when the filter (or the list) changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stripFilter, photos.length]);
  // Photos per star and picked, for the filter's buttons.
  const stripCounts = useMemo(() => {
    const stars = [0, 0, 0, 0, 0];
    let picked = 0;
    for (const p of allPhotos) {
      if (p.rating >= 1 && p.rating <= 5) stars[p.rating - 1]++;
      if (p.pick) picked++;
    }
    return { stars, picked };
  }, [allPhotos]);
  const currentPhoto = currentIndex >= 0 ? photos[currentIndex] : null;
  // Photos picked for export (Ctrl-click toggles, Shift-click takes a range) in
  // the filmstrip or the library; empty means just the open photo.
  const [selection, setSelection] = useState<ReadonlySet<string>>(new Set());
  const selectionAnchor = useRef<string | null>(null);
  // The photo on screen is where a Shift-click range starts from, however it got there (clicked,
  // stepped to with the keys, resumed); a Ctrl-click moves the start without changing photo.
  useEffect(() => {
    if (currentPath) selectionAnchor.current = currentPath;
  }, [currentPath]);
  // The main pane: the loupe (one photo, with the filmstrip) or the library (a grid).
  const [libraryView, setLibraryView] = useState(false);
  const [libraryZoom, setLibraryZoom] = useState(170);
  // The home screen (also the opening screen) has no Cull | Edit | Retouch:
  // nothing to switch to until a folder is open and you're past it.
  // (In Cull, a folder being opened has no photo picked in Edit yet: Cull stays on screen meanwhile.)
  const atHome = home || (!currentPhoto && chosenMode !== "cull");
  const mode: Mode = atHome ? "edit" : chosenMode;
  // For callbacks that need the current tab without re-creating themselves.
  const modeRef = useRef(mode);
  modeRef.current = mode;
  useEffect(() => {
    setVisited((v) => (v[mode] ? v : { ...v, [mode]: true }));
  }, [mode]);
  // A photo's neutral values: the global defaults, with Temperature and Tint
  // starting at its own as-shot white balance once the engine has read it.
  const neutrals: Adjustments = useMemo(
    () => ({ ...DEFAULTS, ...(currentPath ? asShot[currentPath] : undefined) }),
    [currentPath, asShot],
  );
  const values: Adjustments = useMemo(
    () => ({ ...neutrals, ...(currentPath ? edits[currentPath] : undefined) }) as Adjustments,
    [neutrals, currentPath, edits],
  );
  const curves = (currentPath && curvesByPath[currentPath]) || DEFAULT_CURVES;
  const grade = (currentPath && gradeByPath[currentPath]) || DEFAULT_GRADE;
  const mixer = (currentPath && mixerByPath[currentPath]) || DEFAULT_MIXER;
  const profile = (currentPath && profileByPath[currentPath]) || DEFAULT_PROFILE;
  const crop = (currentPath && cropByPath[currentPath]) || NO_CROP;
  const removals = (currentPath && removalsByPath[currentPath]) || NO_REMOVALS;
  const gradients = (currentPath && gradientsByPath[currentPath]) || NO_GRADIENTS;

  // Undo / redo (history.ts): per photo, snapshots of the whole edit.
  const editState = useRef({ edits, curvesByPath, gradeByPath, mixerByPath, profileByPath, cropByPath, removalsByPath, gradientsByPath });
  editState.current = { edits, curvesByPath, gradeByPath, mixerByPath, profileByPath, cropByPath, removalsByPath, gradientsByPath };
  const histories = useRef<Record<string, History>>({});
  const snapshotOf = useCallback((path: string): Snapshot => {
    const s = editState.current;
    return {
      edits: s.edits[path],
      curves: s.curvesByPath[path],
      grade: s.gradeByPath[path],
      mixer: s.mixerByPath[path],
      profile: s.profileByPath[path],
      crop: s.cropByPath[path],
      removals: s.removalsByPath[path],
      gradients: s.gradientsByPath[path],
    };
  }, []);
  /** Call before changing a photo's edit; `key` names the control (null: always a new step). */
  const record = useCallback(
    (path: string, key: string | null) => {
      const h = (histories.current[path] ??= newHistory());
      recordChange(h, key, snapshotOf(path), performance.now());
      try {
        localStorage.setItem(LAST_PHOTO_KEY, path);
      } catch {
        // Not fatal: the start screen just shows the default picture.
      }
    },
    [snapshotOf],
  );
  const restore = useCallback((path: string, snap: Snapshot) => {
    setEdits((prev) => withEntry(prev, path, snap.edits));
    setCurvesByPath((prev) => withEntry(prev, path, snap.curves));
    setGradeByPath((prev) => withEntry(prev, path, snap.grade));
    setMixerByPath((prev) => withEntry(prev, path, snap.mixer));
    setProfileByPath((prev) => withEntry(prev, path, snap.profile));
    setCropByPath((prev) => withEntry(prev, path, snap.crop));
    setRemovalsByPath((prev) => withEntry(prev, path, snap.removals));
    setGradientsByPath((prev) => withEntry(prev, path, snap.gradients));
    // Readable straight away, in case of another undo before the re-render.
    const s = editState.current;
    editState.current = {
      edits: withEntry(s.edits, path, snap.edits),
      curvesByPath: withEntry(s.curvesByPath, path, snap.curves),
      gradeByPath: withEntry(s.gradeByPath, path, snap.grade),
      mixerByPath: withEntry(s.mixerByPath, path, snap.mixer),
      profileByPath: withEntry(s.profileByPath, path, snap.profile),
      cropByPath: withEntry(s.cropByPath, path, snap.crop),
      removalsByPath: withEntry(s.removalsByPath, path, snap.removals),
      gradientsByPath: withEntry(s.gradientsByPath, path, snap.gradients),
    };
  }, []);
  const stepHistory = useCallback(
    (direction: "undo" | "redo") => {
      if (!currentPath) return;
      const h = histories.current[currentPath];
      if (!h) return;
      const snap = (direction === "undo" ? undo : redo)(h, snapshotOf(currentPath));
      if (snap) restore(currentPath, snap);
    },
    [currentPath, snapshotOf, restore],
  );

  // Copy / paste settings: the ticked groups from one photo onto this photo
  // or every photo in the folder (one undo step each). The ticks are
  // remembered between sessions.
  const [copyGroups, setCopyGroups] = useState<string[]>(() => {
    try {
      const saved = JSON.parse(localStorage.getItem(COPY_GROUPS_KEY) ?? "null");
      if (Array.isArray(saved)) return saved.filter((g): g is string => typeof g === "string");
    } catch {
      // Not fatal: the default groups.
    }
    return DEFAULT_COPY_GROUPS;
  });
  const changeCopyGroups = useCallback((groups: string[]) => {
    setCopyGroups(groups);
    try {
      localStorage.setItem(COPY_GROUPS_KEY, JSON.stringify(groups));
    } catch {
      // Not fatal: forgotten next time.
    }
  }, []);
  // What's copied, and whether it has been pasted since (the Paste button glows until then).
  const [copied, setCopied] = useState<{ from: string; snap: Snapshot; groups: string[]; pasted?: boolean } | null>(null);
  // After a paste onto several photos, each is brought up to date in the background, one
  // at a time: its subject found (if a subject mask was pasted) and its thumbnail
  // rendered with the new edit, so the filmstrip and library show it. A later paste
  // adds to the queue; the progress bar says how far it's got.
  const [pasteProgress, setPasteProgress] = useState<{ done: number; total: number; subjects: boolean } | null>(null);
  // The computer's core count (how many photos a style works out at once).
  const cores = useRef(navigator.hardwareConcurrency || 4);
  useEffect(() => {
    coreCount()
      .then((n) => (cores.current = n))
      .catch(() => undefined);
  }, []);
  const pasteQueue = useRef<{ jobs: { path: string; subject: boolean }[]; done: number; total: number; subjects: boolean; running: number }>({
    jobs: [],
    done: 0,
    total: 0,
    subjects: false,
    running: 0,
  });
  // A photo's settings as the viewer renders them (and keys its pictures by), given its
  // as-shot white balance if known: the same as `engineSettings` for the open photo.
  const asShotRef = useRef(asShot);
  asShotRef.current = asShot;
  const viewSettingsFor = useCallback(
    (path: string, shot: { temperature: number; tint: number } | undefined): RenderSettings => {
      const s = snapshotOf(path);
      return engineSettingsFrom(
        { ...DEFAULTS, ...shot, ...s.edits } as Adjustments,
        Boolean(shot),
        s.profile ?? DEFAULT_PROFILE,
        s.mixer ?? DEFAULT_MIXER,
        s.curves ?? DEFAULT_CURVES,
        s.grade ?? DEFAULT_GRADE,
        s.crop ?? NO_CROP,
        s.removals ?? NO_REMOVALS,
        s.gradients ?? NO_GRADIENTS,
      );
    },
    [snapshotOf],
  );
  // The loupe's compare button: the photo with no edit, rendered by RapidOne (as Lightroom's Before:
  // the default look, with the photo's crop so it fills the same frame as After), made the first time
  // it's held and kept (per crop); or, set by a right-click on the button, the camera's own JPEG.
  const [compareWith, setCompareWithState] = useState<"before" | "camera">(() => {
    try {
      return localStorage.getItem("rapidone.compareWith") === "camera" ? "camera" : "before";
    } catch {
      return "before";
    }
  });
  const setCompareWith = useCallback((mode: "before" | "camera") => {
    setCompareWithState(mode);
    try {
      localStorage.setItem("rapidone.compareWith", mode);
    } catch {
      // Not fatal: forgotten next time.
    }
  }, []);
  // The open photo whose saved edit has loaded (set where the edit loads, below): renders wait for it.
  const [editLoadedPath, setEditLoadedPath] = useState<string | null>(null);
  const beforeCache = useRef(new Map<string, string>());
  const [beforePreview, setBeforePreview] = useState<{ path: string; imageBase64: string } | null>(null);
  const requestBefore = useCallback((path: string | null) => {
    if (!path) return;
    // The photo's geometry stays (crop, keystone and rotation, lens corrections), so Before has
    // After's exact frame; only the look goes back to the defaults.
    const snap = snapshotOf(path);
    const crop = snap.crop ?? NO_CROP;
    const geometry = Object.fromEntries(
      [...BEFORE_KEEPS].flatMap((k) => (snap.edits?.[k] !== undefined ? [[k, snap.edits[k]]] : [])),
    );
    const key = `${path}\n${JSON.stringify(crop)}\n${JSON.stringify(geometry)}`;
    const kept = beforeCache.current.get(key);
    if (kept) return setBeforePreview({ path, imageBase64: kept });
    // A Before for another frame isn't shown meanwhile (it wouldn't fit); After stays until it lands.
    setBeforePreview(null);
    const shot = asShotRef.current[path];
    const plain = engineSettingsFrom(
      { ...DEFAULTS, ...shot, ...geometry } as Adjustments,
      Boolean(shot),
      DEFAULT_PROFILE,
      DEFAULT_MIXER,
      DEFAULT_CURVES,
      DEFAULT_GRADE,
      crop,
      NO_REMOVALS,
      NO_GRADIENTS,
    );
    // Not kept in the folder's cache (no key): that's the edited picture's place.
    void renderPreview(path, plain, undefined, false)
      .then((p) => {
        const cache = beforeCache.current;
        cache.set(key, p.imageBase64);
        while (cache.size > 12) cache.delete(cache.keys().next().value!);
        setBeforePreview({ path, imageBase64: p.imageBase64 });
      })
      .catch(() => undefined);
  }, [snapshotOf]);

  // The white balance to render a photo with ahead of time: its as-shot one once known, else its
  // own edit's (a style or paste sets it), so a photo never opened isn't rendered as shot instead.
  const renderShot = (path: string) => {
    const known = asShotRef.current[path];
    const e = editState.current.edits[path];
    return known ?? (e?.temperature != null ? { temperature: e.temperature, tint: e.tint ?? 0 } : undefined);
  };
  // A photo brought up to date with its edit: one render (at background priority, refresh_photo's
  // own threads) makes its thumbnail and its full picture, the latter kept for the loupe (memory and
  // the folder's cache) so opening it shows the edit at once. Skipped if that's already the case.
  // Whether a photo's kept picture (memory, else the folder's cache on disk) is already its current
  // edit's: then it's marked ready with no render. The disk is asked too (just the key, not the
  // picture), as memory keeps only the last 40: without it an up-to-date photo was rendered again.
  const alreadyRendered = async (path: string): Promise<boolean> => {
    const shot0 = asShotRef.current[path];
    if (previewCache.current.get(path)?.key === JSON.stringify(viewSettingsFor(path, shot0))) {
      markReady([path], true);
      return true;
    }
    const disk = await cachedPreviewKey(path).catch(() => null);
    if (!disk) return false;
    const [key, kelvin, tint] = disk;
    const shot = shot0 ?? (kelvin != null && tint != null ? { temperature: Math.round(kelvin), tint: Math.round(tint) } : undefined);
    if (key !== JSON.stringify(viewSettingsFor(path, shot))) return false;
    markReady([path], true);
    return true;
  };
  const renderFull = async (path: string) => {
    const shot0 = asShotRef.current[path];
    if (await alreadyRendered(path)) return;
    const done = await refreshPhoto(path, viewSettingsFor(path, renderShot(path))).catch(() => null);
    if (!done) return;
    setPhotosByPath((prev) => (prev[path] ? { ...prev, [path]: { ...prev[path], thumbBase64: done.thumb } } : prev));
    // Kept under the key the viewer will look for (with the as-shot white balance the render read,
    // if it wasn't known).
    const p = done.preview;
    const shot = shot0 ?? (p.asShotTemperature != null && p.asShotTint != null ? { temperature: Math.round(p.asShotTemperature), tint: Math.round(p.asShotTint) } : undefined);
    if (!shot0 && shot) setAsShot((prev) => (prev[path] ? prev : { ...prev, [path]: shot }));
    const key = JSON.stringify(viewSettingsFor(path, shot));
    rememberPreview(path, key, { ...p, source: "engine", pending: false });
    markReady([path], true);
    await cachePreview(path, key, p).catch(() => undefined);
  };
  // A style's photos: each brought up to date in the background, one at a time (each render already
  // uses every core), nearest the photo on screen first, waiting while a slider moves or a paste is
  // being brought up to date, with no bar: editing carries on meanwhile.
  const fullQueue = useRef<{ paths: Set<string>; running: boolean }>({ paths: new Set(), running: false });
  // For the folder read (declared above prepareFull).
  const prepareFullRef = useRef<(paths: string[]) => void>(() => {});
  const prepareFull = useCallback((paths: string[]) => {
    const f = fullQueue.current;
    for (const p of paths) f.paths.add(p);
    // Every photo checked at once: those already up to date are marked straight away and leave the
    // line, rather than waiting their turn behind photos that do need a render.
    for (const p of paths) void alreadyRendered(p).then((current) => current && f.paths.delete(p));
    if (f.running) return;
    f.running = true;
    void (async () => {
      while (f.paths.size) {
        while ((interactingRef.current || pasteQueue.current.running > 0 || pasteQueue.current.jobs.length > 0) && f.paths.size)
          await new Promise((r) => setTimeout(r, 150));
        const order = new Map(photosRef.current.map((p, i) => [p.path, i]));
        const here = order.get(currentPathRef.current ?? "") ?? 0;
        let path = "";
        let best = Infinity;
        for (const p of f.paths) {
          const d = Math.abs((order.get(p) ?? Infinity) - here);
          if (d < best || !path) [best, path] = [d, p];
        }
        f.paths.delete(path);
        if (order.has(path)) await renderFull(path);
      }
      f.running = false;
    })();
    // Refs and stable callbacks only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  prepareFullRef.current = prepareFull;
  const refreshPasted = useCallback((paths: string[], subject: boolean) => {
    const q = pasteQueue.current;
    // A photo already waiting is done once, with whatever it has by then.
    const fresh = paths.filter((p) => !q.jobs.some((j) => j.path === p));
    q.total += fresh.length;
    if (fresh.length === 0) return;
    q.jobs.push(...fresh.map((path) => ({ path, subject })));
    q.subjects ||= subject;
    setPasteProgress({ done: q.done, total: q.total, subjects: q.subjects });
    const work = async () => {
      while (q.jobs.length) {
        const job = q.jobs.shift()!;
        if (job.subject) await findSubject(job.path).catch(() => false);
        // Its thumbnail and full picture, from one render.
        await renderFull(job.path);
        q.done++;
        setPasteProgress({ done: q.done, total: q.total, subjects: q.subjects });
      }
      q.running--;
      if (q.running > 0) return;
      q.done = 0;
      q.total = 0;
      q.subjects = false;
      setPasteProgress(null);
    };
    // One at a time: each render already uses every core (two at once only slowed both).
    while (q.running < 1 && q.running < q.jobs.length) {
      q.running++;
      void work();
    }
    // viewSettingsFor is stable (snapshotOf is); rememberPreview only touches a ref.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // Stop: the photo being done finishes, the rest are left (their settings stay pasted).
  const stopRefreshing = useCallback(() => {
    const q = pasteQueue.current;
    q.total -= q.jobs.length;
    q.jobs = [];
    setPasteProgress((p) => (p ? { ...p, total: q.total } : p));
  }, []);
  // While it runs, keys don't edit or change photo either (the cover takes the clicks).
  const pasteBusy = pasteProgress !== null;
  useEffect(() => {
    if (!pasteBusy) return;
    const hold = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
    };
    window.addEventListener("keydown", hold, true);
    window.addEventListener("keyup", hold, true);
    return () => {
      window.removeEventListener("keydown", hold, true);
      window.removeEventListener("keyup", hold, true);
    };
  }, [pasteBusy]);
  const copySettings = useCallback(() => {
    if (!currentPath || copyGroups.length === 0) return;
    const from = currentPath;
    const snap = snapshotOf(from);
    const groups = copyGroups;
    const notice = `Copied ${groups.length} setting group${groups.length === 1 ? "" : "s"} from ${baseName(from)}.`;
    // A mask with + / − shapes and a subject in it is copied with this photo's subject as a fixed
    // shape (frozen), so it never adapts to another photo; only a Subject layer on its own does.
    // Copied once the shape is in hand, so a paste can't go out before it's frozen.
    const combined = (snap.gradients ?? []).some((g) => (g.parts?.length ?? 0) > 0 && hasSubject(g));
    if (!groups.includes("gradients") || !combined) {
      setCopied({ from, snap, groups });
      setNotice(notice);
      return;
    }
    void subjectSnapshot(from)
      .catch(() => null)
      .then((frozen) => {
        if (!frozen) {
          setError(`Couldn't copy the masks from ${baseName(from)}: its subject wasn't found, so a mask with + / − shapes can't be fixed to it.`);
          return;
        }
        const freeze = (g: Gradient): Gradient => (g.kind === "subject" && !g.frozen ? { ...g, frozen } : g);
        const gradients = (snap.gradients ?? []).map((g) =>
          (g.parts?.length ?? 0) > 0 && hasSubject(g) ? { ...freeze(g), parts: g.parts?.map((p) => ({ ...p, shape: freeze(p.shape) })) } : g,
        );
        setCopied({ from, snap: { ...snap, gradients }, groups });
        setNotice(notice);
      });
  }, [currentPath, copyGroups, snapshotOf]);
  // Loads photos' saved edits (their .rpd, as opening them does: ensureEditLoaded, below) before a
  // batch changes them. Without it a photo not opened since the folder was read counted as having no
  // edit, and the batch's change was saved over its real one (23 Oct: a Level all wiped the look
  // from 506 photos' .rpd files).
  const loadEditsRef = useRef<(paths: string[]) => Promise<void>>(async () => {});
  const pasteSettings = useCallback(
    (scope: "photo" | "selected" | "all") => {
      if (!copied) return;
      setCopied((c) => (c && !c.pasted ? { ...c, pasted: true } : c));
      // Selected: the photos picked in the filmstrip or library (Ctrl / Shift-click), or the open one.
      const chosen = scope === "selected" && selection.size > 0 ? photos.filter((p) => selection.has(p.path)).map((p) => p.path) : null;
      const paths = (scope === "all" ? photos.map((p) => p.path) : (chosen ?? (currentPath ? [currentPath] : []))).filter((p) => p !== copied.from);
      // Their saved edits first (see loadEditsRef), then the paste on top of them.
      void (async () => {
        await loadEditsRef.current(paths);
        for (const p of paths) {
          record(p, null);
          restore(p, pasteSnapshot(copied.snap, snapshotOf(p), copied.groups));
        }
        markReady(paths, false);
        // Several photos: each brought up to date in the background (its subject, if a subject
        // mask was pasted, and its thumbnail), the progress bar saying how far it's got. The photo
        // copied from is done first too, so it looks the same as the others in the filmstrip and library.
        if (paths.length > 1 || (paths.length === 1 && paths[0] !== currentPath)) {
          const source = photos.some((p) => p.path === copied.from) ? [copied.from] : [];
          // Finding the subject is only needed for a Subject layer that adapts (none frozen in it).
          const live = (g: Gradient) => g.kind === "subject" && !g.frozen;
          const adapts = Boolean(copied.snap.gradients?.some((g) => live(g) || g.parts?.some((p) => live(p.shape))));
          refreshPasted([...source, ...paths], copied.groups.includes("gradients") && adapts);
        }
        if (paths.length > 1 || scope === "all") setNotice(`Pasted ${baseName(copied.from)}'s settings to ${paths.length} photo${paths.length === 1 ? "" : "s"}.`);
      })();
    },
    [copied, photos, currentPath, record, restore, snapshotOf, selection, refreshPasted],
  );

  // The photo's ISO and aperture, in Curves' corner.
  const [shotLabel, setShotLabel] = useState<string | null>(null);
  useEffect(() => {
    setShotLabel(null);
    if (!currentPath) return;
    let cancelled = false;
    shotDetails(currentPath)
      .then((d) => {
        if (cancelled) return;
        const parts = [d.iso ? `ISO ${d.iso}` : null, d.aperture ? `f/${Math.round(d.aperture * 10) / 10}` : null].filter(Boolean);
        setShotLabel(parts.length ? parts.join(" · ") : null);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [currentPath]);

  // Histogram of the current preview, for behind the curve. A new photo clears it; a new render of
  // the same photo keeps the old one up until its own is counted (cleared on every render, it blinked
  // out for the whole of a slider drag), and the quick frames during a drag aren't counted at all.
  const histogramPath = useRef<string | null>(null);
  useEffect(() => {
    const path = preview?.path ?? null;
    if (path !== histogramPath.current) setHistogram(null);
    histogramPath.current = path;
    if (!preview || preview.interactive) return;
    let cancelled = false;
    computeHistogram(preview.imageBase64)
      .then((h) => !cancelled && setHistogram(h))
      .catch(() => {
        // No histogram is fine - the curve still works without one.
      });
    return () => {
      cancelled = true;
    };
  }, [preview]);

  // The folder read wanted now (on its events). From the clock, so a reload can't reuse a number.
  const scanRef = useRef(Date.now());

  // Scan events: thumbnails arrive one by one (hundreds a second once they're cached), so they're
  // gathered and put on screen in batches about ten times a second, not one update (a copy of the
  // whole list) each.
  useEffect(() => {
    let arrived: (PhotoInfo & { scan: number })[] = [];
    let progress: ScanProgress | null = null;
    // The highest count of this scan so far: the reading threads each take a number, then send it,
    // so a lower one can arrive after a higher one (the count jumped back and forth, 25 Oct).
    let highest: ScanProgress | null = null;
    let timer: number | null = null;
    const flush = () => {
      timer = null;
      // Only this read's: another folder may have been opened since these arrived.
      const batch = arrived.filter((p) => p.scan === scanRef.current);
      arrived = [];
      if (batch.length) {
        setPhotosByPath((prev) => {
          const next = { ...prev };
          for (const p of batch) next[p.path] = p;
          return next;
        });
        // Their saved pictures still current: the sparkle back after a reopen.
        const rendered = batch.filter((p) => p.rendered).map((p) => p.path);
        if (rendered.length) markReady(rendered, true);
        // A batch (Denoise all, a style, a preset) stopped before it got to them (the app closed):
        // their pictures carry on in the background.
        const unfinished = batch.filter((p) => !p.rendered && p.edited && (p.denoised || (p.look && p.look.from !== "xmp"))).map((p) => p.path);
        // Their saved edits read first (see loadEditsRef), so each renders with its own edit.
        if (unfinished.length) void loadEditsRef.current(unfinished).then(() => prepareFullRef.current(unfinished));
      }
      if (progress) {
        if (progress.scan === scanRef.current) setProgress(progress);
        progress = null;
      }
    };
    const soon = () => {
      if (timer === null) timer = window.setTimeout(flush, 100);
    };
    const unlisteners = [
      onPhotoReady((photo) => {
        if (photo.scan !== scanRef.current) return;
        arrived.push(photo);
        soon();
      }),
      onScanProgress((p) => {
        if (p.scan !== scanRef.current) return;
        // A new scan starts at 0 (or has another total); within one, the count only goes up.
        if (highest && p.done !== 0 && p.total === highest.total && p.done < highest.done) return;
        highest = p;
        progress = p;
        // The last one (done) at once, so the bar never lingers.
        if (p.done >= p.total) flush();
        else soon();
      }),
    ];
    return () => {
      if (timer !== null) clearTimeout(timer);
      unlisteners.forEach((u) => void u.then((fn) => fn()));
    };
  }, []);

  // Select the first photo once the scan has produced any.
  useEffect(() => {
    if (!currentPath && photos.length > 0) setCurrentPath(photos[0].path);
  }, [photos, currentPath]);

  // The last engine picture of each photo seen, with the settings it was
  // rendered with (JSON), most recently used last: flicking back and forth
  // shows them at once instead of developing again.
  const previewCache = useRef(new Map<string, { key: string; image: ViewerImage }>());
  // thumbFromRender (declared further down): a render's picture as the photo's thumbnail.
  const thumbFromRenderRef = useRef<(path: string, image: string) => void>(() => {});
  const PREVIEW_CACHE_SIZE = 40;
  // Photos whose full picture (loupe and thumbnail) is rendered with their current edit: a sparkle on
  // their thumbnail (the Teach / apply look icon), so a style or paste going through a folder shows
  // where it's got. Off when a photo's edit changes, on when its render lands.
  const [ready, setReady] = useState<ReadonlySet<string>>(new Set());
  // Which photos Auto levelled (1) or levelled and keystoned (2): their icons under the sparkle.
  const autoTransforms = useMemo(() => {
    const out: Record<string, number> = {};
    for (const [p, e] of Object.entries(edits)) if (e?.autoTransform) out[p] = e.autoTransform;
    return out;
  }, [edits]);
  // Where each photo's look came from (a style, a preset, its XMP): the thumbnail's sparkle or palette.
  // Styles and presets are kept in its .rpd (`look`); an XMP's is known from the .rpd's origin.
  const [looks, setLooks] = useState<Record<string, PhotoLook>>({});
  const looksRef = useRef(looks);
  looksRef.current = looks;
  const setLook = useCallback((paths: string[], look: PhotoLook) => {
    setLooks((prev) => ({ ...prev, ...Object.fromEntries(paths.map((p) => [p, look])) }));
  }, []);
  // Shown: what the folder read said (photos not opened yet), then what's known since.
  const shownLooks = useMemo(() => {
    const out: Record<string, PhotoLook> = {};
    for (const photo of allPhotos) if (photo.look) out[photo.path] = photo.look;
    return { ...out, ...looks };
  }, [allPhotos, looks]);
  // Loaded edits say for themselves; photos not opened yet, what the folder read said.
  const denoisedPaths = useMemo(() => {
    const out = new Set<string>();
    for (const photo of allPhotos) {
      const e = edits[photo.path];
      if (e ? (e.noise ?? 0) > 0 : photo.denoised) out.add(photo.path);
    }
    return out;
  }, [allPhotos, edits]);
  const markReady = useCallback((paths: string[], on: boolean) => {
    setReady((prev) => {
      const next = new Set(prev);
      for (const p of paths) {
        if (on) next.add(p);
        else next.delete(p);
      }
      return next.size === prev.size && paths.every((p) => prev.has(p) === on) ? prev : next;
    });
  }, []);
  const rememberPreview = (path: string, key: string, image: ViewerImage) => {
    const cache = previewCache.current;
    cache.delete(path);
    cache.set(path, { key, image });
    while (cache.size > PREVIEW_CACHE_SIZE) cache.delete(cache.keys().next().value!);
  };
  // The current photo's engine settings key (set where it's computed, below).
  const engineKeyRef = useRef("");
  const photosRef = useRef(photos);
  photosRef.current = photos;
  const allPhotosRef = useRef(allPhotos);
  allPhotosRef.current = allPhotos;

  // Load the current photo, ignoring stale responses. The camera's embedded
  // JPEG arrives first (it's quick) and is kept for the compare button; our
  // own engine's development replaces it as soon as it's ready. JPEG-only
  // files, or RAWs the engine can't handle yet, stay on the camera image.
  //
  // A photo seen before shows its last engine picture straight away (from
  // `previewCache`, or the folder's own cache on disk after a reopen); it's
  // re-rendered only if its edit has changed since.
  useEffect(() => {
    if (!currentPath) return;
    const path = currentPath;
    let cancelled = false;
    let engineArrived = false;
    let shown = false;
    // RAW, TIFF and JPEG all go through the engine (imagefile.rs opens the last two).
    const isRaw = true;
    setCameraPreview(null);
    getFullPreview(path)
      .then((p) => {
        if (cancelled) return;
        setCameraPreview(p);
        // A RAW's camera JPEG only stands in until the engine's picture arrives.
        if (!engineArrived && !shown) setPreview({ ...p, source: "camera", pending: isRaw });
      })
      .catch((e) => !cancelled && setError(`Couldn't load preview: ${e}`));
    void (async () => {
      let cached = previewCache.current.get(path);
      if (!cached && isRaw) {
        const disk = await cachedPreview(path).catch(() => null);
        if (cancelled) return;
        if (disk) {
          const p = disk.preview;
          if (p.asShotTemperature != null && p.asShotTint != null) {
            setAsShot((prev) => ({ ...prev, [path]: { temperature: Math.round(p.asShotTemperature!), tint: Math.round(p.asShotTint!) } }));
          }
          rememberPreview(path, disk.key, { ...p, source: "engine", pending: false });
          cached = previewCache.current.get(path);
        }
      }
      if (cached) {
        shown = true;
        setPreview({ ...cached.image, pending: cached.key !== engineKeyRef.current });
      }
      // Decode the neighbours either side in the background, so stepping to them skips the RAW decode.
      const prefetchNeighbours = async () => {
        // This photo's subject mask first, if its edit uses it (the subject-aware vignette, or a mask
        // with the subject in it). Only then: it's an AI model taking 3-4 s of every core, which
        // held up the photo's own render on every photo change (23 Oct). The render finds it itself
        // if it turns out to be needed.
        const st = editState.current;
        const usesSubject =
          (st.edits[currentPath]?.adaptiveVignette ?? 0) !== 0 || (st.gradientsByPath[currentPath] ?? []).some(hasSubject);
        if (!cancelled && usesSubject) await prepareSubject(currentPath).catch(() => false);
        const list = photosRef.current;
        const i = list.findIndex((p) => p.path === currentPath);
        for (const n of [list[i + 1], list[i - 1]]) {
          if (cancelled || !n) continue;
          await prefetchScene(n.path).catch(() => {});
          // With Noise on, clean it too, so it shows at once when stepped to.
          if (!cancelled && (editState.current.edits[n.path]?.noise ?? 0) > 0) await prepareNoise(n.path).catch(() => {});
          if (!cancelled) await prerender(n.path);
        }
      };
      // A neighbour's edited picture, made ahead (as pasted settings are, below), so stepping to it
      // shows RapidOne's render at once rather than the camera JPEG and then a swap.
      const prerender = async (path: string) => {
        // Never while a slider moves: that photo's renders come first.
        while (interactingRef.current && !cancelled) await new Promise((r) => setTimeout(r, 150));
        if (cancelled) return;
        // Its saved edit first, as opening it would load it: without it the render (and the
        // picture saved for it) was the photo unedited.
        await ensureEditLoaded(path, true);
        await new Promise((r) => setTimeout(r, 0));
        if (cancelled) return;
        const shot0 = asShotRef.current[path];
        const kept = previewCache.current.get(path);
        if (kept?.key === JSON.stringify(viewSettingsFor(path, shot0))) {
          // Its film strip / library thumbnail from the same picture (only an edited photo's changes).
          thumbFromRenderRef.current(path, kept.image.imageBase64);
          return markReady([path], true);
        }
        // At background priority: the photo on screen comes first.
        const used = JSON.stringify(viewSettingsFor(path, renderShot(path)));
        const p = await renderPreview(path, viewSettingsFor(path, renderShot(path)), undefined, false, undefined, true).catch(() => null);
        if (!p) return;
        // Kept under the key the viewer will look for (with the as-shot white balance the render read).
        const shot = shot0 ?? (p.asShotTemperature != null && p.asShotTint != null ? { temperature: Math.round(p.asShotTemperature), tint: Math.round(p.asShotTint) } : undefined);
        if (!shot0 && shot) setAsShot((prev) => (prev[path] ? prev : { ...prev, [path]: shot }));
        const key = JSON.stringify(viewSettingsFor(path, shot));
        rememberPreview(path, key, { ...p, source: "engine", pending: false });
        markReady([path], true);
        // Its thumbnail too, so the film strip catches up as you move through the photos; not if its edit
        // changed while this rendered (a style, a paste): that newer thumbnail stays.
        if (JSON.stringify(viewSettingsFor(path, renderShot(path))) === used) thumbFromRenderRef.current(path, p.imageBase64);
        await cachePreview(path, key, p).catch(() => undefined);
      };
      if (isRaw && cached) {
        // Seen before: only make sure the engine still holds it, for the next slider move.
        void prefetchScene(currentPath)
          .catch(() => {})
          .then(prefetchNeighbours);
      } else if (isRaw) {
        developPreview(path)
          .then((p) => {
            engineArrived = true;
            if (cancelled) return;
            // The first develop is unedited: if the photo has an edit, keep it as a
            // stand-in until the edited render lands, rather than flash it.
            const s = editState.current;
            const path = p.path;
            const edited =
              Object.keys(s.edits[path] ?? {}).length > 0 ||
              Boolean(s.curvesByPath[path] || s.gradeByPath[path] || s.mixerByPath[path] || s.profileByPath[path] || s.removalsByPath[path]?.length || s.gradientsByPath[path]?.length);
            setPreview({ ...p, source: "engine", pending: edited });
            const shot = { temperature: p.asShotTemperature, tint: p.asShotTint };
            if (shot.temperature != null && shot.tint != null) {
              setAsShot((prev) => ({ ...prev, [p.path]: { temperature: Math.round(shot.temperature!), tint: Math.round(shot.tint!) } }));
            }
            void prefetchNeighbours();
          })
          .catch(() => {
            // Not fatal: the camera image stays up (now as the picture) and is labelled as such.
            if (!cancelled) setPreview((prev) => (prev && prev.path === path ? { ...prev, pending: false } : prev));
          });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [currentPath]);

  // Engine re-renders for slider changes, latest-wins: at most one render in
  // flight; when it lands, if the slider has moved on, render the newest
  // value next. Intermediate values are skipped, never queued up.
  const engineReady = preview?.source === "engine" && preview.path === currentPath;
  // Patch's tool open (PatchOverlay, below): declared here, as the engine's settings need it.
  const [patchTool, setPatchTool] = useState(false);
  // The settings the engine renders; extend this as sliders move into the engine.
  // Only the sliders the engine renders (ENGINE_SLIDERS); a stable key so the
  // effect re-runs exactly when one of them changes.
  const engineSettingsRaw = engineSettingsFrom(
    values,
    Boolean(currentPath && asShot[currentPath]),
    profile,
    mixer,
    curves,
    grade,
    // While cropping, the whole photo (only its quarter turns) under the frame. Masks, Patch and
    // Erase keep the crop as the window onto the photo (their marks live on the whole photo and are
    // lined up through it, Viewer's stand-in); only a straightened or turned crop, which that
    // can't line up, still shows them the whole photo.
    (removing || gradientTool || patchTool) && !straightCrop(crop) ? NO_CROP : cropping ? { ...NO_CROP, quarter: crop.quarter } : crop,
    removals,
    gradients,
  );
  // Remove, Patch and gradients are placed on the photo as it is, so no keystone while they're open.
  const engineSettings = removing || gradientTool || patchTool ? { ...engineSettingsRaw, keystone: { vertical: 0, horizontal: 0, rotate: 0, scale: 100 } } : engineSettingsRaw;
  const engineKey = JSON.stringify(engineSettings);
  const engineSettingsRef = useRef(engineSettings);
  engineSettingsRef.current = engineSettings;
  engineKeyRef.current = engineKey;

  // Retouch works on the photo as graded here (like Lightroom's Edit in
  // Photoshop): on its tab the current photo is rendered full size, 16-bit,
  // grade and crop included, to a TIFF it opens. Only when the grade changed.
  // Only for photos Retouch asks for (its "3, 2, 1" ran out or an edit
  // began): browsing in Retouch shows Edit's preview and never waits.
  const [handoff, setHandoff] = useState<{ path: string; key: string; tif: string } | null>(null);
  const [handoffBusy, setHandoffBusy] = useState(false);
  const [handoffWanted, setHandoffWanted] = useState<Set<string>>(new Set());
  const handoffWaiters = useRef(new Map<string, { resolve: (tif: string) => void; reject: (e: Error) => void }[]>());
  const requestHandoff = useCallback((source: string) => {
    return new Promise<string>((resolve, reject) => {
      const list = handoffWaiters.current.get(source) ?? [];
      handoffWaiters.current.set(source, [...list, { resolve, reject }]);
      setCurrentPath(source);
      setHandoffWanted((prev) => (prev.has(source) ? prev : new Set(prev).add(source)));
    });
  }, []);
  useEffect(() => {
    if (!handoff) return;
    const waiting = handoffWaiters.current.get(handoff.path);
    if (!waiting) return;
    handoffWaiters.current.delete(handoff.path);
    for (const w of waiting) w.resolve(handoff.tif);
  }, [handoff]);
  // Leaving Retouch: a photo it's still waiting for is only prepared in Retouch, so it's cancelled
  // ("left-retouch", which Retouch takes quietly) - else Retouch's request waits, and its hand-back
  // of the edited photos (which waits for that request) never starts.
  useEffect(() => {
    if (mode === "retouch" || handoffWaiters.current.size === 0) return;
    const waiting = [...handoffWaiters.current.values()].flat();
    handoffWaiters.current.clear();
    setHandoffWanted(new Set());
    for (const w of waiting) w.reject(new Error("left-retouch"));
  }, [mode]);
  useEffect(() => {
    if (mode !== "retouch" || !currentPath || !engineReady || !handoffWanted.has(currentPath)) return;
    if (handoff?.path === currentPath && handoff.key === engineKey) {
      // Already prepared with this grade: anyone waiting has it now.
      const waiting = handoffWaiters.current.get(currentPath);
      if (waiting) {
        handoffWaiters.current.delete(currentPath);
        for (const w of waiting) w.resolve(handoff.tif);
      }
      return;
    }
    let cancelled = false;
    setHandoffBusy(true);
    renderHandoff(currentPath, engineSettings, engineKey)
      .then((tif) => !cancelled && setHandoff({ path: currentPath, key: engineKey, tif }))
      .catch((e) => !cancelled && setError(`Couldn't prepare the photo for Retouch: ${e}`))
      .finally(() => !cancelled && setHandoffBusy(false));
    return () => {
      cancelled = true;
    };
    // engineSettings follows engineKey.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, currentPath, engineKey, engineReady, handoffWanted]);
  // Back in Edit from Cull: the same view (All / Rated / Unrated) as Cull
  // had, with the stars and picks given there read in.
  const lastMode = useRef(mode);
  useEffect(() => {
    const from = lastMode.current;
    lastMode.current = mode;
    if (mode !== "edit" || from !== "cull" || !folder) return;
    void (async () => {
      const paths = allPhotosRef.current.map((p) => p.path);
      if (!paths.length) return;
      const flags = await readFlags(paths).catch(() => []);
      setPhotosByPath((prev) => {
        const next = { ...prev };
        for (const [p, rating, pick] of flags) if (next[p]) next[p] = { ...next[p], rating, pick };
        return next;
      });
    })();
    // loadFolder and photosRef are stable enough here; only a change of tab matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode]);

  // All / Rated / Unrated for the open shoot, once Organise has sorted it.
  const [folderViewsState, setFolderViewsState] = useState<FolderViews | null>(null);
  useEffect(() => {
    let cancelled = false;
    if (!folder) setFolderViewsState(null);
    else void folderViews(folder).then((v) => !cancelled && setFolderViewsState(v)).catch(() => !cancelled && setFolderViewsState(null));
    return () => {
      cancelled = true;
    };
  }, [folder, mode]);

  // The noise model loads in the background the first time Edit is open
  // with a folder, so the first photo with Noise on doesn't wait for it
  // (most get Noise from Lightroom edits, presets or styles, not the panel).
  const denoiseWarmed = useRef(false);
  useEffect(() => {
    if (mode !== "edit" || !folder || denoiseWarmed.current) return;
    denoiseWarmed.current = true;
    void warmDenoise().catch(() => {});
    // Lite's Remove models too (LaMa takes about 5 s to load), so the first Remove is ready.
    void warmLite().catch(() => {});
  }, [mode, folder]);

  // Leaving Edit ends cropping, so the hand-off is cut as the crop says.
  useEffect(() => {
    if (mode !== "edit") {
      setCropping(false);
      setRemoving(false);
    }
  }, [mode]);

  // Remove: what's in the box goes (FLUX.2 Klein), as a removal added to the
  // photo's edit (one undo step). The photo it was asked for gets it, even if
  // the user has moved on.
  // The last removal, for Try again: the same request with the next seed
  // (the model's next version), replacing it in place.
  const [lastRemove, setLastRemove] = useState<{ path: string; file: string; seed: number; run: (seed: number) => Promise<Removal> } | null>(null);
  const runRemove = useCallback(
    async (path: string, run: (seed: number) => Promise<Removal>, seed = 0, replace?: string) => {
      setRemoveBusy(true);
      try {
        const r = await run(seed);
        record(path, null);
        setRemovalsByPath((prev) => {
          const list = prev[path] ?? [];
          const at = replace ? list.findIndex((x) => !isPatch(x) && x.file === replace) : -1;
          return { ...prev, [path]: at >= 0 ? list.map((x, i) => (i === at ? r : x)) : [...list, r] };
        });
        setLastRemove({ path, file: r.file, seed, run });
      } catch (e) {
        setError(`Remove failed: ${e}`);
      } finally {
        setRemoveBusy(false);
      }
    },
    [record],
  );
  const removeArea = useCallback(
    async (area: [number, number, number, number], what: string, outline?: string) => {
      const path = currentPath;
      if (path) await runRemove(path, (seed) => removeObject(path, area, what, outline, seed));
    },
    [currentPath, runRemove],
  );
  // Brush: remove the painted shape.
  const removePaintedShape = useCallback(
    async (painted: string, what: string) => {
      const path = currentPath;
      if (path) await runRemove(path, (seed) => removePainted(path, painted, what, seed));
    },
    [currentPath, runRemove],
  );
  const canRetry = Boolean(lastRemove && lastRemove.path === currentPath && removals.some((r) => !isPatch(r) && r.file === lastRemove.file));
  const retryRemove = useCallback(() => {
    if (lastRemove && canRetry) void runRemove(lastRemove.path, lastRemove.run, lastRemove.seed + 1, lastRemove.file);
  }, [lastRemove, canRetry, runRemove]);
  // Several found objects, one after another (one undo step each).
  const removeAllFound = useCallback(
    async (found: Outline[], what: string) => {
      for (const f of found) await removeArea(f.area, what, f.outline);
    },
    [removeArea],
  );
  const [canFind, setCanFind] = useState(false);
  // Full / Lite: which removers are installed and which is in use.
  const [removeModes, setRemoveModes] = useState<RemovalModes | null>(null);
  const refreshRemover = useCallback(() => {
    canFindObjects()
      .then(setCanFind)
      .catch(() => setCanFind(false));
    removalModes()
      .then(setRemoveModes)
      .catch(() => setRemoveModes(null));
  }, []);
  useEffect(refreshRemover, [refreshRemover]);
  const chooseRemover = useCallback(
    (mode: RemovalMode) => {
      void setRemovalMode(mode)
        .then(refreshRemover)
        .catch(() => undefined);
    },
    [refreshRemover],
  );
  // The Remove models live on the GPU only while the tool is open (they go
  // shortly after it closes).
  useEffect(() => {
    void removalHold(removing).catch(() => undefined);
  }, [removing]);
  // Remove open: SAM 3 analyses the photo in the background, so a box drawn
  // on it is outlined almost at once.
  useEffect(() => {
    if (removing && canFind && currentPath) void prepareFind(currentPath).catch(() => undefined);
  }, [removing, canFind, currentPath, removeModes?.mode]);
  const findInPhoto = useCallback(
    (words: string, area?: [number, number, number, number]) => (currentPath ? findObjects(currentPath, words, area) : Promise.resolve([])),
    [currentPath],
  );
  // Remove's "Undo all": its removals only (the patches stay).
  const clearRemovals = useCallback(() => {
    if (!currentPath) return;
    record(currentPath, null);
    setRemovalsByPath((prev) => {
      const kept = (prev[currentPath] ?? []).filter(isPatch);
      return withEntry(prev, currentPath, kept.length ? kept : undefined);
    });
  }, [currentPath, record]);

  // Patch (PatchOverlay, patch.rs): Heal or Clone, the photo's patches in its removals list.
  const [patchMode, setPatchMode] = useState<Patch["mode"]>("heal");
  useEffect(() => {
    if (removing || gradientTool || cropping || mode !== "edit") setPatchTool(false);
  }, [removing, gradientTool, cropping, mode]);
  const patchList = useMemo(
    () => removals.flatMap((s, index) => (isPatch(s) ? [{ index, patch: s }] : [])),
    [removals],
  );
  const changeSpots = useCallback(
    (change: (list: Spot[]) => Spot[]) => {
      if (!currentPath) return;
      record(currentPath, null);
      setRemovalsByPath((prev) => {
        const list = change(prev[currentPath] ?? []);
        return withEntry(prev, currentPath, list.length ? list : undefined);
      });
    },
    [currentPath, record],
  );
  const toggleRemoving = useCallback(async () => {
    if (removing) {
      setRemoving(false);
      return;
    }
    if (!(await removalAvailable().catch(() => false))) {
      setError("Object removal isn't set up on this computer yet (it needs the Lite models, LaMa and MobileSAM, or FLUX.2 Klein in the models folder).");
      return;
    }
    setCropping(false);
    setRemoving(true);
  }, [removing]);
  const removeByWords = useCallback(async () => {
    if (!(await removalAvailable().catch(() => false))) {
      setError("Object removal isn't set up on this computer yet (it needs the Lite models, LaMa and MobileSAM, or FLUX.2 Klein in the models folder).");
      return;
    }
    setCropping(false);
    setRemoveWhole((n) => n + 1);
    setRemoving(true);
  }, []);
  // A slider being moved (held key or pointer on the panel): renders are
  // real but from a half-size copy (fast), and not kept; the full preview
  // renders when it stops. No quick approximations are shown in between.
  const interacting = held !== null || adjusting;
  // The alignment grid over the photo while a Keystone slider moves (dragged, or its key held).
  const keystoneGrid =
    (adjusting && adjustingGroup === "Keystone") || (held !== null && ["rotate", "vertical", "horizontal", "scale"].includes(held.sliderId));
  const interactingRef = useRef(interacting);
  interactingRef.current = interacting;
  // Zoomed in on full-resolution detail: the preview is hidden under it, so
  // while a slider moves only the detail renders (it would compete for the CPU).
  const detailActive = useRef(false);
  // The size of the renders while a slider moves at Fit (of the 2400 px preview).
  const fitDragScale = useRef(new DragScale(0.6, 0.25));
  const onDetailActive = useCallback((active: boolean) => {
    detailActive.current = active;
  }, []);
  // `exactAfter`: a quick render first, then the exact one (a one-off change on a photo already shown).
  const renderState = useRef<{ busy: boolean; wanted: { path: string; settings: RenderSettings; interactive: boolean; exactAfter?: boolean } | null }>({
    busy: false,
    wanted: null,
  });
  const wasInteracting = useRef(false);
  useEffect(() => {
    // A slider just let go (its quick frames came already: straight to the exact render).
    const justReleased = wasInteracting.current && !interacting;
    wasInteracting.current = interacting;
    if (!engineReady || !currentPath || editLoadedPath !== currentPath) return;
    // Already made, with this very edit (the cache, or the folder's cache on disk): nothing to render.
    // Shown as that picture, not just un-muted: an older render (the photo before its saved edit
    // loaded) may be up instead.
    const cachedHere = previewCache.current.get(currentPath);
    if (cachedHere?.key === engineKey) {
      setPreview((prev) =>
        prev && prev.path === currentPath && (prev.pending || prev.imageBase64 !== cachedHere.image.imageBase64)
          ? { ...cachedHere.image, pending: false }
          : prev,
      );
      markReady([currentPath], true);
      return;
    }
    markReady([currentPath], false);
    const state = renderState.current;
    // A one-off change (a patch, a button, a preset) on a photo already on screen: a quick render
    // first, reusing its last Texture / Clarity / Dehaze measures as a slider drag does (a fraction of
    // a second rather than the seconds those take), then the exact one. Not just after a slider is
    // let go (its quick frames came already) nor for a photo just opened.
    const shownHere = preview?.path === currentPath && preview.source === "engine";
    const oneOff = !interacting && !justReleased && shownHere;
    state.wanted = {
      path: currentPath,
      settings: engineSettings,
      interactive: interacting || oneOff,
      exactAfter: oneOff,
    };
    if (state.busy) return;
    const pump = async () => {
      state.busy = true;
      while (state.wanted) {
        const job = state.wanted;
        state.wanted = null;
        // The exact render after a quick one, unless something newer came meanwhile.
        const exact = job.exactAfter ? { ...job, interactive: false, exactAfter: false } : null;
        if (job.interactive && detailActive.current) {
          if (exact && !state.wanted) state.wanted = exact;
          continue;
        }
        try {
          const key = JSON.stringify(job.settings);
          // While dragging: a smaller render (soft while it moves, sharp the
          // moment it stops: Chris's choice), at the size the drag settles on
          // (100/80/60/40/20%: dropping a level when frames are too slow; DragScale).
          const shownLong = Math.round(2400 * fitDragScale.current.scale);
          const started = performance.now();
          const p = await renderPreview(job.path, job.settings, job.interactive ? undefined : key, job.interactive, shownLong);
          if (job.interactive) fitDragScale.current.frame(performance.now() - started);
          else fitDragScale.current.end();
          // Superseded while it rendered: the edit changed to one whose picture is already made (the
          // photo's saved edit loaded during its first, unedited render, 24 Oct). That picture stays;
          // this one would replace it, and nothing would render again.
          const cachedNow = previewCache.current.get(job.path);
          if (key !== engineKeyRef.current && cachedNow?.key === engineKeyRef.current) continue;
          const image: ViewerImage = {
            ...p,
            source: "engine",
            pending: false,
            interactive: job.interactive,
            uncropped: isNoCrop(job.settings.crop as Crop),
          };
          // A half-size render while a slider moves isn't kept: the full one replaces it.
          if (!job.interactive) {
            rememberPreview(job.path, key, image);
            markReady([job.path], true);
            thumbFromRender(job.path, p.imageBase64);
          }
          // Ignore results for a photo the user has already left.
          setPreview((prev) => (prev && prev.path === p.path ? image : prev));
        } catch (e) {
          setError(`Render failed: ${e}`);
        }
        if (exact && !state.wanted) state.wanted = exact;
      }
      state.busy = false;
    };
    void pump();
  }, [engineReady, currentPath, engineKey, interacting, editLoadedPath]);

  // The open photo's thumbnail (film strip, Library) follows its edits: from the settled render (not
  // while a slider moves), a moment after the last change, for a photo with an edit only (an unedited
  // one keeps the camera's picture, as it has after a reopen; one undone back to no edit gets its
  // plain render once). A reopen reads the same picture from the folder's cache (commands.rs, process_photo).
  const thumbTimers = useRef(new Map<string, number>());
  // Photos whose thumbnail was set from a render this session: one undone back to no edit is set
  // once more (to its plain render), so it doesn't keep the edited look.
  const liveThumbs = useRef(new Set<string>());
  const snapshotOfRef = useRef(snapshotOf);
  snapshotOfRef.current = snapshotOf;
  // For the neighbours' renders made ahead (an effect above this).
  thumbFromRenderRef.current = (path, image) => thumbFromRender(path, image);
  const thumbFromRender = useCallback((path: string, image: string) => {
    const timers = thumbTimers.current;
    window.clearTimeout(timers.get(path));
    timers.set(
      path,
      window.setTimeout(() => {
        timers.delete(path);
        const edited = Object.values(snapshotOfRef.current(path)).some(
          (v) => v !== undefined && !(Array.isArray(v) && v.length === 0) && !(typeof v === "object" && !Array.isArray(v) && Object.keys(v).length === 0),
        );
        if (!edited && !liveThumbs.current.has(path)) return;
        if (edited) liveThumbs.current.add(path);
        else liveThumbs.current.delete(path);
        const img = new Image();
        img.onload = () => {
          const k = Math.min(1, 480 / Math.max(img.naturalWidth, img.naturalHeight));
          const c = document.createElement("canvas");
          c.width = Math.round(img.naturalWidth * k);
          c.height = Math.round(img.naturalHeight * k);
          c.getContext("2d")?.drawImage(img, 0, 0, c.width, c.height);
          const thumb = c.toDataURL("image/jpeg", 0.82);
          setPhotosByPath((prev) => (prev[path] ? { ...prev, [path]: { ...prev[path], thumbBase64: thumb } } : prev));
        };
        img.src = image;
      }, 700),
    );
  }, []);

  // The working folder, shared by Cull, Edit and Retouch: opened here, from
  // the Home prompt, or from inside Cull (which tells us through onFolderChange).
  const loadFolder = useCallback(async (picked: string, exact = false) => {
    const scan = ++scanRef.current;
    // A shoot already organised by Cull opens at its "rated" folder in Edit
    // (in Cull, or chosen on the All / Rated / Unrated switch, the folder is kept).
    const chosen = exact || modeRef.current === "cull" ? picked : ((await ratedSubfolder(picked).catch(() => null)) ?? picked);
    if (scan !== scanRef.current) return;
    setHome(false);
    setFolder(chosen);
    try {
      localStorage.setItem(LAST_FOLDER_KEY, chosen);
    } catch {
      // Not fatal: the home screen just won't offer to resume.
    }
    setStripFilter(NO_STRIP_FILTER);
    setPhotosByPath({});
    setCurrentPath(null);
    setPreview(null);
    previewCache.current.clear();
    setEdits({});
    setAsShot({});
    setCurvesByPath({});
    setGradeByPath({});
    setMixerByPath({});
    setProfileByPath({});
    setCropByPath({});
    setRemovalsByPath({});
    setGradientsByPath({});
    setCropping(false);
    setRemoving(false);
    setGradientTool(false);
    setError(null);
    setNotice(null);
    lightroomChecked.current.clear();
    editLoads.current.clear();
    setEditLoadedPath(null);
    setSelection(new Set());
    setReady(new Set());
    sidecarWritten.current.clear();
    sidecarOrigin.current.clear();
    try {
      const failed = await scanFolder(chosen, scan);
      if (scan !== scanRef.current) return;
      if (failed.length > 0)
        setError(
          `${failed.length} file${failed.length === 1 ? "" : "s"} couldn't be read: ${failed.slice(0, 3).map((f) => f.split(/[\\/]/).pop()).join(", ")}${failed.length > 3 ? "…" : ""}.`,
        );
    } catch (e) {
      if (scan === scanRef.current) setError(`Couldn't open folder: ${e}`);
    }
  }, []);

  // The x on the loading bar: the read stops, the photos read so far stay.
  const cancelFolderRead = useCallback(() => {
    scanRef.current++;
    setProgress(null);
    void cancelScan();
  }, []);

  const openFolder = useCallback(async () => {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") await loadFolder(chosen);
  }, [loadFolder]);

  // Resume last session: the last working folder again, then the photo that was open.
  const resumeSession = useMemo(
    () =>
      lastFolder
        ? {
            name: lastFolder.split(/[\\/]/).filter(Boolean).pop() ?? lastFolder,
            run: () => {
              let photo: string | null = null;
              try {
                photo = localStorage.getItem(LAST_PHOTO_KEY);
              } catch {
                // No storage: the folder opens at its first photo.
              }
              setResumePhoto(photo);
              void loadFolder(lastFolder, true);
            },
          }
        : null,
    [lastFolder, loadFolder],
  );
  useEffect(() => {
    if (!resumePhoto) return;
    if (photosByPath[resumePhoto]) {
      setCurrentPath(resumePhoto);
      setResumePhoto(null);
    } else if (progress && progress.total > 0 && progress.done >= progress.total) {
      setResumePhoto(null); // the photo is gone from the folder: stay on the first
    }
  }, [resumePhoto, photosByPath, progress]);
  // Which photo is open, remembered for the next start (Resume, and the home screen's picture).
  useEffect(() => {
    if (!currentPath) return;
    try {
      localStorage.setItem(LAST_PHOTO_KEY, currentPath);
    } catch {
      // Not fatal.
    }
  }, [currentPath]);

  // The picked mask, with a mask tool open (its numbers are the bottom panel's then).
  const maskMode = gradientTool;
  const pickedMask = maskMode && maskPicked >= 0 ? (gradients[maskPicked] ?? null) : null;
  const maskTarget = useRef<{ index: number; gradient: Gradient } | null>(null);
  maskTarget.current = pickedMask ? { index: maskPicked, gradient: pickedMask } : null;
  const maskModeRef = useRef(maskMode);
  maskModeRef.current = maskMode;
  const setSlider = useCallback(
    (id: string, value: number) => {
      if (!currentPath) return;
      // A mask tool open: the picked mask's slider, never the photo's (the others do nothing).
      if (maskModeRef.current) {
        const t = maskTarget.current;
        if (!t || !isMaskSlider(id)) return;
        // Moving a slider hides the red mask, so the effect itself can be seen (Show mask brings it back).
        setMaskShow(false);
        record(currentPath, `gradient:${t.index}:${id}`);
        const g = { ...t.gradient, [id]: value };
        maskTarget.current = { index: t.index, gradient: g };
        setGradientsByPath((prev) => {
          const list = [...(prev[currentPath] ?? [])];
          if (!list[t.index]) return prev;
          list[t.index] = { ...list[t.index], [id]: value };
          return { ...prev, [currentPath]: list };
        });
        return;
      }
      record(currentPath, `slider:${id}`);
      setEdits((prev) => ({ ...prev, [currentPath]: { ...prev[currentPath], [id]: value } }));
    },
    [currentPath, record],
  );
  // What the bottom panel (and the slider keys) show and move: the photo's values, or with a
  // mask tool open the picked mask's for the sliders masks have.
  const panelValues: Adjustments = useMemo(() => {
    if (!maskMode) return values;
    const out = { ...values } as Record<string, number>;
    for (const id of MASK_SLIDER_IDS) out[id] = pickedMask ? (pickedMask[id] as number) : 0;
    return out as Adjustments;
  }, [maskMode, values, pickedMask]);
  const panelNeutrals: Adjustments = useMemo(() => {
    if (!maskMode) return neutrals;
    const out = { ...neutrals } as Record<string, number>;
    for (const id of MASK_SLIDER_IDS) out[id] = 0;
    return out as Adjustments;
  }, [maskMode, neutrals]);
  // A slider's definition as the keys use it: the mask's version with a mask tool open.
  const defOf = useCallback(
    (id: string): SliderDef | undefined => {
      const def = SLIDERS_BY_ID[id];
      if (!def || !maskModeRef.current) return def;
      return maskSliderDef(def) ?? undefined;
    },
    [],
  );

  // Auto level / auto keystone, for one photo (the Keystone panel's buttons)
  // or many (Straighten in the top bar). The paths are fixed when the button
  // is pressed, even if the user moves on. One undo step per photo changed.
  // The photo remembers which was applied (`autoTransform`: 1 level, 2 level +
  // keystone), so the panel's button shows as on and a second click turns it off.
  const [autoBusy, setAutoBusy] = useState<"level" | "keystone" | null>(null);
  const straighten = useCallback(
    async (kind: "level" | "keystone", paths: string[]) => {
      if (paths.length === 0) return;
      setAutoBusy(kind);
      const batch = paths.length > 1;
      let noLines = 0;
      let levelOnly = 0;
      let failed = 0;
      let done = 0;
      // Many photos: as a look applied to all (applyAutoStyle). The photo on screen first, then outward
      // from it; several measured at once (at background priority); their changes put into the edits
      // a few times a second; each photo's full picture (loupe and thumbnail) rendered in the
      // background as it's done, no bar, its icons appearing on its thumbnail when it's rendered.
      const here = Math.max(0, photos.findIndex((p) => p.path === currentPath));
      const order = new Map(photos.map((p, i) => [p.path, i]));
      const ordered = batch ? [...paths].sort((a, b) => Math.abs((order.get(a) ?? 0) - here) - Math.abs((order.get(b) ?? 0) - here)) : paths;
      let pending: Record<string, Partial<Adjustments>> = {};
      let queued: string[] = [];
      const flush = () => {
        const changes = pending;
        const render = queued;
        pending = {};
        queued = [];
        if (Object.keys(changes).length) {
          setEdits((prev) => {
            const out = { ...prev };
            for (const [p, c] of Object.entries(changes)) out[p] = { ...prev[p], ...c };
            return out;
          });
        }
        if (render.length) prepareFull(render);
      };
      const timer = batch ? window.setInterval(flush, 250) : null;
      const one = async (path: string) => {
        try {
          const result = await autoTransform(path);
          if (result.lineCount === 0) {
            noLines++;
            return;
          }
          // From scratch: any earlier keystone (Vertical, Horizontal, its Scale) gone first, so Level
          // after Keystone is a plain turn.
          const changes: Adjustments = { rotate: roundRotate(kind === "keystone" ? result.rotateDeg : result.rotateLevelDeg), vertical: 0, horizontal: 0, scale: 100, autoTransform: 1 };
          if (kind === "keystone") {
            if (result.verticalTiltDeg === null) levelOnly++;
            else {
              changes.vertical = tiltToVertical(result.verticalTiltDeg);
              changes.autoTransform = 2;
            }
          }
          // Zoomed just enough to crop out the white the turn (and keystone) uncovers, as Lightroom's
          // Constrain Crop: one photo (the Keystone panel's buttons) or many.
          // (At most the slider's 150%: a steeper warp keeps a little white there.)
          changes.scale = Math.min(150, changes.autoTransform === 2 ? result.coverScaleKeystone : result.coverScaleLevel);
          record(path, null);
          markReady([path], false);
          // Readable straight away (as restore's), so its full render, made next, has the change.
          const st = editState.current;
          editState.current = { ...st, edits: { ...st.edits, [path]: { ...st.edits[path], ...changes } } };
          if (!batch || path === currentPath) setEdits((prev) => ({ ...prev, [path]: { ...prev[path], ...changes } }));
          else {
            pending[path] = changes;
            queued.push(path);
          }
        } catch (e) {
          failed++;
          if (!batch) setError(`Auto ${kind} failed: ${e}`);
        } finally {
          done++;
        }
      };
      try {
        // Their saved edits first (see loadEditsRef), so the turn goes on top of them (a look kept).
        await loadEditsRef.current(ordered);
        const next = { i: 0 };
        const workers = batch ? Math.min(8, Math.max(1, Math.floor(cores.current / 4)), ordered.length) : 1;
        await Promise.all(
          Array.from({ length: workers }, async () => {
            while (next.i < ordered.length) await one(ordered[next.i++]);
          }),
        );
      } finally {
        if (timer !== null) window.clearInterval(timer);
        flush();
        setAutoBusy(null);
      }
      if (!batch) {
        if (noLines) setError("No straight lines found to level on in this photo.");
        else if (levelOnly) setError("Not enough vertical lines for auto keystone, so the photo was only levelled.");
        return;
      }
      const changed = paths.length - noLines - failed;
      setNotice(
        `${kind === "keystone" ? "Levelled and keystoned" : "Levelled"} ${changed} photo${changed === 1 ? "" : "s"}` +
          (levelOnly ? `; ${levelOnly} only levelled (too few verticals)` : "") +
          (noLines ? `; ${noLines} left alone (no straight lines)` : "") +
          (failed ? `; ${failed} couldn't be read` : "") + ".",
      );
    },
    [record, photos, currentPath, markReady, prepareFull],
  );
  const autoActive: "level" | "keystone" | null = values.autoTransform === 2 ? "keystone" : values.autoTransform === 1 ? "level" : null;
  const runAutoTransform = useCallback(
    (kind: "level" | "keystone") => {
      const path = currentPath;
      if (!path) return;
      if (kind === autoActive) {
        // Off: straight back to no rotation or keystone, one undo step.
        record(path, null);
        setEdits((prev) => {
          const { autoTransform: _, ...rest } = prev[path] ?? {};
          return { ...prev, [path]: { ...rest, rotate: 0, vertical: 0, horizontal: 0, scale: 100 } };
        });
        return;
      }
      void straighten(kind, [path]);
    },
    [currentPath, straighten, autoActive, record],
  );

  const setCurves = useCallback(
    (next: Curves) => {
      if (!currentPath) return;
      record(currentPath, "curves");
      setCurvesByPath((prev) => ({ ...prev, [currentPath]: next }));
    },
    [currentPath, record],
  );

  const setGrade = useCallback(
    (next: Grade) => {
      if (!currentPath) return;
      record(currentPath, "grade");
      setGradeByPath((prev) => ({ ...prev, [currentPath]: next }));
    },
    [currentPath, record],
  );

  const setMixer = useCallback(
    (next: Mixer) => {
      if (!currentPath) return;
      record(currentPath, "mixer");
      setMixerByPath((prev) => ({ ...prev, [currentPath]: next }));
    },
    [currentPath, record],
  );

  const setCrop = useCallback(
    (next: Crop) => {
      if (!currentPath) return;
      record(currentPath, "crop");
      setCropByPath((prev) => ({ ...prev, [currentPath]: next }));
    },
    [currentPath, record],
  );
  // Changing photo closes the crop tool.
  useEffect(() => setCropping(false), [currentPath]);

  const setProfile = useCallback(
    (next: ProfileId) => {
      if (!currentPath) return;
      record(currentPath, "profile");
      setProfileByPath((prev) => ({ ...prev, [currentPath]: next }));
    },
    [currentPath, record],
  );

  // Lens corrections (the Lens button). "Correct lenses automatically" is app-wide (each
  // photo by its own lens, so mixed shoots just work); a photo can be switched on or off by
  // itself. Nothing is redone in the background: each photo is corrected when it's next shown
  // or exported (the change is small, not worth re-rendering a whole folder for).
  const [lensAuto, setLensAutoState] = useState(() => {
    try {
      return localStorage.getItem(LENS_AUTO_KEY) === "1";
    } catch {
      return false;
    }
  });
  // Bumped when a lens is chosen or profiles are added: renders aren't taken from the cache.
  const [lensRev, setLensRev] = useState(0);
  lensPrefs.auto = lensAuto;
  lensPrefs.rev = lensRev;
  const setLensAuto = useCallback(
    (on: boolean) => {
      try {
        localStorage.setItem(LENS_AUTO_KEY, on ? "1" : "0");
      } catch {
        // Not kept past this session.
      }
      lensPrefs.auto = on;
      setLensAutoState(on);
    },
    [],
  );
  const setLensPhoto = useCallback(
    (on: boolean) => {
      if (!currentPath) return;
      record(currentPath, "lens");
      setEdits((prev) => ({ ...prev, [currentPath]: { ...prev[currentPath], lensCorrect: on ? 100 : 0 } }));
    },
    [currentPath, record],
  );
  // A lens chosen for every photo with this EXIF lens name: remembered by the engine; the
  // photos with that name (read from their metadata) are redone.
  const chooseLens = useCallback(
    async (exif: string, lensId: number) => {
      try {
        await lensChoose(exif, lensId);
        lensPrefs.rev += 1;
        setLensRev(lensPrefs.rev);
      } catch (e) {
        setError(`Couldn't choose the lens: ${e}`);
      }
    },
    [],
  );
  const loadLensProfiles = useCallback(async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const chosen = await open({ multiple: true, title: "Load lens profiles (Lensfun .xml)", filters: [{ name: "Lensfun lens profiles", extensions: ["xml", "XML"] }] });
    const paths = chosen ? (Array.isArray(chosen) ? chosen : [chosen]) : [];
    if (!paths.length) return;
    try {
      const [added, problems] = await lensAddProfiles(paths);
      if (added.length) {
        lensPrefs.rev += 1;
        setLensRev(lensPrefs.rev);
      }
      const parts = [];
      if (added.length) parts.push(`Lens profiles added: ${added.join(", ")}.`);
      if (problems.length) parts.push(`Not added: ${problems.join("; ")}.`);
      setNotice(parts.join(" ") || null);
    } catch (e) {
      setError(`Couldn't load the lens profiles: ${e}`);
    }
  }, []);

  // Lightroom settings (a preset, or a photo's own Lightroom edit) applied to
  // some photos: each keeps its own state for whatever the settings leave out.
  const applyLightroomTo = useCallback(
    (paths: string[], lr: LightroomSettings) => {
      // On each photo's edit as it is now (editState: the latest, saved edits just loaded included),
      // merged: what the look doesn't set (a Level's rotation, a crop) stays as it was.
      const now = editState.current;
      const next = paths.map((p) => ({
        path: p,
        state: applyLightroom(
          {
            edits: now.edits[p] ?? {},
            profile: now.profileByPath[p] ?? DEFAULT_PROFILE,
            mixer: now.mixerByPath[p] ?? DEFAULT_MIXER,
            curves: now.curvesByPath[p] ?? DEFAULT_CURVES,
            grade: now.gradeByPath[p] ?? DEFAULT_GRADE,
          },
          lr,
          adobeAvailable,
        ),
      }));
      const pick = <T,>(key: (s: (typeof next)[number]["state"]) => T) =>
        Object.fromEntries(next.map(({ path, state }) => [path, key(state)]));
      const edits = pick((s) => s.edits);
      const profiles = pick((s) => s.profile);
      const mixers = pick((s) => s.mixer);
      const curves = pick((s) => s.curves);
      const grades = pick((s) => s.grade);
      setEdits((prev) => ({ ...prev, ...edits }));
      setProfileByPath((prev) => ({ ...prev, ...profiles }));
      setMixerByPath((prev) => ({ ...prev, ...mixers }));
      setCurvesByPath((prev) => ({ ...prev, ...curves }));
      setGradeByPath((prev) => ({ ...prev, ...grades }));
      // Readable straight away (as restore's), for what follows (a style's own white balance, renders).
      editState.current = {
        ...now,
        edits: { ...now.edits, ...edits },
        profileByPath: { ...now.profileByPath, ...profiles },
        mixerByPath: { ...now.mixerByPath, ...mixers },
        curvesByPath: { ...now.curvesByPath, ...curves },
        gradeByPath: { ...now.gradeByPath, ...grades },
      };
    },
    [adobeAvailable],
  );

  // Presets (RapidOne's own, in the app's presets folder; Lightroom presets
  // are converted into them on import). Sorted by name in the menu.
  const [presets, setPresets] = useState<RapidPreset[]>([]);
  const reloadPresets = useCallback(() => {
    listPresets()
      .then((texts) => setPresets(texts.map(parsePreset).filter((p): p is RapidPreset => p !== null)))
      .catch((e) => setError(`Couldn't read the presets: ${e}`));
  }, []);
  useEffect(reloadPresets, [reloadPresets]);

  // Auto Style: white balance and exposure learned from the photographer's
  // own edits, for this photo or every photo in the folder (one undo step each).
  const [autoStyles, setAutoStyles] = useState<AutoStyle[]>([]);
  const [autoStyleProgress, setAutoStyleProgress] = useState<string | null>(null);
  const reloadStyles = useCallback(() => {
    autoStyleList().then(setAutoStyles).catch(() => setAutoStyles([]));
  }, []);
  useEffect(reloadStyles, [reloadStyles]);

  // Train Style / Increase training: progress on the Style button while it
  // runs (reading the edits, then training), a notice when done.
  const trainStyle = useCallback(
    async (name: string, folders: string[], style?: AutoStyle) => {
      setAutoStyleProgress("Training…");
      const unlisten = await onStyleProgress((p) => {
        const pct = Math.round((p.done / Math.max(p.total, 1)) * 100);
        setAutoStyleProgress(
          p.stage === "reading" ? `Reading edits ${p.done}/${p.total}` : p.stage === "checking" ? "Checking contrast…" : `Training ${pct}%`,
        );
      });
      try {
        const trained = await autoStyleTrain(name, folders, style?.path);
        const added = style ? trained.photos - style.photos : trained.photos;
        setNotice(
          style
            ? `"${trained.name}" now knows ${trained.photos.toLocaleString()} photos from ${trained.shoots} shoots (${added.toLocaleString()} added).`
            : `Style "${trained.name}" trained on ${trained.photos.toLocaleString()} edited photos from ${trained.shoots} shoot${trained.shoots === 1 ? "" : "s"}. It's in the Style menu.`,
        );
        reloadStyles();
      } catch (e) {
        setError(`${style ? "Increase training" : "Train Style"} failed: ${e}`);
      } finally {
        unlisten();
        setAutoStyleProgress(null);
      }
    },
    [reloadStyles],
  );
  // Denoise all: the Noise slider on every photo shot at `minIso` or above (one undo step each);
  // photos below it, or with no ISO in their file, keep theirs.
  const [denoiseProgress, setDenoiseProgress] = useState<string | null>(null);
  const applyDenoiseAll = useCallback(
    async (minIso: number, amount: number) => {
      // The photo on screen first, then outward from it, so what you're looking at changes first.
      const here = Math.max(0, photos.findIndex((p) => p.path === currentPath));
      const all = photos.map((p, i) => ({ p: p.path, d: Math.abs(i - here) })).sort((a, b) => a.d - b.d).map((x) => x.p);
      if (all.length === 0) return;
      let read = 0;
      let unknown = 0;
      const isos = new Map<string, number>();
      const next = { i: 0 };
      setDenoiseProgress(`Reading ISO 0/${all.length}`);
      await Promise.all(
        Array.from({ length: Math.min(8, all.length) }, async () => {
          while (next.i < all.length) {
            const p = all[next.i++];
            const iso = await shotDetails(p).then((d) => d.iso).catch(() => null);
            if (iso == null) unknown++;
            else isos.set(p, iso);
            if (++read % 10 === 0) setDenoiseProgress(`Reading ISO ${read}/${all.length}`);
          }
        }),
      );
      const paths = all.filter((p) => (isos.get(p) ?? 0) >= minIso);
      if (paths.length) {
        // Their saved edits first (see loadEditsRef), so Noise goes on top of them.
        await loadEditsRef.current(paths);
        // The photo on screen recorded last, so it stays the one reopened next time.
        for (const p of [...paths].reverse()) record(p, null);
        markReady(paths, false);
        setEdits((prev) => {
          const out = { ...prev };
          for (const p of paths) out[p] = { ...prev[p], noise: amount };
          return out;
        });
        const st = editState.current;
        editState.current = { ...st, edits: { ...st.edits, ...Object.fromEntries(paths.map((p) => [p, { ...st.edits[p], noise: amount }])) } };
        prepareFull(paths.filter((p) => p !== currentPath));
      }
      setDenoiseProgress(null);
      setNotice(
        `Denoise ${amount} applied to ${paths.length} photo${paths.length === 1 ? "" : "s"} at ISO ${minIso.toLocaleString()} or above` +
          (unknown ? ` (${unknown} had no ISO in the file and were left as they were)` : "") +
          ".",
      );
    },
    [photos, currentPath, record, markReady, prepareFull],
  );
  const applyAutoStyle = useCallback(
    async (style: AutoStyle, scope: AutoStyleScope) => {
      // With "+ straighten": each photo levelled too, in the same pass (one render for both).
      const level = scope === "all-straighten";
      const all = scope !== "photo";
      let levelled = 0;
      // The photo on screen first, then outward from it, so what you're looking at changes first.
      const here = Math.max(0, photos.findIndex((p) => p.path === currentPath));
      const paths =
        all
          ? photos.map((p, i) => ({ p: p.path, d: Math.abs(i - here) })).sort((a, b) => a.d - b.d).map((x) => x.p)
          : currentPath
            ? [currentPath]
            : [];
      if (paths.length === 0) return;
      // One undo step per photo for the whole style: its look first (every
      // setting the edits share; masks never), then each photo's white
      // balance, tint and exposure.
      // Their saved edits first (see loadEditsRef), so the look goes on top of them.
      await loadEditsRef.current(paths);
      for (const p of paths) record(p, null);
      markReady(paths, false);
      setLook(paths, { from: "style", name: style.name });
      const found = await autoStyleLook(style.path).catch(() => null);
      // A look from RapidOne edits: into preset settings as a preset saved from an edit is, so it goes
      // on exactly as one from an .xmp.
      const own = found?.rapidone ? snapshotFromEdit(found.rapidone) : null;
      const learned =
        found?.lightroom ??
        (own
          ? presetFromState(style.name, {
              edits: own.edits ?? {},
              profile: own.profile ?? DEFAULT_PROFILE,
              mixer: own.mixer ?? DEFAULT_MIXER,
              curves: own.curves ?? DEFAULT_CURVES,
              grade: own.grade ?? DEFAULT_GRADE,
            }).settings
          : null);
      // Never its noise reduction (Lightroom's Luminance becomes Noise): RapidOne's denoise is its
      // own, chosen per photo, not part of a look (Chris, 23 Oct).
      const look = learned && { ...learned, sliders: Object.fromEntries(Object.entries(learned.sliders).filter(([k]) => k !== "noise")) };
      if (look) applyLightroomTo(paths, look);
      let done = 0;
      let withContrast = false;
      const failed: string[] = [];
      // Several photos at once: each is a small RAW develop and a tiny network (about 0.13 s), so
      // one at a time left most of the cores idle.
      const next = { i: 0 };
      // Each photo's own settings gathered and put into the edits a few times a second, not one by one
      // (the whole interface redrew for each, about 60 a second, and editing stuttered meanwhile).
      let pending: Record<string, Partial<Adjustments>> = {};
      let queued: string[] = [];
      const flush = () => {
        const batch = pending;
        const render = queued;
        pending = {};
        queued = [];
        if (Object.keys(batch).length) {
          setEdits((prev) => {
            const out = { ...prev };
            for (const [p, own] of Object.entries(batch)) out[p] = { ...prev[p], ...own };
            return out;
          });
        }
        // Its full picture (loupe and thumbnail at once) follows in the background, nearest the photo
        // on screen first, no bar (Chris, 23 Oct: no thumbnail pass, no progress bar).
        if (render.length) prepareFull(render);
        setAutoStyleProgress(`Applying style ${done}/${paths.length}`);
      };
      const timer = window.setInterval(flush, 250);
      const one = async (p: string) => {
        try {
          // The style's own settings and (with "+ straighten") the level, worked out side by side.
          const [s, turn] = await Promise.all([autoStylePhoto(style.path, p), level ? autoTransform(p).catch(() => null) : null]);
          // Contrast too from a style that learned it (trained from 23 Oct); else its look's stands.
          const own: Partial<Adjustments> = {
            temperature: Math.round(s.temperature),
            tint: Math.round(s.tint),
            exposure: Math.round(s.exposure * 100) / 100,
            ...(s.contrast != null ? { contrast: Math.round(s.contrast) } : {}),
          };
          if (s.contrast != null) withContrast = true;
          // Levelled from scratch (any keystone gone), zoomed just enough to crop out the white the
          // turn uncovers (at most the slider's 150%), as Level all did.
          if (turn && turn.lineCount > 0) {
            Object.assign(own, { rotate: roundRotate(turn.rotateLevelDeg), vertical: 0, horizontal: 0, scale: Math.min(150, turn.coverScaleLevel), autoTransform: 1 });
            levelled++;
          }
          // The photo on screen at once (it's what you're looking at); the rest in the next batch.
          if (p === currentPath) setEdits((prev) => ({ ...prev, [p]: { ...prev[p], ...own } }));
          else pending[p] = own;
          // Readable straight away (as restore's), so its full render, made next, has the whole style.
          const st = editState.current;
          editState.current = { ...st, edits: { ...st.edits, [p]: { ...st.edits[p], ...own } } };
        } catch {
          failed.push(p);
        }
        // One whose own white balance couldn't be worked out still has the style's look.
        if (all && p !== currentPath) queued.push(p);
        done++;
      };
      setAutoStyleProgress(`Applying style 0/${paths.length}`);
      await Promise.all(
        // A quarter of the cores' worth at once (8 on 32 cores, 4 on 16), so the rest stay free.
        Array.from({ length: Math.min(Math.min(8, Math.max(1, Math.floor(cores.current / 4))), paths.length) }, async () => {
          while (next.i < paths.length) await one(paths[next.i++]);
        }),
      );
      window.clearInterval(timer);
      flush();
      setAutoStyleProgress(null);
      setNotice(
        `Applied style "${style.name}"${look ? " (its look, then" : " ("} white balance, tint${withContrast ? ", exposure and contrast" : " and exposure"}) to ${done - failed.length} photo${done - failed.length === 1 ? "" : "s"}` +
          (level ? `, ${levelled} levelled` : "") +
          (failed.length
            ? `; ${failed.length} couldn't be read (${failed.slice(0, 3).map(baseName).join(", ")}${failed.length > 3 ? "…" : ""})${look ? `, so ${failed.length === 1 ? "it has" : "they have"} the look only` : ""}`
            : "") +
          ".",
      );
    },
    [photos, currentPath, record, applyLightroomTo, prepareFull, setLook],
  );

  const applyPreset = useCallback(
    (preset: RapidPreset, scope: PresetScope) => {
      // The photo on screen first, then outward from it, so what you're looking at changes first (as a style).
      const here = Math.max(0, photos.findIndex((p) => p.path === currentPath));
      const paths =
        scope === "all"
          ? photos.map((p, i) => ({ p: p.path, d: Math.abs(i - here) })).sort((a, b) => a.d - b.d).map((x) => x.p)
          : currentPath
            ? [currentPath]
            : [];
      if (paths.length === 0) return;
      // Their saved edits first (see loadEditsRef), so the preset goes on top of them.
      void loadEditsRef.current(paths).then(() => {
        // One undo step on each photo it touches.
        for (const p of paths) record(p, null);
        markReady(paths, false);
        applyLightroomTo(paths, preset.settings);
        setLook(paths, { from: "preset", name: preset.name });
        // Their full pictures (loupe and thumbnail at once) follow in the background, as a style's do.
        if (scope === "all") prepareFull(paths.filter((p) => p !== currentPath));
        const what = scope === "all" ? `Preset (all ${paths.length} photos)` : "Preset";
        setNotice(describeApplied(preset.settings, what, adobeAvailable));
      });
    },
    [photos, currentPath, applyLightroomTo, adobeAvailable, record, markReady, setLook, prepareFull],
  );

  const savePresetFromPhoto = useCallback(
    (name: string) => {
      if (!currentPath) return;
      const snap = snapshotOf(currentPath);
      const preset = presetFromState(name, {
        edits: snap.edits ?? {},
        profile: snap.profile ?? DEFAULT_PROFILE,
        mixer: snap.mixer ?? DEFAULT_MIXER,
        curves: snap.curves ?? DEFAULT_CURVES,
        grade: snap.grade ?? DEFAULT_GRADE,
      });
      savePreset(name, presetText(preset))
        .then(() => {
          reloadPresets();
          setNotice(`Preset "${name}" saved.`);
        })
        .catch((e) => setError(`Couldn't save the preset: ${e}`));
    },
    [currentPath, snapshotOf, reloadPresets],
  );

  const removePreset = useCallback(
    (name: string) => {
      deletePreset(name)
        .then(reloadPresets)
        .catch((e) => setError(`Couldn't delete the preset: ${e}`));
    },
    [reloadPresets],
  );

  // Import presets: a Lightroom .xmp is read, converted and saved as a
  // RapidOne preset (named as in Lightroom); a RapidOne backup (.rpp) is
  // saved as it is. The same name replaces a preset.
  const importLightroomPresets = useCallback(async () => {
    const chosen = await open({ multiple: true, filters: [{ name: "Presets (Lightroom .xmp, RapidOne .rpp)", extensions: ["xmp", "XMP", "rpp", "RPP"] }] });
    const files = typeof chosen === "string" ? [chosen] : (chosen ?? []);
    if (files.length === 0) return;
    const names: string[] = [];
    const failed: string[] = [];
    for (const file of files) {
      const fileName = file.split(/[\\/]/).pop() ?? file;
      try {
        const preset = /\.rpp$/i.test(file) ? parsePreset(await readTextFile(file)) : presetFromLightroom(await readLightroomPreset(file), fileName);
        if (!preset) throw new Error("not a RapidOne preset");
        await savePreset(preset.name, presetText(preset));
        names.push(preset.name);
      } catch {
        failed.push(fileName);
      }
    }
    reloadPresets();
    if (names.length > 0) setNotice(`Imported ${names.length} preset${names.length === 1 ? "" : "s"}: ${names.join(", ")}.`);
    if (failed.length > 0) setError(`Couldn't read: ${failed.join(", ")}.`);
  }, [reloadPresets]);

  // Export every preset into a chosen folder, as a backup.
  const exportAllPresets = useCallback(async () => {
    const folderChosen = await open({ directory: true, multiple: false, title: "Folder for the presets backup" });
    if (typeof folderChosen !== "string") return;
    exportPresets(folderChosen)
      .then((n) => setNotice(`${n} preset${n === 1 ? "" : "s"} backed up to ${folderChosen}.`))
      .catch((e) => setError(`Couldn't export the presets: ${e}`));
  }, []);

  // The first time a photo is opened: its RapidOne sidecar (.rpd) if it has
  // one; if not, its Lightroom edit (XMP), which is then saved as the .rpd
  // straight away (by the writer below), so RapidOne takes over from there.
  const lightroomChecked = useRef(new Set<string>());
  const applyRef = useRef(applyLightroomTo);
  applyRef.current = applyLightroomTo;
  const editedRef = useRef(edits);
  editedRef.current = edits;
  // Per photo: the edit last written to its .rpd (JSON, to skip unchanged
  // writes), and where its edit first came from.
  const sidecarWritten = useRef(new Map<string, string>());
  const sidecarOrigin = useRef(new Map<string, SidecarFile["origin"]>());
  // Nudges the writer below when a photo needs an .rpd without its edit changing.
  const [sidecarTick, bumpSidecars] = useState(0);
  // Reads a photo's saved edit into the app once; asking again waits for (or
  // returns) the same load. Opening a photo does this, and so does Export for
  // photos not opened this session, so their .rpd edit is not missed. `quiet`:
  // no notice about a Lightroom edit being read.
  const editLoads = useRef(new Map<string, Promise<void>>());
  const ensureEditLoaded = useCallback(
    (path: string, quiet = false): Promise<void> => {
      const existing = editLoads.current.get(path);
      if (existing) return existing;
      lightroomChecked.current.add(path);
      const load = (async () => {
        const own = parseSidecar(await readSidecar(path).catch(() => null));
        // An empty edit of RapidOne's (written on first opening), or one taken from an earlier .xmp, gives
        // way to a Lightroom / Aftershoot .xmp written since: the backend says so by handing it over
        // (sidecar::lightroom_applies).
        const replaceable = own !== null && (!editHasContent(own.edit) || own.origin === "lightroom-xmp");
        const newerXmp = replaceable ? await readLightroomEdit(path).catch(() => null) : null;
        if (own && !newerXmp) {
          const snap = snapshotFromEdit(own.edit);
          sidecarOrigin.current.set(path, own.origin);
          const look = own.look ?? (own.origin === "lightroom-xmp" ? { from: "xmp" as const, name: "Lightroom XMP" } : null);
          if (look) setLook([path], look);
          sidecarWritten.current.set(path, JSON.stringify([editFromSnapshot(snap), own.rating, own.pick, own.look ?? null]));
          restore(path, snap);
          return;
        }
        const lr = newerXmp ?? (await readLightroomEdit(path).catch(() => null));
        // Every photo opened gets an .rpd (an empty edit if it has none), so its
        // cached picture, which belongs to the .rpd, is kept for next time.
        sidecarWritten.current.set(path, "");
        bumpSidecars((n) => n + 1);
        if (!lr || Object.keys(editedRef.current[path] ?? {}).length > 0) return;
        sidecarOrigin.current.set(path, "lightroom-xmp");
        setLook([path], { from: "xmp", name: "Lightroom XMP" });
        applyRef.current([path], lr);
        // Its crop too (converted from Lightroom's in preset.rs).
        const lrCrop = lr.crop;
        if (lrCrop) setCropByPath((prev) => withEntry(prev, path, lrCrop));
        const lrGradients = lr.gradients;
        if (lrGradients?.length) setGradientsByPath((prev) => withEntry(prev, path, lrGradients));
        if (!quiet) setNotice(describeApplied(lr, "Lightroom edit", adobeAvailable));
        // Let the state land, so the edit can be read back (Export does).
        await new Promise((resolve) => setTimeout(resolve, 100));
      })();
      editLoads.current.set(path, load);
      return load;
    },
    [adobeAvailable, restore],
  );
  // The open photo whose saved edit has loaded: it isn't rendered before then. Its picture from the
  // folder's cache shows meanwhile, and it's checked against the edit only once the edit is known
  // (before, an edit not yet read looked like none, and the photo was rendered unedited for nothing).
  useEffect(() => {
    if (!currentPath) return;
    const path = currentPath;
    // Only while this photo is still the open one: a slower load for a photo just left (a folder
    // opening on one photo, then settling on another) finished last and set its own name here, so the
    // open photo looked unloaded and never rendered, sliders and all (Windows, 24 Oct).
    let live = true;
    void ensureEditLoaded(path)
      .catch(() => undefined)
      .then(() => {
        if (live) setEditLoadedPath(path);
      });
    return () => {
      live = false;
    };
  }, [currentPath, ensureEditLoaded]);
  loadEditsRef.current = async (paths: string[]) => {
    await Promise.all(paths.map((p) => ensureEditLoaded(p, true)));
  };
  // Photos with an edit (the "edited" icon): from the edit itself once it's loaded (opened, or read
  // by a batch), else from what the folder read found in its .rpd / .xmp.
  const editedPaths = useMemo(() => {
    const out = new Set<string>();
    for (const photo of photos) {
      const has = editLoads.current.has(photo.path)
        ? Object.values(snapshotOf(photo.path)).some(
            (v) => v !== undefined && !(Array.isArray(v) && v.length === 0) && !(typeof v === "object" && !Array.isArray(v) && Object.keys(v).length === 0),
          )
        : Boolean(photo.edited);
      if (has) out.add(photo.path);
    }
    return out;
    // The edits' own maps: snapshotOf reads them through a ref.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [photos, edits, curvesByPath, gradeByPath, mixerByPath, profileByPath, cropByPath, removalsByPath, gradientsByPath]);

  // Saves each photo's edit to its .rpd shortly after it changes (and once
  // straight after a Lightroom edit is read). Only photos with an edit, or
  // with an .rpd already, are written. At most 400 ms after a change even while
  // changes keep coming (a style going through a folder changes edits four times
  // a second, and a wait restarted by each held every .rpd back until the end).
  const sidecarSave = useRef<() => void>(() => {});
  const sidecarTimer = useRef<number | null>(null);
  useEffect(() => {
    sidecarSave.current = () => {
      const paths = new Set([
        ...Object.keys(edits),
        ...Object.keys(curvesByPath),
        ...Object.keys(gradeByPath),
        ...Object.keys(mixerByPath),
        ...Object.keys(profileByPath),
        ...Object.keys(cropByPath),
        ...Object.keys(removalsByPath),
        ...Object.keys(gradientsByPath),
        ...sidecarWritten.current.keys(),
      ]);
      for (const path of paths) {
        const edit = editFromSnapshot(snapshotOf(path));
        const photo = photosByPath[path];
        // Only a style's or a preset's look is written (an XMP's is the origin).
        const found = looksRef.current[path];
        const look = found && found.from !== "xmp" ? found : undefined;
        const key = JSON.stringify([edit, photo?.rating, photo?.pick, look ?? null]);
        if (sidecarWritten.current.get(path) === key) continue;
        sidecarWritten.current.set(path, key);
        const text = sidecarText({
          origin: sidecarOrigin.current.get(path) ?? "rapidone",
          edit,
          ...(look ? { look } : {}),
          ...(photo ? { rating: photo.rating, pick: photo.pick } : {}),
        });
        writeSidecar(path, text).catch((e) => setError(`Couldn't save the edit: ${e}`));
      }
    };
    if (sidecarTimer.current === null) {
      sidecarTimer.current = window.setTimeout(() => {
        sidecarTimer.current = null;
        sidecarSave.current();
      }, 400);
    }
  }, [edits, curvesByPath, gradeByPath, mixerByPath, profileByPath, cropByPath, removalsByPath, gradientsByPath, photosByPath, snapshotOf, sidecarTick, looks]);

  const toggleNames = useCallback(() => {
    setShowNames((prev) => {
      saveShowNames(!prev);
      return !prev;
    });
  }, []);

  const updateFlags = useCallback(
    (path: string, rating: number, pick: boolean) => {
      const previous = photosByPath[path];
      setPhotosByPath((prev) => (prev[path] ? { ...prev, [path]: { ...prev[path], rating, pick } } : prev));
      setRatingAndPick(path, rating, pick).catch((e) => {
        if (previous) setPhotosByPath((prev) => ({ ...prev, [path]: previous }));
        setError(`Couldn't save rating/pick: ${e}`);
      });
    },
    [photosByPath],
  );

  // Right-click menus: Delete (move to trash folder) on the filmstrip, Reset
  // adjustments on the loupe. Each asks for a second click before it acts.
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    items: ContextMenuItem[];
    ask?: boolean;
    header?: (askThen: (q: string, yes: () => void, no?: () => void) => void, close: () => void) => React.ReactNode;
  } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  // Photos moved out of the folder shown (to the trash folder, or to Unrated)
  // leave the app: the next photo opens if the open one went.
  const dropPhotos = useCallback(
    (moved: string[]) => {
      const gone = new Set(moved);
      if (gone.size === 0) return;
      if (currentPath && gone.has(currentPath)) {
        const i = photos.findIndex((p) => p.path === currentPath);
        const next = photos.slice(i + 1).find((p) => !gone.has(p.path)) ?? photos.slice(0, Math.max(0, i)).reverse().find((p) => !gone.has(p.path));
        setCurrentPath(next?.path ?? null);
      }
      for (const p of gone) {
        sidecarWritten.current.delete(p);
        sidecarOrigin.current.delete(p);
        editLoads.current.delete(p);
        delete histories.current[p];
      }
      const drop = <T,>(m: Record<string, T>): Record<string, T> => {
        if (!moved.some((p) => p in m)) return m;
        const next = { ...m };
        for (const p of gone) delete next[p];
        return next;
      };
      setPhotosByPath(drop);
      setEdits(drop);
      setCurvesByPath(drop);
      setGradeByPath(drop);
      setMixerByPath(drop);
      setProfileByPath(drop);
      setCropByPath(drop);
      setRemovalsByPath(drop);
      setGradientsByPath(drop);
      setSelection((prev) => new Set([...prev].filter((p) => !gone.has(p))));
    },
    [currentPath, photos],
  );
  // Undo after a delete: the photos back where they were, with their .xmp and .rpd, and in the film strip.
  const undoTrash = useCallback(async (items: Trashed[]) => {
    setUndoDelete(null);
    const back = await restorePhotos(items).catch((e) => {
      setError(`Couldn't put them back: ${e}`);
      return [] as string[];
    });
    const infos = (await Promise.all(back.map((p) => addPhoto(p).catch(() => null)))).filter((x): x is PhotoInfo => x !== null);
    if (infos.length)
      setPhotosByPath((prev) => {
        const next = { ...prev };
        for (const info of infos) next[info.path] = info;
        return next;
      });
    if (infos[0]) setCurrentPath(infos[0].path);
    setNotice(back.length ? `${back.length === 1 ? baseName(back[0]) : `${back.length} photos`} put back.` : "Nothing to put back (moved or replaced since).");
  }, []);
  // For the keys (Ctrl+Z puts a delete back while its bar is up).
  const undoDeleteRef = useRef(undoDelete);
  undoDeleteRef.current = undoDelete;
  const undoTrashRef = useRef(undoTrash);
  undoTrashRef.current = undoTrash;
  // Photos to `sidecars/trash` (with their .xmp and .rpd; nothing is deleted).
  const trashPaths = useCallback(
    async (paths: string[]) => {
      let items: Trashed[];
      try {
        items = await trashPhotos(paths);
      } catch (e) {
        setError(`Couldn't move to the trash folder: ${e}`);
        return;
      }
      const moved = items.map((t) => t.path);
      if (moved.length === 0) return;
      dropPhotos(moved);
      // The message, with Undo, in its own bar.
      setUndoDelete({ text: `${moved.length === 1 ? baseName(moved[0]) : `${moved.length} photos`} moved to the trash folder (sidecars/trash).`, items });
    },
    [dropPhotos],
  );
  // The Delete key: the selected photos (or the open one) to the trash folder,
  // asked first as on the right-click (in the middle of the window).
  const deleteKey = useRef({ paths: [] as string[], blocked: true });
  deleteKey.current = {
    paths: selection.size > 0 ? photos.filter((p) => selection.has(p.path)).map((p) => p.path) : currentPath ? [currentPath] : [],
    blocked: mode !== "edit" || atHome || cropping || menu !== null,
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Delete" || e.ctrlKey || e.metaKey || e.altKey || exportOpenRef.current) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      const { paths, blocked } = deleteKey.current;
      if (blocked || paths.length === 0) return;
      e.preventDefault();
      setMenu({
        x: window.innerWidth / 2 - 130,
        y: window.innerHeight / 2 - 60,
        ask: true,
        items: [
          {
            label: paths.length > 1 ? `Delete ${paths.length} photos (move to trash folder)` : "Delete (move to trash folder)",
            icon: <Trash2 size={14} />,
            question: paths.length > 1 ? `Move these ${paths.length} photos to the trash folder?` : "Are you sure? It moves to the trash folder.",
            onConfirm: () => void trashPaths(paths),
          },
        ],
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [trashPaths]);
  // A photo's menu (the loupe's photo, or one in the filmstrip or library): Reset
  // adjustments (greyed out when there's nothing to reset), then Delete. In the
  // filmstrip and library, a photo in a selection of several acts on the selection.
  const openPhotoMenu = useCallback(
    async (path: string, x: number, y: number, withSelection: boolean) => {
      const paths = withSelection && selection.size > 1 && selection.has(path) ? photos.filter((p) => selection.has(p.path)).map((p) => p.path) : [path];
      const many = paths.length > 1;
      // Photos not opened this session: read their saved edits first, so Reset knows.
      await Promise.all(paths.map((p) => ensureEditLoaded(p, true)));
      const hasEdit = (p: string) =>
        Object.values(snapshotOf(p)).some((v) => v !== undefined && !(Array.isArray(v) && v.length === 0) && !(typeof v === "object" && !Array.isArray(v) && Object.keys(v).length === 0));
      const edited = paths.filter(hasEdit);
      // Stars and pick, first: for the photo, or every photo in the selection. A photo
      // that's no longer rated by it (no stars, not picked: Cull's rule) in an
      // organised shoot's Rated folder is asked about, then moved to Unrated.
      const shown = photosByPath[path];
      const setFlags = (change: { rating?: number; pick?: boolean }, askThen: (q: string, yes: () => void, no?: () => void) => void, close: () => void) => {
        const before = paths.map((p) => ({ path: p, rating: photosByPath[p]?.rating ?? 0, pick: photosByPath[p]?.pick ?? false }));
        const next = before.map((b) => ({ path: b.path, rating: change.rating ?? b.rating, pick: change.pick ?? b.pick }));
        const inRated = (p: string) => /[\\/]rated$/i.test(p.replace(/[\\/][^\\/]*$/, ""));
        const leaving = next.filter((f) => f.rating < 1 && !f.pick && inRated(f.path));
        if (leaving.length === 0) {
          for (const f of next) updateFlags(f.path, f.rating, f.pick);
          close();
          return;
        }
        // The change is made, then asked about; No (or closing the question) undoes it.
        for (const f of next) updateFlags(f.path, f.rating, f.pick);
        askThen(
          leaving.length === 1
            ? `${next.length === 1 ? "This photo" : baseName(leaving[0].path)} is now unrated and unpicked. This will move it to the Unrated folder. Are you sure?`
            : `${leaving.length} photos are now unrated and unpicked. This will move them to the Unrated folder. Are you sure?`,
          () =>
            void (async () => {
              try {
                await Promise.all(next.map((f) => setRatingAndPick(f.path, f.rating, f.pick)));
                const moved = await moveToUnrated(leaving.map((f) => f.path));
                if (!moved.length) return;
                // Rated view: they leave it. All view: the folder is read again (their paths changed).
                if (viewOf(folder, folderViewsState) === "rated") dropPhotos(moved);
                else if (folder) await loadFolder(folder, true);
                setNotice(`${moved.length === 1 ? baseName(moved[0]) : `${moved.length} photos`} moved to Unrated.`);
              } catch (e) {
                setError(`Couldn't move to Unrated: ${e}`);
              }
            })(),
          () => {
            for (const b of before) updateFlags(b.path, b.rating, b.pick);
          },
        );
      };
      const header = (askThen: (q: string, yes: () => void, no?: () => void) => void, close: () => void) => (
        <div className="ctxmenu__flags" title={paths.length > 1 ? `Stars and pick for the ${paths.length} photos` : "Stars and pick"}>
          {[0, 1, 2, 3, 4, 5].map((n) => (
            <button
              key={n}
              type="button"
              className={`ctxmenu__star${n > 0 && (shown?.rating ?? 0) >= n ? " ctxmenu__star--on" : ""}${n === 0 ? " ctxmenu__star--none" : ""}`}
              title={n === 0 ? "No stars" : `${n} star${n === 1 ? "" : "s"}`}
              onClick={() => setFlags({ rating: n }, askThen, close)}
            >
              {n === 0 ? "0" : <Star size={14} fill={(shown?.rating ?? 0) >= n ? "currentColor" : "none"} />}
            </button>
          ))}
          <button
            type="button"
            className={`ctxmenu__pick${shown?.pick ? " ctxmenu__pick--on" : ""}`}
            title={shown?.pick ? "Picked: click to untick" : "Pick"}
            onClick={() => setFlags({ pick: !shown?.pick }, askThen, close)}
          >
            <Check size={14} />
          </button>
        </div>
      );
      setMenu({
        x,
        y,
        header,
        items: [
          {
            label: many ? `Reset adjustments on ${paths.length} photos${edited.length ? "" : " (none yet)"}` : `Reset adjustments${edited.length ? "" : " (none yet)"}`,
            icon: <RotateCcw size={14} />,
            disabled: edited.length === 0,
            question: many ? `Reset every adjustment on these ${paths.length} photos?` : "Are you sure? Every adjustment is reset.",
            onConfirm: () => {
              // One undo step per photo (Ctrl+Z on a photo brings its edit back).
              for (const p of edited) {
                record(p, null);
                restore(p, {});
              }
              markReady(edited, false);
              // Their thumbnails back to the camera's picture, as an unedited photo has after a reopen.
              for (const p of edited) liveThumbs.current.delete(p);
              void cameraThumbs(edited)
                .then((list) => setPhotosByPath((prev) => {
                  const next = { ...prev };
                  for (const [p, thumb] of list) if (next[p]) next[p] = { ...next[p], thumbBase64: thumb };
                  return next;
                }))
                .catch(() => undefined);
              setNotice(
                many
                  ? keyLabel(`Adjustments reset on ${edited.length} photo${edited.length === 1 ? "" : "s"} (Ctrl+Z on a photo brings its edit back).`)
                  : keyLabel(`Adjustments reset${path === currentPath ? "" : ` on ${baseName(path)}`} (Ctrl+Z brings them back).`),
              );
            },
          },
          {
            label: many ? `Delete ${paths.length} photos (move to trash folder)` : "Delete (move to trash folder)",
            icon: <Trash2 size={14} />,
            question: many ? `Move these ${paths.length} photos to the trash folder?` : "Are you sure? It moves to the trash folder.",
            onConfirm: () => void trashPaths(paths),
          },
        ],
      });
    },
    [ensureEditLoaded, snapshotOf, selection, photos, record, restore, currentPath, trashPaths, photosByPath, updateFlags, folder, folderViewsState, dropPhotos, loadFolder],
  );
  const openLoupeMenu = useCallback(
    (x: number, y: number) => {
      if (currentPath) void openPhotoMenu(currentPath, x, y, false);
    },
    [currentPath, openPhotoMenu],
  );
  // A right-click on a bottom-panel group: Reset its settings (asked first), for the open photo.
  // Its sliders back to their neutral (White balance to as shot), and Curves, Mixer and Grade to none.
  const resetPanel = useCallback(
    (name: string) => {
      if (!currentPath) return;
      const path = currentPath;
      const ids = (group: string) => SLIDER_GROUPS.find((g) => g.name === group)?.sliders.map((d) => d.id) ?? [];
      const sliders =
        name === "Curves"
          ? ids("Parametric")
          : name === "Effects"
            ? [...ids("Effects"), ...ids("Legacy vignette")]
            : name === "Keystone"
              ? [...ids("Keystone"), "autoTransform"]
              : ids(name);
      record(path, null);
      if (sliders.length)
        setEdits((prev) => {
          const e = { ...(prev[path] ?? {}) } as Record<string, number>;
          for (const id of sliders) delete e[id];
          return withEntry(prev, path, Object.keys(e).length ? (e as Adjustments) : undefined);
        });
      if (name === "Curves") setCurvesByPath((prev) => withEntry(prev, path, undefined));
      if (name === "Mixer") setMixerByPath((prev) => withEntry(prev, path, undefined));
      if (name === "Grade") setGradeByPath((prev) => withEntry(prev, path, undefined));
    },
    [currentPath, record],
  );
  const openPanelMenu = useCallback(
    (name: string, x: number, y: number) => {
      // Not with a mask tool open: the panel's sliders are the picked mask's then.
      if (!currentPath || maskModeRef.current) return;
      setMenu({
        x,
        y,
        items: [
          {
            label: `Reset ${name} settings`,
            icon: <RotateCcw size={14} />,
            question: `Reset ${name} settings on this photo?`,
            onConfirm: () => resetPanel(name),
          },
        ],
      });
    },
    [currentPath, resetPanel],
  );
  // The filmstrip and the library: the photo, or the selection it's part of.
  const openLibraryMenu = useCallback((path: string, x: number, y: number) => void openPhotoMenu(path, x, y, true), [openPhotoMenu]);


  // Held keys that adjust something other than a plain slider: the Mixer's
  // picked band (mixerHue / Sat / Lum) and Grade's picked range (gradeHue,
  // gradeSat, gradeLum) plus Blending and Pivot. `steps` up (+) or down (-).
  const adjustHeld = useCallback(
    (target: string, steps: number, coarse: boolean, reset = false) => {
      if (!currentPath) return;
      const path = currentPath;
      const step = coarse ? 5 : 1;
      const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
      const band = MIXER_HOLD[target];
      if (band) {
        record(path, "mixer");
        setMixerByPath((prev) => {
          const m = prev[path] ?? DEFAULT_MIXER;
          const b = m[picks.mixerBand];
          const v = reset ? 0 : clamp(b[band] + steps * step, -100, 100);
          return { ...prev, [path]: { ...m, [picks.mixerBand]: { ...b, [band]: v } } };
        });
        return;
      }
      if (target.startsWith("grade")) {
        record(path, "grade");
        setGradeByPath((prev) => {
          const g = prev[path] ?? DEFAULT_GRADE;
          const r = g[picks.gradeRange];
          if (target === "gradeBlending") return { ...prev, [path]: { ...g, blending: reset ? DEFAULT_GRADE.blending : clamp(g.blending + steps * step, 0, 100) } };
          if (target === "gradeBalance") return { ...prev, [path]: { ...g, balance: reset ? DEFAULT_GRADE.balance : clamp(g.balance + steps * step, -100, 100) } };
          if (target === "gradeLum") return { ...prev, [path]: { ...g, [picks.gradeRange]: { ...r, lum: reset ? 0 : clamp(r.lum + steps * step, -100, 100) } } };
          // The wheel: hue round (2 degrees a step, 10 with Ctrl), saturation out (1 / 5 a step).
          let hue = Math.atan2(-r.y, r.x);
          let sat = Math.hypot(r.x, r.y);
          if (reset) sat = 0;
          else if (target === "gradeHue") hue += (steps * (coarse ? 10 : 2) * Math.PI) / 180;
          else sat = clamp(sat + (steps * step) / 100, 0, 1);
          const x = Math.round(sat * Math.cos(hue) * 1000) / 1000;
          const y = Math.round(-sat * Math.sin(hue) * 1000) / 1000;
          return { ...prev, [path]: { ...g, [picks.gradeRange]: { ...r, x, y } } };
        });
      }
    },
    [currentPath, record, picks],
  );

  // Latest state for the keyboard handlers, which are registered once.
  // Export's dialog is open, in front: the shortcuts are off (set once `exporter` is made, below).
  const exportOpenRef = useRef(false);
  const live = useRef({ photos, currentIndex, currentPhoto, values: panelValues, neutrals: panelNeutrals, held, setSlider, updateFlags, stepHistory, home, selectedPanel, picks, adjustHeld, cropping, mode, copySettings, pasteSettings, removeByWords, defOf, maskMode, toggleRemoving });
  live.current = { photos, currentIndex, currentPhoto, values: panelValues, neutrals: panelNeutrals, held, setSlider, updateFlags, stepHistory, home, selectedPanel, picks, adjustHeld, cropping, mode, copySettings, pasteSettings, removeByWords, defOf, maskMode, toggleRemoving };

  useEffect(() => {
    function goTo(delta: number) {
      const { photos, currentIndex } = live.current;
      if (photos.length === 0) return;
      const next = Math.min(photos.length - 1, Math.max(0, currentIndex + delta));
      setCurrentPath(photos[next].path);
    }

    // A letter pressed and released without adjusting anything is a tap (in
    // Grade: H held with the arrows = Hue, H tapped = Highlights).
    const tap = { code: "", used: false };
    // A pick letter pressed and not yet released (picks happen on release).
    let pendingPick = "";

    // A tap: pick something in the selected panel with this letter (its
    // band, range or channel). Panels themselves change only with Shift.
    function tapped(code: string) {
      const selected = live.current.selectedPanel;
      const pick = selected ? tapPick(selected, code) : undefined;
      if (pick) setPicks((prev) => ({ ...prev, ...pick }));
    }

    function onKeyDown(e: KeyboardEvent) {
      if (live.current.mode !== "edit" || exportOpenRef.current) return;
      const { held, values, neutrals, currentPhoto, setSlider, updateFlags, stepHistory } = live.current;

      // Undo: Ctrl+Z. Redo: Ctrl+Shift+Z (or Ctrl+Y). Not while typing in a box (a slider, tick box or
      // button keeps the focus after a click but isn't one: Straighten's slider swallowed Ctrl+Z).
      if ((e.ctrlKey || e.metaKey) && (e.code === "KeyZ" || e.code === "KeyY")) {
        const t = e.target as HTMLElement | null;
        const typing =
          t &&
          ((t.tagName === "INPUT" && !["range", "checkbox", "radio", "button", "submit", "color"].includes((t as HTMLInputElement).type)) ||
            t.tagName === "TEXTAREA" ||
            t.isContentEditable);
        if (typing) return;
        e.preventDefault();
        // Just after a delete (its Undo bar up): Ctrl+Z puts the photos back.
        const deleted = undoDeleteRef.current;
        if (deleted && e.code === "KeyZ" && !e.shiftKey) {
          void undoTrashRef.current(deleted.items);
          return;
        }
        stepHistory(e.code === "KeyY" || e.shiftKey ? "redo" : "undo");
        return;
      }

      // Ctrl+A: select every photo shown (filmstrip or library), as in Lightroom. Not while typing.
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.code === "KeyA") {
        const t = e.target as HTMLElement | null;
        if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
        if (live.current.home) return;
        e.preventDefault();
        setSelection(new Set(live.current.photos.map((p) => p.path)));
        return;
      }

      // Ctrl+E: remove something by saying what it is (the whole photo selected).
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.code === "KeyE") {
        const t = e.target as HTMLElement | null;
        if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
        if (live.current.home || !live.current.currentPhoto) return;
        e.preventDefault();
        void live.current.removeByWords();
        return;
      }

      // Copy settings: Ctrl+Shift+C (the ticked groups). Paste to this photo: Ctrl+Shift+V.
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && (e.code === "KeyC" || e.code === "KeyV")) {
        const t = e.target as HTMLElement | null;
        if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
        if (live.current.home) return;
        e.preventDefault();
        if (e.code === "KeyC") live.current.copySettings();
        else live.current.pasteSettings("selected");
        return;
      }

      // Holding a slider key: the OS auto-repeats it, but only the arrows
      // should repeat, so swallow repeats of letters.
      if (e.repeat && e.code.startsWith("Key")) {
        e.preventDefault();
        return;
      }

      // Panels: Shift + a panel's letter selects it, from anywhere (Shift + G:
      // Grade, even in the Mixer where G is Green); again, or Esc, deselects.
      // Plain letters only work inside the selected panel: tap to pick
      // something there (a band, range, channel), hold to adjust.
      if (e.code.startsWith("Key") && !e.ctrlKey && !e.metaKey && !e.altKey && !live.current.home) {
        const selected = live.current.selectedPanel;
        if (e.shiftKey) {
          const panel = panelForKey(e.code);
          if (panel) {
            e.preventDefault();
            setSelectedPanel(panel === selected ? null : panel);
          } else if (e.code === "KeyR" && live.current.currentPhoto) {
            // Shift + R: Crop on and off from anywhere, also in a panel that uses R itself (no panel's
            // letter is R).
            e.preventDefault();
            setRemoving(false);
            setGradientTool(false);
            setCropping((c) => !c);
          }
          return;
        }
        if (selected) {
          const held0 = holdTarget(selected, e.code, live.current.picks);
          // A mask tool open: only the sliders masks have can be held.
          const target = held0 && (!live.current.maskMode || isMaskSlider(held0)) ? held0 : undefined;
          if (target) {
            // Held = adjust it; a quick tap with no adjusting = the pick with
            // this letter in the panel, if it has one (see onKeyUp).
            e.preventDefault();
            tap.code = e.code;
            tap.used = false;
            setHeld({ sliderId: target, code: e.code });
            return;
          }
          if (tapPick(selected, e.code)) {
            // Picks something in the panel (a band, range, channel) on release.
            e.preventDefault();
            pendingPick = e.code;
            return;
          }
        }
      }

      // Photos: ←/→ (below), or Shift + ↑/↓ (↑ previous, ↓ next; the filmstrip
      // runs top to bottom). Plain ↑/↓ only adjust a held slider.
      if (e.shiftKey && e.key.startsWith("Arrow") && !held) {
        e.preventDefault();
        if (live.current.home) return;
        goTo(e.key === "ArrowDown" || e.key === "ArrowRight" ? 1 : -1);
        return;
      }

      if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        // On the home screen ↑/↓ move through the prompt's choices (TerminalPrompt).
        if (live.current.home) return;
        const direction = e.key === "ArrowUp" ? 1 : -1;
        if (held) {
          const def = live.current.defOf(held.sliderId);
          // Plain: the small step (0.05 stop of exposure, 1 elsewhere). Ctrl: the big jump (1/3 stop, 5).
          tap.used = true;
          if (def) setSlider(def.id, nudge(def, values[def.id], direction, e.ctrlKey || e.metaKey));
          else live.current.adjustHeld(held.sliderId, direction, e.ctrlKey || e.metaKey);
        }
        // With no slider held, ↑/↓ do nothing (photos are Shift + arrows).
        return;
      }

      if (held && e.key === "Backspace") {
        e.preventDefault();
        tap.used = true;
        if (live.current.defOf(held.sliderId)) setSlider(held.sliderId, neutrals[held.sliderId]);
        else live.current.adjustHeld(held.sliderId, 0, false, true);
        return;
      }

      // ←/→ change photo too (not while a slider letter is held, nor on the home screen).
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        if (!held && !live.current.home) goTo(e.key === "ArrowRight" ? 1 : -1);
        return;
      }

      // F: full frame on and off (no panel uses F; Shift+F is Fine tuning).
      if (e.code === "KeyF" && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey && !live.current.home && !live.current.cropping) {
        e.preventDefault();
        setFullFrame((f) => !f);
        return;
      }

      // E: Erase on and off (box tool, nothing typed: B then switches to the brush; Ctrl+E is "remove
      // by words", which opens it ready to type). As with R, a panel that uses E takes it first, above.
      if (e.code === "KeyE" && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey && !live.current.home && live.current.currentPhoto) {
        e.preventDefault();
        void live.current.toggleRemoving();
        return;
      }

      // M: Masks on and off; Q: Patch on and off (Lightroom's key for its healing tool; P is Pick). As
      // with R, a panel that uses the letter itself takes it first, above.
      if ((e.code === "KeyM" || e.code === "KeyQ") && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey && !live.current.home && live.current.currentPhoto) {
        e.preventDefault();
        setCropping(false);
        setRemoving(false);
        if (e.code === "KeyM") setGradientTool((t) => !t);
        else {
          setGradientTool(false);
          setPatchTool((t) => !t);
        }
        return;
      }

      // R: Crop on and off, as in Lightroom (a panel that uses R itself, e.g. the
      // Mixer's Red, takes it first, above).
      if (e.code === "KeyR" && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey && !live.current.home && live.current.currentPhoto) {
        e.preventDefault();
        setRemoving(false);
        setGradientTool(false);
        setCropping((c) => !c);
        return;
      }

      if (e.key === "Enter" && live.current.cropping) {
        // Crop: Enter is Done.
        e.preventDefault();
        setCropping(false);
        return;
      }

      // (Remove's found objects take Tab first: it steps through them.)
      if (e.key === "Tab" && !e.defaultPrevented) {
        e.preventDefault();
        setPanelVisible((v) => !v);
        return;
      }

      if (e.key === "Escape") {
        setHeld(null);
        setHome(false);
        setSelectedPanel(null);
        setCropping(false);
        setFullFrame(false);
        return;
      }

      // Culling keys, same behaviour as RapidCulling.
      if (!currentPhoto || held) return;
      const { path, rating, pick } = currentPhoto;
      // Number-pad digits belong to Curves (1/5/9 + arrows), so ratings are top-row only.
      if (e.key >= "0" && e.key <= "5" && !e.shiftKey && !e.altKey && !e.code.startsWith("Numpad")) {
        updateFlags(path, Number(e.key), pick);
      } else if (e.code === "KeyP") {
        const nextPick = !pick;
        updateFlags(path, nextPick && rating === -1 ? 0 : rating, nextPick);
      } else if (e.code === "KeyX") {
        if (rating === -1) updateFlags(path, 0, pick);
        else updateFlags(path, -1, false);
      }
    }

    function onKeyUp(e: KeyboardEvent) {
      if (live.current.mode !== "edit" || exportOpenRef.current) return;
      // A panel letter takes effect when it's let go.
      if (pendingPick === e.code) {
        pendingPick = "";
        tapped(e.code);
      }
      if (live.current.held?.code === e.code) {
        setHeld(null);
        // A quick tap that adjusted nothing: switch to the panel with this letter.
        if (tap.code === e.code && !tap.used) tapped(e.code);
        tap.code = "";
      }
      // Stop Alt from focusing the (non-existent) window menu on Windows.
      if (e.key === "Alt") e.preventDefault();
    }

    // If the window loses focus mid-hold, the keyup never arrives - release
    // so the slider can't get stuck "held".
    function onBlur() {
      setHeld(null);
    }

    // Scroll wheel / touchpad while holding a slider's key: up raises it, down
    // lowers it (Ctrl: the big step, as with the arrows). A mouse wheel moves one
    // step per notch; a touchpad's smooth scrolling is added up into steps.
    const TOUCHPAD_PX_PER_STEP = 24;
    const wheel = { sliderId: "", value: 0, at: 0, travel: 0 };
    function onWheel(e: WheelEvent) {
      if (live.current.mode !== "edit" || exportOpenRef.current) return;
      const { held, values, setSlider } = live.current;
      if (!held) return;
      e.preventDefault();
      const def = live.current.defOf(held.sliderId) ?? null;
      const now = performance.now();
      // Carry on from our own last value while scrolling: re-renders lag behind a fast touchpad.
      if (wheel.sliderId !== held.sliderId || now - wheel.at > 300) {
        wheel.sliderId = held.sliderId;
        wheel.value = def ? values[def.id] : 0;
        wheel.travel = 0;
      }
      wheel.at = now;
      let steps: number;
      const notch = e.deltaMode !== 0 || (Math.abs(e.deltaY) >= 50 && Number.isInteger(e.deltaY));
      if (notch) {
        steps = -Math.sign(e.deltaY);
      } else {
        wheel.travel -= e.deltaY;
        steps = Math.trunc(wheel.travel / TOUCHPAD_PX_PER_STEP);
        wheel.travel -= steps * TOUCHPAD_PX_PER_STEP;
      }
      if (steps === 0) return;
      const direction = steps > 0 ? 1 : -1;
      tap.used = true;
      if (!def) {
        live.current.adjustHeld(held.sliderId, steps, e.ctrlKey || e.metaKey);
        return;
      }
      for (let i = 0; i < Math.abs(steps); i++) wheel.value = nudge(def, wheel.value, direction, e.ctrlKey || e.metaKey);
      setSlider(def.id, wheel.value);
    }

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", onBlur);
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("wheel", onWheel);
    };
  }, []);

  // A click on a photo in the filmstrip or library: Ctrl toggles it in the
  // selection (the open photo counts as selected until then), Shift takes the
  // range from the last one clicked; a plain click opens it and clears the selection.
  const pickPhoto = useCallback(
    (i: number, keys?: { ctrl: boolean; shift: boolean }) => {
      const path = photos[i]?.path;
      if (!path) return;
      // Shift: the range from the last photo clicked, or, with none clicked yet (the photo was resumed
      // or stepped to with the keys), from the photo on screen.
      const anchor = selectionAnchor.current ?? currentPath;
      if (keys?.shift && anchor) {
        const from = photos.findIndex((p) => p.path === anchor);
        const [lo, hi] = [Math.min(from < 0 ? i : from, i), Math.max(from < 0 ? i : from, i)];
        setSelection(new Set(photos.slice(lo, hi + 1).map((p) => p.path)));
      } else if (keys?.ctrl) {
        setSelection((prev) => {
          const next = new Set(prev.size > 0 ? prev : currentPath ? [currentPath] : []);
          if (!next.delete(path)) next.add(path);
          return next;
        });
        selectionAnchor.current = path;
      } else {
        setSelection(new Set());
        selectionAnchor.current = path;
        setCurrentPath(path);
      }
    },
    [photos, currentPath],
  );

  // Export. The edit of a photo not opened this session is read first, then
  // the engine settings are built from it exactly as the viewer's are, except
  // that Temperature and Tint are sent only when the photo's edit sets them
  // (otherwise the engine uses the photo's own as-shot values).
  const photoOf = useCallback((path: string) => photosByPath[path], [photosByPath]);
  const exportLoad = useCallback((path: string) => ensureEditLoaded(path, true), [ensureEditLoaded]);
  const settingsForExport = useCallback(
    (path: string): RenderSettings => {
      const s = snapshotOf(path);
      const values = { ...DEFAULTS, ...s.edits } as Adjustments;
      const settings = engineSettingsFrom(
        values,
        false,
        s.profile ?? DEFAULT_PROFILE,
        s.mixer ?? DEFAULT_MIXER,
        s.curves ?? DEFAULT_CURVES,
        s.grade ?? DEFAULT_GRADE,
        s.crop ?? NO_CROP,
        s.removals ?? NO_REMOVALS,
        s.gradients ?? NO_GRADIENTS,
      );
      if (s.edits?.temperature != null) settings.temperature = s.edits.temperature;
      if (s.edits?.tint != null) settings.tint = s.edits.tint;
      return settings;
    },
    [snapshotOf],
  );
  // The same file for a photo that isn't the current one, without making it the
  // current one (Retouch hands back photos it was only pasted onto, on leaving the
  // tab): its saved Edit settings are read, and it is graded with them.
  const prepareHandoff = useCallback(
    async (source: string) => {
      await ensureEditLoaded(source, true);
      const settings = settingsForExport(source);
      return renderHandoff(source, settings, JSON.stringify(settings));
    },
    [ensureEditLoaded, settingsForExport],
  );
  const exportFinished = useCallback((message: string) => setNotice(message), []);
  const exporter = useExport({
    photoOf,
    ensureEdit: exportLoad,
    settingsFor: settingsForExport,
    onFinished: exportFinished,
    startFolder: folder,
  });
  exportOpenRef.current = exporter.dialog !== null;
  const startExport = useCallback(() => {
    exporter.openFor({
      // The selected photos (Ctrl / Shift-click), else the open one; the dialog switches between this and all.
      selected: selection.size > 0 ? photos.filter((p) => selection.has(p.path)).map((p) => p.path) : currentPath ? [currentPath] : [],
      all: photos.map((p) => p.path),
    });
  }, [exporter.openFor, photos, selection, currentPath]);

  // Retouch hands its result back as a TIFF next to the RAW: `<name>_retouched.tif`,
  // then `_retouched-2`, `-3` ... so an earlier pass is never overwritten.
  const handedBack = useRef(new Set<string>());
  const handBackPath = useCallback(
    (source: string) => {
      const slash = Math.max(source.lastIndexOf("/"), source.lastIndexOf("\\"));
      const dir = source.slice(0, slash + 1);
      const base = source.slice(slash + 1).replace(/\.[^.]+$/, "").replace(/_retouched(-\d+)?$/i, "");
      const taken = new Set([...Object.keys(photosByPath), ...handedBack.current].map((p) => p.replace(/\\/g, "/").toLowerCase()));
      for (let n = 1; ; n++) {
        const name = `${base}_retouched${n > 1 ? `-${n}` : ""}.tif`;
        if (!taken.has((dir + name).replace(/\\/g, "/").toLowerCase())) return dir + name;
      }
    },
    [photosByPath],
  );
  // The TIFF is written: it joins the library (stacked on top of its photo, with
  // the stars and pick the photo has), and Edit carries on with it if that photo
  // is still the one open - the TIFF starts neutral, the original keeps its settings.
  const currentPathRef = useRef(currentPath);
  currentPathRef.current = currentPath;
  const photoMapRef = useRef(photosByPath);
  photoMapRef.current = photosByPath;
  const onHandBack = useCallback(async (source: string, tif: string) => {
    handedBack.current.add(tif);
    const name = tif.split(/[\\/]/).pop();
    setHandBackStage(2);
    try {
      const added = await addPhoto(tif);
      const from = photoMapRef.current[source];
      const rating = from?.rating ?? added.rating;
      const pick = from?.pick ?? added.pick;
      if (from && (rating !== added.rating || pick !== added.pick)) await setRatingAndPick(tif, rating, pick).catch(() => undefined);
      setPhotosByPath((prev) => ({ ...prev, [added.path]: { ...added, rating, pick } }));
      if (currentPathRef.current === source) {
        setCurrentPath(added.path);
        setHandBackOpening(added.path);
        setHandBackStage(3);
      } else {
        // More photos still to come in this batch: the bar carries on to the next one.
        const more = batchRef.current !== null && batchRef.current.index < batchRef.current.total;
        setHandBackStage(more ? 1 : null);
      }
      const batch = batchRef.current;
      if (batch && batch.total > 1) {
        setNotice(batch.index >= batch.total ? `${batch.total} retouched photos saved and stacked on their originals.` : null);
      } else {
        setNotice(`Retouched photo saved as ${name}: edit it here, or open the original from its stack.`);
      }
      if (!batch || batch.index >= batch.total) {
        batchRef.current = null;
        setHandBackBatch(null);
      }
    } catch (e) {
      setHandBackStage(null);
      setNotice(`Retouched photo saved as ${name}, but it couldn't be added to the library: ${e}`);
    }
  }, []);
  // Leaving Retouch hands back every photo with edits, one after another: which of how many.
  // (index 0: they are being prepared, none started yet.)
  const batchRef = useRef<{ index: number; total: number } | null>(null);
  const [handBackBatch, setHandBackBatch] = useState<{ index: number; total: number } | null>(null);
  const onHandBackBatch = useCallback((total: number) => {
    setNotice(null);
    batchRef.current = { index: 0, total };
    setHandBackBatch({ index: 0, total });
    setHandBackStage(1);
  }, []);
  const onHandBackStart = useCallback((_source: string, batch?: { index: number; total: number }) => {
    setNotice(null);
    batchRef.current = batch ?? null;
    setHandBackBatch(batch ?? null);
    setHandBackStage(1);
  }, []);
  // Which step of getting the retouched TIFF back is running (1 saving, 2 adding it
  // to the library, 3 opening it), for the progress bar: a big Fuji TIFF takes a few
  // seconds at each. It ends when the engine's picture of the TIFF is on screen.
  const [handBackStage, setHandBackStage] = useState<1 | 2 | 3 | null>(null);
  const [handBackOpening, setHandBackOpening] = useState<string | null>(null);
  useEffect(() => {
    if (handBackStage !== 3) return;
    // Done when the engine's picture of the TIFF is up; given up if the user moved to another photo.
    if ((engineReady && currentPath === handBackOpening) || currentPath !== handBackOpening) {
      setHandBackStage(null);
      setHandBackOpening(null);
    }
  }, [handBackStage, handBackOpening, engineReady, currentPath]);
  // Retouch's "hand back everything with edits now" (its work is only in memory until then).
  const retouchFlush = useRef<(() => Promise<void>) | null>(null);
  const handBack = useMemo(
    () => ({
      path: handBackPath,
      onHandBack,
      onStart: onHandBackStart,
      onBatch: onHandBackBatch,
      register: (flush: (() => Promise<void>) | null) => {
        retouchFlush.current = flush;
      },
    }),
    [handBackPath, onHandBack, onHandBackStart, onHandBackBatch],
  );

  // Closing the app: Retouch's photos with edits are handed back first (as leaving its
  // tab does), the last edits written to their .rpd, then the hand-off TIFFs of every
  // folder opened this session cleared (a cache, ~100 MB each), then the window goes.
  // Closing again while that runs closes straight away.
  const foldersSeen = useRef(new Set<string>());
  useEffect(() => {
    if (folder) foldersSeen.current.add(folder);
  }, [folder]);
  useEffect(() => {
    const win = getCurrentWindow();
    let closing = false;
    const unlisten = win.onCloseRequested(async (event) => {
      event.preventDefault();
      if (closing) {
        void win.destroy();
        return;
      }
      closing = true;
      try {
        if (retouchFlush.current) {
          setNotice("Saving Retouch's edits before closing…");
          await retouchFlush.current();
        }
      } catch {
        // Closing anyway: whatever was handed back is saved.
      }
      // The .rpd writer saves 0.4 s after a change.
      await new Promise((resolve) => setTimeout(resolve, 600));
      await clearHandoffs([...foldersSeen.current]).catch(() => 0);
      void win.destroy();
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, []);

  // All | Rated | Unrated (once the shoot is organised), for both filmstrips.
  const viewSwitch = folderViewsState && (
    <ViewSwitch
      view={viewOf(folder, folderViewsState)}
      views={folderViewsState}
      onView={(v) => void loadFolder(folderFor(v, folderViewsState), true)}
    />
  );

  // The filmstrip (right-hand bar).
  const filmstrip = (
    <Filmstrip
      photos={photos}
      total={allPhotos.length}
      counts={stripCounts}
      filter={stripFilter}
      onFilter={setStripFilter}
      currentIndex={currentIndex}
      onSelect={pickPhoto}
      selection={selection}
      stacks={stackBadges}
      onToggleStack={toggleStack}
      ready={ready}
      transforms={autoTransforms}
      denoised={denoisedPaths}
      looks={shownLooks}
      edited={editedPaths}
      viewSwitch={viewSwitch}
      onMenu={openLibraryMenu}
    />
  );

  return (
    <div className="app-shell">
      <TitleBar
        progress={progress}
        onCancelRead={cancelFolderRead}
        mode={atHome ? undefined : mode}
        statusSlot={mode === "retouch" && !atHome ? setRetouchStatus : undefined}
        // A retouched photo coming back into Edit: in the title bar's loading bar, as everything else.
        handBack={
          handBackStage === null
            ? null
            : handBackBatch && handBackBatch.total > 1
              ? {
                  label:
                    handBackBatch.index === 0
                      ? `Rendering ${handBackBatch.total} retouched photos`
                      : `Retouched ${handBackBatch.index} of ${handBackBatch.total}: ${handBackStage === 1 ? "saving the TIFF" : handBackStage === 2 ? "adding it to the library" : "opening it"}`,
                  fraction: (Math.max(handBackBatch.index, 1) - 1 + handBackStage / 3) / handBackBatch.total,
                }
              : {
                  label: handBackStage === 1 ? "Saving the retouched TIFF" : handBackStage === 2 ? "Adding it to the library" : "Opening the retouched photo",
                  fraction: handBackStage / 3,
                }
        }
      />
      {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} ask={menu.ask} header={menu.header} onClose={closeMenu} />}
      {!atHome && (
      <TopBar
        folder={folder}
        profile={profile}
        profileEnabled={Boolean(currentPhoto)}
        adobeAvailable={adobeAvailable}
        userProfiles={userProfilesList}
        onAddProfiles={() => void addOwnProfiles()}
        camera={preview?.path === currentPath ? preview?.camera : undefined}
        onOpenFolder={openFolder}
        onHome={() => setHome(true)}
        mode={mode}
        onMode={setMode}
        atHome={atHome}
        retouchSlots={{ left: setRetouchLeft, right: setRetouchRight }}
        cropping={cropping}
        onCrop={() => {
          if (!currentPhoto || home) return;
          setRemoving(false);
          setCropping((c) => !c);
        }}
        removing={removing}
        onRemove={() => currentPhoto && !home && void toggleRemoving()}
        patching={patchTool}
        onPatch={() => {
          if (!currentPhoto || home) return;
          setCropping(false);
          setRemoving(false);
          setGradientTool(false);
          setPatchTool((t) => !t);
        }}
        gradientTool={gradientTool}
        onGradient={() => {
          if (!currentPhoto || home) return;
          setCropping(false);
          setRemoving(false);
          setGradientTool((t) => !t);
        }}
        homeEnabled={Boolean(currentPhoto) && !home}
        library={libraryView}
        onLibrary={(on) => {
          if (on) {
            setCropping(false);
            setRemoving(false);
            setGradientTool(false);
          }
          setLibraryView(on);
        }}
        selectedCount={selection.size}
        exportEnabled={photos.length > 0 && !home && !exporter.dialog?.running}
        onExport={startExport}
        onProfileChange={setProfile}
        lensMenu={
          <LensMenu
            path={currentPath && !home ? currentPath : null}
            auto={lensAuto}
            on={(values.lensCorrect ?? (lensAuto ? 100 : 0)) > 0}
            rev={lensRev}
            onAuto={setLensAuto}
            onPhoto={setLensPhoto}
            onChoose={(exif, id) => void chooseLens(exif, id)}
            onLoadProfiles={() => void loadLensProfiles()}
          />
        }
        autoStyleMenu={
          <>
            <AutoStyleMenu
              styles={autoStyles}
              applyDisabled={!folder}
              progress={autoStyleProgress}
              onApply={(s, scope) => void applyAutoStyle(s, scope)}
              onTrain={(name, folders) => void trainStyle(name, folders)}
              onIncrease={(s, folders) => void trainStyle(s.name, folders, s)}
            />
            <DenoiseMenu disabled={!folder || photos.length === 0} progress={denoiseProgress} onApply={(iso, amount) => void applyDenoiseAll(iso, amount)} />
            <CopyPasteMenu
              disabled={!currentPhoto || home}
              groups={copyGroups}
              onGroupsChange={changeCopyGroups}
              copied={copied ? `${baseName(copied.from)}, ${copied.groups.length} group${copied.groups.length === 1 ? "" : "s"}` : null}
              onCopy={copySettings}
              onPaste={pasteSettings}
              pasteFresh={Boolean(copied && !copied.pasted)}
              selectedCount={selection.size}
            />
          </>
        }
        presetMenu={
          <PresetMenu
            presets={presets}
            disabled={!folder}
            onApply={applyPreset}
            onSave={savePresetFromPhoto}
            onDelete={removePreset}
            onImport={() => void importLightroomPresets()}
            onExport={() => void exportAllPresets()}
          />
        }
      />
      )}
      {undoDelete && (
        <div className="notice-banner">
          <span>{undoDelete.text}</span>
          <button type="button" className="notice-banner__action" onClick={() => void undoTrash(undoDelete.items)} title={keyLabel("Put them back (Ctrl+Z)")}>
            Undo
          </button>
          <button type="button" onClick={() => setUndoDelete(null)} title="Dismiss">
            ×
          </button>
        </div>
      )}
      {notice && (
        <div className="notice-banner">
          <span>{notice}</span>
          <button type="button" onClick={() => setNotice(null)} title="Dismiss">
            ×
          </button>
        </div>
      )}
      {pasteProgress && (
        <>
          {/* Editing waits while pasted photos are brought up to date (so none is opened
              half-ready): a see-through cover over the editing area (below) takes the
              clicks; the keys are held below. */}
          <div className="handback-progress handback-progress--lock" role="status">
            <span className="handback-progress__label">
              {pasteProgress.subjects ? "Updating photos (finding subjects)" : "Updating photos"}: {pasteProgress.done} of {pasteProgress.total}
            </span>
            <div className="handback-progress__bar">
              <div className="handback-progress__fill" style={{ width: `${Math.max(3, (pasteProgress.done / Math.max(1, pasteProgress.total)) * 100)}%` }} />
            </div>
            <button
              type="button"
              className="handback-progress__stop"
              onClick={stopRefreshing}
              title="Stop after this photo: the settings stay pasted; the rest are rendered when they're opened"
            >
              Stop
            </button>
          </div>
        </>
      )}
      {error && (
        <div className="error-banner">
          <span>{error}</span>
          <button type="button" onClick={() => setError(null)} title="Dismiss">
            ×
          </button>
        </div>
      )}
      {mode === "edit" && (
      <div className="workspace">
        {pasteProgress && <div className="busy-lock" aria-hidden="true" />}
        <div className="workspace__main">
          {libraryView && !atHome && (
            <Library
              bottomInset={panelVisible && currentPhoto ? dockHeight : 0}
              photos={photos}
              ready={ready}
              transforms={autoTransforms}
              denoised={denoisedPaths}
              looks={shownLooks}
              edited={editedPaths}
              total={allPhotos.length}
              counts={stripCounts}
              filter={stripFilter}
              onFilter={setStripFilter}
              viewSwitch={viewSwitch}
              currentPath={currentPath}
              selection={selection}
              stacks={stackBadges}
              onToggleStack={toggleStack}
              onSelect={pickPhoto}
              onOpen={(i) => {
                setSelection(new Set());
                setCurrentPath(photos[i].path);
                setLibraryView(false);
              }}
              onSelectAll={() => setSelection(new Set(photos.map((p) => p.path)))}
              onSelectNone={() => setSelection(new Set())}
              onMenu={openLibraryMenu}
              size={libraryZoom}
              onSize={setLibraryZoom}
            />
          )}
          <Viewer
            photo={currentPhoto}
            grid={keystoneGrid}
            preview={preview}
            cameraPreview={cameraPreview}
            beforePreview={beforePreview}
            compareWith={compareWith}
            onCompareWith={setCompareWith}
            noCompare={libraryView}
            onCompareStart={() => compareWith === "before" && requestBefore(currentPath)}
            keysActive={mode === "edit" && !atHome && !cropping && menu === null}
            values={values}
            curves={curves}
            grade={grade}
            index={currentIndex}
            total={photos.length}
            onOpenFolder={openFolder}
            home={home}
            folderName={folder ? (folder.split(/[\\/]/).filter(Boolean).pop() ?? folder) : null}
            onResume={() => setHome(false)}
            resumeSession={resumeSession}
            progress={progress}
            startImage={startImage}
            still={held !== null || adjusting}
            // With the masks open, right-click does nothing (no photo menu over the layers).
            onMenu={gradientTool ? () => undefined : openLoupeMenu}
            fullFrame={fullFrame}
            bottomInset={panelVisible && !home && currentPhoto ? dockHeight : 0}
            surround={surround}
            cropping={cropping}
            crop={crop}
            cropAspect={cropAspect}
            onCrop={setCrop}
            onCropAspect={setCropAspect}
            onCropDone={() => setCropping(false)}
            removing={removing || gradientTool || patchTool}
            overlayZooms={gradientTool || patchTool}
            onDetailActive={onDetailActive}
            detailKey={`${currentPath}\u0001${engineKey}`}
            onDetail={(rect, w, h, interactive) => (currentPath ? renderDetail(currentPath, engineSettingsRef.current, rect, w, h, interactive) : Promise.reject())}
            removeOverlay={(img) => patchTool ? (
              <PatchOverlay
                image={img}
                patches={patchList}
                mode={patchMode}
                onMode={setPatchMode}
                onAdd={(patch) => changeSpots((list) => [...list, patch])}
                onChange={(index, patch) => changeSpots((list) => list.map((s, i) => (i === index ? patch : s)))}
                onDelete={(index) => changeSpots((list) => list.filter((_, i) => i !== index))}
                onDone={() => setPatchTool(false)}
              />
            ) : gradientTool ? (
              <GradientOverlay
                image={img}
                gradients={gradients}
                onChange={(list, key) => {
                  if (!currentPath) return;
                  record(currentPath, key);
                  setGradientsByPath((prev) => withEntry(prev, currentPath, list.length ? list : undefined));
                }}
                maskOverlay={maskOverlayFor}
                onPicked={setMaskPicked}
                showMask={maskShow}
                onShowMask={() => setMaskShow((v) => !v)}
              />
            ) : (
              <RemoveOverlay
                key={removeWhole}
                startWhole={removeWhole > 0}
                image={img}
                busy={removeBusy}
                count={removals.filter((r) => !isPatch(r)).length}
                canFind={canFind}
                modes={removeModes}
                onMode={chooseRemover}
                onFind={findInPhoto}
                onRemoveAll={(found, what) => void removeAllFound(found, what)}
                onRemovePainted={(painted, what) => void removePaintedShape(painted, what)}
                onRemove={(area, what, outline) => void removeArea(area, what, outline)}
                onClearAll={clearRemovals}
                onRetry={canRetry ? retryRemove : undefined}
                onDone={() => {
                  setRemoving(false);
                  setRemoveWhole(0);
                }}
              />
            )}
          />
        </div>
        {/* The filmstrip ends above the docked panel, which runs the workspace's full width under it. */}
        {!atHome && !libraryView && (
          <div className="filmstrip-wrap" style={{ paddingBottom: panelVisible && currentPhoto ? dockHeight : 0 }}>
            {filmstrip}
          </div>
        )}
          {panelVisible && !home && currentPhoto && (
            <div className="panel-dock" ref={dockRef}>
            <BottomPanel
              onGroupMenu={openPanelMenu}
              values={panelValues}
              neutrals={panelNeutrals}
              mask={
                maskMode
                  ? {
                      defOf: maskSliderDef,
                      picked: pickedMask !== null,
                      showMask: maskShow,
                      onShowMask: () => setMaskShow((v) => !v),
                      invert: Boolean(pickedMask?.invert),
                      onInvert: () => {
                        const t = maskTarget.current;
                        if (!currentPath || !t) return;
                        record(currentPath, null);
                        setGradientsByPath((prev) => {
                          const list = [...(prev[currentPath] ?? [])];
                          if (!list[t.index]) return prev;
                          list[t.index] = { ...list[t.index], invert: !list[t.index].invert };
                          return { ...prev, [currentPath]: list };
                        });
                      },
                    }
                  : undefined
              }
              activeSliderId={held?.sliderId ?? null}
              selectedPanel={selectedPanel}
              onSelectPanel={setSelectedPanel}
              picks={picks}
              onPicks={onPicks}
              showNames={showNames}
              onToggleNames={toggleNames}
              surround={surround}
              onToggleSurround={toggleSurround}
              curves={curves}
              histogram={histogram}
              shotLabel={shotLabel}
              grade={grade}
              onChange={setSlider}
              onCurvesChange={setCurves}
              onGradeChange={setGrade}
              mixer={mixer}
              onMixerChange={setMixer}
              autoBusy={autoBusy}
              autoActive={autoActive}
              onAutoLevel={() => void runAutoTransform("level")}
              onAutoKeystone={() => void runAutoTransform("keystone")}
            />
            </div>
          )}
      </div>
      )}
      {/* Export: RapidRetouch's dialog (its styles are scoped to .rapidretouch), RapidOne's engine. */}
      {exporter.dialog && (
        <div
          className="rapidretouch"
          style={{ height: 0, background: "none" }}
          // Typing in the dialog (a capital in a file name, say) isn't a shortcut: its keys stop here.
          onKeyDown={(e) => e.stopPropagation()}
          onKeyUp={(e) => e.stopPropagation()}
        >
          <ExportDialog {...exporter.dialog} />
        </div>
      )}
      {/* The Cull and Retouch screens (RapidCulling / RapidRetouch modules). */}
      {visited.cull && (
        <div className="module-host" style={{ display: mode === "cull" ? undefined : "none" }}>
          <RapidCulling
            active={mode === "cull"}
            folder={folder}
            onFolderChange={(f) => {
              if (f !== folder) void loadFolder(f);
            }}
          />
        </div>
      )}
      {visited.retouch && (
        <div className="module-host" style={{ display: mode === "retouch" ? undefined : "none" }}>
          <RapidRetouch
            appIcon={rapidOneIcon}
            // Active while the Retouch tab is the one chosen (and not the Home screen): a moment
            // with no current photo (the list changing) isn't leaving the tab, and must not hand the photo back.
            active={chosenMode === "retouch" && !home}
            folder={folder}
            photo={handoff?.path === currentPath ? handoff.tif : null}
            photoVersion={handoff?.key}
            gradedInput
            photoSource={handoff?.path === currentPath ? currentPath : null}
            // The same photos, view and filter as Edit's filmstrip; a click
            // makes it the current photo, which hands it over graded.
            library={{
              photos: photos.map((p) => ({ source: p.path, thumb: p.thumbBase64 })),
              header: (
                <StripFilterBar
                  filter={stripFilter}
                  onFilter={setStripFilter}
                  counts={stripCounts}
                  shown={photos.length}
                  total={allPhotos.length}
                  viewSwitch={viewSwitch}
                />
              ),
              current: currentPath,
              onOpen: (source) => setCurrentPath(source),
              preview:
                engineReady && preview && currentPath
                  ? { source: currentPath, image: preview.imageBase64, width: preview.width, height: preview.height }
                  : null,
              requestFile: requestHandoff,
              prepareFile: prepareHandoff,
            }}
            toolbarSlots={{ left: retouchLeft, right: retouchRight, status: retouchStatus }}
            handBack={handBack}
          />
          {handoffBusy && <div className="handoff-busy">Preparing the graded photo for Retouch…</div>}
        </div>
      )}
      {mode === "edit" && !panelVisible && held && SLIDERS_BY_ID[held.sliderId] && (
        <div className="held-toast">
          {SLIDERS_BY_ID[held.sliderId].label}: {formatValue(SLIDERS_BY_ID[held.sliderId], values[held.sliderId])}
        </div>
      )}
    </div>
  );
}
