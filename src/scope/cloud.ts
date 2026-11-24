/**
 * The Depth Scope's point cloud: `scene_points`' binary (base64) turned into what the graphics card
 * and the picking want, without blocking the interface for long: the decode is done in chunks, a
 * yield to the browser between each, so the pointer stays live while a 300k-point cloud loads.
 *
 * Format version 2 (see `scene.rs`): "R1PT", a u16 version, a u32 count, then 13 bytes a point:
 * x, y, z as half-precision floats (metres, camera space: x right, y down, z forward), r, g, b as
 * bytes, and u, v as u16 (the point's place in the upright, uncropped frame, x 65535).
 *
 * World space here has y up (the camera space's y is negated once), so the picture is the right
 * way up on screen.
 */

export const POINTS_MAGIC = "R1PT";
export const POINTS_VERSION = 2;
export const POINT_BYTES = 13;
export const HEADER_BYTES = 10;

export interface ScopeCloud {
  count: number;
  /** x, y, z (world: metres, y up) and r, g, b (0..1): 6 floats a point, as the card takes them. */
  data: Float32Array;
  /** u, v (0..1: the point's place in the upright, uncropped photo): 2 floats a point. */
  uv: Float32Array;
  /** The nearest and farthest point, metres along the optical axis. */
  zMin: number;
  zMax: number;
  /** The middle distance: what the camera orbits around, and how far it starts. */
  medianZ: number;
  /** The lowest point (world y is up, so this is the smallest y): where the ground grid sits. */
  floorY: number;
  /** The highest point. */
  ceilY: number;
  /** Which of the model's sizes this came from, for the readout ("MoGe-2 ViT-B"). */
  points: number;
}

/** IEEE half (as `scene.rs` writes it) to a number. */
export function halfToFloat(h: number): number {
  const sign = h & 0x8000 ? -1 : 1;
  const exp = (h >> 10) & 0x1f;
  const mant = h & 0x3ff;
  if (exp === 0) return sign * mant * 5.960464477539063e-8; // 2^-24: subnormal
  if (exp === 31) return mant ? NaN : sign * Infinity;
  return sign * (mant + 1024) * Math.pow(2, exp - 25);
}

function base64ToBytes(base64: string): Uint8Array {
  const clean = base64.includes(",") ? base64.slice(base64.indexOf(",") + 1) : base64;
  const bin = atob(clean);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

const yieldToBrowser = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** Points decoded between two yields: small enough to keep a frame under a few milliseconds. */
const CHUNK = 40_000;

/**
 * `scene_points`' base64 turned into a cloud. Throws (with what's wrong) on anything that isn't a
 * version 2 cloud: a short buffer, another version, or a count that doesn't match the length.
 */
export async function decodeCloud(base64: string): Promise<ScopeCloud> {
  const bytes = base64ToBytes(base64);
  const text = String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3]);
  if (text !== POINTS_MAGIC) throw new Error("not a point cloud");
  const version = bytes[4] | (bytes[5] << 8);
  if (version !== POINTS_VERSION) throw new Error(`point cloud version ${version}, expected ${POINTS_VERSION}`);
  const count = (bytes[6] | (bytes[7] << 8) | (bytes[8] << 16) | (bytes[9] << 24)) >>> 0;
  if (bytes.length !== HEADER_BYTES + count * POINT_BYTES) throw new Error("the point cloud's length doesn't match its count");

  const data = new Float32Array(count * 6);
  const uv = new Float32Array(count * 2);
  const view = new DataView(bytes.buffer, bytes.byteOffset + HEADER_BYTES, count * POINT_BYTES);
  const zs: number[] = [];
  // A sample of the distances, for the median (the orbit's centre): about 4000 of them.
  const sampleEvery = Math.max(1, Math.floor(count / 4096));
  let zMin = Infinity;
  let zMax = -Infinity;
  let floorY = Infinity;
  let ceilY = -Infinity;

  for (let i = 0; i < count; i++) {
    const at = i * POINT_BYTES;
    const x = halfToFloat(view.getUint16(at, true));
    const y = halfToFloat(view.getUint16(at + 2, true));
    const z = halfToFloat(view.getUint16(at + 4, true));
    // y down (the camera's) becomes y up (the world's).
    data[i * 6] = x;
    data[i * 6 + 1] = -y;
    data[i * 6 + 2] = z;
    data[i * 6 + 3] = bytes[HEADER_BYTES + at + 6] / 255;
    data[i * 6 + 4] = bytes[HEADER_BYTES + at + 7] / 255;
    data[i * 6 + 5] = bytes[HEADER_BYTES + at + 8] / 255;
    uv[i * 2] = view.getUint16(at + 9, true) / 65535;
    uv[i * 2 + 1] = view.getUint16(at + 11, true) / 65535;
    if (Number.isFinite(z) && z > 0) {
      if (z < zMin) zMin = z;
      if (z > zMax) zMax = z;
      // A sample of the distances, for the median (the orbit's centre).
      if (i % sampleEvery === 0) zs.push(z);
    }
    const wy = -y;
    if (Number.isFinite(wy)) {
      if (wy < floorY) floorY = wy;
      if (wy > ceilY) ceilY = wy;
    }
    if (i % CHUNK === CHUNK - 1) await yieldToBrowser();
  }
  if (!Number.isFinite(zMin) || !Number.isFinite(zMax)) throw new Error("the point cloud has no real geometry");
  zs.sort((a, b) => a - b);
  const medianZ = zs.length ? zs[zs.length >> 1] : (zMin + zMax) / 2;
  return {
    count,
    data,
    uv,
    zMin,
    zMax,
    medianZ: Math.max(medianZ, zMin),
    floorY: Number.isFinite(floorY) ? floorY : 0,
    ceilY: Number.isFinite(ceilY) ? ceilY : 0,
    points: count,
  };
}

/** Point `i`'s world position (x, y up, z). */
export function pointAt(cloud: ScopeCloud, i: number): [number, number, number] {
  return [cloud.data[i * 6], cloud.data[i * 6 + 1], cloud.data[i * 6 + 2]];
}

/** Point `i`'s place in the photo, 0..1. */
export function uvAt(cloud: ScopeCloud, i: number): [number, number] {
  return [cloud.uv[i * 2], cloud.uv[i * 2 + 1]];
}
