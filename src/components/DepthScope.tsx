import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Box, RotateCcw } from "lucide-react";
import { depthAt as depthAtPoint, sceneInfo, type SceneInfo } from "../api";
import { scopeCloud } from "../scope/cache";
import { type ScopeCloud, pointAt, uvAt } from "../scope/cloud";
import { ScopeRenderer, type ScopeFrame, viewFovY } from "../scope/renderer";
import {
  MAX_PITCH,
  type Orbit,
  type Vec3,
  clamp,
  controlOfMetres,
  damp,
  dampVec3,
  depthDragZ,
  frameHalfSize,
  metresOfControl,
  near,
  orbitDrag,
  panOrbit,
  photoView,
  projectToScreen,
  zoomOrbit,
} from "../scope/math";

/** The near and far planes (control depth 0..1, 1 = nearest), their fade, the focus plane, and
 *  Mist's start plane. */
export interface ScopePlanes {
  near: number;
  far: number;
  fadeNear?: number;
  fadeFar?: number;
  focus?: number;
  /** Mist's near limit (its End, control depth) when Mist is on. */
  fog?: number;
  /** Mist's far limit (control depth) when it has one: shown, not dragged. */
  fogFar?: number;
  /** Mist's height fog strength, 0..1. */
  fogHeight?: number;
}

/** A Relight light as the scope draws it: where it sits (u, v, depth, or the sun's direction) and
 *  its colour. `selected` draws it larger. */
export interface ScopeLight {
  id: number;
  kind: "point" | "spot";
  u: number;
  v: number;
  depth: number;
  azimuth: number;
  elevation: number;
  color: [number, number, number];
  selected: boolean;
}

export interface DepthScopeProps {
  /** The open photo (its scene geometry is read for it). */
  path: string;
  /** The picked depth mask's band, in the depth bar's units (0..1, 1 = nearest). */
  planes: ScopePlanes;
  /** A plane dragged: the new band, and a key so a whole drag is one undo step (null: its own). */
  onPlanesChange: (planes: ScopePlanes, key: string | null) => void;
  /** A point clicked in 3D: the depth mask's Pick at that depth (Shift: widen, Alt: the near end). */
  onPick?: (control: number, modifiers: { widen: boolean; edge: boolean }) => void;
  /** Where the pointer is on the photo (0..1), while the scope is open: a crosshair in 3D. */
  hoverUV?: { u: number; v: number } | null;
  /** A point hovered in 3D: where it is on the photo, for its marker there (null: none). */
  onHoverUV?: (uv: { u: number; v: number } | null) => void;
  /** Popped out: Redock, or Esc while it has focus (it goes back in its panel). */
  onRedock?: () => void;
  /** The Depth mask's scope has no panel to go back to: Close (or Esc) instead. */
  onClose?: () => void;
  /** Docked: the Undock button. */
  onUndock?: () => void;
  /** In a panel rather than floating: fills its box, no move or resize. */
  docked?: boolean;
  /** Relight's lights, drawn as rings (the selected one larger). */
  lights?: ScopeLight[];
  /** Docked: the panel's own buttons and notes, at the top of the scope's left column (its view
   *  buttons follow), as every Creative panel keeps its buttons down the left. */
  side?: React.ReactNode;
}

/** A Relight light's place in the scope's camera space (metres): a point light on its plane at its
 *  Depth, or a sun out at the far anchor along its direction. */
function lightAt(l: ScopeLight, inf: SceneInfo): [number, number, number] {
  const near = Math.max(0.05, inf.dNear * 0.5);
  const far = Math.max(inf.dFar, inf.dNear) * 1.5;
  const t = Math.max(0, Math.min(1, l.depth / 100));
  const z = near + (far - near) * t;
  const [hw, hh] = frameHalfSize(z, inf.fovMogeDeg, inf.width / Math.max(inf.height, 1));
  return [(l.u - 0.5) * 2 * hw, (l.v - 0.5) * 2 * hh, z];
}

/** Where the panel was last time (its size and place, kept per session). */
interface PanelBox {
  left: number;
  top: number;
  w: number;
  h: number;
}

const BOX_KEY = "rapidone.depthscope.box";
const DEFAULT_W = 520;
const DEFAULT_H = 360;
const MIN_W = 340;
const MIN_H = 260;
/** How close (pixels) a press must be to a plane's outline to grab it. */
const EDGE_PX = 9;

/** What a drag is moving: the view around the scene, the scene across the view, or a plane. */
type DragState =
  | { kind: "orbit"; x: number; y: number; moved: boolean }
  | { kind: "pan"; x: number; y: number; moved: boolean }
  | { kind: "plane"; which: "near" | "far" | "focus" | "fog"; x: number; y: number; z0: number; moved: boolean };

function loadBox(): PanelBox | null {
  try {
    const raw = localStorage.getItem(BOX_KEY);
    if (!raw) return null;
    const b = JSON.parse(raw) as Partial<PanelBox>;
    const num = (v: unknown) => typeof v === "number" && Number.isFinite(v);
    if (!num(b.left) || !num(b.top) || !num(b.w) || !num(b.h)) return null;
    return { left: b.left as number, top: b.top as number, w: Math.max(MIN_W, b.w as number), h: Math.max(MIN_H, b.h as number) };
  } catch {
    // Private mode, a full quota, or something else in the key: the default place is fine.
    return null;
  }
}

function saveBox(box: PanelBox) {
  try {
    localStorage.setItem(BOX_KEY, JSON.stringify(box));
  } catch {
    // Not remembered this session; the scope still works.
  }
}

/** A distance as the scope shows it: "1.8 m", "24 m", "∞". */
function metresText(m: number | null): string {
  if (m === null) return "—";
  if (!Number.isFinite(m)) return "∞";
  return m < 10 ? `${m.toFixed(1)} m` : `${Math.round(m)} m`;
}

/**
 * The Depth Scope: the photo as a 3D point cloud in its true metric shape (MoGe-2's geometry),
 * with the picked depth mask's near and far planes drawn in it. Orbit, pan and zoom to look at
 * the scene from any side; drag a plane (or its label) to move the band; click a point to put the
 * band there, as Pick on the photo does. Hovering the photo marks the matching point here, and
 * hovering a point marks it on the photo.
 *
 * It is a plain WebGL2 component with no knowledge of the masks UI: `planes` in, `onPlanesChange`
 * out, so the Creative tab can dock it later.
 */
export default function DepthScope({ path, planes, onPlanesChange, onPick, hoverUV, onHoverUV, onRedock, onClose, onUndock, docked = false, lights = [], side }: DepthScopeProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const nearLabel = useRef<HTMLDivElement>(null);
  const farLabel = useRef<HTMLDivElement>(null);

  const [info, setInfo] = useState<SceneInfo | null>(null);
  const [cloud, setCloud] = useState<ScopeCloud | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [tint, setTint] = useState(true);
  const [cursor, setCursor] = useState<{ metres: number; control: number } | null>(null);
  const [box, setBox] = useState<PanelBox | null>(() => loadBox());

  const rendererRef = useRef<ScopeRenderer | null>(null);
  // The camera: where it is (eased) and where it's going.
  const orbit = useRef<Orbit>(photoView(1));
  const goal = useRef<Orbit>(photoView(1));
  const planesRef = useRef(planes);
  planesRef.current = planes;
  const lightsRef = useRef(lights);
  lightsRef.current = lights;
  const infoRef = useRef<SceneInfo | null>(null);
  infoRef.current = info;
  const cloudRef = useRef<ScopeCloud | null>(null);
  cloudRef.current = cloud;
  const sizeRef = useRef(size);
  sizeRef.current = size;
  const tintRef = useRef(tint);
  tintRef.current = tint;
  const pathRef = useRef(path);
  pathRef.current = path;

  const dirty = useRef(true);
  const photoPoint = useRef<Vec3 | null>(null);
  const hoverPoint = useRef<Vec3 | null>(null);
  const orbiting = useRef(false);

  /** The band's two ends, nearest first (a mask's near is never below its far). */
  const band = useCallback((): { near: number; far: number; fade: number; focus: number | null; fog: number | null; fogHeight: number } => {
    const p = planesRef.current;
    const a = p.near;
    const b = p.far;
    return { near: Math.max(a, b), far: Math.min(a, b), fade: Math.max(0, Math.min(1, p.fadeNear ?? p.fadeFar ?? 0.1)), focus: p.focus ?? null, fog: p.fog ?? null, fogHeight: Math.max(0, Math.min(1, p.fogHeight ?? 0)) };
  }, []);

  const frameFor = useCallback((): ScopeFrame | null => {
    const inf = infoRef.current;
    const c = cloudRef.current;
    const s = sizeRef.current;
    if (!inf || !c || s.w < 4 || s.h < 4) return null;
    const o = orbit.current;
    const b = band();
    const marker = hoverPoint.current
      ? { pos: hoverPoint.current, color: [0.88, 0.99, 1] as [number, number, number] }
      : photoPoint.current
        ? { pos: photoPoint.current, color: [0.24, 0.9, 1] as [number, number, number] }
        : null;
    return {
      yaw: o.yaw,
      pitch: o.pitch,
      distance: o.distance,
      target: o.target,
      // MoGe-2's own field of view, not EXIF's: the cloud's x and y were built from the pixels
      // with MoGe's focal length, so the view must use the same one to look exactly like the
      // photo (EXIF's, when it disagrees, would stretch the cloud across the frame).
      fovX: inf.fovMogeDeg,
      photoAspect: inf.width / Math.max(inf.height, 1),
      width: s.w,
      height: s.h,
      dpr: Math.min(window.devicePixelRatio || 1, 2),
      near: b.near,
      far: b.far,
      fade: b.fade,
      dNear: inf.dNear,
      dFar: inf.dFar,
      focus: b.focus,
      fogStart: b.fog,
      fogFar: planesRef.current.fogFar ?? null,
      fogHeight: b.fogHeight,
      tint: tintRef.current,
      // The ground grid and the sky ring always show (their button is hidden: Chris, 22 Nov).
      grid: true,
      marker,
      lights: lightsRef.current.map((l) => ({ id: l.id, pos: lightAt(l, inf), color: l.color, selected: l.selected })),
      decimate: orbiting.current,
    };
  }, [band]);

  /** Puts each plane's label where its middle is on screen (hidden when behind the camera). */
  const placeLabels = useCallback(() => {
    const r = rendererRef.current;
    const f = frameFor();
    if (!r || !f) return;
    const inf = infoRef.current;
    if (!inf) return;
    const b = band();
    const mvp = r.lastMvp;
    // Near above the middle, far below: two planes at the same distance would otherwise put their
    // labels in the same place, and from the photo's own viewpoint they always do.
    const put = (el: HTMLDivElement | null, control: number, side: number) => {
      if (!el) return;
      const z = metresOfControl(control, inf.dNear, inf.dFar);
      const depth = Number.isFinite(z) ? z : inf.dFar;
      const [, hh] = frameHalfSize(depth, f.fovX, f.photoAspect);
      const s = projectToScreen(mvp, [0, side * hh * 0.55, depth], f.width, f.height);
      const on = s.inFront && s.z < 1;
      el.style.opacity = on ? "1" : "0";
      el.style.pointerEvents = on ? "auto" : "none";
      if (on) {
        el.style.left = `${s.x}px`;
        el.style.top = `${s.y}px`;
      }
    };
    put(nearLabel.current, b.near, -1);
    put(farLabel.current, b.far, 1);
  }, [band, frameFor]);

  const draw = useCallback(() => {
    const r = rendererRef.current;
    const f = frameFor();
    if (!r || !f) return;
    r.render(f);
    placeLabels();
  }, [frameFor, placeLabels]);
  // The loop below outlives a render, so it calls whatever `draw` is now.
  const drawRef = useRef(draw);
  drawRef.current = draw;

  // --- Drawing: a frame when something changed, and more only while the camera eases ----------
  // Frames are asked for one at a time and the chain ends when the camera has arrived: an idle
  // scope (or one whose window is hidden) costs no frames at all. `schedule` marks the view as
  // changed and starts the chain again.
  const pumpRef = useRef<() => void>(() => {});
  const schedule = useCallback(() => {
    dirty.current = true;
    pumpRef.current();
  }, []);

  useEffect(() => {
    let raf = 0;
    let last = 0;
    const tick = (t: number) => {
      raf = 0;
      const dt = last ? Math.min(0.05, (t - last) / 1000) : 1 / 60;
      last = t;
      const o = orbit.current;
      const g = goal.current;
      const moving = !near(o.yaw, g.yaw) || !near(o.pitch, g.pitch) || !near(o.distance, g.distance) || !near(o.target[0], g.target[0]) || !near(o.target[1], g.target[1]) || !near(o.target[2], g.target[2]);
      if (moving) {
        orbit.current = {
          yaw: damp(o.yaw, g.yaw, dt, 18),
          pitch: damp(o.pitch, g.pitch, dt, 18),
          distance: damp(o.distance, g.distance, dt, 18),
          target: dampVec3(o.target, g.target, dt, 18),
        };
        orbiting.current = true;
      } else if (orbiting.current) {
        // Arrived: one last frame at full quality (the easing drew half the points).
        orbit.current = { ...g };
        orbiting.current = false;
        dirty.current = true;
      }
      if (moving || dirty.current) {
        drawRef.current();
        dirty.current = false;
      }
      // Another frame only while the camera is still on its way; otherwise the chain ends here
      // (and the next change starts it again through `schedule`).
      if (moving) raf = requestAnimationFrame(tick);
      else last = 0;
    };
    const pump = () => {
      if (raf) return;
      if (document.hidden) {
        // Nothing can be seen: skip the animation, put the camera where it was going and draw
        // that one frame, so the view is right whenever the window is looked at again.
        last = 0;
        orbit.current = { ...goal.current, target: [...goal.current.target] as Vec3 };
        orbiting.current = false;
        drawRef.current();
        dirty.current = false;
        return;
      }
      last = 0;
      raf = requestAnimationFrame(tick);
    };
    pumpRef.current = pump;
    pump();
    return () => {
      if (raf) cancelAnimationFrame(raf);
      pumpRef.current = () => {};
    };
  }, []);

  // --- Loading: the scene's facts, and the cloud (once per photo, kept for the session) ------
  useEffect(() => {
    let alive = true;
    setInfo(null);
    setCloud(null);
    setProblem(null);
    photoPoint.current = null;
    hoverPoint.current = null;
    sceneInfo(path)
      .then((i) => {
        if (!alive) return;
        if (!i) {
          setProblem("No scene geometry for this photo yet: MoGe-2 isn't installed, or it couldn't run.");
          return;
        }
        setInfo(i);
      })
      .catch(() => {
        if (alive) setProblem("The photo's scene couldn't be read.");
      });
    scopeCloud(path)
      .then((c) => {
        if (!alive) return;
        if (!c) {
          setProblem("No scene geometry for this photo yet: MoGe-2 isn't installed, or it couldn't run.");
          return;
        }
        setCloud(c);
      })
      .catch(() => {
        if (alive) setProblem("The point cloud couldn't be decoded.");
      });
    return () => {
      alive = false;
    };
  }, [path, schedule]);

  // --- The graphics card: one renderer, its context's loss handled inside --------------------
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const r = new ScopeRenderer(canvas);
    rendererRef.current = r;
    if (!r.ok) setProblem("This computer's viewer doesn't support WebGL2, so the 3D view can't be shown here.");
    return () => {
      r.dispose();
      rendererRef.current = null;
    };
  }, []);

  useEffect(() => {
    rendererRef.current?.setCloud(cloud);
    schedule();
  }, [cloud, schedule]);

  // Each photo starts a little to one side, once its cloud, its lens and the box's size are all known.
  const started = useRef<ScopeCloud | null>(null);
  useEffect(() => {
    if (!cloud || !info || size.w < 4 || started.current === cloud) return;
    started.current = cloud;
    orbit.current = startView(cloud);
    goal.current = orbit.current;
    schedule();
  }, [cloud, info, size, schedule]);

  // --- The body's size: the canvas' pixels, and the projection's aspect ----------------------
  useLayoutEffect(() => {
    const body = bodyRef.current;
    if (!body) return;
    const measure = () => {
      const w = Math.max(1, Math.round(body.clientWidth));
      const h = Math.max(1, Math.round(body.clientHeight));
      setSize((s) => (s.w === w && s.h === h ? s : { w, h }));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(body);
    return () => ro.disconnect();
  }, []);

  // --- What changed, drawn next frame --------------------------------------------------------
  useEffect(() => {
    schedule();
  }, [planes.near, planes.far, planes.fadeNear, planes.fadeFar, planes.focus, planes.fog, planes.fogFar, planes.fogHeight, tint, size, cloud, info, lights, schedule]);

  // --- Hovering the photo: the matching point here, and the cursor's distance ----------------
  const hoverQueue = useRef<{ u: number; v: number } | null>(null);
  const hoverBusy = useRef(false);
  const pumpHover = useCallback(() => {
    if (hoverBusy.current) return;
    const uv = hoverQueue.current;
    const inf = infoRef.current;
    if (!uv || !inf) return;
    hoverQueue.current = null;
    hoverBusy.current = true;
    depthAtPoint(pathRef.current, uv.u, uv.v)
      .then((d) => {
        const now = infoRef.current;
        if (!now) return;
        const metres = d.metres;
        if (d.sky < 0.5 && typeof metres === "number" && Number.isFinite(metres) && metres > 0) {
          // The pixel's ray, at that distance: camera space (x right, y down), then world (y up).
          const fx = Math.max(now.focalPx, 1);
          const x = ((uv.u * now.width - now.width / 2) / fx) * metres;
          const y = ((uv.v * now.height - now.height / 2) / fx) * metres;
          photoPoint.current = [x, -y, metres];
          setCursor({ metres, control: d.control });
        } else {
          photoPoint.current = null;
          setCursor(null);
        }
        schedule();
      })
      .catch(() => {
        photoPoint.current = null;
      })
      .finally(() => {
        hoverBusy.current = false;
        if (hoverQueue.current) pumpHover();
      });
  }, [schedule]);

  useEffect(() => {
    if (!hoverUV) {
      hoverQueue.current = null;
      if (photoPoint.current) {
        photoPoint.current = null;
        schedule();
      }
      return;
    }
    hoverQueue.current = hoverUV;
    pumpHover();
  }, [hoverUV, pumpHover, schedule]);

  // The marker on the photo goes when the scope does (the parent's callback may change each
  // render, so it's held in a ref: a cleanup on the callback's identity would loop).
  const hoverOut = useRef(onHoverUV);
  hoverOut.current = onHoverUV;
  useEffect(
    () => () => {
      hoverOut.current?.(null);
    },
    [],
  );

  // --- Pointer: orbit, pan, planes, pick -----------------------------------------------------
  const drag = useRef<null | DragState>(null);
  const hoverTimer = useRef(0);
  const hoverAt = useRef<{ x: number; y: number } | null>(null);

  const localPoint = (e: { clientX: number; clientY: number }) => {
    const r = canvasRef.current?.getBoundingClientRect();
    return r ? { x: e.clientX - r.left, y: e.clientY - r.top } : null;
  };

  /** Which plane's outline a press is on (or near), if any. */
  const planeAt = (x: number, y: number): "near" | "far" | "focus" | "fog" | null => {
    const inf = infoRef.current;
    const r = rendererRef.current;
    const f = frameFor();
    if (!inf || !r || !f) return null;
    const b = band();
    const mvp = r.lastMvp;
    const hit = (control: number): boolean => {
      const z = metresOfControl(control, inf.dNear, inf.dFar);
      if (!Number.isFinite(z)) return false;
      const [hw, hh] = frameHalfSize(z, f.fovX, f.photoAspect);
      // The press near one of the four edges, in pixels (from the side the plane is only a line).
      const corners: Vec3[] = [
        [-hw, -hh, z],
        [hw, -hh, z],
        [hw, hh, z],
        [-hw, hh, z],
      ];
      const s = corners.map((c) => projectToScreen(mvp, c, f.width, f.height));
      let best = Infinity;
      for (let i = 0; i < 4; i++) {
        const a = s[i];
        const c = s[(i + 1) % 4];
        if (!a.inFront || !c.inFront) continue;
        best = Math.min(best, segmentDistance(x, y, a.x, a.y, c.x, c.y));
      }
      return best <= EDGE_PX;
    };
    // The nearer plane wins a press that touches both (it's the one in front).
    const candidates: ["near" | "far" | "focus" | "fog", number][] = [
      ["near", b.near],
      ["far", b.far],
    ];
    // The focus plane only exists when the caller gives one (the Creative tab), and it is only
    // grabbed where it isn't the same plane as near or far (a zero focus range).
    if (b.focus !== null && Math.abs(b.focus - b.near) > 1e-4 && Math.abs(b.focus - b.far) > 1e-4) {
      candidates.push(["focus", b.focus]);
    }
    if (b.fog !== null && Math.abs(b.fog - b.near) > 1e-4 && Math.abs(b.fog - b.far) > 1e-4 && (b.focus === null || Math.abs(b.fog - b.focus) > 1e-4)) {
      candidates.push(["fog", b.fog]);
    }
    let found: "near" | "far" | "focus" | "fog" | null = null;
    let nearestZ = Infinity;
    for (const [which, control] of candidates) {
      if (!hit(control)) continue;
      const z = metresOfControl(control, inf.dNear, inf.dFar);
      if (z < nearestZ) {
        nearestZ = z;
        found = which;
      }
    }
    return found;
  };

  const startPlane = (which: "near" | "far" | "focus" | "fog", e: { clientX: number; clientY: number; pointerId?: number }, target?: EventTarget | null) => {
    const p = localPoint(e);
    const inf = infoRef.current;
    if (!p || !inf) return;
    const b = band();
    const control = which === "near" ? b.near : which === "far" ? b.far : which === "focus" ? (b.focus ?? b.near) : (b.fog ?? b.near);
    const z0 = metresOfControl(control, inf.dNear, inf.dFar);
    if (!Number.isFinite(z0)) return;
    drag.current = { kind: "plane", which, x: e.clientX, y: e.clientY, z0, moved: false };
    const el = target instanceof HTMLElement ? target : canvasRef.current;
    if (e.pointerId !== undefined && el) el.setPointerCapture(e.pointerId);
  };

  const onPointerDown = (e: React.PointerEvent) => {
    e.stopPropagation();
    rootRef.current?.focus();
    const p = localPoint(e);
    if (!p) return;
    // Right or middle drag pans, and so does Ctrl-drag (⌘ on a Mac). Shift and Alt are left for
    // the click's Pick, as on the photo (Shift: widen, Alt: the near end).
    if (e.button === 2 || e.button === 1 || (e.button === 0 && (e.ctrlKey || e.metaKey))) {
      drag.current = { kind: "pan", x: e.clientX, y: e.clientY, moved: false };
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      return;
    }
    if (e.button !== 0) return;
    const which = planeAt(p.x, p.y);
    if (which) {
      startPlane(which, e, e.currentTarget);
      return;
    }
    drag.current = { kind: "orbit", x: e.clientX, y: e.clientY, moved: false };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: React.PointerEvent) => {
    e.stopPropagation();
    const d = drag.current;
    if (d) {
      const dx = e.clientX - d.x;
      const dy = e.clientY - d.y;
      if (!d.moved && Math.hypot(dx, dy) > 3) d.moved = true;
      if (d.kind === "orbit") {
        goal.current = orbitDrag(goal.current, dx, dy);
        d.x = e.clientX;
        d.y = e.clientY;
        schedule();
      } else if (d.kind === "pan") {
        const f = frameFor();
        if (f) goal.current = panOrbit(goal.current, dx, dy, f.height, fovYOf(f));
        d.x = e.clientX;
        d.y = e.clientY;
        schedule();
      } else {
        // A plane: along the axis away from the camera, so its label follows the pointer.
        const f = frameFor();
        const r = rendererRef.current;
        const inf = infoRef.current;
        if (!f || !r || !inf) return;
        const z = depthDragZ(r.lastMvp, f.width, f.height, d.z0, dx, dy);
        const control = controlOfMetres(z, inf.dNear, inf.dFar);
        const b = band();
        const next =
          d.which === "near"
            ? { ...b, near: Math.max(control, b.far + 0.002) }
            : d.which === "far"
              ? { ...b, far: Math.min(control, b.near - 0.002) }
              : d.which === "focus"
                ? { ...b, focus: control }
                : { ...b, fog: control };
        onPlanesChange(
          {
            near: next.near,
            far: next.far,
            fadeNear: next.fade,
            fadeFar: next.fade,
            focus: next.focus ?? undefined,
            fog: next.fog ?? undefined,
            fogHeight: next.fogHeight,
          },
          `scope:${d.which}`,
        );
      }
      return;
    }
    // Not dragging: the point under the pointer, at most a few times a second (a read-back
    // stops the card, so it's not done on every frame of a sweep).
    hoverAt.current = localPoint(e);
    if (!hoverTimer.current) {
      hoverTimer.current = window.setTimeout(() => {
        hoverTimer.current = 0;
        const at = hoverAt.current;
        const f = frameFor();
        const r = rendererRef.current;
        const c = cloudRef.current;
        if (!at || !f || !r || !c) return;
        // Only over the cloud itself: the readout strip and the header aren't part of the view.
        if (at.x < 0 || at.y < 0 || at.x > f.width || at.y > f.height) {
          if (hoverPoint.current) {
            hoverPoint.current = null;
            onHoverUV?.(null);
            schedule();
          }
          return;
        }
        const id = r.pick(at.x, at.y, f);
        if (id >= 0) {
          hoverPoint.current = pointAt(c, id);
          const [u, v] = uvAt(c, id);
          onHoverUV?.({ u, v });
          setCursor({ metres: hoverPoint.current[2], control: controlOfMetres(hoverPoint.current[2], f.dNear, f.dFar) });
        } else {
          hoverPoint.current = null;
          onHoverUV?.(null);
        }
        schedule();
      }, 40);
    }
  };

  const onPointerUp = (e: React.PointerEvent) => {
    e.stopPropagation();
    const d = drag.current;
    drag.current = null;
    if (!d || d.kind === "plane" || d.moved) return;
    if (d.kind !== "orbit") return;
    // A click on the cloud: the depth mask's Pick there.
    const p = localPoint(e);
    const f = frameFor();
    const r = rendererRef.current;
    const c = cloudRef.current;
    if (!p || !f || !r || !c) return;
    const id = r.pick(p.x, p.y, f);
    if (id < 0) return;
    const z = pointAt(c, id)[2];
    onPick?.(controlOfMetres(z, f.dNear, f.dFar), { widen: e.shiftKey, edge: e.altKey });
  };

  // The wheel zooms. A native listener, not React's: React's is passive, so it can't stop the
  // picture behind the panel from zooming too.
  useEffect(() => {
    const c = canvasRef.current;
    if (!c) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const base = cloudRef.current?.medianZ ?? 1;
      goal.current = zoomOrbit(goal.current, e.deltaY, Math.max(0.05, base * 0.02), Math.max(50, base * 60));
      schedule();
    };
    c.addEventListener("wheel", onWheel, { passive: false });
    return () => c.removeEventListener("wheel", onWheel);
  }, [schedule]);

  // Popped out, the scope takes the keyboard: Esc redocks it and R resets the view straight away.
  // Docked it's always there, so it only takes it when it's clicked.
  useEffect(() => {
    if (!docked) rootRef.current?.focus({ preventScroll: true });
  }, [docked]);

  const resetView = useCallback(() => {
    const c = cloudRef.current;
    if (!c) return;
    goal.current = startView(c);
    schedule();
  }, [schedule]);

  /** A view from a fixed side: the photo's own, from the side, or from above. */
  const viewFrom = (which: "photo" | "side" | "top") => {
    const c = cloudRef.current;
    if (!c) return;
    if (which === "photo") {
      goal.current = photoView(c.medianZ);
      schedule();
      return;
    }
    const mid = c.medianZ;
    goal.current =
      which === "side"
        ? infoRef.current
          ? sideView(c, infoRef.current, sizeRef.current)
          : goal.current
        : { yaw: 0, pitch: -MAX_PITCH * 0.95, distance: Math.max(mid, c.zMax * 0.5), target: [0, 0, mid] };
    schedule();
  };

  const leave = onRedock ?? onClose;
  const onKeyDown = (e: React.KeyboardEvent) => {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
    if (e.key === "Escape" && !docked) {
      e.preventDefault();
      e.stopPropagation();
      leave?.();
    } else if (e.key === "r" || e.key === "R") {
      e.preventDefault();
      e.stopPropagation();
      resetView();
    }
  };

  // --- The panel: move, resize ------------------------------------------------------
  const bodySize = () => {
    const host = rootRef.current?.offsetParent as HTMLElement | null;
    return { w: host?.clientWidth ?? 0, h: host?.clientHeight ?? 0 };
  };

  /** The head's own buttons: the pointer isn't captured for a move there, or their click goes to the head. */
  const onButton = (e: React.SyntheticEvent) => e.target instanceof Element && e.target.closest("button") !== null;

  const startMove = (e: React.PointerEvent) => {
    if (e.button !== 0 || onButton(e)) return;
    e.stopPropagation();
    const start = box ?? defaultBox();
    const sx = e.clientX;
    const sy = e.clientY;
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const host = bodySize();
      const left = clamp(start.left + ev.clientX - sx, -start.w + 80, Math.max(0, host.w - 80));
      const top = clamp(start.top + ev.clientY - sy, 0, Math.max(0, host.h - 40));
      setBox({ ...start, left, top });
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      setBox((b) => {
        if (b) saveBox(b);
        return b;
      });
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  };

  const startResize = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    const start = box ?? defaultBox();
    const sx = e.clientX;
    const sy = e.clientY;
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const host = bodySize();
      const w = clamp(start.w + ev.clientX - sx, MIN_W, Math.max(MIN_W, host.w - start.left));
      const h = clamp(start.h + ev.clientY - sy, MIN_H, Math.max(MIN_H, host.h - start.top));
      setBox({ ...start, w, h });
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      setBox((b) => {
        if (b) saveBox(b);
        return b;
      });
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  };

  const defaultBox = (): PanelBox => {
    const host = bodySize();
    // Below the masks' floating bar at the top of the photo, so it doesn't cover it.
    const top = 64;
    return {
      left: Math.max(8, Math.round((host.w - DEFAULT_W) / 2)),
      top,
      w: Math.min(DEFAULT_W, Math.max(MIN_W, host.w - 16)),
      h: Math.min(DEFAULT_H, Math.max(MIN_H, host.h - top - 12)),
    };
  };

  // A panel kept from a bigger window: pulled back inside.
  useLayoutEffect(() => {
    if (docked) return;
    const host = bodySize();
    if (!host.w || !box) return;
    const left = clamp(box.left, -box.w + 80, Math.max(0, host.w - 80));
    const top = clamp(box.top, 0, Math.max(0, host.h - 40));
    if (left !== box.left || top !== box.top) setBox({ ...box, left, top });
  }, [box, docked]);

  // First open (or nothing kept): its own place, inside the viewer.
  useLayoutEffect(() => {
    if (!docked && !box) setBox(defaultBox());
  }, [box, docked]);

  // The hover's timer goes when the scope does.
  useEffect(
    () => () => {
      if (hoverTimer.current) window.clearTimeout(hoverTimer.current);
    },
    [],
  );

  const shown = box ?? null;
  const style: React.CSSProperties | undefined = docked
    ? undefined
    : shown
      ? { left: shown.left, top: shown.top, width: shown.w, height: shown.h }
      : undefined;

  const inf = info;
  const b = band();
  const nearZ = inf ? metresOfControl(b.near, inf.dNear, inf.dFar) : null;
  const farZ = inf ? metresOfControl(b.far, inf.dNear, inf.dFar) : null;
  // Far minus near: the band's depth is how much further the far plane is (control depth runs
  // the other way round, 1 = nearest, so nearZ is the smaller distance).
  const thickness = nearZ !== null && farZ !== null && Number.isFinite(nearZ) && Number.isFinite(farZ) ? farZ - nearZ : null;
  const lens = inf ? [inf.focalLengthMm ? `${Math.round(inf.focalLengthMm)} mm` : inf.focal35mm ? `${Math.round(inf.focal35mm)} mm (eq)` : null, inf.fNumber ? `f/${Number(inf.fNumber.toFixed(1))}` : null].filter(Boolean).join(" · ") : "";

  return (
    <div
      className={`depthscope${docked ? " depthscope--docked" : ""}`}
      ref={rootRef}
      style={style}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onPointerDown={(e) => e.stopPropagation()}
      onPointerMove={(e) => {
        e.stopPropagation();
        onPointerMove(e);
      }}
      onPointerUp={(e) => {
        e.stopPropagation();
        onPointerUp(e);
      }}
      onPointerCancel={() => {
        drag.current = null;
      }}
    >
      {docked ? (
        <>
          <div className="creative-side creative-side--two depthscope__side">
            {side}
            <button type="button" onClick={() => viewFrom("photo")} title="The photo's own viewpoint">
              Photo
            </button>
            <button type="button" onClick={() => viewFrom("side")} title="Look at the scene from the side">
              Side
            </button>
            <button type="button" onClick={() => viewFrom("top")} title="Look at the scene from above">
              Top
            </button>
            <button type="button" className={tint ? "is-on" : undefined} onClick={() => setTint((v) => !v)} title="Tint the points in the band and dim the rest">
              Tint
            </button>
            <button type="button" onClick={resetView} title="Back to where it started (R, or double-click)">
              Reset
            </button>
            <button type="button" className="btn-undock" onClick={onUndock} title="Float the Depth Scope over the photo">
              Undock
            </button>
            <span className="creative-side__note">{inf ? `${inf.model} · ${inf.ep}` : ""}</span>
          </div>
          <div className="depthscope__main">
      <div className="depthscope__body" ref={bodyRef}>
        <canvas
          ref={canvasRef}
          className="depthscope__canvas"
          title="Drag to orbit · right-drag (or Ctrl-drag) to pan · the wheel to zoom · double-click to go back to where it started"
          onPointerDown={onPointerDown}
          onPointerLeave={() => {
            hoverAt.current = null;
            if (hoverPoint.current) {
              hoverPoint.current = null;
              onHoverUV?.(null);
              schedule();
            }
          }}
          onDoubleClick={resetView}
          onContextMenu={(e) => {
            // The right drag pans: not its panel's menu.
            e.preventDefault();
            e.stopPropagation();
          }}
        />
        <div className="depthscope__label depthscope__label--near" ref={nearLabel} onPointerDown={(e) => startPlane("near", e, e.currentTarget)} title="Near plane: drag to move it">
          {metresText(nearZ)}
        </div>
        <div className="depthscope__label depthscope__label--far" ref={farLabel} onPointerDown={(e) => startPlane("far", e, e.currentTarget)} title="Far plane: drag to move it">
          {metresText(farZ)}
        </div>
        {problem && <div className="depthscope__note">{problem}</div>}
      </div>
      <div className="depthscope__readout">
        <span title="The depth mask's band">
          <b>Near</b> {metresText(nearZ)} <span className="depthscope__dash">–</span> <b>Far</b> {metresText(farZ)}
        </span>
        <span title="How deep the band is">{thickness !== null ? `${metresText(thickness)} deep` : ""}</span>
        {lens && !docked && <span title="The lens, from the photo's EXIF">{lens}</span>}
        <span className="depthscope__cursor" title="The distance under the pointer">
          {cursor ? metresText(cursor.metres) : "—"}
        </span>
        {!docked && (
          <button type="button" className="depthscope__reset" onClick={resetView} title="Back to where it started (R, or double-click)">
            <RotateCcw size={11} /> Reset
          </button>
        )}
      </div>
          </div>
        </>
      ) : (
        <>
      <div className="depthscope__head" onPointerDown={docked ? undefined : startMove} onDoubleClick={docked ? undefined : (e) => !onButton(e) && setBox(() => { const d = defaultBox(); saveBox(d); return d; })}>
        <Box size={13} />
        <span className="depthscope__title">Depth Scope</span>
        <span className="depthscope__meta">{inf ? `${inf.model} · ${inf.ep}` : ""}</span>
        <div className="depthscope__tools">
          <button type="button" onClick={() => viewFrom("photo")} title="The photo's own viewpoint">
            Photo
          </button>
          <button type="button" onClick={() => viewFrom("side")} title="Look at the scene from the side">
            Side
          </button>
          <button type="button" onClick={() => viewFrom("top")} title="Look at the scene from above">
            Top
          </button>
          <button type="button" className={tint ? "is-on" : undefined} onClick={() => setTint((v) => !v)} title="Tint the points in the band and dim the rest">
            Tint
          </button>
          <button type="button" className={docked ? "btn-undock" : onRedock ? "btn-redock" : undefined} onClick={docked ? onUndock : leave} title={docked ? "Float the Depth Scope over the photo" : onRedock ? "Put the Depth Scope back in its panel (Esc)" : "Close the Depth Scope (Esc)"}>
            {docked ? "Undock" : onRedock ? "Redock" : "Close"}
          </button>
        </div>
      </div>
      <div className="depthscope__body" ref={bodyRef}>
        <canvas
          ref={canvasRef}
          className="depthscope__canvas"
          title="Drag to orbit · right-drag (or Ctrl-drag) to pan · the wheel to zoom · double-click to go back to where it started"
          onPointerDown={onPointerDown}
          onPointerLeave={() => {
            hoverAt.current = null;
            if (hoverPoint.current) {
              hoverPoint.current = null;
              onHoverUV?.(null);
              schedule();
            }
          }}
          onDoubleClick={resetView}
          onContextMenu={(e) => {
            // The right drag pans: not its panel's menu.
            e.preventDefault();
            e.stopPropagation();
          }}
        />
        <div className="depthscope__label depthscope__label--near" ref={nearLabel} onPointerDown={(e) => startPlane("near", e, e.currentTarget)} title="Near plane: drag to move it">
          {metresText(nearZ)}
        </div>
        <div className="depthscope__label depthscope__label--far" ref={farLabel} onPointerDown={(e) => startPlane("far", e, e.currentTarget)} title="Far plane: drag to move it">
          {metresText(farZ)}
        </div>
        {problem && <div className="depthscope__note">{problem}</div>}
      </div>
      <div className="depthscope__readout">
        <span title="The depth mask's band">
          <b>Near</b> {metresText(nearZ)} <span className="depthscope__dash">–</span> <b>Far</b> {metresText(farZ)}
        </span>
        <span title="How deep the band is">{thickness !== null ? `${metresText(thickness)} deep` : ""}</span>
        {lens && !docked && <span title="The lens, from the photo's EXIF">{lens}</span>}
        <span className="depthscope__cursor" title="The distance under the pointer">
          {cursor ? metresText(cursor.metres) : "—"}
        </span>
        {!docked && (
          <button type="button" className="depthscope__reset" onClick={resetView} title="Back to where it started (R, or double-click)">
            <RotateCcw size={11} /> Reset
          </button>
        )}
      </div>
        </>
      )}
      {!docked && <div className="depthscope__grip" onPointerDown={startResize} title="Drag to resize" />}
    </div>
  );
}

// --- Small helpers the camera maths needs here ------------------------------------------------

/** Where each photo starts: the picture turned 15 degrees, filling 80% of the view. */
function startView(c: ScopeCloud): Orbit {
  return { yaw: Math.PI / 12, pitch: 0, distance: c.medianZ / 0.8, target: [0, 0, c.medianZ] };
}

/** The scene from its side, where the planes are edge-on and their distances easy to read: far
 *  enough back to see from the nearest point to well past the middle distance. */
function sideView(c: ScopeCloud, info: SceneInfo, size: { w: number; h: number }): Orbit {
  const aspect = size.w / Math.max(size.h, 1);
  const halfV = Math.tan((viewFovY(info.fovMogeDeg, info.width / Math.max(info.height, 1), aspect) * Math.PI) / 360);
  const nearZ = c.zMin * 0.8;
  const farZ = Math.max(nearZ + 0.1, Math.min(c.zMax, info.dFar * 1.05));
  const mid = (nearZ + farZ) / 2;
  const [hw, hh] = frameHalfSize(mid, info.fovMogeDeg, info.width / Math.max(info.height, 1));
  // The scene's near side is that much closer to the camera than its middle.
  const distance = hw + Math.max((farZ - nearZ) / 2 / (halfV * aspect), hh / halfV) * 1.1;
  return { yaw: Math.PI / 2, pitch: 0, distance, target: [0, 0, mid] };
}

/** The canvas' vertical field of view: the one the renderer's camera draws with. */
function fovYOf(f: ScopeFrame): number {
  return viewFovY(f.fovX, f.photoAspect, f.width / Math.max(f.height, 1));
}

/** The distance from (px, py) to the segment (ax, ay)-(bx, by), in pixels. */
function segmentDistance(px: number, py: number, ax: number, ay: number, bx: number, by: number): number {
  const dx = bx - ax;
  const dy = by - ay;
  const len2 = dx * dx + dy * dy;
  const t = len2 > 1e-9 ? clamp(((px - ax) * dx + (py - ay) * dy) / len2, 0, 1) : 0;
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy));
}
