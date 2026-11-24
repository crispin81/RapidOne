/**
 * The Depth Scope's maths, in plain TypeScript with no dependencies: a small column-major matrix
 * library, the orbit camera, the ray a pixel casts, and the log scale the depth mask's control
 * values run on (0..1, 1 = nearest, as `scene.rs` and the depth bar use).
 *
 * World space is the scene's camera space with y up: x right, y up, z forward (into the picture),
 * metres. `scene.rs` gives y down, so the cloud is decoded with y negated once.
 *
 * Everything here is pure, so `scripts/scope-selftest.ts` checks it under node without a browser.
 */

export type Vec3 = [number, number, number];
/** A 4 x 4 matrix, column-major, as WebGL wants it. */
export type Mat4 = Float32Array;

export function vec3(x: number, y: number, z: number): Vec3 {
  return [x, y, z];
}

export function add(a: Vec3, b: Vec3): Vec3 {
  return [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
}

export function sub(a: Vec3, b: Vec3): Vec3 {
  return [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
}

export function scale(a: Vec3, s: number): Vec3 {
  return [a[0] * s, a[1] * s, a[2] * s];
}

export function dot(a: Vec3, b: Vec3): number {
  return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
}

export function cross(a: Vec3, b: Vec3): Vec3 {
  return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
}

export function length(a: Vec3): number {
  return Math.hypot(a[0], a[1], a[2]);
}

export function normalize(a: Vec3): Vec3 {
  const l = length(a);
  return l > 1e-12 ? [a[0] / l, a[1] / l, a[2] / l] : [0, 0, 1];
}

export function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

/** 0 below `edge0`, 1 above `edge1`, smooth between (as `gradient.rs`'s `smooth`). */
export function smoothstep(edge0: number, edge1: number, v: number): number {
  if (edge1 <= edge0) return v >= edge1 ? 1 : 0;
  const t = clamp((v - edge0) / (edge1 - edge0), 0, 1);
  return t * t * (3 - 2 * t);
}

/** A right-handed perspective projection (OpenGL's clip space, z -1..1), column-major. */
export function perspective(fovYDeg: number, aspect: number, near: number, far: number): Mat4 {
  const f = 1 / Math.tan((fovYDeg * Math.PI) / 360);
  const nf = 1 / (near - far);
  const m = new Float32Array(16);
  m[0] = f / Math.max(aspect, 1e-6);
  m[5] = f;
  m[10] = (far + near) * nf;
  m[11] = -1;
  m[14] = 2 * far * near * nf;
  return m;
}

/**
 * The view matrix for a camera at `eye` looking along `forward` (unit), with `up` (unit) as its
 * up. Eye space is OpenGL's: x right, y up, and the camera looks down -z.
 */
export function viewMatrix(eye: Vec3, forward: Vec3, up: Vec3): Mat4 {
  const right = normalize(cross(up, forward));
  const u = cross(forward, right);
  const m = new Float32Array(16);
  m[0] = right[0];
  m[4] = right[1];
  m[8] = right[2];
  m[12] = -dot(right, eye);
  m[1] = u[0];
  m[5] = u[1];
  m[9] = u[2];
  m[13] = -dot(u, eye);
  m[2] = -forward[0];
  m[6] = -forward[1];
  m[10] = -forward[2];
  m[14] = dot(forward, eye);
  m[15] = 1;
  return m;
}

/** `a * b`, both column-major: the matrix that applies `b` first, then `a`. */
export function multiply(a: Mat4, b: Mat4): Mat4 {
  const out = new Float32Array(16);
  for (let c = 0; c < 4; c++) {
    for (let r = 0; r < 4; r++) {
      out[c * 4 + r] =
        a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
    }
  }
  return out;
}

/** `m * p` as (x, y, z, w), w left in: the caller divides for a perspective. */
export function transform4(m: Mat4, p: Vec3): [number, number, number, number] {
  return [
    m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
    m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
    m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15],
  ];
}

/** A point's place on screen (pixels, y down from the top) and its depth in the clip's -1..1. */
export interface ScreenPoint {
  x: number;
  y: number;
  /** Clip-space z: -1 at the near plane, 1 at the far one. */
  z: number;
  /** False when the point is behind the camera (nothing to draw). */
  inFront: boolean;
}

/** A world point through `mvp` (projection x view) onto a `width` x `height` picture. */
export function projectToScreen(mvp: Mat4, p: Vec3, width: number, height: number): ScreenPoint {
  const [x, y, z, w] = transform4(mvp, p);
  if (!(w > 1e-9)) return { x: 0, y: 0, z: 1, inFront: false };
  return {
    x: ((x / w) * 0.5 + 0.5) * width,
    y: (0.5 - (y / w) * 0.5) * height,
    z: z / w,
    inFront: true,
  };
}

// --- The orbit camera -------------------------------------------------------------------------

/**
 * Where the camera is: yaw and pitch (radians) around the target, and its distance from it. The
 * photo's own viewpoint is yaw 0, pitch 0, target (0, 0, z) and distance z: the camera then sits
 * at the origin looking down +z, exactly as the lens saw the scene.
 */
export interface Orbit {
  yaw: number;
  pitch: number;
  distance: number;
  target: Vec3;
}

export interface CameraFrame {
  eye: Vec3;
  /** Unit, from the camera into the scene. */
  forward: Vec3;
  right: Vec3;
  up: Vec3;
}

/** The camera's position and axes for an orbit. */
export function orbitCamera(o: Orbit): CameraFrame {
  const cp = Math.cos(o.pitch);
  const sp = Math.sin(o.pitch);
  const forward: Vec3 = [Math.sin(o.yaw) * cp, sp, Math.cos(o.yaw) * cp];
  const eye: Vec3 = [o.target[0] - forward[0] * o.distance, o.target[1] - forward[1] * o.distance, o.target[2] - forward[2] * o.distance];
  const right = normalize(cross([0, 1, 0], forward));
  return { eye, forward, right, up: cross(forward, right) };
}

/** The photo's own viewpoint for a scene whose depth is around `z`. */
export function photoView(z: number): Orbit {
  return { yaw: 0, pitch: 0, distance: Math.max(z, 0.05), target: [0, 0, Math.max(z, 0.05)] };
}

/** The furthest the pitch may go before the up vector flips (a little inside a right angle). */
export const MAX_PITCH = 1.45;

/**
 * The orbit after dragging the pointer by (dx, dy) pixels: the scene turns with the drag (so
 * dragging down looks at it from above), and the pitch stops just short of straight up or down.
 */
export function orbitDrag(o: Orbit, dxPx: number, dyPx: number, speed = 0.006): Orbit {
  return {
    ...o,
    // As Blender's (and Maya's) orbit: the scene turns with the hand, so its front follows the
    // pointer both ways (it used to turn against the pointer sideways).
    yaw: o.yaw + dxPx * speed,
    pitch: clamp(o.pitch - dyPx * speed, -MAX_PITCH, MAX_PITCH),
  };
}

/**
 * The orbit after panning by (dx, dy) pixels: the target moves with the pointer across the
 * camera's own right and up, by however many metres a pixel covers at the target's distance.
 */
export function panOrbit(o: Orbit, dxPx: number, dyPx: number, viewH: number, fovYDeg: number): Orbit {
  const cam = orbitCamera(o);
  const perPixel = (2 * o.distance * Math.tan((fovYDeg * Math.PI) / 360)) / Math.max(viewH, 1);
  const move: Vec3 = [
    -cam.right[0] * dxPx * perPixel + cam.up[0] * dyPx * perPixel,
    -cam.right[1] * dxPx * perPixel + cam.up[1] * dyPx * perPixel,
    -cam.right[2] * dxPx * perPixel + cam.up[2] * dyPx * perPixel,
  ];
  return { ...o, target: add(o.target, move) };
}

/** The orbit after a wheel notch: `delta` positive moves away from the target. */
export function zoomOrbit(o: Orbit, delta: number, min: number, max: number): Orbit {
  return { ...o, distance: clamp(o.distance * Math.exp(delta * 0.0016), min, max) };
}

// --- Rays and planes ---------------------------------------------------------------------------

export interface Ray {
  origin: Vec3;
  /** Unit. */
  dir: Vec3;
}

/** The ray through a pixel (x, y from the top left) of a `width` x `height` view. */
export function rayFromScreen(x: number, y: number, width: number, height: number, cam: CameraFrame, fovYDeg: number): Ray {
  const t = Math.tan((fovYDeg * Math.PI) / 360);
  const aspect = width / Math.max(height, 1);
  const ndcX = (x / Math.max(width, 1)) * 2 - 1;
  const ndcY = 1 - (y / Math.max(height, 1)) * 2;
  const dir = normalize([
    cam.forward[0] + cam.right[0] * ndcX * t * aspect + cam.up[0] * ndcY * t,
    cam.forward[1] + cam.right[1] * ndcX * t * aspect + cam.up[1] * ndcY * t,
    cam.forward[2] + cam.right[2] * ndcX * t * aspect + cam.up[2] * ndcY * t,
  ]);
  return { origin: cam.eye, dir };
}

/** How far along `ray` the plane z = `z` is, or null when the ray never reaches it in front. */
export function rayPlaneZ(ray: Ray, z: number): number | null {
  if (Math.abs(ray.dir[2]) < 1e-9) return null;
  const t = (z - ray.origin[2]) / ray.dir[2];
  return t > 0 ? t : null;
}

/** Where the ray meets the plane z = `z` (null when it doesn't, in front). */
export function rayAtZ(ray: Ray, z: number): Vec3 | null {
  const t = rayPlaneZ(ray, z);
  if (t === null) return null;
  return [ray.origin[0] + ray.dir[0] * t, ray.origin[1] + ray.dir[1] * t, ray.origin[2] + ray.dir[2] * t];
}

/**
 * The distance a plane at z = `z0` moves to when the pointer drags by (dx, dy) pixels. A plane is
 * grabbed by its label, at the middle of the picture: seen from the side, that middle slides along
 * the optical axis as the plane goes away, and the drag follows it. From the photo's own viewpoint
 * the middle doesn't move at all (a plane fills the frame at any distance), so there the drag is
 * read vertically: up sends the plane away, down brings it nearer.
 */
export function depthDragZ(mvp: Mat4, width: number, height: number, z0: number, dxPx: number, dyPx: number): number {
  const z = Math.max(z0, 1e-3);
  const p0 = projectToScreen(mvp, [0, 0, z], width, height);
  const p1 = projectToScreen(mvp, [0, 0, z * 1.05], width, height);
  if (p0.inFront && p1.inFront) {
    const ux = p1.x - p0.x;
    const uy = p1.y - p0.y;
    const len = Math.hypot(ux, uy);
    if (len > 1.5) {
      const along = (dxPx * ux + dyPx * uy) / len;
      return clamp(z * Math.pow(1.05, along / len), 1e-3, 1e6);
    }
  }
  return clamp(z * Math.exp(-dyPx * 0.005), 1e-3, 1e6);
}

// --- Depth: metres and control values ----------------------------------------------------------

/**
 * The metres a control-depth value stands for (0..1, 1 = nearest), read back through the scene's
 * log scale between its robust anchors (`scene.rs`'s `Scene3D::metres`). 0 is the sky: Infinity.
 */
export function metresOfControl(control: number, dNear: number, dFar: number): number {
  const c = clamp(control, 0, 1);
  if (!(dNear > 0) || !(dFar > 0) || dFar <= dNear) return dNear > 0 ? dNear : 1;
  if (c <= 0) return Infinity;
  return dNear * Math.pow(dFar / dNear, 1 - c);
}

/** The control-depth value for a distance in metres (the inverse of `metresOfControl`). */
export function controlOfMetres(metres: number, dNear: number, dFar: number): number {
  if (!Number.isFinite(metres) || metres <= 0 || !(dNear > 0) || !(dFar > 0) || dFar <= dNear) return 0;
  const denom = Math.log(dFar / dNear);
  if (Math.abs(denom) < 1e-9) return 0.5;
  return clamp(1 - Math.log(metres / dNear) / denom, 0, 1);
}

/**
 * How much a point at `control` belongs to the band: 1 between `far` and `near`, easing to 0 over
 * `fade` either side (as `gradient.rs`'s `Grid::depth_band`, the mask itself).
 */
export function bandWeight(control: number, near: number, far: number, fade: number): number {
  const n = Math.max(near, far);
  const f = Math.min(near, far);
  const d = Math.max(0, Math.min(1, fade));
  if (d <= 1e-4) return control >= f && control <= n ? 1 : 0;
  if (control > n) return 1 - smoothstep(n, n + d, control);
  if (control < f) return 1 - smoothstep(f - d, f, control);
  return 1;
}

// --- Smoothing ---------------------------------------------------------------------------------

/** `current` eased towards `target` by `rate` over `dt` seconds (frame-rate independent). */
export function damp(current: number, target: number, dt: number, rate = 16): number {
  return current + (target - current) * (1 - Math.exp(-rate * Math.max(0, dt)));
}

export function dampVec3(current: Vec3, target: Vec3, dt: number, rate = 16): Vec3 {
  return [damp(current[0], target[0], dt, rate), damp(current[1], target[1], dt, rate), damp(current[2], target[2], dt, rate)];
}

/** Whether two numbers agree to within `eps` (a damped value has arrived). */
export function near(a: number, b: number, eps = 1e-4): boolean {
  return Math.abs(a - b) <= eps * Math.max(1, Math.abs(a), Math.abs(b));
}

// --- The frame at a distance -------------------------------------------------------------------

/**
 * Half the width and height (metres) of the photo's own frame at distance `z`, for a horizontal
 * field of view `fovXDeg` and a picture `aspect` (width / height): the size a depth plane spans.
 */
export function frameHalfSize(z: number, fovXDeg: number, aspect: number): [number, number] {
  const halfW = Math.tan((fovXDeg * Math.PI) / 360) * z;
  return [halfW, halfW / Math.max(aspect, 1e-6)];
}

/** A vertical field of view (degrees) from the horizontal one and the picture's aspect. */
export function fovYFromFovX(fovXDeg: number, aspect: number): number {
  const halfX = Math.tan((fovXDeg * Math.PI) / 360);
  return (2 * Math.atan(halfX / Math.max(aspect, 1e-6)) * 180) / Math.PI;
}

/** Rodrigues' rotation of `v` about `axis` by `angle` radians. */
export function rotateAround(v: Vec3, axis: Vec3, angle: number): Vec3 {
  const k = normalize(axis);
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  const kv = cross(k, v);
  const kd = dot(k, v) * (1 - c);
  return [v[0] * c + kv[0] * s + k[0] * kd, v[1] * c + kv[1] * s + k[1] * kd, v[2] * c + kv[2] * s + k[2] * kd];
}
