import { type Vec3, cross, dot, normalize } from "./math";

/**
 * The studio light drawn in Relight's 3D space as the real thing it is named after: a softbox, a
 * strip box, an octabox, a shoot-through umbrella, a beauty dish, a snoot with a grid, a bare head
 * or a speedlight with a bounce card, on a stand with tripod legs. Pure geometry in the scope's
 * world (metres, y up): faces and lines, each with a material; `RelightScope` projects, shades and
 * sorts them. Our own drawing, no third-party assets.
 */

export type Material = "fabric" | "diffuser" | "silver" | "head" | "canopy" | "canopyRib" | "card" | "grid" | "bulb";

export interface RigFace {
  pts: Vec3[];
  mat: Material;
}

export interface RigLine {
  a: Vec3;
  b: Vec3;
  kind: "stand" | "leg" | "shaft" | "grid";
}

export interface Rig {
  faces: RigFace[];
  lines: RigLine[];
  /** The light's emitting face: its centre and radius, where the beam starts. */
  front: Vec3;
  frontR: number;
}

/** The light's own frame: `f` where it faces, `r` its right, `u` its up (kept level, so a box
 *  never rolls as it turns). */
export function frameOf(dir: Vec3): { f: Vec3; r: Vec3; u: Vec3 } {
  const f = normalize(dir);
  const ref: Vec3 = Math.abs(f[1]) > 0.96 ? [0, 0, 1] : [0, 1, 0];
  const r = normalize(cross(ref, f));
  const u = normalize(cross(f, r));
  return { f, r, u };
}

/** A ring of `n` points `rad` out from the axis, `c` along it (an ellipse with `ry`). */
function ring(at: (a: number, b: number, c: number) => Vec3, n: number, rad: number, c: number, ry = rad, phase = 0): Vec3[] {
  const out: Vec3[] = [];
  for (let i = 0; i < n; i++) {
    const t = phase + (i / n) * Math.PI * 2;
    out.push(at(Math.cos(t) * rad, Math.sin(t) * ry, c));
  }
  return out;
}

/** A rectangle's four corners, `w` × `h`, `c` along the axis. */
function rect(at: (a: number, b: number, c: number) => Vec3, w: number, h: number, c: number): Vec3[] {
  return [at(-w / 2, -h / 2, c), at(w / 2, -h / 2, c), at(w / 2, h / 2, c), at(-w / 2, h / 2, c)];
}

/** The side quads joining two loops of the same count, and optionally their caps. */
function tube(faces: RigFace[], back: Vec3[], front: Vec3[], mat: Material) {
  for (let i = 0; i < back.length; i++) {
    const j = (i + 1) % back.length;
    faces.push({ pts: [back[i], back[j], front[j], front[i]], mat });
  }
}

/**
 * A light's rig. `type` is a `LIGHT_TYPES` id; `size` 0–100 scales the modifier as the slider
 * does; `k` scales the whole rig for a far scene, so a light 30 m out is still something to grab;
 * `floorY` is where the stand's feet go.
 */
export function buildRig(type: string, stand: Vec3, dir: Vec3, size: number, k: number, floorY: number): Rig {
  const { f, r, u } = frameOf(dir);
  const s = Math.max(0, Math.min(1, size / 100));
  const at = (a: number, b: number, c: number): Vec3 => [
    stand[0] + (r[0] * a + u[0] * b + f[0] * c) * k,
    stand[1] + (r[1] * a + u[1] * b + f[1] * c) * k,
    stand[2] + (r[2] * a + u[2] * b + f[2] * c) * k,
  ];
  const faces: RigFace[] = [];
  const lines: RigLine[] = [];
  let frontR = 0.1;
  // Where the strobe's head starts (behind the modifier) and how long it is.
  let headAt = -0.1;
  let headLen = 0.24;
  let headR = 0.075;

  switch (type) {
    case "softbox":
    case "rim":
    case "fill": {
      // Softbox: a tall rectangle. Rim: a strip box, narrow and tall. Fill: an octabox.
      const octa = type === "fill";
      let front: Vec3[];
      let back: Vec3[];
      let depth: number;
      if (octa) {
        const R = 0.45 + 0.5 * s;
        depth = 0.42 * R;
        front = ring(at, 8, R, 0, R, Math.PI / 8);
        back = ring(at, 8, 0.1, -depth, 0.1, Math.PI / 8);
        frontR = R;
      } else {
        const w = type === "rim" ? 0.26 + 0.1 * s : 0.5 + 0.5 * s;
        const h = type === "rim" ? 0.9 + 0.6 * s : w * 1.35;
        depth = type === "rim" ? 0.26 : 0.22 + 0.3 * w;
        front = rect(at, w, h, 0);
        back = rect(at, 0.16, 0.16, -depth);
        frontR = Math.max(w, h) / 2;
      }
      tube(faces, back, front, "fabric");
      faces.push({ pts: back, mat: "fabric" });
      // The front diffuser, with its recessed baffle edge.
      faces.push({ pts: front, mat: "diffuser" });
      headAt = -depth;
      break;
    }
    case "umbrella": {
      // A shoot-through umbrella: the canopy bulges toward the subject, its shaft runs back through
      // the head. Alternate panels between the ribs.
      const R = 0.4 + 0.4 * s;
      const n = 16;
      const apex = at(0, 0, 0.12 + 0.32 * R);
      const rim = ring(at, n, R, 0);
      const mid = ring(at, n, R * 0.62, 0.08 + 0.22 * R);
      for (let i = 0; i < n; i++) {
        const j = (i + 1) % n;
        const mat: Material = i % 2 ? "canopy" : "canopyRib";
        faces.push({ pts: [rim[i], rim[j], mid[j], mid[i]], mat });
        faces.push({ pts: [mid[i], mid[j], apex], mat });
      }
      lines.push({ a: apex, b: at(0, 0, -0.45), kind: "shaft" });
      frontR = R;
      headAt = -0.18;
      break;
    }
    case "dish": {
      // Beauty dish: a wide, shallow silver bowl with a small deflector in front of the bulb.
      const R = 0.22 + 0.14 * s;
      const n = 20;
      const back = ring(at, n, 0.09, -0.2);
      const mid = ring(at, n, R * 0.72, -0.09);
      const rim = ring(at, n, R, 0);
      tube(faces, back, mid, "silver");
      tube(faces, mid, rim, "silver");
      faces.push({ pts: back, mat: "head" });
      faces.push({ pts: ring(at, 14, 0.075, -0.01), mat: "silver" });
      frontR = R;
      headAt = -0.2;
      break;
    }
    case "grid": {
      // A snoot: a narrowing tube, and a honeycomb grid across its mouth.
      const n = 14;
      const back = ring(at, n, 0.085, -0.3);
      const front = ring(at, n, 0.055, 0);
      tube(faces, back, front, "head");
      faces.push({ pts: front, mat: "grid" });
      for (const t of [-0.6, -0.2, 0.2, 0.6]) {
        const w = Math.sqrt(1 - t * t) * 0.055;
        lines.push({ a: at(-w, t * 0.055, 0.001), b: at(w, t * 0.055, 0.001), kind: "grid" });
        lines.push({ a: at(t * 0.055, -w, 0.001), b: at(t * 0.055, w, 0.001), kind: "grid" });
      }
      frontR = 0.055;
      headAt = -0.3;
      break;
    }
    case "bounce": {
      // A speedlight: a small body, its head, and a white bounce card standing up off the head.
      const m = 1;
      const body = [rect(at, 0.07 * m, 0.05 * m, -0.2 * m), rect(at, 0.07 * m, 0.05 * m, -0.06 * m)];
      tube(faces, body[0], body[1], "head");
      faces.push({ pts: body[0], mat: "head" });
      const hb = rect(at, 0.08 * m, 0.045 * m, -0.06 * m);
      const hf = rect(at, 0.08 * m, 0.045 * m, 0 * m);
      tube(faces, hb, hf, "head");
      faces.push({ pts: hf, mat: "bulb" });
      faces.push({ pts: [at(-0.05 * m, 0.02 * m, 0.005 * m), at(0.05 * m, 0.02 * m, 0.005 * m), at(0.05 * m, 0.14 * m, -0.01 * m), at(-0.05 * m, 0.14 * m, -0.01 * m)], mat: "card" });
      frontR = 0.06 * m;
      headAt = -0.2 * m;
      headLen = 0;
      break;
    }
    default: {
      // Bare head: the standard little reflector and a lit flash tube inside it.
      const n = 16;
      const back = ring(at, n, 0.08, -0.09);
      const front = ring(at, n, 0.115, 0);
      tube(faces, back, front, "silver");
      faces.push({ pts: ring(at, 12, 0.05, -0.05), mat: "bulb" });
      frontR = 0.11;
      headAt = -0.09;
      break;
    }
  }

  if (headLen > 0) {
    const n = 12;
    const a = ring(at, n, headR, headAt);
    const b = ring(at, n, headR, headAt - headLen);
    tube(faces, a, b, "head");
    faces.push({ pts: b, mat: "head" });
  }

  // The stand: from the head's mount straight down to the floor, and three tripod legs.
  const mount = at(0, -0.04, headAt - headLen * 0.5);
  if (mount[1] > floorY + 0.05) {
    const legH = Math.min(0.32 * k, (mount[1] - floorY) * 0.5);
    const collar: Vec3 = [mount[0], floorY + legH, mount[2]];
    lines.push({ a: mount, b: [mount[0], floorY + legH * 0.4, mount[2]], kind: "stand" });
    const spread = 0.36 * k;
    for (let i = 0; i < 3; i++) {
      const t = (i / 3) * Math.PI * 2 + Math.PI / 2;
      lines.push({ a: collar, b: [mount[0] + Math.cos(t) * spread, floorY, mount[2] + Math.sin(t) * spread], kind: "leg" });
    }
  }

  return { faces, lines, front: at(0, 0, 0.01), frontR: frontR * k };
}

/** A face's outward normal (Newell's method), for shading. */
export function faceNormal(pts: Vec3[]): Vec3 {
  let x = 0;
  let y = 0;
  let z = 0;
  for (let i = 0; i < pts.length; i++) {
    const a = pts[i];
    const b = pts[(i + 1) % pts.length];
    x += (a[1] - b[1]) * (a[2] + b[2]);
    y += (a[2] - b[2]) * (a[0] + b[0]);
    z += (a[0] - b[0]) * (a[1] + b[1]);
  }
  return normalize([x, y, z]);
}

export function centroid(pts: Vec3[]): Vec3 {
  const c: Vec3 = [0, 0, 0];
  for (const p of pts) {
    c[0] += p[0];
    c[1] += p[1];
    c[2] += p[2];
  }
  return [c[0] / pts.length, c[1] / pts.length, c[2] / pts.length];
}

/** A face's colour: the material lit by a fixed key from the upper left of the viewer, so the
 *  rig reads as solid; `glow` is the light's own colour for the parts that emit. */
export function shade(mat: Material, n: Vec3, viewDir: Vec3, glow: [number, number, number]): string {
  const key = normalize([-0.4, 0.7, -0.6]);
  const facing = Math.abs(dot(n, key));
  const rim = 1 - Math.abs(dot(n, viewDir));
  const mix = (c: [number, number, number], l: number) => `rgb(${c.map((v) => Math.round(Math.min(255, v * l))).join(",")})`;
  switch (mat) {
    case "fabric":
      return mix([46, 50, 56], 0.55 + 0.6 * facing + 0.25 * rim);
    case "head":
      return mix([58, 64, 72], 0.5 + 0.7 * facing + 0.2 * rim);
    case "silver":
      return mix([170, 178, 186], 0.5 + 0.55 * facing + 0.25 * rim);
    case "canopy":
      return mix([236, 238, 240], 0.62 + 0.38 * facing);
    case "canopyRib":
      return mix([212, 216, 222], 0.6 + 0.38 * facing);
    case "card":
      return mix([245, 245, 245], 0.75 + 0.25 * facing);
    case "grid":
      return mix([40, 44, 48], 1);
    case "bulb":
    case "diffuser": {
      const w: [number, number, number] = [
        255 * 0.55 + glow[0] * 0.45,
        255 * 0.55 + glow[1] * 0.45,
        255 * 0.55 + glow[2] * 0.45,
      ];
      return mix(w, 0.92 + 0.08 * facing);
    }
  }
}
