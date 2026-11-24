import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { sceneInfo, type SceneInfo } from "../api";
import { scopeCloud } from "../scope/cache";
import type { ScopeCloud } from "../scope/cloud";
import { ScopeRenderer, viewFovY } from "../scope/renderer";
import {
  type Orbit,
  type Vec3,
  clamp,
  cross,
  dot,
  fovYFromFovX,
  frameHalfSize,
  normalize,
  orbitCamera,
  orbitDrag,
  panOrbit,
  photoView,
  projectToScreen,
  rotateAround,
  zoomOrbit,
} from "../scope/math";
import { isCelestial, kelvinSwatch, LIGHT_TYPES, moonAmbient, skyAmbient, sunAmbient, sunKelvin, typeOf, withLightType, type Light } from "../creative";
import LightIcon from "./LightIcon";
import { Plus, ZoomIn, ZoomOut } from "lucide-react";
import { buildRig, centroid, faceNormal, frameOf, shade } from "../scope/rig";

/**
 * Relight's own 3D space: the photo's point cloud and the lighting rig. Every light is drawn at its
 * real size as the thing its type names (`scope/rig.ts`: softbox, strip box, octabox, umbrella,
 * beauty dish, snoot with a grid, bare head, speedlight with a bounce card) on a stand. Moving a
 * light is moving its stand on the studio floor: drag sideways / up the screen to slide it sideways /
 * away, whatever the view; Shift-drag, the green knob on its pole or the wheel over it set its
 * height, never below the floor. The selected light's beam ends in an aim ring: drag it to turn the
 * light; double-click aims it at the middle. The toolbar picks, adds, retypes and removes lights;
 * the corner buttons zoom; Undock floats the whole thing over the photo.
 */
export interface RelightScopeProps {
  path: string | null;
  lights: Light[];
  selected: number | null;
  onSelect: (id: number) => void;
  /** A light moved or turned: the same fields the rest of the tool stores. */
  onMove: (
    id: number,
    patch: { u?: number; v?: number; depth?: number; aimU?: number; aimV?: number; yaw?: number; pitch?: number },
    last: boolean,
  ) => void;
  /** Any other change to a light (its type, on/off), as the panel makes it. */
  onEdit: (id: number, patch: Partial<Light>, key?: string | null) => void;
  /** A new light of a `LIGHT_TYPES` type, at a place (and turned to the subject) when the scene
   *  is known; it becomes the selected one. */
  onAdd: (typeId: string, place?: Partial<Light>) => void;
  onRemove: (id: number) => void;
  /** The photo's white balance as set (Kelvin): a new lamp starts at it, so it reads as white in
   *  this photo rather than cold or warm against it. */
  photoKelvin?: number;
  /** Relight's Ambient, which follows the sun's height while it's left at the sun's own value. */
  ambient?: number;
  onAmbient?: (ambient: number, key: string) => void;
  docked?: boolean;
  onUndock?: () => void;
  onRedock?: () => void;
}

/** A light's stand in the scope's world (metres, y up). Depth 50 is the scene's middle (the
 *  geometric mean of the anchors, as `relight.rs` uses); 0 is at the camera, 100 as far behind the
 *  middle again. */
function standOf(l: Light, info: SceneInfo): Vec3 {
  const focus = Math.sqrt(Math.max(0.05, info.dNear) * Math.max(info.dFar, info.dNear));
  const t = clamp(l.depth / 100, 0, 1);
  const half = Math.max(0.02, focus - 0.02);
  const z = Math.max(0.02, focus + (t - 0.5) * 2 * half);
  const [hw, hh] = frameHalfSize(z, info.fovMogeDeg, info.width / Math.max(info.height, 1));
  return [(l.u - 0.5) * 2 * hw, -(l.v - 0.5) * 2 * hh, z];
}

/** The aim point's world place: the dot where the stand's arrow points (older files' way). */
function aimOf(l: Light, info: SceneInfo, z: number): Vec3 {
  const [hw, hh] = frameHalfSize(z, info.fovMogeDeg, info.width / Math.max(info.height, 1));
  return [(l.aimU - 0.5) * 2 * hw, -(l.aimV - 0.5) * 2 * hh, z];
}

/** The direction a point or spot faces: its own yaw/pitch if it has them, else its aim point. */
function facingOf(l: Light, info: SceneInfo, stand: Vec3, sceneZ: number): Vec3 {
  if (l.yaw !== undefined && l.pitch !== undefined) {
    const y = (l.yaw * Math.PI) / 180;
    const p = (l.pitch * Math.PI) / 180;
    return [Math.cos(p) * Math.sin(y), Math.sin(p), Math.cos(p) * Math.cos(y)];
  }
  const a = aimOf(l, info, Math.max(stand[2], sceneZ));
  const d: Vec3 = [a[0] - stand[0], a[1] - stand[1], a[2] - stand[2]];
  const len = Math.hypot(d[0], d[1], d[2]);
  return len > 1e-6 ? [d[0] / len, d[1] / len, d[2] / len] : [0, 0, 1];
}

/** The yaw/pitch (degrees, world y up) a facing direction stands for. */
function anglesOf(dir: Vec3): { yaw: number; pitch: number } {
  const yaw = (Math.atan2(dir[0], dir[2]) * 180) / Math.PI;
  const pitch = (Math.atan2(dir[1], Math.hypot(dir[0], dir[2])) * 180) / Math.PI;
  return { yaw: (yaw + 360) % 360, pitch };
}

interface Pt {
  x: number;
  y: number;
}

/** One thing drawn for a light: a face of its modifier or a line of its stand, in screen space,
 *  with its distance from the scope's eye for painter's order. */
interface Item {
  key: string;
  light: number;
  d: string;
  fill: string | null;
  stroke: string | null;
  width: number;
  dist: number;
  ghost: boolean;
  selected: boolean;
}

/** The selected light's handles: where it stands, its axes, its beam and the aim ring at its tip. */
interface Sel {
  stand: Pt;
  /** The arrow knobs: sideways and nearer / farther on the floor, height on the pole. */
  knobs: Knob[];
  beam: string | null;
  front: Pt | null;
  tip: Pt | null;
  colour: string;
  /** The sphere the aim ring turns on: the stand and the beam's length. */
  centre: Vec3;
  reach: number;
  /** The sun or moon: where its light goes (the subject on screen), shown as a dashed line. */
  sky?: { to: Pt | null };
}

type KnobAxis = "side" | "depth" | "height";

/** An arrow knob: where it sits, the world axis it moves along, and that axis on the screen. */
interface Knob {
  axis: KnobAxis;
  at: Pt;
  /** The axis' screen direction (unit) and how many pixels a metre along it covers there. */
  dir: Pt;
  pxPerM: number;
}

const KNOB_AXES: Record<KnobAxis, Vec3> = { side: [1, 0, 0], depth: [0, 0, 1], height: [0, 1, 0] };

interface Labels {
  id: number;
  at: Pt;
  selected: boolean;
}

interface Gizmo {
  items: Item[];
  labels: Labels[];
  sel: Sel | null;
}

const EMPTY_GIZMO: Gizmo = { items: [], labels: [], sel: null };

/** The inverse of `standOf`: the u / v / depth that put a light's stand at `p`. */
function placeOf(p: Vec3, info: SceneInfo): { u: number; v: number; depth: number } {
  const focus = Math.sqrt(Math.max(0.05, info.dNear) * Math.max(info.dFar, info.dNear));
  const half = Math.max(0.02, focus - 0.02);
  const t = clamp((p[2] - focus) / (2 * half) + 0.5, 0, 1);
  const z = Math.max(0.02, focus + (t - 0.5) * 2 * half);
  const [hw, hh] = frameHalfSize(z, info.fovMogeDeg, info.width / Math.max(info.height, 1));
  return {
    u: clamp(p[0] / (2 * hw) + 0.5, -0.5, 1.5),
    v: clamp(-p[1] / (2 * hh) + 0.5, -0.5, 1.5),
    depth: Math.round(t * 1000) / 10,
  };
}

/** Where the subject is in the space: the nearest solid thing in the middle of the picture (a
 *  quarter of the way into the depths of the points there), and its middle. What new lights are
 *  placed around and aimed at, and what "aim at the subject" means. */
const subjects = new WeakMap<ScopeCloud, Vec3>();
function subjectOf(c: ScopeCloud): Vec3 {
  const known = subjects.get(c);
  if (known) return known;
  const found = findSubject(c);
  subjects.set(c, found);
  return found;
}
function findSubject(c: ScopeCloud): Vec3 {
  const zs: number[] = [];
  for (let i = 0; i < c.count; i += 3) {
    const u = c.uv[i * 2];
    const v = c.uv[i * 2 + 1];
    if (u > 0.3 && u < 0.7 && v > 0.15 && v < 0.75) zs.push(c.data[i * 6 + 2]);
  }
  if (zs.length < 20) return [0, 0, c.medianZ];
  zs.sort((a, b) => a - b);
  const z = zs[Math.floor(zs.length * 0.25)];
  // Its middle: the points within a little of that depth, in the same central box.
  let sx = 0;
  let sy = 0;
  let n = 0;
  for (let i = 0; i < c.count; i += 3) {
    const u = c.uv[i * 2];
    const v = c.uv[i * 2 + 1];
    const pz = c.data[i * 6 + 2];
    if (u > 0.25 && u < 0.75 && v > 0.1 && v < 0.8 && Math.abs(pz - z) < Math.max(0.3, z * 0.15)) {
      sx += c.data[i * 6];
      sy += c.data[i * 6 + 1];
      n++;
    }
  }
  return n > 0 ? [sx / n, sy / n, z] : [0, 0, z];
}

/** The view the space opens on (and Reset view returns to): the studio from above and to one
 *  side, far enough out to see the camera's end, the lights and the subject together. */
function overview(c: ScopeCloud): Orbit {
  const s = subjectOf(c);
  const z = Math.max(0.3, s[2]);
  return { yaw: 0.6, pitch: -0.42, distance: Math.max(2.8, z * 2.2), target: [s[0], (c.floorY + s[1]) * 0.5, z * 0.75] };
}

/** The sun's or moon's direction in the space (world, y up): from the scene towards it, as
 *  `relight.rs`'s `sun_direction` (azimuth 0 from the right, 90 from behind the subject, 270 from
 *  behind the camera; elevation above the horizon). */
function skyDir(l: Light): Vec3 {
  const az = (l.azimuth * Math.PI) / 180;
  const el = (l.elevation * Math.PI) / 180;
  return [Math.cos(el) * Math.cos(az), Math.sin(el), Math.cos(el) * Math.sin(az)];
}

/** How far out the sun and moon are drawn: on a dome round the subject, beyond the lights. */
const skyRadius = (subject: Vec3) => Math.max(3, subject[2] * 1.2);

/** Where a new light starts, as a photographer would put it: the first a key 45° to the left,
 *  a little above and in front of the subject; the second a fill on the other side, lower; the
 *  third a rim behind and to the right; then more keys a step aside. Real distances (about a
 *  metre and a half from the subject) up to a few metres out, growing with a far subject. */
function placementFor(index: number, subject: Vec3): { side: number; up: number; toward: number } {
  const k = Math.max(1, subject[2] / 4);
  const at = [
    { side: -1.0, up: 0.5, toward: 1.1 },
    { side: 1.1, up: 0.2, toward: 1.0 },
    { side: 0.7, up: 0.8, toward: -0.9 },
  ][index] ?? { side: -1.0 - 0.4 * (index - 2), up: 0.5, toward: 1.1 };
  return { side: at.side * k, up: at.up * k, toward: at.toward * k };
}

/** The lowest a light's head may go above the floor (a stand's shortest), metres. */
const MIN_HEAD = 0.3;

/** The convex hull of some screen points (monotone chain), for the beam's outline. */
function hull(pts: Pt[]): Pt[] {
  const p = pts.slice().sort((a, b) => a.x - b.x || a.y - b.y);
  if (p.length < 3) return p;
  const turn = (o: Pt, a: Pt, b: Pt) => (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
  const lower: Pt[] = [];
  for (const q of p) {
    while (lower.length >= 2 && turn(lower[lower.length - 2], lower[lower.length - 1], q) <= 0) lower.pop();
    lower.push(q);
  }
  const upper: Pt[] = [];
  for (let i = p.length - 1; i >= 0; i--) {
    const q = p[i];
    while (upper.length >= 2 && turn(upper[upper.length - 2], upper[upper.length - 1], q) <= 0) upper.pop();
    upper.push(q);
  }
  return [...lower.slice(0, -1), ...upper.slice(0, -1)];
}

/** Two curved arrows round the aim ring: it turns. */
function turnArrows(c: Pt): string {
  const r = 13;
  const arc = (a0: number, a1: number) => {
    const p0 = { x: c.x + Math.cos(a0) * r, y: c.y + Math.sin(a0) * r };
    const p1 = { x: c.x + Math.cos(a1) * r, y: c.y + Math.sin(a1) * r };
    const head = { x: c.x + Math.cos(a1 - 0.35) * (r + 3.5), y: c.y + Math.sin(a1 - 0.35) * (r + 3.5) };
    const head2 = { x: c.x + Math.cos(a1 - 0.35) * (r - 3.5), y: c.y + Math.sin(a1 - 0.35) * (r - 3.5) };
    return `M${p0.x.toFixed(1)} ${p0.y.toFixed(1)} A${r} ${r} 0 0 1 ${p1.x.toFixed(1)} ${p1.y.toFixed(1)} M${head.x.toFixed(1)} ${head.y.toFixed(1)} L${p1.x.toFixed(1)} ${p1.y.toFixed(1)} L${head2.x.toFixed(1)} ${head2.y.toFixed(1)}`;
  };
  return `${arc(-2.6, -1.2)} ${arc(0.55, 1.95)}`;
}


export default function RelightScope({ path, lights, selected, onSelect, onMove, onEdit, onAdd, onRemove, ambient, onAmbient, photoKelvin, docked = true, onUndock, onRedock }: RelightScopeProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const hostRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const rendererRef = useRef<ScopeRenderer | null>(null);
  const [info, setInfo] = useState<SceneInfo | null>(null);
  const [cloud, setCloud] = useState<ScopeCloud | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const sizeRef = useRef(size);
  sizeRef.current = size;
  const orbit = useRef<Orbit>(photoView(1));
  const [frameTick, setFrameTick] = useState(0);
  const [marks, setMarks] = useState<Gizmo>(EMPTY_GIZMO);
  const [padMode, setPadMode] = useState<"move" | "turn" | "depth">("move");
  const [hover, setHover] = useState<number | null>(null);

  // --- Undocked: a floating box over the photo, moved by its head and sized by its grip ----------
  const [box, setBox] = useState<{ left: number; top: number; w: number; h: number } | null>(() => {
    try {
      const raw = localStorage.getItem("rapidone.relightScopeBox");
      if (!raw) return null;
      const b = JSON.parse(raw) as { left?: number; top?: number; w?: number; h?: number };
      return typeof b.left === "number" && typeof b.top === "number" && typeof b.w === "number" && typeof b.h === "number" ? (b as { left: number; top: number; w: number; h: number }) : null;
    } catch {
      return null;
    }
  });
  const hostSize = () => {
    const host = rootRef.current?.offsetParent as HTMLElement | null;
    return { w: host?.clientWidth ?? 0, h: host?.clientHeight ?? 0 };
  };
  const defaultBox = () => {
    const host = hostSize();
    const top = 64;
    return {
      left: Math.max(8, Math.round((host.w - 620) / 2)),
      top,
      w: Math.min(620, Math.max(340, host.w - 16)),
      h: Math.min(420, Math.max(280, host.h - top - 12)),
    };
  };
  const saveBox = (b: { left: number; top: number; w: number; h: number }) => {
    try {
      localStorage.setItem("rapidone.relightScopeBox", JSON.stringify(b));
    } catch {
      // Not fatal: the box just isn't remembered.
    }
  };
  useLayoutEffect(() => {
    if (!docked && !box) setBox(defaultBox());
  }, [box, docked]);
  useLayoutEffect(() => {
    if (docked || !box) return;
    const host = hostSize();
    if (!host.w) return;
    const left = clamp(box.left, -box.w + 80, Math.max(0, host.w - 80));
    const top = clamp(box.top, 0, Math.max(0, host.h - 40));
    if (left !== box.left || top !== box.top) setBox({ ...box, left, top });
  }, [box, docked]);
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
      const host = hostSize();
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
      const host = hostSize();
      const w = clamp(start.w + ev.clientX - sx, 340, Math.max(340, host.w - start.left));
      const h = clamp(start.h + ev.clientY - sy, 280, Math.max(280, host.h - start.top));
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

  // The scene and its cloud, once per photo.
  useEffect(() => {
    let alive = true;
    setInfo(null);
    setCloud(null);
    setProblem(null);
    // A clean slate: nothing of the last photo's lights stays drawn while this one's scene loads.
    setMarks(EMPTY_GIZMO);
    if (!path) return;
    sceneInfo(path)
      .then((i) => {
        if (!alive) return;
        if (!i) return setProblem("No scene geometry for this photo yet");
        setInfo(i);
      })
      .catch(() => alive && setProblem("The photo's scene couldn't be read"));
    scopeCloud(path)
      .then((c) => {
        if (!alive) return;
        if (!c) return setProblem("No scene geometry for this photo yet");
        setCloud(c);
        orbit.current = overview(c);
      })
      .catch(() => alive && setProblem("The point cloud couldn't be decoded"));
    return () => {
      alive = false;
    };
  }, [path]);

  // The host is absolutely placed inside its box (CSS), so the canvas' own pixel size can never
  // feed back into the layout.
  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const measure = () => {
      const w = Math.max(1, Math.round(host.clientWidth));
      const h = Math.max(1, Math.round(host.clientHeight));
      setSize((s) => (s.w === w && s.h === h ? s : { w, h }));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(host);
    return () => ro.disconnect();
  }, [docked]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    if (!rendererRef.current) rendererRef.current = new ScopeRenderer(canvas);
    // Also with none: a new photo without its geometry yet must not keep showing the last one's.
    rendererRef.current.setCloud(cloud);
    if (!cloud) {
      const gl = canvas.getContext("webgl2");
      gl?.clearColor(0, 0, 0, 0);
      gl?.clear(gl.COLOR_BUFFER_BIT);
      setMarks(EMPTY_GIZMO);
    }
  }, [cloud]);

  useEffect(
    () => () => {
      rendererRef.current?.dispose();
      rendererRef.current = null;
    },
    [],
  );

  const draw = useCallback(() => {
    const r = rendererRef.current;
    if (!r || !info || !cloud || size.w < 4 || size.h < 4) return;
    r.render({
      yaw: orbit.current.yaw,
      pitch: orbit.current.pitch,
      distance: orbit.current.distance,
      target: orbit.current.target,
      fovX: info.fovMogeDeg,
      photoAspect: info.width / Math.max(info.height, 1),
      width: size.w,
      height: size.h,
      dpr: Math.min(window.devicePixelRatio || 1, 2),
      near: 1.0,
      far: 0.0,
      fade: 0.0,
      dNear: info.dNear,
      dFar: info.dFar,
      focus: null,
      fogStart: null,
      fogFar: null,
      fogHeight: 0,
      tint: false,
      grid: true,
      marker: null,
      // The lights are drawn as their rigs over the cloud (below), not as the Depth Scope's rings.
      lights: [],
      decimate: false,
    });

    const mvp = r.lastMvp;
    const at = (p: Vec3): Pt | null => {
      if (!mvp) return null;
      const s = projectToScreen(mvp, p, size.w, size.h);
      return s.inFront && s.z < 1 ? { x: s.x, y: s.y } : null;
    };
    const cam = orbitCamera(orbit.current);
    const distOf = (p: Vec3) => Math.hypot(p[0] - cam.eye[0], p[1] - cam.eye[1], p[2] - cam.eye[2]);
    const loop = (pts: Pt[]) => pts.map((p, i) => `${i === 0 ? "M" : "L"}${p.x.toFixed(1)} ${p.y.toFixed(1)}`).join(" ") + " Z";

    const items: Item[] = [];
    const labels: Labels[] = [];
    let sel: Sel | null = null;
    const subj = subjectOf(cloud);
    for (const l of lights) {
      if (isCelestial(l)) {
        // The sun or moon: a disc in the sky on its dome round the subject, the way its light comes.
        const R = skyRadius(subj);
        const d = skyDir(l);
        const pos: Vec3 = [subj[0] + d[0] * R, subj[1] + d[1] * R, subj[2] + d[2] * R];
        const c = at(pos);
        if (!c) continue;
        const isSel = l.id === selected;
        const r = isSel ? 13 : 11;
        const disc = (cx: number, cy: number, rad: number) => `M${(cx + rad).toFixed(1)} ${cy.toFixed(1)} A${rad} ${rad} 0 1 0 ${(cx - rad).toFixed(1)} ${cy.toFixed(1)} A${rad} ${rad} 0 1 0 ${(cx + rad).toFixed(1)} ${cy.toFixed(1)} Z`;
        const dist = distOf(pos);
        const glowC = kelvinSwatch(l.kelvin);
        if (l.kind === "sun") {
          for (let k = 0; k < 8; k++) {
            const a = (k / 8) * Math.PI * 2;
            items.push({ key: `${l.id}r${k}`, light: l.id, d: `M${(c.x + Math.cos(a) * (r + 3)).toFixed(1)} ${(c.y + Math.sin(a) * (r + 3)).toFixed(1)} L${(c.x + Math.cos(a) * (r + 8)).toFixed(1)} ${(c.y + Math.sin(a) * (r + 8)).toFixed(1)}`, fill: null, stroke: `rgb(${glowC.join(",")})`, width: 2, dist: dist + 0.01, ghost: !l.on, selected: isSel });
          }
          items.push({ key: `${l.id}d`, light: l.id, d: disc(c.x, c.y, r), fill: `rgb(${glowC.map((v) => Math.round(v * 0.5 + 127)).join(",")})`, stroke: `rgb(${glowC.join(",")})`, width: 2, dist, ghost: !l.on, selected: isSel });
        } else {
          items.push({ key: `${l.id}d`, light: l.id, d: disc(c.x, c.y, r), fill: "#e3e9f2", stroke: "#9fb3c8", width: 1, dist, ghost: !l.on, selected: isSel });
          items.push({ key: `${l.id}c`, light: l.id, d: disc(c.x + r * 0.45, c.y - r * 0.2, r * 0.85), fill: "#0b1016", stroke: null, width: 0, dist: dist - 0.001, ghost: !l.on, selected: isSel });
        }
        labels.push({ id: l.id, at: c, selected: isSel });
        if (isSel) sel = { stand: c, knobs: [], beam: null, front: null, tip: null, colour: `rgb(${glowC.join(",")})`, centre: subj, reach: R, sky: { to: at(subj) } };
        continue;
      }
      const stand = standOf(l, info);
      const standAt = at(stand);
      if (!standAt) continue;
      const dir = facingOf(l, info, stand, cloud.medianZ);
      // Rigs are drawn at their real size in the scene's metres.
      const k = 1;
      const type = typeOf(l);
      const rig = buildRig(type, stand, dir, l.size, k, cloud.floorY);
      const glow = kelvinSwatch(l.kelvin);
      const isSel = l.id === selected;
      const ghost = !l.on;
      rig.faces.forEach((f, i) => {
        const pts: Pt[] = [];
        for (const p of f.pts) {
          const q = at(p);
          if (!q) return;
          pts.push(q);
        }
        const c = centroid(f.pts);
        const n = faceNormal(f.pts);
        const view = normalize([c[0] - cam.eye[0], c[1] - cam.eye[1], c[2] - cam.eye[2]]);
        items.push({ key: `${l.id}f${i}`, light: l.id, d: loop(pts), fill: shade(f.mat, n, view, glow), stroke: f.mat === "fabric" || f.mat === "head" ? "rgba(0,0,0,0.35)" : null, width: 0.5, dist: distOf(c), ghost, selected: isSel });
      });
      rig.lines.forEach((ln, i) => {
        const a = at(ln.a);
        const b = at(ln.b);
        if (!a || !b) return;
        const mid: Vec3 = [(ln.a[0] + ln.b[0]) / 2, (ln.a[1] + ln.b[1]) / 2, (ln.a[2] + ln.b[2]) / 2];
        const stroke = ln.kind === "grid" ? "rgba(150,160,170,0.8)" : ln.kind === "shaft" ? "#9aa4ad" : "#6c7882";
        items.push({ key: `${l.id}l${i}`, light: l.id, d: `M${a.x.toFixed(1)} ${a.y.toFixed(1)} L${b.x.toFixed(1)} ${b.y.toFixed(1)}`, fill: null, stroke, width: ln.kind === "grid" ? 0.6 : ln.kind === "leg" ? 1.6 : 2, dist: distOf(mid) + (ln.kind === "stand" || ln.kind === "leg" ? 0.05 : 0), ghost, selected: isSel });
      });
      labels.push({ id: l.id, at: standAt, selected: isSel });

      if (isSel) {
        // The arrow knobs: two on the floor by the stand's foot, one on its pole.
        const knobs: Knob[] = [];
        const floor = cloud.floorY;
        const places: [KnobAxis, Vec3][] = [
          ["side", [stand[0] + 0.5, floor, stand[2]]],
          ["depth", [stand[0], floor, stand[2] - 0.5]],
          ["height", [stand[0], Math.max(floor + 0.12, (stand[1] + floor) / 2), stand[2]]],
        ];
        for (const [axis, c] of places) {
          const a = at(c);
          const v = KNOB_AXES[axis];
          const b = at([c[0] + v[0] * 0.2, c[1] + v[1] * 0.2, c[2] + v[2] * 0.2]);
          if (!a || !b) continue;
          const len = Math.hypot(b.x - a.x, b.y - a.y);
          if (len < 1e-3) continue;
          knobs.push({ axis, at: a, dir: { x: (b.x - a.x) / len, y: (b.y - a.y) / len }, pxPerM: len / 0.2 });
        }
        // The beam: from the modifier's face out by the light's own spread (a spot's Cone; 30° else).
        const halfA = ((l.kind === "spot" ? l.cone / 2 : 30) * Math.PI) / 180;
        const reach = Math.max(0.7 * k, stand[2] * 0.55);
        const farR = Math.min(2.6 * k, rig.frontR + reach * Math.tan(halfA));
        const { r: rr, u: uu } = frameOf(dir);
        const farC: Vec3 = [stand[0] + dir[0] * reach, stand[1] + dir[1] * reach, stand[2] + dir[2] * reach];
        const circle = (c: Vec3, rad: number): Pt[] | null => {
          const out: Pt[] = [];
          for (let i = 0; i < 24; i++) {
            const t = (i / 24) * Math.PI * 2;
            const q = at([c[0] + (rr[0] * Math.cos(t) + uu[0] * Math.sin(t)) * rad, c[1] + (rr[1] * Math.cos(t) + uu[1] * Math.sin(t)) * rad, c[2] + (rr[2] * Math.cos(t) + uu[2] * Math.sin(t)) * rad]);
            if (!q) return null;
            out.push(q);
          }
          return out;
        };
        const nearC = circle(rig.front, rig.frontR * 0.8);
        const farPts = circle(farC, farR);
        // The beam's silhouette: the hull of both circles is close enough to the cone's outline.
        let beam: string | null = null;
        if (nearC && farPts) beam = loop(hull([...nearC, ...farPts]));
        sel = {
          stand: standAt,
          knobs,
          beam,
          front: at(rig.front),
          tip: at(farC),
          colour: `rgb(${glow.join(",")})`,
          centre: stand,
          reach,
        };
      }
    }
    items.sort((a, b) => b.dist - a.dist);
    setMarks({ items, labels, sel });
  }, [info, cloud, size, lights, selected]);
  useEffect(draw, [draw, frameTick]);

  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const r = rendererRef.current;
    console.warn(
      `[relight-scope] size ${size.w}x${size.h} info ${info ? "yes" : "none"} cloud ${cloud?.count ?? 0} renderer ${r ? "gl-ok" : "none"} mvp ${r?.lastMvp ? "set" : "none"} lights ${lights.length}`,
    );
  }, [size, info, cloud, lights.length, selected]);

  // --- The camera: drag to orbit, right-drag to pan, wheel to zoom -----------------------------
  const drag = useRef<{ kind: "orbit" | "pan"; x: number; y: number } | null>(null);
  /** A new view, unless it would put the eye under the floor (it then stays where it was). */
  const setOrbit = (next: Orbit) => {
    const floor = cloud?.floorY;
    if (floor !== undefined && orbitCamera(next).eye[1] < floor + 0.05) {
      // Keep the zoom or pan part that is above ground: try the old pitch with the new rest.
      const keep = { ...next, pitch: orbit.current.pitch };
      if (orbitCamera(keep).eye[1] < floor + 0.05) return;
      orbit.current = keep;
    } else orbit.current = next;
    setFrameTick((t) => t + 1);
  };
  const zoomBy = (delta: number) => setOrbit(zoomOrbit(orbit.current, delta, (cloud?.zMin ?? 0.2) * 0.6, (cloud?.zMax ?? 50) * 3));
  const camera = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.x;
    const dy = e.clientY - d.y;
    drag.current = { ...d, x: e.clientX, y: e.clientY };
    if (d.kind === "orbit") setOrbit(orbitDrag(orbit.current, dx, dy));
    else setOrbit(panOrbit(orbit.current, dx, dy, size.h, fovYFromFovX(info?.fovMogeDeg ?? 60, (info?.width ?? 3) / Math.max(info?.height ?? 2, 1))));
    setFrameTick((t) => t + 1);
  };

  const light = lights.find((l) => l.id === selected) ?? null;
  const stand = info && light ? standOf(light, info) : null;

  /** The pointer's ray in the scope's world: the inverse of `projectToScreen`, with the same camera
   *  and field of view. */
  const pointerRay = (e: { clientX: number; clientY: number }): { eye: Vec3; ray: Vec3 } | null => {
    const host = hostRef.current;
    if (!host || !info) return null;
    const { w, h } = sizeRef.current;
    if (w < 4 || h < 4) return null;
    const rect = host.getBoundingClientRect();
    const cam = orbitCamera(orbit.current);
    const aspect = w / h;
    const fovY = (viewFovY(info.fovMogeDeg, info.width / Math.max(info.height, 1), aspect) * Math.PI) / 180;
    const tanY = Math.tan(fovY / 2);
    const tanX = tanY * aspect;
    const ndcX = ((e.clientX - rect.left) / w) * 2 - 1;
    const ndcY = 1 - ((e.clientY - rect.top) / h) * 2;
    const ray = normalize([
      cam.forward[0] + cam.right[0] * ndcX * tanX + cam.up[0] * ndcY * tanY,
      cam.forward[1] + cam.right[1] * ndcX * tanX + cam.up[1] * ndcY * tanY,
      cam.forward[2] + cam.right[2] * ndcX * tanX + cam.up[2] * ndcY * tanY,
    ]);
    return { eye: cam.eye, ray };
  };

  /**
   * The world direction from `centre` to where the pointer's ray meets a sphere of `radius`: the
   * inverse of `projectToScreen`, using the same camera and field of view. A pointer past the
   * sphere's edge lands on its rim in that direction, so the handle never runs away.
   */
  const pointerOnSphere = (e: React.PointerEvent, centre: Vec3, radius: number, facing: Vec3): Vec3 | null => {
    const pr = pointerRay(e);
    if (!pr || radius < 1e-4) return null;
    const { ray } = pr;
    const cam = { eye: pr.eye };
    const o: Vec3 = [cam.eye[0] - centre[0], cam.eye[1] - centre[1], cam.eye[2] - centre[2]];
    const b = dot(ray, o);
    const disc = b * b - (dot(o, o) - radius * radius);
    // A ray through the ball crosses the sphere twice: near side and far side. Take the one on
    // the side the handle is actually on (`facing`), or a beam pointing away from the camera
    // would be mirrored onto the near surface — the "drags left, moves right" bug.
    const roots = disc > 0 ? [-b - Math.sqrt(disc), -b + Math.sqrt(disc)] : [-b];
    let best: Vec3 | null = null;
    let bestDot = -Infinity;
    for (const t of roots) {
      if (t <= 1e-4) continue;
      const hit: Vec3 = [cam.eye[0] + ray[0] * t, cam.eye[1] + ray[1] * t, cam.eye[2] + ray[2] * t];
      const dir = normalize([hit[0] - centre[0], hit[1] - centre[1], hit[2] - centre[2]]);
      const d = dot(dir, facing);
      if (d > bestDot) {
        bestDot = d;
        best = dir;
      }
    }
    if (best) return best;
    // A pointer past the sphere's edge: the closest approach, on the rim in that direction.
    const t = Math.max(1e-4, -b);
    const hit: Vec3 = [cam.eye[0] + ray[0] * t, cam.eye[1] + ray[1] * t, cam.eye[2] + ray[2] * t];
    return normalize([hit[0] - centre[0], hit[1] - centre[1], hit[2] - centre[2]]);
  };

  // --- Moving a light is moving its stand around the studio floor: drag left / right to slide it
  // sideways, up / down the screen to walk it away / towards you, whatever the view. Its height
  // stays put; Shift-drag, the knob on its pole or the wheel over it raise and lower it, never
  // below the floor. ---------------------------------------------------------------------------
  const moveDrag = useRef<{ id: number; x: number; y: number; start: Vec3; perPx: number; side: Vec3; away: Vec3; height: boolean; moved: boolean } | null>(null);
  /** The view's sideways and away directions along the floor, and metres per pixel at `p`. */
  const groundAxes = (p: Vec3) => {
    const cam = orbitCamera(orbit.current);
    const flat = (v: Vec3): Vec3 | null => {
      const l = Math.hypot(v[0], v[2]);
      return l > 0.25 ? [v[0] / l, 0, v[2] / l] : null;
    };
    const side = flat(cam.right) ?? [1, 0, 0];
    // Up the screen is "away": the view's forward along the floor, or its up when looking straight down.
    const away = flat(cam.forward) ?? flat(cam.up) ?? [0, 0, 1];
    const { h } = sizeRef.current;
    const aspect = sizeRef.current.w / Math.max(1, h);
    const fovY = info ? viewFovY(info.fovMogeDeg, info.width / Math.max(info.height, 1), aspect) : 50;
    const dist = Math.hypot(p[0] - cam.eye[0], p[1] - cam.eye[1], p[2] - cam.eye[2]);
    const perPx = (2 * dist * Math.tan((fovY * Math.PI) / 360)) / Math.max(1, h);
    return { side, away, perPx };
  };
  /** Every move goes through here: a patch that would sink the light's head below the floor
   *  (the depth rail, an arrow, the pad) is lifted back to the lowest a stand goes. */
  const safeMove = (id: number, patchIn: Parameters<typeof onMove>[1], last: boolean) => {
    let patch = patchIn;
    const l = lights.find((x) => x.id === id);
    if (!l || !info || !cloud) return onMove(id, patch, last);
    // (`patch` is rebuilt below: kept above the floor, and turned round when it crosses the subject.)
    let next = { ...l, ...patch };
    let p = standOf(next, info);
    const lowest = cloud.floorY + MIN_HEAD;
    if (p[1] < lowest) {
      const [, hh] = frameHalfSize(p[2], info.fovMogeDeg, info.width / Math.max(info.height, 1));
      patch = { ...patch, v: clamp(-lowest / (2 * hh) + 0.5, -0.5, 1.5) };
      next = { ...l, ...patch };
      p = standOf(next, info);
    }
    // Crossing the subject's plane (into a backlight, or back out to the front) turns the light
    // round to face the subject, as you would turn a real one: a light behind the sitter pointing
    // away from them does nothing useful. Only when it moves, never when it's being aimed.
    const moved = patch.u !== undefined || patch.v !== undefined || patch.depth !== undefined;
    if (moved && patch.yaw === undefined) {
      const subj = subjectOf(cloud);
      const was = standOf(l, info)[2] > subj[2];
      const now = p[2] > subj[2];
      if (was !== now) {
        const a = anglesOf(normalize([subj[0] - p[0], subj[1] - p[1], subj[2] - p[2]]));
        patch = { ...patch, yaw: Math.round(a.yaw * 10) / 10, pitch: Math.round(a.pitch * 10) / 10 };
      }
    }
    onMove(id, patch, last);
  };
  /** A light's stand put at a world place, kept above the floor. */
  const putAt = (id: number, p: Vec3, last: boolean) => {
    if (!info) return;
    const floor = cloud?.floorY ?? -10;
    safeMove(id, placeOf([p[0], Math.max(p[1], floor + MIN_HEAD), p[2]], info), last);
  };
  /** A sun moved to `elevation`: its colour and the scene's Ambient follow its height, each
   *  only while it's still at the value the sun gave it (set by hand, it stays). */
  const followSun = (l: Light, elevation: number, patch: Partial<Light>) => {
    if (!isCelestial(l)) return;
    if (l.kind === "sun" && Math.abs(l.kelvin - sunKelvin(l.elevation)) < 60) patch.kelvin = sunKelvin(elevation);
    // The moon's night brightens a little as it climbs; the sun's day much more.
    const natural = l.kind === "sun" ? sunAmbient : moonAmbient;
    if (ambient !== undefined && onAmbient && Math.abs(ambient - natural(l.elevation)) <= 1) {
      const next = natural(elevation);
      if (next !== ambient) onAmbient(next, `relight:${l.id}:sky-ambient`);
    }
  };
  // The sun and moon: dragged round their dome (the direction their light comes from).
  const skyDrag = useRef<{ id: number; dir: Vec3; moved: boolean } | null>(null);
  const moveSky = (e: React.PointerEvent, last: boolean) => {
    const d = skyDrag.current;
    const l = d ? lights.find((x) => x.id === d.id) : null;
    if (!d || !l || !cloud) return;
    const subj = subjectOf(cloud);
    const dir = pointerOnSphere(e, subj, skyRadius(subj), d.dir);
    if (!dir) return;
    d.dir = dir;
    d.moved = true;
    // Never below the horizon (no light), never quite straight overhead (the azimuth would be lost).
    const elevation = Math.round(Math.min(89, Math.max(0, (Math.asin(clamp(dir[1], -1, 1)) * 180) / Math.PI)) * 10) / 10;
    const azimuth = Math.round(((((Math.atan2(dir[2], dir[0]) * 180) / Math.PI) % 360) + 360) % 360 * 10) / 10;
    const patch: Partial<Light> = { azimuth, elevation };
    // A sun left at its natural colour and ambient keeps them as it moves: warm and dim low
    // down, white and bright high up.
    followSun(l, elevation, patch);
    void last;
    onEdit(l.id, patch, `relight:${l.id}:sky`);
  };
  const startMoveLight = (e: React.PointerEvent, id: number, height = false) => {
    if (e.button !== 0 || !info) return;
    e.stopPropagation();
    const l = lights.find((x) => x.id === id);
    if (!l) return;
    onSelect(id);
    if (isCelestial(l)) {
      skyDrag.current = { id, dir: skyDir(l), moved: false };
      (e.currentTarget as Element).setPointerCapture(e.pointerId);
      return;
    }
    const start = standOf(l, info);
    const g = groundAxes(start);
    moveDrag.current = { id, x: e.clientX, y: e.clientY, start, ...g, height, moved: false };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  };
  const moveLight = (e: React.PointerEvent, last: boolean) => {
    if (skyDrag.current) return moveSky(e, last);
    const d = moveDrag.current;
    if (!d) return;
    const dx = e.clientX - d.x;
    const dy = e.clientY - d.y;
    if (!d.moved && Math.hypot(dx, dy) < 2) return;
    d.moved = true;
    const p: Vec3 =
      d.height || e.shiftKey
        ? [d.start[0], d.start[1] - dy * d.perPx, d.start[2]]
        : [d.start[0] + d.side[0] * dx * d.perPx - d.away[0] * dy * d.perPx, d.start[1], d.start[2] + d.side[2] * dx * d.perPx - d.away[2] * dy * d.perPx];
    putAt(d.id, p, last);
  };
  const endMoveLight = (e: React.PointerEvent) => {
    if (moveDrag.current?.moved || skyDrag.current?.moved) moveLight(e, true);
    moveDrag.current = null;
    skyDrag.current = null;
    try {
      (e.currentTarget as Element).releasePointerCapture(e.pointerId);
    } catch {
      // Already released.
    }
  };
  /** Raise or lower a light by `metres`, never below the floor. */
  const raise = (id: number, metres: number) => {
    const l = lights.find((x) => x.id === id);
    if (!l || !info) return;
    if (isCelestial(l)) {
      // The wheel over the sun or moon: higher or lower in the sky, 2° a notch.
      const elevation = Math.min(89, Math.max(0, l.elevation + (metres > 0 ? 2 : -2)));
      const patch: Partial<Light> = { elevation };
      followSun(l, elevation, patch);
      return onEdit(id, patch, `relight:${id}:sky`);
    }
    const p = standOf(l, info);
    putAt(id, [p[0], p[1] + metres, p[2]], true);
  };

  /** Turn a light to face the middle of the scene (the photo's centre at the subject's depth). */
  const aimAtMiddle = (id: number) => {
    const l = lights.find((x) => x.id === id);
    if (!l || !info || !cloud) return;
    if (isCelestial(l)) {
      const at = placeOf(subjectOf(cloud), info);
      return onEdit(id, { aimU: clamp(at.u, 0, 1), aimV: clamp(at.v, 0, 1) }, null);
    }
    const st = standOf(l, info);
    const subj = subjectOf(cloud);
    const d = normalize([subj[0] - st[0], subj[1] - st[1], subj[2] - st[2]]);
    const a = anglesOf(d);
    safeMove(id, { yaw: Math.round(a.yaw * 10) / 10, pitch: Math.round(a.pitch * 10) / 10 }, true);
  };

  // --- The arrow knobs: drag one to move the light along its own axis only --------------------
  const knobDrag = useRef<{ id: number; axis: KnobAxis; start: Vec3; x: number; y: number; dir: Pt; pxPerM: number } | null>(null);
  const startKnob = (e: React.PointerEvent, k: Knob) => {
    if (e.button !== 0 || !light || !info) return;
    e.stopPropagation();
    knobDrag.current = { id: light.id, axis: k.axis, start: standOf(light, info), x: e.clientX, y: e.clientY, dir: k.dir, pxPerM: Math.max(1e-3, k.pxPerM) };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  };
  const moveKnob = (e: React.PointerEvent, last: boolean) => {
    const d = knobDrag.current;
    if (!d) return;
    const metres = ((e.clientX - d.x) * d.dir.x + (e.clientY - d.y) * d.dir.y) / d.pxPerM;
    const v = KNOB_AXES[d.axis];
    putAt(d.id, [d.start[0] + v[0] * metres, d.start[1] + v[1] * metres, d.start[2] + v[2] * metres], last);
  };
  const endKnob = (e: React.PointerEvent) => {
    if (knobDrag.current) moveKnob(e, true);
    knobDrag.current = null;
    try {
      (e.currentTarget as Element).releasePointerCapture(e.pointerId);
    } catch {
      // Already released.
    }
  };

  // --- Aim: the beam's own front. The drag turns it in the scope's view, so it follows the mouse.
  const beamDrag = useRef<{ id: number; dir: Vec3; prev: Vec3; stand: Vec3; reach: number } | null>(null);
  const startAim = (e: React.PointerEvent) => {
    if (!light || !info || !stand || !marks.sel) return;
    e.stopPropagation();
    const dir = facingOf(light, info, stand, cloud?.medianZ ?? stand[2]);
    const prev = pointerOnSphere(e, marks.sel.centre, marks.sel.reach, dir);
    if (!prev) return;
    beamDrag.current = { id: light.id, dir, prev, stand: marks.sel.centre, reach: marks.sel.reach };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  };
  const moveAim = (e: React.PointerEvent, last: boolean) => {
    const d = beamDrag.current;
    if (!d) return;
    const curr = pointerOnSphere(e, d.stand, d.reach, d.dir);
    if (!curr) return;
    // Turn the beam by exactly the turn the hand made on the sphere: the tip keeps up with the
    // pointer, and there is no incremental rotation to invert when the beam faces the camera.
    const axis = cross(d.prev, curr);
    const sin = Math.hypot(axis[0], axis[1], axis[2]);
    const cos = dot(d.prev, curr);
    if (sin > 1e-6) d.dir = normalize(rotateAround(d.dir, axis, Math.atan2(sin, cos)));
    d.prev = curr;
    const a = anglesOf(d.dir);
    safeMove(d.id, { yaw: a.yaw, pitch: a.pitch }, last);
  };
  const endAim = (e: React.PointerEvent) => {
    beamDrag.current = null;
    try {
      (e.currentTarget as Element).releasePointerCapture(e.pointerId);
    } catch {
      // Already released.
    }
  };

  /** The pad's four arrows: a step of the place, the turn, or the depth. */
  const nudge = (dir: "left" | "right" | "up" | "down") => {
    if (!light) return;
    if (padMode === "move" && info && stand) {
      const g = groundAxes(stand);
      const m = 0.1; // 10 cm a press
      const sx = dir === "left" ? -m : dir === "right" ? m : 0;
      const sz = dir === "up" ? m : dir === "down" ? -m : 0;
      putAt(light.id, [stand[0] + g.side[0] * sx + g.away[0] * sz, stand[1], stand[2] + g.side[2] * sx + g.away[2] * sz], true);
      return;
    }
    if (padMode === "depth") {
      if (dir === "up") raise(light.id, 0.1);
      else if (dir === "down") raise(light.id, -0.1);
      return;
    }
    if (!info || !stand) return;
    const cam = orbitCamera(orbit.current);
    let d = facingOf(light, info, stand, cloud?.medianZ ?? stand[2]);
    const step = (5 * Math.PI) / 180;
    if (dir === "left") d = rotateAround(d, cam.up, -step);
    else if (dir === "right") d = rotateAround(d, cam.up, step);
    else if (dir === "up") d = rotateAround(d, cam.right, -step);
    else d = rotateAround(d, cam.right, step);
    const a = anglesOf(d);
    safeMove(light.id, { yaw: a.yaw, pitch: a.pitch }, true);
  };

  /** A new light, placed round the subject as a photographer would and turned to face them. */
  /** Where the sun or moon starts: from the front left, low (golden hour) or higher for the moon,
   *  its Spread centred on the subject. */
  const skyPlace = (typeId: string): Partial<Light> => {
    const subj = cloud ? subjectOf(cloud) : null;
    const at = subj && info ? placeOf(subj, info) : { u: 0.5, v: 0.5 };
    const elevation = typeId === "moon" ? 35 : 20;
    return { azimuth: 220, elevation, aimU: clamp(at.u, 0, 1), aimV: clamp(at.v, 0, 1), ...(typeId === "sun" ? { kelvin: sunKelvin(elevation) } : {}) };
  };
  /** A lamp's colour to start with: the type's own (white in the photo). */
  // Warm now reads against daylight on every photo (the engine's white is 5500 K, whatever preset
  // the camera was on), so a lamp's own 5500 K is white; `photoKelvin` is kept for when a lamp
  // should follow a white balance changed by hand.
  const lampWhite = (): Partial<Light> => (photoKelvin === undefined ? {} : {});
  const addLight = (typeId: string) => {
    if (typeId === "sun" || typeId === "moon") return onAdd(typeId, skyPlace(typeId));
    if (!info || !cloud) return onAdd(typeId);
    const subj = subjectOf(cloud);
    const at = placementFor(lights.length, subj);
    const floor = cloud.floorY;
    const p: Vec3 = [subj[0] + at.side, Math.max(floor + MIN_HEAD, subj[1] + at.up), Math.max(0.3, subj[2] - at.toward)];
    const place = placeOf(p, info);
    const a = anglesOf(normalize([subj[0] - p[0], subj[1] - p[1], subj[2] - p[2]]));
    onAdd(typeId, { ...place, yaw: Math.round(a.yaw * 10) / 10, pitch: Math.round(a.pitch * 10) / 10, ...lampWhite() });
  };

  /** The next or previous light, wrapping round. */
  const step = (by: number) => {
    if (lights.length === 0) return;
    const i = light ? lights.indexOf(light) : by > 0 ? -1 : 0;
    onSelect(lights[(i + by + lights.length) % lights.length].id);
  };
  const typeName = (l: Light) => LIGHT_TYPES.find((t) => t.id === typeOf(l))?.label ?? "Light";

  const angles = light && info && stand ? anglesOf(facingOf(light, info, stand, cloud?.medianZ ?? stand[2])) : null;

  const scopeBody = (
    <div className="relight-scope" ref={hostRef}>
      <canvas
        ref={canvasRef}
        className="relight-scope__canvas"
        onPointerDown={(e) => {
          if (e.button === 2 || e.button === 1) {
            drag.current = { kind: "pan", x: e.clientX, y: e.clientY };
            (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
          } else if (e.button === 0) {
            drag.current = { kind: "orbit", x: e.clientX, y: e.clientY };
            (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
          }
        }}
        onPointerMove={(e) => camera(e)}
        onPointerUp={(e) => {
          drag.current = null;
          (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
        }}
        onPointerCancel={() => (drag.current = null)}
        onWheel={(e) => {
          e.preventDefault();
          zoomBy(e.deltaY);
        }}
        onContextMenu={(e) => e.preventDefault()}
      />
      <svg className="relight-gizmo" viewBox={`0 0 ${Math.max(1, size.w)} ${Math.max(1, size.h)}`} preserveAspectRatio="none" aria-hidden="true">
        {/* The selected light's beam, under the rigs. */}
        {marks.sel?.beam && <path className="relight-gizmo__beam" d={marks.sel.beam} fill={marks.sel.colour} pointerEvents="none" />}
        {marks.sel?.sky?.to && (
          <line className="relight-gizmo__sky" x1={marks.sel.stand.x} y1={marks.sel.stand.y} x2={marks.sel.sky.to.x} y2={marks.sel.sky.to.y} stroke={marks.sel.colour} pointerEvents="none" />
        )}
        {marks.items.map((it) => (
          <path
            key={it.key}
            className={`relight-rig${it.selected ? " is-selected" : ""}${it.ghost ? " is-off" : ""}${it.light === hover ? " is-hover" : ""}`}
            d={it.d}
            fill={it.fill ?? "none"}
            stroke={it.stroke ?? "none"}
            strokeWidth={it.width}
            strokeLinecap="round"
            onPointerEnter={() => setHover(it.light)}
            onPointerLeave={() => setHover((h) => (h === it.light ? null : h))}
            onPointerDown={(e) => startMoveLight(e, it.light)}
            onPointerMove={(e) => moveLight(e, false)}
            onPointerUp={endMoveLight}
            onPointerCancel={endMoveLight}
            onDoubleClick={(e) => {
              e.stopPropagation();
              aimAtMiddle(it.light);
            }}
            onWheel={(e) => {
              e.stopPropagation();
              raise(it.light, e.deltaY > 0 ? -0.05 : 0.05);
            }}
          >
            <title>Drag to move this light across the floor · Shift-drag or wheel: higher / lower · double-click: aim at the middle</title>
          </path>
        ))}
        {light && marks.sel && (
          <>
            {marks.sel.knobs.map((k) => (
              <g
                key={k.axis}
                className={`relight-gizmo__knob relight-gizmo__knob--${k.axis}`}
                transform={`translate(${k.at.x.toFixed(1)} ${k.at.y.toFixed(1)}) rotate(${((Math.atan2(k.dir.y, k.dir.x) * 180) / Math.PI).toFixed(1)})`}
                onPointerDown={(e) => startKnob(e, k)}
                onPointerMove={(e) => moveKnob(e, false)}
                onPointerUp={endKnob}
                onPointerCancel={endKnob}
                onWheel={(e) => {
                  if (k.axis !== "height") return;
                  e.stopPropagation();
                  raise(light.id, e.deltaY > 0 ? -0.05 : 0.05);
                }}
              >
                <title>{k.axis === "side" ? "Drag: left / right only" : k.axis === "depth" ? "Drag: nearer / farther only" : "Height: drag up or down (never below the floor)"}</title>
                <rect x={-13} y={-8} width={26} height={16} rx={8} />
                <path d="M-5 -4 L-9 0 L-5 4 M5 -4 L9 0 L5 4" />
              </g>
            ))}
            {marks.sel.front && marks.sel.tip && (
              <g
                className="relight-gizmo__aim"
                style={{ ["--light" as string]: marks.sel.colour }}
                onPointerDown={startAim}
                onPointerMove={(e) => moveAim(e, false)}
                onPointerUp={(e) => {
                  moveAim(e, true);
                  endAim(e);
                }}
                onPointerCancel={endAim}
              >
                <title>Aim: drag this ring to turn the light (double-click the light to aim it at the middle)</title>
                <line className="relight-gizmo__aim-line" x1={marks.sel.front.x} y1={marks.sel.front.y} x2={marks.sel.tip.x} y2={marks.sel.tip.y} />
                <circle className="relight-gizmo__aim-hit" cx={marks.sel.tip.x} cy={marks.sel.tip.y} r={16} />
                <circle className="relight-gizmo__aim-ring" cx={marks.sel.tip.x} cy={marks.sel.tip.y} r={9} />
                <path className="relight-gizmo__aim-arrows" d={turnArrows(marks.sel.tip)} />
                <circle className="relight-gizmo__aim-dot" cx={marks.sel.tip.x} cy={marks.sel.tip.y} r={2.5} />
              </g>
            )}
          </>
        )}
      </svg>
      {marks.labels.map((lb) => (
        <span key={lb.id} className={`relight-scope__tag${lb.selected ? " is-selected" : ""}`} style={{ left: lb.at.x, top: lb.at.y }}>
          {lb.id}
        </span>
      ))}
      {light && isCelestial(light) && (
        <div className="relight-scope__readout" aria-label="Sun or moon placement">
          <span>{light.kind === "sun" ? "Sun" : "Moon"}</span>
          <span>around {Math.round(light.azimuth)}°</span>
          <span>up {Math.round(light.elevation)}°</span>
        </div>
      )}
      {light && !isCelestial(light) && angles && (
        <div className="relight-scope__readout" aria-label="Light placement">
          <span>x {light.u.toFixed(2)}</span>
          <span>y {light.v.toFixed(2)}</span>
          <span>z {Math.round(light.depth)}</span>
          <span>{Math.round(angles.yaw)}° / {Math.round(angles.pitch)}°</span>
        </div>
      )}
      {!light && !problem && (
        <div className="relight-scope__hint">{lights.length ? "Click a light to select it · drag it to move it across the floor · drag empty space to look around" : "Pick a light type above, or + Add"}</div>
      )}
      {light && !isCelestial(light) && (
        <div className="relight-pad" onPointerDown={(e) => e.stopPropagation()} onWheel={(e) => e.stopPropagation()}>
          <button
            type="button"
            className={`relight-pad__mode${padMode === "turn" ? " is-tilt" : padMode === "depth" ? " is-depth" : ""}`}
            onClick={() => setPadMode((m) => (m === "move" ? "turn" : m === "turn" ? "depth" : "move"))}
            title="Move (across the floor) / Turn / Height: click to change what the arrows do"
          >
            {padMode === "move" ? "Move" : padMode === "turn" ? "Turn" : "Height"}
          </button>
          <button type="button" className="relight-pad__mode" onClick={() => aimAtMiddle(light.id)} title="Turn the light to face the middle of the scene (or double-click the light)">
            Aim ◎
          </button>
          <div className="relight-pad__grid">
            <span />
            <button type="button" onClick={() => nudge("up")} title={padMode === "move" ? "Away from you, 10 cm" : padMode === "turn" ? "Pitch up" : "Higher, 10 cm"}>↑</button>
            <span />
            <button type="button" disabled={padMode === "depth"} onClick={() => nudge("left")} title={padMode === "depth" ? "Height uses ↑ / ↓" : padMode === "move" ? "Left, 10 cm" : "Yaw left"}>←</button>
            <span className="relight-pad__dot" />
            <button type="button" disabled={padMode === "depth"} onClick={() => nudge("right")} title={padMode === "depth" ? "Height uses ↑ / ↓" : padMode === "move" ? "Right, 10 cm" : "Yaw right"}>→</button>
            <span />
            <button type="button" onClick={() => nudge("down")} title={padMode === "move" ? "Towards you, 10 cm" : padMode === "turn" ? "Pitch down" : "Lower, 10 cm (never below the floor)"}>↓</button>
            <span />
          </div>
        </div>
      )}
      {problem && <span className="relight-scope__note">{problem}</span>}
    </div>
  );

  /** The space's buttons, down its left as every Creative panel has them: the lights, their
   *  types, add / on / remove, the view, and Undock. */
  const sideBar = (
    <div className="creative-side relight-side" onPointerDown={(e) => e.stopPropagation()} onWheel={(e) => e.stopPropagation()}>
      <div className="relight-side__row">
        <button type="button" disabled={lights.length === 0} onClick={() => step(-1)} title="Previous light" aria-label="Previous light">
          ‹
        </button>
        <span className="relight-side__count" title={light ? `${typeName(light)} ${light.id}` : "No light selected"}>
          {light ? `${lights.indexOf(light) + 1}/${lights.length}` : `–/${lights.length}`}
        </span>
        <button type="button" disabled={lights.length === 0} onClick={() => step(1)} title="Next light" aria-label="Next light">
          ›
        </button>
      </div>
      <div className="relight-side__types" role="radiogroup" aria-label="Light type">
        {LIGHT_TYPES.map((t) => {
          const on = light ? typeOf(light) === t.id : false;
          return (
            <button
              key={t.id}
              type="button"
              role="radio"
              aria-checked={on}
              className={on ? "is-on" : undefined}
              onClick={() => {
                if (!light) return addLight(t.id);
                const typed = withLightType(light, t.id);
                // Into the sky (or back into the room): its sky place, or a place round the subject.
                const sky = t.id === "sun" || t.id === "moon";
                if (sky) {
                  // Into the sky, or sun ↔ moon: its sky place if it wasn't there yet, and the
                  // ambient for day or night (the other lights, without this one, decide).
                  const place = isCelestial(light) ? { elevation: light.elevation } : skyPlace(t.id);
                  onEdit(light.id, { ...typed, ...place }, `relight:${light.id}:type`);
                  if (ambient !== undefined && onAmbient) {
                    const next = skyAmbient({ enabled: true, ambient, relief: 0, lights: lights.filter((x) => x.id !== light.id) }, t.id === "moon" ? "moon" : "sun", place.elevation ?? light.elevation);
                    if (next !== ambient) onAmbient(next, `relight:${light.id}:type-ambient`);
                  }
                  return;
                }
                // A lamp turned into another keeps its colour (only its character changes).
                if (!sky && !isCelestial(light)) return onEdit(light.id, { ...typed, kelvin: light.kelvin }, `relight:${light.id}:type`);
                if (!sky && isCelestial(light) && info && cloud) {
                  const subj = subjectOf(cloud);
                  const at = placementFor(0, subj);
                  const p: Vec3 = [subj[0] + at.side, Math.max(cloud.floorY + MIN_HEAD, subj[1] + at.up), Math.max(0.3, subj[2] - at.toward)];
                  const a = anglesOf(normalize([subj[0] - p[0], subj[1] - p[1], subj[2] - p[2]]));
                  return onEdit(light.id, { ...typed, ...placeOf(p, info), yaw: a.yaw, pitch: a.pitch, ...lampWhite() }, `relight:${light.id}:type`);
                }
                onEdit(light.id, typed, `relight:${light.id}:type`);
              }}
              title={light ? `Make light ${light.id} a ${t.label.toLowerCase()}` : `Add a ${t.label.toLowerCase()}`}
            >
              <LightIcon type={t.id} size={16} />
            </button>
          );
        })}
      </div>
      <div className="relight-side__row">
        <button type="button" onClick={() => addLight(light ? typeOf(light) : "softbox")} title={light ? `Add another ${typeName(light).toLowerCase()}` : "Add a softbox"} aria-label="Add a light" className="relight-side__add">
          <Plus size={15} strokeWidth={3.2} />
        </button>
        <button type="button" disabled={!light} className={light?.on ? "is-on" : undefined} onClick={() => light && onEdit(light.id, { on: !light.on }, `relight:${light.id}:on`)} title={light ? (light.on ? "Switch this light off" : "Switch this light on") : "Select a light first"} aria-pressed={light?.on ?? false}>
          ⏻
        </button>
        <button type="button" disabled={!light} className="relight-side__remove" onClick={() => light && onRemove(light.id)} title={light ? `Remove light ${light.id}` : "Select a light first"} aria-label="Remove the light">
          🗑
        </button>
      </div>
      <div className="relight-side__row">
        <button type="button" disabled={!cloud} onClick={() => zoomBy(-250)} title="Zoom in" aria-label="Zoom in">
          <ZoomIn size={13} />
        </button>
        <button type="button" disabled={!cloud} onClick={() => zoomBy(250)} title="Zoom out" aria-label="Zoom out">
          <ZoomOut size={13} />
        </button>
        <button
          type="button"
          disabled={!cloud}
          onClick={() => {
            if (!cloud) return;
            orbit.current = overview(cloud);
            setFrameTick((t) => t + 1);
          }}
          title="Reset the view: the whole studio from above"
          aria-label="Reset view"
        >
          ⌂
        </button>
      </div>
      {docked ? (
        onUndock && (
          <button type="button" className="relight-side__text btn-undock" onClick={onUndock} title="Float the lighting space over the photo">
            Undock
          </button>
        )
      ) : (
        <button type="button" className="relight-side__text btn-redock" onClick={onRedock} title="Put the lighting space back in the Relight panel">
          Redock
        </button>
      )}
    </div>
  );
  const framed = (
    <div className="relight-frame">
      {sideBar}
      <div className="relight-view">{scopeBody}</div>
    </div>
  );

  if (docked) return framed;

  const shown = box ?? null;
  return (
    <div
      className="relightscope"
      ref={rootRef}
      style={shown ? { left: shown.left, top: shown.top, width: shown.w, height: shown.h } : undefined}
      onPointerDown={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.preventDefault()}
    >
      <div className="relightscope__head" onPointerDown={startMove} onDoubleClick={() => setBox(defaultBox())} title="Drag to move; double-click to reset the size">
        <span className="relightscope__title">Relight space</span>
        <span className="relightscope__meta">{light ? `Light ${light.id}` : "No light selected"}</span>
      </div>
      <div className="relightscope__body">{framed}</div>
      <div className="relightscope__grip" onPointerDown={startResize} title="Drag to resize" />
    </div>
  );
}
