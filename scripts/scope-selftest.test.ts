/**
 * The Depth Scope's maths, checked without a browser or any new package: node runs TypeScript
 * directly (type stripping) and brings its own test runner, so `npm run test:scope` needs nothing
 * installed. Run it with: node --test scripts/scope-selftest.test.ts
 *
 * What's covered is what the scope relies on and can't see: the orbit camera's start (the photo's
 * own viewpoint), projection (right is right, up is up), the pixel's ray, ray-plane hits, the
 * metres/control log scale the depth bar and the mask share, the band's fade, and dragging a
 * plane by its label.
 */

import test from "node:test";
import assert from "node:assert/strict";
import {
  MAX_PITCH,
  bandWeight,
  controlOfMetres,
  depthDragZ,
  fovYFromFovX,
  frameHalfSize,
  metresOfControl,
  multiply,
  orbitCamera,
  orbitDrag,
  panOrbit,
  perspective,
  photoView,
  projectToScreen,
  rayAtZ,
  rayFromScreen,
  viewMatrix,
  zoomOrbit,
  type Vec3,
} from "../src/scope/math.ts";
import { decodeCloud, halfToFloat, pointAt, uvAt } from "../src/scope/cloud.ts";

const close = (a: number, b: number, eps = 1e-6) => assert.ok(Math.abs(a - b) <= eps, `${a} vs ${b}`);

test("the photo's viewpoint is the origin, looking down +z", () => {
  const cam = orbitCamera(photoView(4));
  close(cam.eye[0], 0);
  close(cam.eye[1], 0);
  close(cam.eye[2], 0);
  close(cam.forward[2], 1);
  close(cam.forward[0], 0);
  close(cam.up[1], 1, 1e-9);
  close(cam.right[0], 1, 1e-9);
});

test("a point on the optical axis lands in the middle, +x to the right and +y up", () => {
  const w = 800;
  const h = 500;
  const o = photoView(5);
  const cam = orbitCamera(o);
  const view = viewMatrix(cam.eye, cam.forward, cam.up);
  const mvp = multiply(perspective(fovYFromFovX(60, w / h), w / h, 0.02, 1000), view);
  const centre = projectToScreen(mvp, [0, 0, 5], w, h);
  assert.ok(centre.inFront);
  close(centre.x, w / 2, 1e-3);
  close(centre.y, h / 2, 1e-3);
  const right = projectToScreen(mvp, [1, 0, 5], w, h);
  assert.ok(right.x > centre.x, "x is to the right");
  const up = projectToScreen(mvp, [0, 1, 5], w, h);
  assert.ok(up.y < centre.y, "y is up");
  const behind = projectToScreen(mvp, [0, 0, -1], w, h);
  assert.equal(behind.inFront, false, "behind the camera is nothing to draw");
});

test("the pixel's ray spans the photo's own field of view at that distance", () => {
  const w = 1000;
  const h = 600;
  const fovX = 70;
  const fovY = fovYFromFovX(fovX, w / h);
  const cam = orbitCamera(photoView(3));
  const middle = rayFromScreen(w / 2, h / 2, w, h, cam, fovY);
  close(middle.dir[0], 0, 1e-9);
  close(middle.dir[1], 0, 1e-9);
  close(middle.dir[2], 1, 1e-9);
  // The right edge of the picture, at 4 m: half the frame's width across.
  const edge = rayFromScreen(w, h / 2, w, h, cam, fovY);
  const p = rayAtZ(edge, 4);
  assert.ok(p);
  const [hw] = frameHalfSize(4, fovX, w / h);
  close(p![0], hw, 1e-3);
  close(p![2], 4, 1e-6);
  // A ray that misses the plane behind the camera has no hit.
  const back = rayFromScreen(w / 2, h / 2, w, h, cam, fovY);
  assert.equal(rayAtZ(back, -2), null);
});

test("orbiting turns the scene with the drag and stops short of the poles", () => {
  // As Blender's orbit: the scene's front (the side towards the camera) follows the pointer.
  const start = { ...photoView(5), yaw: 0.2, pitch: -0.2 };
  const frontAt = (o: ReturnType<typeof photoView>) => {
    const c = orbitCamera(o);
    const mvp = multiply(perspective(50, 2, 0.05, 500), viewMatrix(c.eye, c.forward, c.up));
    return projectToScreen(mvp, [0, 0, 4.5], 600, 300);
  };
  const right = orbitDrag(start, 100, 0);
  assert.ok(frontAt(right).x > frontAt(start).x, "dragging right turns the scene's front to the right");
  const down = orbitDrag(start, 0, 100);
  assert.ok(down.pitch < start.pitch, "dragging down looks from above");
  assert.ok(frontAt(down).y > frontAt(start).y, "dragging down turns the scene's front down");
  const far = orbitDrag(start, 0, 100000);
  assert.ok(Math.abs(far.pitch) <= MAX_PITCH + 1e-9);
});

test("panning moves the scene with the pointer, by the pixel's own size", () => {
  const o = photoView(5);
  const fovY = 50;
  const viewH = 500;
  const moved = panOrbit(o, 100, 0, viewH, fovY);
  const perPixel = (2 * o.distance * Math.tan((fovY * Math.PI) / 360)) / viewH;
  close(moved.target[0], -100 * perPixel, 1e-9);
  const up = panOrbit(o, 0, -50, viewH, fovY);
  close(up.target[1], -50 * perPixel, 1e-9);
});

test("the wheel zooms in and out, within its limits", () => {
  const o = photoView(5);
  assert.ok(zoomOrbit(o, -100, 0.1, 100).distance < o.distance);
  assert.ok(zoomOrbit(o, 100, 0.1, 100).distance > o.distance);
  close(zoomOrbit(o, -100000, 0.1, 100).distance, 0.1);
  close(zoomOrbit(o, 100000, 0.1, 100).distance, 100);
});

test("metres and the depth bar's control values are one log scale, both ways", () => {
  const dNear = 0.8;
  const dFar = 40;
  close(metresOfControl(1, dNear, dFar), dNear, 1e-6);
  assert.equal(metresOfControl(0, dNear, dFar), Infinity, "the far end is the sky");
  close(metresOfControl(0.5, dNear, dFar), Math.sqrt(dNear * dFar), 1e-6);
  for (const c of [0.1, 0.25, 0.5, 0.75, 1]) {
    close(controlOfMetres(metresOfControl(c, dNear, dFar), dNear, dFar), c, 1e-6);
  }
  close(controlOfMetres(1e6, dNear, dFar), 0, 1e-9);
});

test("the band is flat inside and eases off over its fade", () => {
  close(bandWeight(0.7, 0.8, 0.4, 0.1), 1);
  close(bandWeight(0.85, 0.8, 0.4, 0.1), 0.5, 1e-6);
  close(bandWeight(0.9, 0.8, 0.4, 0.1), 0);
  close(bandWeight(0.35, 0.8, 0.4, 0.1), 0.5, 1e-6);
  close(bandWeight(0.2, 0.8, 0.4, 0), 0);
  close(bandWeight(0.6, 0.8, 0.4, 0), 1);
});

test("dragging a plane's label moves it along the axis, and a still press leaves it", () => {
  const w = 800;
  const h = 500;
  const fovX = 60;
  const aspect = w / h;
  const photo = photoView(6);
  const atPhoto = multiply(perspective(fovYFromFovX(fovX, aspect), aspect, 0.02, 1000), viewMatrix(orbitCamera(photo).eye, orbitCamera(photo).forward, orbitCamera(photo).up));
  // Still: the plane doesn't move.
  close(depthDragZ(atPhoto, w, h, 4, 0, 0), 4, 1e-9);
  // From the photo's own viewpoint a plane fills the frame at any distance, so the drag is read
  // vertically: up sends it away, down brings it nearer.
  assert.ok(depthDragZ(atPhoto, w, h, 4, 0, -50) > 4, "up: further");
  assert.ok(depthDragZ(atPhoto, w, h, 4, 0, 50) < 4, "down: nearer");
  close(depthDragZ(atPhoto, w, h, 4, 0, -100), 4 * Math.exp(0.5), 1e-6);
  // Seen from the side, the plane's middle slides along the axis: dragging along it moves the
  // plane by the same fraction of the way the middle moved.
  const side = { ...photo, yaw: 0.7 };
  const cam = orbitCamera(side);
  const mvp = multiply(perspective(fovYFromFovX(fovX, aspect), aspect, 0.02, 1000), viewMatrix(cam.eye, cam.forward, cam.up));
  const p0 = projectToScreen(mvp, [0, 0, 4], w, h);
  const p1 = projectToScreen(mvp, [0, 0, 4 * 1.05], w, h);
  const dx = p1.x - p0.x;
  const dy = p1.y - p0.y;
  assert.ok(Math.hypot(dx, dy) > 2, "the middle moves on screen from the side");
  close(depthDragZ(mvp, w, h, 4, dx, dy), 4 * 1.05, 1e-6);
  close(depthDragZ(mvp, w, h, 4, -dx, -dy), 4 / 1.05, 1e-6);
});

test("the frame's size at a distance is the photo's field of view", () => {
  const [hw, hh] = frameHalfSize(10, 90, 1.5);
  close(hw, 10, 1e-6);
  close(hh, 10 / 1.5, 1e-6);
  close(fovYFromFovX(90, 1), 90, 1e-9);
});

test("a plane's corners project as a rectangle on the photo's own viewpoint", () => {
  const w = 900;
  const h = 600;
  const cam = orbitCamera(photoView(4));
  const mvp = multiply(perspective(fovYFromFovX(60, w / h), w / h, 0.02, 1000), viewMatrix(cam.eye, cam.forward, cam.up));
  const corners: Vec3[] = [
    [-1, -1, 4],
    [1, -1, 4],
    [1, 1, 4],
    [-1, 1, 4],
  ];
  const s = corners.map((c) => projectToScreen(mvp, c, w, h));
  close(s[0].y, s[1].y, 1e-6);
  close(s[1].x, s[2].x, 1e-6);
  close(s[0].x + s[1].x, w, 1e-6);
});

// --- The cloud's binary (scene_points' version 2) ---------------------------------------------

/** A half-precision bit pattern for a value that's exact in half precision (a test fixture). */
function f16(v: number): number {
  const table: Record<string, number> = { "0": 0, "0.5": 0x3800, "1": 0x3c00, "1.5": 0x3e00, "2": 0x4000, "2.5": 0x4100, "3": 0x4200, "4": 0x4400, "6": 0x4600 };
  const h = table[String(Math.abs(v))];
  if (h === undefined) throw new Error(`no exact half for ${v}`);
  return (v < 0 ? 0x8000 : 0) | h;
}

/** A version 2 cloud as scene.rs writes it: header, then 13 bytes a point. */
function cloudBytes(points: { p: [number, number, number]; rgb: [number, number, number]; uv: [number, number] }[], version = 2): string {
  const bytes = new Uint8Array(10 + points.length * 13);
  bytes.set([0x52, 0x31, 0x50, 0x54], 0); // "R1PT"
  const view = new DataView(bytes.buffer);
  view.setUint16(4, version, true);
  view.setUint32(6, points.length, true);
  points.forEach(({ p, rgb, uv }, i) => {
    const at = 10 + i * 13;
    view.setUint16(at, f16(p[0]), true);
    view.setUint16(at + 2, f16(p[1]), true);
    view.setUint16(at + 4, f16(p[2]), true);
    bytes.set(rgb, at + 6);
    view.setUint16(at + 9, uv[0], true);
    view.setUint16(at + 11, uv[1], true);
  });
  // Node's own base64, so the test needs nothing from a browser.
  return Buffer.from(bytes).toString("base64");
}

test("half precision decodes what scene.rs writes", () => {
  for (const [h, v] of [[0x3c00, 1], [0x3e00, 1.5], [0x4000, 2], [0x4200, 3], [0x4400, 4], [0x3800, 0.5]] as const) {
    close(halfToFloat(h), v);
  }
  assert.equal(halfToFloat(0x7c00), Infinity);
  assert.ok(Number.isNaN(halfToFloat(0x7e00)));
});

test("the cloud's version 2 decodes: count, metres (y up), colour, u and v", async () => {
  const base64 = cloudBytes([
    { p: [0.5, 1.5, 2], rgb: [255, 0, 0], uv: [0, 0] },
    { p: [0, 0, 4], rgb: [0, 255, 0], uv: [65535, 32768] },
    { p: [-1.5, -2, 6], rgb: [0, 0, 255], uv: [100, 200] },
  ]);
  const cloud = await decodeCloud(base64);
  assert.equal(cloud.count, 3);
  // y is the camera's (down) turned up for the world, the rest is as sent.
  const [x, y, z] = pointAt(cloud, 0);
  close(x, 0.5);
  close(y, -1.5);
  close(z, 2);
  close(cloud.data[3], 1, 1e-6);
  close(cloud.data[4], 0, 1e-6);
  close(cloud.data[5], 0, 1e-6);
  const [u, v] = uvAt(cloud, 1);
  close(u, 1, 1e-6);
  close(v, 32768 / 65535, 1e-6);
  close(uvAt(cloud, 2)[0], 100 / 65535, 1e-6);
  // The scene's spread, for the orbit's middle and the ground.
  close(cloud.zMin, 2);
  close(cloud.zMax, 6);
  close(cloud.medianZ, 4);
  close(cloud.floorY, -1.5, 1e-6);
  close(cloud.ceilY, 2, 1e-6);
});

test("another version, or a broken length, is refused rather than drawn wrong", async () => {
  const v1 = cloudBytes([{ p: [0, 0, 2], rgb: [0, 0, 0], uv: [0, 0] }], 1);
  await assert.rejects(() => decodeCloud(v1), /version 1/);
  const good = cloudBytes([{ p: [0, 0, 2], rgb: [0, 0, 0], uv: [0, 0] }]);
  const bytes = Buffer.from(good, "base64");
  const cut = Buffer.concat([bytes.subarray(0, bytes.length - 1)]).toString("base64");
  await assert.rejects(() => decodeCloud(cut), /length/);
});
