import HSlider from "./HSlider";
import { memo, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { Adjustments } from "../adjustments";
import { channelFunction, CURVE_CHANNELS, DEFAULT_CURVES, isIdentity, type Curves } from "../curves";
import { exposureFunction } from "../exposure";
import { DEFAULT_GRADE, gradeChannel, isNeutralGrade, type Grade } from "../grade";
import { previewTransform } from "../transform";
import type { FullPreview, PhotoInfo, ViewerImage } from "../types";
import { RotateCcw, RotateCw } from "lucide-react";
import { BarGrip, useFloatingBar } from "../floatingBar";
import { CROP_ASPECTS, NO_CROP, aspectRatio, cropOutline, cropToRatio, fitCrop, isNoCrop, straightCrop, type Crop } from "../crop";
import CropOverlay from "./CropOverlay";
import GridScene, { PHOTO_RING, corridorFalloff, corridorPath, type Box } from "./GridScene";
import TerminalPrompt from "./TerminalPrompt";
import { keyLabel } from "../macKeys";

/** A touchpad's scroll is a stream of events; this long without one ends it. */
const TOUCHPAD_STREAM_MS = 400;

/**
 * Whether a wheel event is a mouse wheel's notch (zoom a step) rather than a
 * touchpad's two-finger scroll (pan). A notch moves straight up or down by a
 * whole 50 or more; a touchpad sends small, sideways or fractional amounts -
 * but a fast swipe can also send a big round one, so once the touchpad has
 * shown itself, the rest of its stream (`touchpadUntil`) pans too. WebKitGTK
 * (Linux) scales a notch by the display's scale, so it needn't be whole
 * (226.5 at 1.333x); there a notch is a straight jump of 100 or more.
 */
const WEBKITGTK = /Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent);
/** macOS's WebKit: a notch is small and accelerated, so the size test can't tell it from a
 *  touchpad. But every notch's stream begins with exactly one line, 4.000244 px, straight up or
 *  down (a fast spin then speeds up: 40, 160, 250...); a touchpad's starts fractional. */
const MAC_WEBKIT = /Macintosh/.test(navigator.userAgent);
const MAC_NOTCH = 4.000244140625;
/** On a Mac, a mouse stream's events zoom at most a step this often (ms), so a fast spin's
 *  accelerated burst zooms a few steps rather than straight to the limit. */
const MAC_STEP_MS = 60;
/** Two parts of the photo (fractions) the same, to a hair. */
const sameRect = (a: readonly number[], b: readonly number[]) => a.every((v, i) => Math.abs(v - b[i]) < 1e-4);
/** Windows' WebView2 (Chromium's engine): it skips painting fully transparent layers. */
const CHROMIUM = /Edg\//.test(navigator.userAgent);
function isMouseWheel(e: WheelEvent, now: number, touchpadUntil: number): boolean {
  if (e.deltaMode === 1) return true;
  if (now < touchpadUntil) return false;
  if (WEBKITGTK) return e.deltaX === 0 && Math.abs(e.deltaY) >= 100;
  return e.deltaX === 0 && Math.abs(e.deltaY) >= 50 && Number.isInteger(e.deltaY);
}

/** The zoom dropdown's steps (%). */
const ZOOM_STEPS = [50, 100, 150, 200, 250, 300, 350, 400];

interface Props {
  photo: PhotoInfo | null;
  preview: ViewerImage | null;
  /** The camera's embedded JPEG, for the hold-to-compare button. */
  cameraPreview: FullPreview | null;
  /** RapidOne's own render of the photo with no edit (as Lightroom's Before): what the
   *  hold-to-compare button shows, unless it's set to the camera JPEG. Null until asked for. */
  beforePreview?: { path: string; imageBase64: string } | null;
  /** What the compare button shows: the unedited render ("before") or the camera's JPEG. */
  compareWith?: "before" | "camera";
  onCompareWith?: (mode: "before" | "camera") => void;
  /** The button was pressed: the unedited render is wanted (made once, then kept). */
  onCompareStart?: () => void;
  /** Its keys (\ for Before / After) listen: Edit is the tab showing. */
  keysActive?: boolean;
  values: Adjustments;
  curves: Curves;
  grade: Grade;
  index: number;
  total: number;
  onOpenFolder: () => void;
  /** The home screen: the start prompt with the folder's name, over the current photo. */
  home: boolean;
  folderName: string | null;
  /** The folder being read, while no photo is ready yet (shown in the start screen's prompt). */
  progress?: { done: number; total: number } | null;
  onResume: () => void;
  /** On a fresh start: the last working folder to go back to, shown next to Open folder. */
  resumeSession?: { name: string; run: () => void } | null;
  /** The start screen's picture: the last photo worked on (Storr if none). */
  startImage?: string;
  /** A slider is being adjusted (its key held, or dragged): the wheel is the slider's, not zoom's. */
  still?: boolean;
  /** The alignment grid over the photo (while levelling or keystoning). */
  grid?: boolean;
  /** Right-click on the photo (where the pointer is): the loupe's menu. */
  onMenu?: (x: number, y: number) => void;
  /** Full frame (F): the photo fills the viewer, without the corridor. */
  fullFrame?: boolean;
  /** Height (px) of the slider panel docked over the viewer's bottom: the photo stays above it. */
  bottomInset?: number;
  /** The D65 corridor lines and halo round the photo (tick box). */
  surround?: boolean;
  /** The crop tool (open: the engine renders the photo uncropped under the frame). */
  cropping: boolean;
  crop: Crop;
  cropAspect: string;
  onCrop: (c: Crop) => void;
  onCropAspect: (a: string) => void;
  onCropDone: () => void;
  /** The Remove tool is open (the photo shows uncropped, no keystone, like Crop). */
  removing?: boolean;
  /** Its overlay, given the displayed photo. */
  removeOverlay?: (image: HTMLImageElement | null) => ReactNode;
  /** The overlay works zoomed in too (the gradient tools): zoom stays on
   *  while it's open, and it's handed the zoomed picture. */
  overlayZooms?: boolean;
  /** Zoomed in past the preview: the part on screen at full resolution
   *  (rect as fractions of the photo as shown, size in pixels). */
  onDetail?: (rect: [number, number, number, number], width: number, height: number, interactive: boolean) => Promise<string>;
  /** Changes with the photo or its settings: older detail is then dropped. */
  detailKey?: string;
  /** Told when the zoomed view is showing full-resolution detail (the
   *  preview under it is hidden, so it needn't render while sliders move). */
  onDetailActive?: (active: boolean) => void;
  /** Something covers the loupe (the Library's grid): no Before | After buttons over it. */
  noCompare?: boolean;
}

/**
 * Rough live feedback using CSS filters until the real RAW engine exists.
 * Only exposure, contrast and saturation are approximated, and not
 * accurately - this is for feeling out the controls, not judging colour.
 */
// `engine`: the engine already rendered contrast and saturation from real RAW
// data, so the quick CSS versions (only right for the camera-JPEG fallback)
// are skipped.
function roughFilter(v: Adjustments, withTables: boolean, engine: boolean): string | undefined {
  const contrast = engine ? 1 : 1 + v.contrast / 100;
  const saturate = engine ? 1 : 1 + v.saturation / 100;
  // Nothing to change (the engine's own picture at rest): no filter at all. Even contrast(1)
  // saturate(1) made Windows' WebView2 draw the photo through its filter path, tile by tile, on
  // every zoom step: half-drawn squares while zooming quickly (24 Oct).
  if (!withTables && contrast === 1 && saturate === 1) return undefined;
  const tables = withTables ? "url(#rapidone-tables) " : "";
  return `${tables}contrast(${contrast}) saturate(${saturate})`;
}

/**
 * Applies the tone curves and colour grade to the preview with an SVG
 * lookup-table filter - per channel, curves first then the grade - on the
 * 8-bit embedded preview.
 */
function TablesFilter({ exposure, curves, grade, id = "rapidone-tables" }: { exposure: number; curves: Curves; grade: Grade; id?: string }) {
  const table = (c: "r" | "g" | "b", i: 0 | 1 | 2) => {
    const graded = gradeChannel(grade, i);
    const curve = channelFunction(curves, c);
    const expose = exposureFunction(exposure);
    // Exposure first (linear, with a highlight shoulder), then curves, then the grade.
    return Array.from({ length: 256 }, (_, n) => n / 255)
      .map((v) => graded(curve(expose(v))))
      .map((v) => v.toFixed(4))
      .join(" ");
  };
  return (
    <svg width="0" height="0" style={{ position: "absolute" }} aria-hidden="true">
      <filter id={id} colorInterpolationFilters="sRGB">
        <feComponentTransfer>
          <feFuncR type="table" tableValues={table("r", 0)} />
          <feFuncG type="table" tableValues={table("g", 1)} />
          <feFuncB type="table" tableValues={table("b", 2)} />
        </feComponentTransfer>
      </filter>
    </svg>
  );
}

/** The resolutions a drag's renders step through (Chris: 20/40/60/80/100%). */
const DRAG_LEVELS = [0.2, 0.4, 0.6, 0.8, 1];

/** A frame slower than this misses the response speed: the drag drops a resolution level. */
const DRAG_SLOW_MS = 150;
/** Quicker than this while a drag is still finding its level: one level up is tried. */
const DRAG_QUICK_MS = 72;
/** A drag's first frames find its level; after that it stays put. */
const DRAG_PROBE_FRAMES = 3;

/**
 * A drag's render resolution (DRAG_LEVELS). Its first frames find the level that keeps up (down a
 * level for a slow frame, up for a quick one); then it settles there until the slider is let go,
 * dropping further only if frames miss the response speed, never back up. Re-deciding on every frame
 * (as before) hopped between levels on Clarity / Vibrance renders near the limits, and each hop
 * changed the picture's softness and placement a little: it wobbled (Chris, 27 Oct). The next drag
 * starts from the level this one settled on.
 */
export class DragScale {
  private frames = 0;
  // The first frame at a new level also makes the photo's copy at that size (once): not counted, or
  // that one-off cost read as slow and each drop led to another, down to the smallest level.
  private warming = false;
  constructor(
    public scale: number,
    private readonly min: number,
  ) {}
  /** A drag's frame took `ms`. */
  frame(ms: number): void {
    const levels = DRAG_LEVELS.filter((l) => l >= this.min - 1e-6);
    const i = levels.reduce((best, l, j) => (Math.abs(l - this.scale) < Math.abs(levels[best] - this.scale) ? j : best), 0);
    if (this.warming) {
      this.warming = false;
      return;
    }
    const probing = this.frames < DRAG_PROBE_FRAMES;
    this.frames++;
    const was = this.scale;
    if (ms > DRAG_SLOW_MS) this.scale = levels[Math.max(0, i - 1)];
    else if (probing && ms < DRAG_QUICK_MS) this.scale = levels[Math.min(levels.length - 1, i + 1)];
    else this.scale = levels[i];
    this.warming = this.scale !== was;
  }
  /** The slider was let go: the next drag finds its level again, from this one. */
  end(): void {
    this.frames = 0;
    this.warming = false;
  }
}

/**
 * Fit, and the zoom to pick from (50% steps; 100% = the photo's own pixels), in the top bar. Its own
 * piece, redrawn only when the zoom changes: redrawn with the loupe on every step of a pan, React set
 * the dropdown's value each time and WebKitGTK repainted it and Fit (they flashed).
 */
const ZoomControls = memo(function ZoomControls({ fit, percent, onFit, onPick }: { fit: boolean; percent: number; onFit: () => void; onPick: (percent: number) => void }) {
  return (
    <span className="topbar__zoomctl">
      <button type="button" className={fit ? "is-on" : undefined} onClick={onFit} title={keyLabel("Fit the photo to the view (Ctrl+0)")}>
        Fit
      </button>
      <select
        className="topbar__zoomsel"
        value={String(percent)}
        onChange={(e) => onPick(Number(e.currentTarget.value))}
        title={keyLabel("Zoom (100% = the photo's own pixels; also the mouse wheel, Ctrl+= / Ctrl+-, a double-click on the photo)")}
        aria-label="Zoom"
      >
        {!ZOOM_STEPS.includes(percent) && <option value={String(percent)}>{percent}%</option>}
        {ZOOM_STEPS.map((p) => (
          <option key={p} value={String(p)}>
            {p}%
          </option>
        ))}
      </select>
    </span>
  );
});

export default function Viewer({
  photo,
  preview,
  cameraPreview,
  beforePreview = null,
  compareWith = "before",
  noCompare = false,
  onCompareWith,
  onCompareStart,
  keysActive = false,
  values,
  curves,
  grade,
  index,
  total,
  onOpenFolder,
  home,
  folderName,
  progress,
  onResume,
  resumeSession,
  startImage,
  still,
  grid,
  onMenu,
  fullFrame = false,
  bottomInset = 0,
  surround = true,
  cropping,
  crop,
  cropAspect,
  onCrop,
  onCropAspect,
  onCropDone,
  removing = false,
  removeOverlay,
  overlayZooms = false,
  onDetail,
  detailKey = "",
  onDetailActive,
}: Props) {
  const [comparing, setComparing] = useState(false);
  // A new photo starts on After; \ switches Before / After (Edit only, not while typing).
  useEffect(() => setComparing(false), [photo?.path]);
  const compareLive = useRef({ keysActive, compareWith, onCompareStart });
  compareLive.current = { keysActive, compareWith, onCompareStart };
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const live = compareLive.current;
      if (e.key !== "\\" || !live.keysActive || e.ctrlKey || e.metaKey || e.altKey) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      e.preventDefault();
      setComparing((on) => {
        if (!on && live.compareWith === "before") live.onCompareStart?.();
        return !on;
      });
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, []);
  // What the compare button shows: the unedited render once it's made (After stays meanwhile: the
  // camera's JPEG has neither the crop nor the frame), or the camera's JPEG when set to it.
  const cameraSrc = cameraPreview?.path === photo?.path ? cameraPreview?.imageBase64 : undefined;
  const compareSrc =
    compareWith === "before" ? (beforePreview?.path === photo?.path ? beforePreview?.imageBase64 : undefined) : cameraSrc;
  // Exposure the image on screen still lacks. The engine bakes exposure in
  // from real RAW data; until its next render lands, the quick approximation
  // covers just the difference, so the slider always responds instantly.
  const bakedExposure = preview && photo && preview.path === photo.path ? (preview.exposure ?? 0) : 0;
  // Over the engine's renders nothing is approximated: each picture shown is a
  // real render (Lightroom-style); only the camera JPEG stand-in is bridged.
  const residualExposure = preview?.source === "engine" ? 0 : values.exposure - bakedExposure;
  // The engine renders Curves and Grade itself; the quick tables only stand
  // in for them on the camera JPEG.
  const engine = preview?.source === "engine";
  const tableCurves = engine ? DEFAULT_CURVES : curves;
  const tableGrade = engine ? DEFAULT_GRADE : grade;
  const tablesActive =
    Math.abs(residualExposure) > 1e-6 ||
    CURVE_CHANNELS.some((c) => !isIdentity(tableCurves[c])) ||
    !isNeutralGrade(tableGrade);

  // The perspective for keystone preview is the image's on-screen long edge,
  // so track its displayed size.
  const [imgEl, setImgEl] = useState<HTMLImageElement | null>(null);
  // The zoomed picture (zoom layer), for an overlay that works zoomed in.
  const [zoomImgEl, setZoomImgEl] = useState<HTMLImageElement | null>(null);
  const [longEdge, setLongEdge] = useState(1000);
  useEffect(() => {
    const img = imgEl;
    if (!img) return;
    const observer = new ResizeObserver(() => setLongEdge(Math.max(img.clientWidth, img.clientHeight)));
    observer.observe(img);
    return () => observer.disconnect();
  }, [imgEl]);

  // The crop bar can be dragged by its grip (its place kept until the app closes).
  const cropBar = useFloatingBar("crop");
  // The corridor ends at the photo itself: its rectangle (0-100 of the
  // viewer), fitted inside the closest frame at the photo's own shape. The
  // shape is the last known one while a new photo's picture is on its way.
  const mainRef = useRef<HTMLDivElement>(null);
  // x, y: where the viewer sits in the window, for putting the photo's edges on whole screen pixels.
  const [box, setBox] = useState({ w: 1600, h: 900, x: 0, y: 0 });
  useEffect(() => {
    const el = mainRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      setBox({ w: el.clientWidth || 1, h: el.clientHeight || 1, x: r.left, y: r.top });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  // The picture in the loupe: one element, never swapped for another, whose picture changes only
  // once the next one is decoded (thumbnail, camera JPEG, render). A fresh element, or one whose
  // picture isn't ready, shows empty for a frame or two in WebKitGTK's GPU drawing: the flicker.
  // The previous picture (even the last photo's) stays up meanwhile, a few milliseconds.
  type OnScreen = { src: string; engine: boolean; pending: boolean; interactive: boolean; uncropped: boolean };
  const target: OnScreen | null = !photo
    ? null
    : preview?.path === photo.path
      ? {
          src: preview.imageBase64,
          engine: preview.source === "engine",
          pending: Boolean(preview.pending),
          interactive: Boolean(preview.interactive),
          uncropped: Boolean(preview.uncropped),
        }
      : { src: photo.thumbBase64, engine: false, pending: true, interactive: false, uncropped: false };
  const [onScreen, setOnScreen] = useState<OnScreen | null>(null);
  // The newest picture asked for, and the newest put on screen (see the decode below).
  const decodeSeq = useRef(0);
  const shownSeq = useRef(0);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const [aspect, setAspect] = useState(3 / 2);
  // The photo the frame's shape was last taken from.
  const aspectPath = useRef<string | null>(null);
  useEffect(() => {
    if (!target) return;
    if (onScreen?.src === target.src) {
      if (
        onScreen.engine !== target.engine ||
        onScreen.pending !== target.pending ||
        onScreen.interactive !== target.interactive ||
        onScreen.uncropped !== target.uncropped
      ) {
        // This is the newest picture now: a decode still running for an older one mustn't replace it.
        shownSeq.current = ++decodeSeq.current;
        setOnScreen(target);
      }
      return;
    }
    // Each picture asked for is numbered, and whichever finishes decoding goes up if it's newer than
    // what's showing. A newer request never cancels an older decode: while a slider moves, renders
    // came faster than WebView2 (Windows) decoded them, each cancelled the one before, and the photo
    // stopped changing (24 Oct). The newest still wins, and a stale one never replaces a newer one.
    const seq = ++decodeSeq.current;
    const im = new Image();
    im.src = target.src;
    im.decode()
      .catch(() => {})
      .then(() => {
        if (seq <= shownSeq.current || !mounted.current) return;
        shownSeq.current = seq;
        setOnScreen(target);
        // The frame takes the picture's shape as it goes up. A new shape (a crop, the next photo) at
        // once; renders of the same shape at other sizes (a quick half-size one, then the exact one)
        // round a hair differently (well under 0.5%), and following that nudged the photo a pixel
        // (the crop tool's first second, 24 Oct), so within a photo that's kept as it is.
        if (im.naturalWidth > 0 && im.naturalHeight > 0) {
          const next = im.naturalWidth / im.naturalHeight;
          const samePhoto = aspectPath.current === photo?.path;
          aspectPath.current = photo?.path ?? null;
          setAspect((prev) => (samePhoto && Math.abs(next - prev) / prev < 0.005 ? prev : next));
        }
      });
    // target is rebuilt every render; these are what it is.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target?.src, target?.engine, target?.pending, target?.interactive, target?.uncropped]);
  // Masks, Patch and Erase show the whole photo (their marks are placed on it), so in the loupe the
  // photo is clipped to the crop: what's cut away simply isn't drawn, and the corridor and halo sit
  // round the crop. The tools' marks and handles stay where they are. Only once the uncropped render
  // is up, never over the cropped picture still showing as they open.
  const clipOutline =
    removing && !cropping && !isNoCrop(crop) && onScreen?.engine && onScreen.uncropped ? cropOutline(crop, aspect) : null;
  const clipPath = clipOutline ? `polygon(${clipOutline.map(([x, y]) => `${x * 100}% ${y * 100}%`).join(", ")})` : undefined;
  // The crop is a window onto the photo: with a plain crop, Masks, Patch and Erase work over the
  // cropped picture as it is. Their marks live on the whole photo, so they're handed a stand-in
  // for it (invisible, the whole photo's size and place, the crop lined up with the picture on
  // screen) to measure instead of the picture: everything they place lines up, nothing is
  // re-rendered, and what lies outside the crop is kept for a later re-crop.
  const windowed = Boolean(removing && !cropping && !isNoCrop(crop) && straightCrop(crop) && onScreen?.engine && !onScreen.uncropped);
  const [proxyEl, setProxyEl] = useState<HTMLImageElement | null>(null);
  const [zoomProxyEl, setZoomProxyEl] = useState<HTMLImageElement | null>(null);
  const cropW = Math.max(1e-6, crop.x1 - crop.x0);
  const cropH = Math.max(1e-6, crop.y1 - crop.y0);
  // Its pixel size is the whole photo's at the preview's scale (Erase paints its mask at that size).
  const proxySrc =
    windowed && preview
      ? `data:image/svg+xml,${encodeURIComponent(
          `<svg xmlns="http://www.w3.org/2000/svg" width="${Math.round(preview.width / cropW)}" height="${Math.round(preview.height / cropH)}"/>`,
        )}`
      : undefined;
  // The alignment grid's rows: square cells, twelve across.
  const gridRows = Math.max(2, Math.round(12 / aspect));
  // Full frame (F), or D65 Glow off (no corridor to make room for): the photo fills the viewer.
  const margin = fullFrame || !surround ? 1 : PHOTO_RING;
  // The frame glides to its new size only for Full frame and D65 Glow (the photo stays its shape);
  // a new shape (a crop, the next photo) is there at once, not the old picture stretched on the way.
  const [gliding, setGliding] = useState(false);
  const glideFirst = useRef(true);
  useEffect(() => {
    if (glideFirst.current) {
      glideFirst.current = false;
      return;
    }
    setGliding(true);
    const t = window.setTimeout(() => setGliding(false), 350);
    return () => window.clearTimeout(t);
  }, [fullFrame, surround]);
  const photoBox: Box = (() => {
    // The area above the docked panel; the corridor still runs to the viewer's
    // edges, under the panel.
    const areaH = Math.max(1, box.h - bottomInset);
    const availW = box.w * (1 - (2 * margin) / 100);
    const availH = areaH * (1 - (2 * margin) / 100);
    const w = Math.min(availW, availH * aspect);
    const h = w / aspect;
    // Every edge on a whole screen pixel (from the window's corner: the viewer itself can start part
    // way into one, e.g. 28 px down at 1.6x). With D65 Glow the frame came out a fraction of a pixel
    // across, its last row and column part-covered, and the quick colour filter on the photo while a
    // slider moves turned those part-covered pixels white: a white line at the bottom (27 Oct).
    const ratio = window.devicePixelRatio || 1;
    const snapX = (v: number) => Math.round((box.x + v) * ratio) / ratio - box.x;
    const snapY = (v: number) => Math.round((box.y + v) * ratio) / ratio - box.y;
    const l = snapX((box.w - w) / 2);
    const r = snapX((box.w + w) / 2);
    const t = snapY((areaH - h) / 2);
    const b = snapY((areaH + h) / 2);
    return { l: (l / box.w) * 100, t: (t / box.h) * 100, r: (r / box.w) * 100, b: (b / box.h) * 100 };
  })();
  // Where the corridor and halo end: the photo's box, or with the photo clipped to its crop (above),
  // the crop's box within it.
  const shownBox: Box = (() => {
    if (!clipOutline) return photoBox;
    const xs = clipOutline.map(([x]) => x);
    const ys = clipOutline.map(([, y]) => y);
    const at = (a: number, b: number, f: number) => a + (b - a) * Math.min(1, Math.max(0, f));
    return {
      l: at(photoBox.l, photoBox.r, Math.min(...xs)),
      r: at(photoBox.l, photoBox.r, Math.max(...xs)),
      t: at(photoBox.t, photoBox.b, Math.min(...ys)),
      b: at(photoBox.t, photoBox.b, Math.max(...ys)),
    };
  })();
  // The photo's size as the frame sees it (after quarter turns): the uncropped engine preview.
  const pw = preview?.width ?? 1;
  const ph = preview?.height ?? 1;

  // ---- Zoom (Fit, in and out, 100%) ----
  // null = Fit. z: the photo's size relative to Fit; cx, cy: the point of the
  // photo (fractions) at the middle of the view. Kept when the photo changes,
  // so a burst can be checked at the same spot; Crop and Remove work at Fit.
  const [zoom, setZoom] = useState<{ z: number; cx: number; cy: number } | null>(null);
  const zoomable = Boolean(photo) && !home && !cropping && (!removing || overlayZooms);
  useEffect(() => {
    if (!zoomable) setZoom(null);
  }, [zoomable]);
  const areaW = box.w;
  const areaH = Math.max(1, box.h - bottomInset);
  const fitW = Math.max(1, ((photoBox.r - photoBox.l) / 100) * box.w);
  const fitH = Math.max(1, ((photoBox.b - photoBox.t) / 100) * box.h);
  // 100%: one of the photo's full-resolution pixels per screen pixel.
  const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
  const shown = preview && photo && preview.path === photo.path ? preview : null;
  const z100 = shown ? Math.max(0.05, (shown.width * (shown.fullScale && shown.fullScale > 0 ? shown.fullScale : 1)) / dpr / fitW) : 1;
  const zMax = Math.max(z100 * 4, 4);
  // Masks and Patch open on the whole photo (rendered uncropped) and close back to the crop, so Fit
  // changes size under the zoom: keep its percentage and the spot it's on, rather than the zoom
  // jumping (to 32% or so). Measured only while the picture up is the one the zoom is sized from.
  const zoomKeep = useRef<{ path: string; uncropped: boolean; percent: number; cx: number; cy: number } | null>(null);
  const settledPicture = Boolean(shown && onScreen && shown.imageBase64 === onScreen.src);
  useEffect(() => {
    if (!photo || !onScreen || !settledPicture) return;
    const was = zoomKeep.current;
    const uncropped = onScreen.uncropped;
    if (zoom && was && was.path === photo.path && was.uncropped !== uncropped) {
      // The spot: crop fractions <-> whole-photo fractions (a straight, unturned crop; otherwise as is).
      const plain = crop.angle === 0 && crop.quarter % 4 === 0;
      const [sx, sy] = [crop.x1 - crop.x0, crop.y1 - crop.y0];
      const cx = !plain ? was.cx : uncropped ? crop.x0 + was.cx * sx : (was.cx - crop.x0) / sx;
      const cy = !plain ? was.cy : uncropped ? crop.y0 + was.cy * sy : (was.cy - crop.y0) / sy;
      const z = Math.min(zMax, Math.max(1, was.percent * z100));
      zoomKeep.current = { path: photo.path, uncropped, percent: was.percent, cx, cy };
      const at = place(z, Math.min(1, Math.max(0, cx)), Math.min(1, Math.max(0, cy)));
      setZoom({ z, cx: at.cx, cy: at.cy });
      return;
    }
    zoomKeep.current = { path: photo.path, uncropped, percent: (zoom?.z ?? 1) / z100, cx: zoom?.cx ?? 0.5, cy: zoom?.cy ?? 0.5 };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [photo?.path, onScreen?.src, onScreen?.uncropped, settledPicture, zoom?.z, zoom?.cx, zoom?.cy, z100]);
  /** Where the photo sits at zoom z centred on (cx, cy): left, top, width, height in px. */
  // Zoomed, the photo's edges on whole screen pixels too, as the frame's (photoBox).
  const place = (z: number, cx: number, cy: number) => {
    const w = fitW * z;
    const h = fitH * z;
    const x = w > areaW ? Math.min(Math.max(cx, areaW / 2 / w), 1 - areaW / 2 / w) : 0.5;
    const y = h > areaH ? Math.min(Math.max(cy, areaH / 2 / h), 1 - areaH / 2 / h) : 0.5;
    const left = Math.round((box.x + areaW / 2 - x * w) * dpr) / dpr - box.x;
    const top = Math.round((box.y + areaH / 2 - y * h) * dpr) / dpr - box.y;
    const right = Math.round((box.x + left + w) * dpr) / dpr - box.x;
    const bottom = Math.round((box.y + top + h) * dpr) / dpr - box.y;
    return { left, top, w: right - left, h: bottom - top, cx: x, cy: y };
  };
  const zoomGeo = useRef({ zoom, place, z100, zMax, zoomable });
  zoomGeo.current = { zoom, place, z100, zMax, zoomable };
  /** Zoom to z keeping the photo point under (px, py) (view px) where it is. */
  const zoomAt = (z: number, px: number, py: number) => {
    const g = zoomGeo.current;
    const cur = g.zoom ?? { z: 1, cx: 0.5, cy: 0.5 };
    const now = g.place(cur.z, cur.cx, cur.cy);
    const next = Math.min(Math.max(z, 1), g.zMax);
    if (next <= 1.001) {
      setZoom(null);
      return;
    }
    const fx = Math.min(Math.max((px - now.left) / now.w, 0), 1);
    const fy = Math.min(Math.max((py - now.top) / now.h, 0), 1);
    const w = fitW * next;
    const h = fitH * next;
    const p = g.place(next, (areaW / 2 - (px - fx * w)) / w, (areaH / 2 - (py - fy * h)) / h);
    setZoom({ z: next, cx: p.cx, cy: p.cy });
  };
  const zoomAtRef = useRef(zoomAt);
  zoomAtRef.current = zoomAt;
  const panBy = (dx: number, dy: number) => {
    const g = zoomGeo.current;
    if (!g.zoom) return;
    const p = g.place(g.zoom.z, g.zoom.cx + dx / (fitW * g.zoom.z), g.zoom.cy + dy / (fitH * g.zoom.z));
    setZoom({ z: g.zoom.z, cx: p.cx, cy: p.cy });
  };
  const panByRef = useRef(panBy);
  panByRef.current = panBy;
  const stillRef = useRef(Boolean(still));
  stillRef.current = Boolean(still);
  // Mouse wheel: zoom by steps at the pointer. Touchpad pinch (wheel with Ctrl,
  // as Windows / Chromium report it; WebKit on macOS reports it as gesture events
  // with a scale instead): smooth zoom. Touchpad two-finger scroll: pan.
  useEffect(() => {
    const el = mainRef.current;
    if (!el) return;
    // WebKit's pinch: the scale since the gesture began, applied to the zoom it began at.
    let pinch: { z: number } | null = null;
    let pinchEnded = 0;
    // Until when wheel events count as a touchpad's (see `isMouseWheel`).
    let touchpadUntil = 0;
    // macOS: until when wheel events belong to a mouse notch's stream, and the last step's time.
    let macMouseUntil = 0;
    let macLastStep = 0;
    const onWheel = (e: WheelEvent) => {
      if (!zoomGeo.current.zoomable) return;
      // During a WebKit pinch the gesture events do the zooming (in case Ctrl+wheel comes too).
      if (e.ctrlKey && (pinch || performance.now() - pinchEnded < 250)) {
        e.preventDefault();
        return;
      }
      // A slider is being adjusted (its hotkey held, or dragged in the panel):
      // the wheel belongs to the slider (App raises / lowers the held one), not to zoom.
      if (stillRef.current) return;
      e.preventDefault();
      const r = el.getBoundingClientRect();
      const px = e.clientX - r.left;
      const py = e.clientY - r.top;
      const z = zoomGeo.current.zoom?.z ?? 1;
      const now = performance.now();
      let mouseWheel: boolean;
      if (MAC_WEBKIT) {
        if (e.deltaX === 0 && Math.abs(Math.abs(e.deltaY) - MAC_NOTCH) < 1e-6) macMouseUntil = now + TOUCHPAD_STREAM_MS;
        mouseWheel = e.deltaX === 0 && !e.ctrlKey && now < macMouseUntil;
        if (mouseWheel) {
          macMouseUntil = now + TOUCHPAD_STREAM_MS;
          if (now - macLastStep < MAC_STEP_MS) return;
          macLastStep = now;
        }
      } else {
        mouseWheel = isMouseWheel(e, now, touchpadUntil);
        touchpadUntil = mouseWheel ? 0 : now + TOUCHPAD_STREAM_MS;
      }
      // A notch zooms a step, Ctrl or not; a touchpad pinch (Ctrl, small amounts) zooms smoothly.
      if (mouseWheel) zoomAtRef.current(z * (e.deltaY < 0 ? 1.25 : 0.8), px, py);
      else if (e.ctrlKey) zoomAtRef.current(z * Math.exp(-e.deltaY * 0.01), px, py);
      else panByRef.current(e.deltaX, e.deltaY);
    };
    type Gesture = UIEvent & { scale: number; clientX: number; clientY: number };
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      pinch = zoomGeo.current.zoomable && !stillRef.current ? { z: zoomGeo.current.zoom?.z ?? 1 } : null;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      if (!pinch || !zoomGeo.current.zoomable) return;
      const g = e as Gesture;
      const r = el.getBoundingClientRect();
      zoomAtRef.current(pinch.z * g.scale, g.clientX - r.left, g.clientY - r.top);
    };
    const onGestureEnd = (e: Event) => {
      e.preventDefault();
      pinch = null;
      pinchEnded = performance.now();
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("gesturestart", onGestureStart);
    el.addEventListener("gesturechange", onGestureChange);
    el.addEventListener("gestureend", onGestureEnd);
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("gesturestart", onGestureStart);
      el.removeEventListener("gesturechange", onGestureChange);
      el.removeEventListener("gestureend", onGestureEnd);
    };
  }, [photo !== null, home]);
  // Ctrl+0 Fit, Ctrl+1 100%, Ctrl+= / Ctrl+- in and out (at the middle).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || !zoomGeo.current.zoomable) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;
      const g = zoomGeo.current;
      const z = g.zoom?.z ?? 1;
      const mid = [areaW / 2, areaH / 2] as const;
      if (e.code === "Digit0" || e.code === "Numpad0") setZoom(null);
      else if (e.code === "Digit1" || e.code === "Numpad1") zoomAtRef.current(g.z100, ...mid);
      else if (e.code === "Equal" || e.code === "NumpadAdd") zoomAtRef.current(z * 1.5, ...mid);
      else if (e.code === "Minus" || e.code === "NumpadSubtract") zoomAtRef.current(z / 1.5, ...mid);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [areaW, areaH]);
  // Drag to pan; double-click for 100% there (or back to Fit).
  const drag = useRef<{ x: number; y: number } | null>(null);
  const onZoomPointerDown = (e: React.PointerEvent) => {
    if (!zoom || e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, y: e.clientY };
  };
  const onZoomPointerMove = (e: React.PointerEvent) => {
    if (!drag.current) return;
    panBy(drag.current.x - e.clientX, drag.current.y - e.clientY);
    drag.current = { x: e.clientX, y: e.clientY };
  };
  const onZoomPointerUp = () => {
    drag.current = null;
  };
  const onZoomDoubleClick = (e: React.MouseEvent) => {
    if (!zoomable) return;
    const r = mainRef.current?.getBoundingClientRect();
    if (!r) return;
    if (zoom) setZoom(null);
    else zoomAt(z100 > 1.05 ? z100 : 2, e.clientX - r.left, e.clientY - r.top);
  };
  const placed = zoom ? place(zoom.z, zoom.cx, zoom.cy) : null;

  // Full-resolution detail for the part on screen, once zoomed past what the
  // preview holds: asked for after a short pause (longer on a slower
  // computer: the last render's time sets it), the newest request winning;
  // dropped when the photo or its settings change. The preview stays under
  // it, so every change shows at once and sharpens a moment later.
  const [detail, setDetail] = useState<{ key: string; rect: [number, number, number, number]; src: string; exposure: number } | null>(null);
  const lastDetailMs = useRef(150);
  // While dragging: the resolution steps through 100/80/60/40/20% - down a
  // level while frames take over ~0.15 s, up a level when comfortably quicker
  // - kept for the next drag: as sharp as this computer and file allow.
  const dragScale = useRef(new DragScale(0.8, 0.2));
  const needDetail = Boolean(placed && shown && shown.source === "engine" && !shown.pending && placed.w * dpr > shown.width * 1.05);
  const visible: [number, number, number, number] | null = placed
    ? [
        Math.max(0, -placed.left / placed.w),
        Math.max(0, -placed.top / placed.h),
        Math.min(1, (areaW - placed.left) / placed.w),
        Math.min(1, (areaH - placed.top) / placed.h),
      ]
    : null;
  const visibleKey = visible ? visible.map((v) => v.toFixed(4)).join(",") : "";
  // One render at a time; whatever was asked for meanwhile goes next (the newest only).
  const wanted = useRef<{ key: string; rect: [number, number, number, number]; w: number; h: number; exposure: number; interactive: boolean } | null>(null);
  const inFlight = useRef(false);
  const onDetailRef = useRef(onDetail);
  onDetailRef.current = onDetail;
  const pump = useCallback(() => {
    const job = wanted.current;
    if (inFlight.current || !job || !onDetailRef.current) return;
    wanted.current = null;
    inFlight.current = true;
    const started = performance.now();
    onDetailRef.current(job.rect, job.w, job.h, job.interactive)
      .then((src) => {
        const ms = performance.now() - started;
        lastDetailMs.current = ms;
        if (job.interactive) dragScale.current.frame(ms);
        else dragScale.current.end();
        setDetail({ key: job.key, rect: job.rect, src, exposure: job.exposure });
      })
      .catch(() => undefined)
      .finally(() => {
        inFlight.current = false;
        pump();
      });
  }, []);
  // A computer that renders the view quickly keeps it sharp while sliders
  // move (each render shows as it lands); a slower one shows the preview
  // while they move and sharpens after a pause.
  // Renders are asked for straight away (one at a time, newest next) unless
  // this computer is slow at them.
  const fastDetail = lastDetailMs.current < 350;
  // While a slider is being moved (`still`): half resolution, about four times
  // faster, every frame a real render; full resolution once it stops.
  // While a slider moves: the photo's latest whole-photo measures stand in
  // (no whole-photo render to wait for), and the view renders at half
  // resolution (about four times faster): soft while it moves, sharp the
  // moment it stops.
  const interactive = Boolean(still);
  useEffect(() => {
    if (!needDetail || !visible || !placed || !onDetail) {
      wanted.current = null;
      return;
    }
    const k = interactive ? dragScale.current.scale : 1;
    wanted.current = {
      key: detailKey,
      rect: visible,
      w: Math.round((visible[2] - visible[0]) * placed.w * dpr * k),
      h: Math.round((visible[3] - visible[1]) * placed.h * dpr * k),
      exposure: values.exposure,
      interactive,
    };
    // While a slider moves: straight away (one at a time, newest next); a
    // pause-wait would restart with every change and never fire mid-drag.
    if (fastDetail || interactive) {
      pump();
      return;
    }
    const timer = window.setTimeout(pump, Math.min(Math.max(lastDetailMs.current * 0.6, 60), 500));
    return () => window.clearTimeout(timer);
    // `visible` and `placed` follow visibleKey and the zoom.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [needDetail, visibleKey, detailKey, zoom?.z, dpr, interactive]);
  // While a newer render is on its way, the detail shown is behind on
  // Exposure: bridged with the same quick approximation as the preview, so it
  // follows the slider at once and the next render only settles the rest.
  const detailExposure = 0;
  useEffect(() => {
    onDetailActive?.(needDetail);
  }, [needDetail, onDetailActive]);
  const samePhoto = (k: string) => k.split("")[0] === detailKey.split("")[0];
  const detailShown =
    // The latest real render of this photo stays up until the next replaces
    // it: never an approximation, so nothing jumps when the newest lands.
    // Except one made with other settings, outside a drag: zooming in after
    // a change showed the patch from before it (a second at the old
    // exposure, 24 Oct), so the soft preview, which is current, shows until
    // the patch for these settings lands. While a slider moves, the newest
    // patch stays, or the view would flick between sharp and soft.
    detail && placed && needDetail && samePhoto(detail.key) && (interactive || detail.key === detailKey) ? detail : null;
  const lastDetail = useRef<{ src: string; rect: [number, number, number, number] } | null>(null);
  const underDetail = useRef<{ src: string; rect: [number, number, number, number] } | null>(null);
  if ((detailShown?.src ?? null) !== (lastDetail.current?.src ?? null)) {
    underDetail.current = detailShown ? lastDetail.current : null;
    lastDetail.current = detailShown ? { src: detailShown.src, rect: detailShown.rect } : null;
  }
  // The zoom layer stays in place at Fit too, unseen, sized as Fit: WebKitGTK paints a layer made
  // on the first zoom step (or remade after Fit) a frame or two out of place, a visible jump.
  const layerPlace = placed ?? place(1, 0.5, 0.5);
  const zoomedStyle: React.CSSProperties = {
    position: "absolute",
    left: layerPlace.left,
    top: layerPlace.top,
    width: layerPlace.w,
    height: layerPlace.h,
    maxWidth: "none",
    maxHeight: "none",
  };
  const zoomPercent = Math.round(((zoom?.z ?? 1) / z100) * 100);
  // The zoom controls' actions, fixed for the Viewer's life (they read the zoom through refs), so the
  // controls redraw only when the zoom does, not on every step of a pan.
  const pickRef = useRef((pct: number) => zoomAt((z100 * pct) / 100, areaW / 2, areaH / 2));
  pickRef.current = (pct: number) => zoomAt((z100 * pct) / 100, areaW / 2, areaH / 2);
  const zoomFit = useCallback(() => setZoom(null), []);
  const zoomPick = useCallback((pct: number) => pickRef.current(pct), []);
  // The top bar's place for the zoom controls (TopBar's #topbar-zoom).
  const [zoomSlot, setZoomSlot] = useState<HTMLElement | null>(null);
  // Looked for after every render: the top bar shows it only in Edit, off the home screen.
  useEffect(() => {
    const el = document.getElementById("topbar-zoom");
    if (el !== zoomSlot) setZoomSlot(el);
  });
  return (
    <div className="viewer">
      {tablesActive && <TablesFilter exposure={residualExposure} curves={tableCurves} grade={tableGrade} />}
      <div
        className="viewer__main"
        ref={mainRef}
        style={!photo || home ? { paddingBottom: bottomInset } : undefined}
        onContextMenu={
          onMenu && photo && !home && !cropping
            ? (e) => {
                e.preventDefault();
                onMenu(e.clientX, e.clientY);
              }
            : undefined
        }
      >
        {!photo || home ? (
          // The start / home screen: one screen, shared. Its picture is the photo
          // being edited, else the last one worked on (Storr if none); its prompt
          // offers to resume when a folder is open, else to open one.
          <div className="viewer__empty">
            <GridScene
              image={photo ? (preview?.path === photo.path ? preview.imageBase64 : photo.thumbBase64) : startImage}
              bottomInset={bottomInset}
            />
            {photo ? (
              <TerminalPrompt folderName={folderName} onPrompt={onResume} onOpenFolder={onOpenFolder} />
            ) : (
              <TerminalPrompt
                onPrompt={onOpenFolder}
                loading={folderName && progress && progress.done < progress.total ? { folderName, ...progress } : null}
                resume={resumeSession}
              />
            )}
          </div>
        ) : (
          <>
            {/* The corridor from the home screen, behind the photo; it rushes past when a new photo arrives. */}
            <svg key={`corridor-${photo.path}`} className={`viewer__corridor${fullFrame || !surround || zoom ? " viewer__corridor--hidden" : ""}`}
              style={corridorFalloff(shownBox)} viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
              <path d={corridorPath(shownBox)} vectorEffect="non-scaling-stroke" />
            </svg>
            {/* The photo in the corridor's closest frame, flying forward from the far end on a new photo. */}
            <div
              className={`viewer__frame${fullFrame ? " viewer__frame--full" : ""}${surround ? "" : " viewer__frame--plain"}${gliding ? " viewer__frame--glide" : ""}`}
              style={{
                left: `${photoBox.l}%`,
                top: `${photoBox.t}%`,
                right: `${100 - photoBox.r}%`,
                bottom: `${100 - photoBox.b}%`,
                // Zoomed: shown by the zoom layer instead; the frame keeps its
                // place and animations, so nothing jumps either way.
                visibility: placed ? "hidden" : undefined,
                // The halo round the crop when the photo is clipped to it (App.css, ::before).
                ...(clipOutline
                  ? ({
                      "--halo-l": `${((shownBox.l - photoBox.l) / (photoBox.r - photoBox.l)) * 100}%`,
                      "--halo-r": `${((photoBox.r - shownBox.r) / (photoBox.r - photoBox.l)) * 100}%`,
                      "--halo-t": `${((shownBox.t - photoBox.t) / (photoBox.b - photoBox.t)) * 100}%`,
                      "--halo-b": `${((photoBox.b - shownBox.b) / (photoBox.b - photoBox.t)) * 100}%`,
                    } as React.CSSProperties)
                  : undefined),
              }}
              onDoubleClick={onZoomDoubleClick}
            >
              {onScreen && (
                <img
                  ref={setImgEl}
                  className={`viewer__img${onScreen.engine && !onScreen.pending ? " viewer__img--frame" : ""}`}
                  src={onScreen.src}
                  alt={photo.fileName}
                  draggable={false}
                  style={
                    // A stand-in (thumbnail, camera JPEG, unedited render) as it is; a photo the engine
                    // can't render gets the quick CSS approximation of its edit.
                    onScreen.pending
                      ? undefined
                      : {
                          filter: roughFilter(values, tablesActive, onScreen.engine),
                          // The engine warps the keystone itself (and never under the crop frame or Remove).
                          transform: cropping || removing || onScreen.engine ? undefined : previewTransform(values, longEdge),
                          clipPath,
                        }
                  }
                />
              )}
              {grid && (
                // Square cells, about twelve across, to line verticals and horizons up against.
                <svg className="viewer__grid" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
                  {Array.from({ length: 11 }, (_, i) => (
                    <line key={`v${i}`} className={i === 5 ? "viewer__grid-mid" : undefined} x1={((i + 1) * 100) / 12} y1={0} x2={((i + 1) * 100) / 12} y2={100} vectorEffect="non-scaling-stroke" />
                  ))}
                  {Array.from({ length: gridRows - 1 }, (_, i) => (
                    <line key={`h${i}`} className={i === (gridRows - 2) / 2 ? "viewer__grid-mid" : undefined} x1={0} y1={((i + 1) * 100) / gridRows} x2={100} y2={((i + 1) * 100) / gridRows} vectorEffect="non-scaling-stroke" />
                  ))}
                </svg>
              )}
              {proxySrc && (
                <img
                  ref={setProxyEl}
                  className="viewer__proxy"
                  src={proxySrc}
                  data-picture={onScreen?.src}
                  data-window={`${crop.x0},${crop.y0},${crop.x1},${crop.y1}`}
                  alt=""
                  aria-hidden="true"
                  draggable={false}
                  style={{
                    left: `${(-crop.x0 / cropW) * 100}%`,
                    top: `${(-crop.y0 / cropH) * 100}%`,
                    width: `${100 / cropW}%`,
                    height: `${100 / cropH}%`,
                  }}
                />
              )}
              {comparing && compareSrc && (
                // Before: RapidOne's render with no edit (as Lightroom's), or the camera's own JPEG, over the picture.
                <img className="viewer__img viewer__img--over" src={compareSrc} alt={photo.fileName} draggable={false} />
              )}
            </div>
            {/* Before | After, bottom right (as Retouch's): Before is the photo with no edit (as
                Lightroom's), or the camera's JPEG when a right-click on it says so; \ switches. */}
            {!noCompare && cameraPreview?.path === photo.path && preview?.source === "engine" && (
              // Retouch's own Before | After (its classes, so its stylesheet draws both the same), in a
              // box of no size of its own; placed 14px above the bottom panel over the loupe, as
              // Retouch's is above its bar.
              <div className="rapidretouch" style={{ display: "contents" }}>
                <div className="tabs before-after" title="Before / after (\ key)" style={{ bottom: bottomInset + 14 }}>
                  <button
                    className={comparing ? "active" : ""}
                    onClick={() => {
                      if (compareWith === "before") onCompareStart?.();
                      setComparing(true);
                    }}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      onCompareWith?.(compareWith === "camera" ? "before" : "camera");
                    }}
                    title={
                      compareWith === "camera"
                        ? "The camera's own JPEG (\ key). Right-click: the photo with no edit instead"
                        : "The photo with no edit, as Lightroom's Before (\ key). Right-click: the camera's own JPEG instead"
                    }
                  >
                    {compareWith === "camera" ? "Camera" : "Before"}
                  </button>
                  <button className={comparing ? "" : "active"} onClick={() => setComparing(false)} title="The photo as edited (\ key)">
                    After
                  </button>
                </div>
              </div>
            )}
            {/* Zoomed in: the photo at its zoom, clipped to the view; drag or scroll to pan. */}
            {photo && (
              <div
                className="viewer__zoomlayer"
                // Hidden by opacity, not visibility: WebKit keeps painting it, so it's up to date
                // (after D65 Glow or the window changes the Fit size) when the first zoom shows it.
                // Windows' WebView2 (Chromium) skips painting what's fully transparent, so the first
                // few zooms showed it blank for a frame (a black flicker, 24 Oct): there it's 1%, the
                // same picture exactly over the photo, unseen but painted.
                style={{ height: areaH, ...(placed ? undefined : { opacity: CHROMIUM ? 0.01 : 0, pointerEvents: "none" }) }}
                aria-hidden={placed ? undefined : true}
                onPointerDown={onZoomPointerDown}
                onPointerMove={onZoomPointerMove}
                onPointerUp={onZoomPointerUp}
                onPointerCancel={onZoomPointerUp}
                onDoubleClick={onZoomDoubleClick}
              >
                <img
                  ref={setZoomImgEl}
                  className={`viewer__img${onScreen?.pending ? " viewer__img--pending" : ""}`}
                  // The loupe's own picture (onScreen), whose shape the frame has: a newer render not yet
                  // decoded, of another shape (a crop over a stand-in), would show stretched for a moment.
                  src={comparing && compareSrc ? compareSrc : (onScreen?.src ?? photo.thumbBase64)}
                  alt=""
                  draggable={false}
                  style={{
                    ...zoomedStyle,
                    filter: onScreen && !onScreen.pending && !comparing ? roughFilter(values, tablesActive, onScreen.engine) : undefined,
                    clipPath,
                  }}
                />
                {detailShown && Math.abs(detailExposure) > 1e-6 && (
                  <TablesFilter id="rapidone-detail-tables" exposure={detailExposure} curves={DEFAULT_CURVES} grade={DEFAULT_GRADE} />
                )}
                {/* The sharp detail is the edited photo's: not while Before is held (it would cover it).
                    The previous patch stays under the new one only where they cover the same part (the
                    new one fading in over it): from another zoom or place it stuck out round the new
                    one, an odd square, at an older exposure after a change (24 Oct). */}
                {underDetail.current && placed && !comparing && detailShown && sameRect(underDetail.current.rect, detailShown.rect) && (
                  <img
                    className="viewer__detail"
                    src={underDetail.current.src}
                    alt=""
                    draggable={false}
                    aria-hidden="true"
                    style={{
                      left: placed.left + underDetail.current.rect[0] * placed.w,
                      top: placed.top + underDetail.current.rect[1] * placed.h,
                      width: (underDetail.current.rect[2] - underDetail.current.rect[0]) * placed.w,
                      height: (underDetail.current.rect[3] - underDetail.current.rect[1]) * placed.h,
                    }}
                  />
                )}
                {detailShown && placed && !comparing && (
                  <img
                    key={detailShown.src.length + detailShown.src.slice(-32)}
                    className={`viewer__detail${underDetail.current ? " viewer__img--fade" : ""}`}
                    src={detailShown.src}
                    alt=""
                    draggable={false}
                    style={{
                    filter: Math.abs(detailExposure) > 1e-6 ? "url(#rapidone-detail-tables)" : undefined,
                      left: placed.left + detailShown.rect[0] * placed.w,
                      top: placed.top + detailShown.rect[1] * placed.h,
                      width: (detailShown.rect[2] - detailShown.rect[0]) * placed.w,
                      height: (detailShown.rect[3] - detailShown.rect[1]) * placed.h,
                    }}
                  />
                )}
                {proxySrc && (
                  <img
                    ref={setZoomProxyEl}
                    className="viewer__proxy"
                    src={proxySrc}
                    data-picture={onScreen?.src}
                    data-window={`${crop.x0},${crop.y0},${crop.x1},${crop.y1}`}
                    alt=""
                    aria-hidden="true"
                    draggable={false}
                    style={{
                      left: layerPlace.left - (crop.x0 / cropW) * layerPlace.w,
                      top: layerPlace.top - (crop.y0 / cropH) * layerPlace.h,
                      width: layerPlace.w / cropW,
                      height: layerPlace.h / cropH,
                    }}
                  />
                )}
              </div>
            )}
            {/* The zoom controls live in the top bar (Fit, and the percentage), not over the photo. */}
            {zoomable &&
              zoomSlot &&
              createPortal(
                <ZoomControls fit={!zoom} percent={zoomPercent} onFit={zoomFit} onPick={zoomPick} />,
                zoomSlot,
              )}
            {removing && preview?.path === photo.path && !preview.pending && removeOverlay?.(placed && overlayZooms ? (windowed ? zoomProxyEl : zoomImgEl) : windowed ? proxyEl : imgEl)}
            {cropping && preview?.path === photo.path && !preview.pending && (
              <>
                <CropOverlay
                  image={imgEl}
                  width={pw}
                  height={ph}
                  crop={crop}
                  ratio={aspectRatio(cropAspect, pw, ph)}
                  onChange={onCrop}
                />
                <div ref={cropBar.ref} className="cropbar" style={cropBar.style}>
                  <BarGrip bar={cropBar} />
                  <span className="cropbar__what">Crop</span>
                  <label className="cropbar__straighten" title="Straighten: turn the photo (double-click to reset)">
                    Straighten
                    <HSlider
                      min={-45}
                      max={45}
                      step={0.1}
                      value={crop.angle}
                      neutral={0}
                      width={140}
                      aria-label="Straighten"
                      onChange={(angle) => {
                        const next = fitCrop({ ...crop, angle }, pw, ph);
                        const r = aspectRatio(cropAspect, pw, ph);
                        onCrop(r === null ? next : cropToRatio(next, r, pw, ph));
                      }}
                    />
                    <span className="cropbar__angle">
                      {crop.angle > 0 ? "+" : ""}
                      {crop.angle.toFixed(1)}°
                    </span>
                  </label>
                  <div className="cropbar__aspects">
                    {CROP_ASPECTS.map((a) => (
                      <button
                        key={a}
                        type="button"
                        className={cropAspect === a ? "is-on" : undefined}
                        onClick={() => {
                          onCropAspect(a);
                          const r = aspectRatio(a, pw, ph);
                          if (r !== null) onCrop(cropToRatio(crop, r, pw, ph));
                        }}
                      >
                        {a}
                      </button>
                    ))}
                  </div>
                  <button
                    type="button"
                    onClick={() => onCrop({ ...NO_CROP, quarter: (crop.quarter + 1) % 4 })}
                    title="Turn 90° clockwise, for a photo that opened sideways (starts the crop afresh)"
                  >
                    <RotateCw size={14} /> 90°
                  </button>
                  <button type="button" onClick={() => onCrop(NO_CROP)} title="Back to the whole photo: no crop, straighten or 90° turns">
                    <RotateCcw size={14} /> Reset
                  </button>
                  <button type="button" className="cropbar__done" onClick={onCropDone} title="Done (Enter or Esc)">
                    Done
                  </button>
                </div>
              </>
            )}
            {/* The file and camera line sits above the panel's tick-box row (Names, D65 Glow) on the
                loupe's bottom edge, not behind it. */}
            <div className="viewer__meta" style={{ bottom: bottomInset + (bottomInset > 0 ? 34 : 6) }}>
              {/* All on the left: the file, its camera, which of how many, and (when it isn't the edited
                  photo itself) what's shown; the right is Before | After's. */}
              <span>
                {photo.fileName}
                {preview?.path === photo.path && preview.camera && ` · ${preview.camera}`}
                {` · ${index + 1} / ${total}`}
                {comparing
                  ? compareWith === "camera" || beforePreview?.path !== photo.path
                    ? compareWith === "camera" ? " · Camera JPEG" : " · Before (rendering…)"
                    : " · Before (no edit)"
                  : preview?.path !== photo.path
                    ? " · Loading…"
                    : preview.source === "engine"
                      ? ""
                      : " · Camera JPEG (engine can't read this file yet)"}
              </span>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
