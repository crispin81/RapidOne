/**
 * The Depth Scope's WebGL2 drawing: the photo as a coloured point cloud (round sprites, sized by
 * distance), the near and far depth planes with their fade ends, a faint ground grid, the sky
 * ring at the far anchor, and a marker for the point under the pointer on the photo.
 *
 * Plain WebGL2 and our own shaders (no RapidRAW code: theirs is AGPL). A cloud is uploaded once;
 * everything else is rebuilt only when the planes or the camera move, and the component draws a
 * frame only when something changed. The graphics context's loss is handled by rebuilding every
 * buffer and program when the browser gives it back.
 */

import type { ScopeCloud } from "./cloud";
import { type CameraFrame, type Mat4, type Vec3, fovYFromFovX, frameHalfSize, metresOfControl, multiply, orbitCamera, perspective, viewMatrix } from "./math";

/** What to draw this frame. */
export interface ScopeFrame {
  /** The camera's orbit, in radians and metres. */
  yaw: number;
  pitch: number;
  distance: number;
  target: Vec3;
  /** The picture's horizontal field of view, degrees. */
  fovX: number;
  /** The photo's own aspect (width / height): the proportions of a depth plane. */
  photoAspect: number;
  /** The canvas' size in CSS pixels, and its device pixel ratio. */
  width: number;
  height: number;
  dpr: number;
  /** The band, control depth 0..1 (1 = nearest), its fade either side, and the scene's anchors. */
  near: number;
  far: number;
  fade: number;
  dNear: number;
  dFar: number;
  /** The focus plane, control depth (0..1), if there is one. */
  focus: number | null;
  /** Mist: how near the fog comes, control depth, if Mist is on. */
  fogStart: number | null;
  /** Mist: how far it goes, control depth, if it has a limit. */
  fogFar: number | null;
  /** Mist: the height fog's strength, 0 uniform .. 1 ground-hugging. */
  fogHeight: number;
  /** Tint the points in the band and dim the rest. */
  tint: boolean;
  /** The ground grid and the sky ring. */
  grid: boolean;
  /** A point to mark (the photo's cursor, or a hovered point). */
  marker: { pos: Vec3; color: [number, number, number] } | null;
  /** Relight's lights, in camera-space metres: a ring each, the selected one larger. */
  lights?: { id: number; pos: Vec3; color: [number, number, number]; selected: boolean }[];
  /** Draw half the points (while orbiting: the eye can't see the difference, the card can). */
  decimate: boolean;
}

/** Where the planes are, in metres: what the labels and the geometry need. */
export interface PlaneGeometry {
  nearZ: number;
  farZ: number;
  /** The fade's outer ends (near + fade, far - fade), metres. */
  fadeNearZ: number;
  fadeFarZ: number;
  /** The sky ring's distance, metres. */
  skyZ: number;
}

const POINT_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec3 aPos;
layout(location = 1) in vec3 aColor;
uniform mat4 uMVP;
uniform vec3 uEye;
uniform float uSize;
uniform float uDpr;
uniform float uDNear;
uniform float uDFar;
uniform float uBandNear;
uniform float uBandFar;
uniform float uBandFade;
uniform float uTint;
uniform float uPick;
out vec3 vColor;
flat out vec3 vPick;

float bandWeight(float c) {
  float n = max(uBandNear, uBandFar);
  float f = min(uBandNear, uBandFar);
  float d = clamp(uBandFade, 0.0, 1.0);
  if (d <= 1e-4) return (c >= f && c <= n) ? 1.0 : 0.0;
  if (c > n) return 1.0 - smoothstep(n, n + d, c);
  if (c < f) return 1.0 - smoothstep(f - d, f, c);
  return 1.0;
}

void main() {
  gl_Position = uMVP * vec4(aPos, 1.0);
  float dist = max(length(aPos - uEye), 0.02);
  gl_PointSize = clamp(uSize / dist, 1.0, 4.0) * uDpr;
  vColor = aColor;
  if (uTint > 0.5) {
    float z = max(aPos.z, 1e-4);
    float denom = log(uDFar / uDNear);
    float c = abs(denom) > 1e-6 ? 1.0 - clamp(log(z / uDNear) / denom, 0.0, 1.0) : 0.5;
    float w = bandWeight(c);
    vec3 inside = mix(aColor, vec3(1.0, 0.9, 0.0), 0.8);
    vColor = mix(aColor * 0.4, inside, w);
  }
  uint id = uint(gl_VertexID) + 1u;
  vPick = vec3(float(id & 255u), float((id >> 8u) & 255u), float((id >> 16u) & 255u)) / 255.0;
}`;

const POINT_FS = `#version 300 es
precision highp float;
in vec3 vColor;
flat in vec3 vPick;
uniform float uPick;
out vec4 outColor;
void main() {
  vec2 d = gl_PointCoord * 2.0 - 1.0;
  if (dot(d, d) > 1.0) discard;
  if (uPick > 0.5) { outColor = vec4(vPick, 1.0); return; }
  outColor = vec4(vColor, 1.0);
}`;

const QUAD_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec3 aPos;
layout(location = 1) in vec2 aUV;
uniform mat4 uMVP;
out vec2 vUV;
void main() {
  gl_Position = uMVP * vec4(aPos, 1.0);
  vUV = aUV;
}`;

const QUAD_FS = `#version 300 es
precision highp float;
in vec2 vUV;
uniform vec3 uColor;
uniform float uAlpha;
uniform float uEdge;
out vec4 outColor;
void main() {
  float e = smoothstep(0.0, 0.16, min(min(vUV.x, 1.0 - vUV.x), min(vUV.y, 1.0 - vUV.y)));
  outColor = vec4(uColor, uAlpha * mix(1.0, e, uEdge));
}`;

const LINE_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec3 aPos;
layout(location = 1) in vec4 aColor;
uniform mat4 uMVP;
out vec4 vColor;
void main() {
  gl_Position = uMVP * vec4(aPos, 1.0);
  vColor = aColor;
}`;

const LINE_FS = `#version 300 es
precision highp float;
in vec4 vColor;
out vec4 outColor;
void main() { outColor = vColor; }`;

const MARKER_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec3 aPos;
uniform mat4 uMVP;
uniform float uSize;
uniform float uDpr;
void main() {
  gl_Position = uMVP * vec4(aPos, 1.0);
  gl_PointSize = uSize * uDpr;
}`;

const MARKER_FS = `#version 300 es
precision highp float;
uniform vec3 uColor;
out vec4 outColor;
void main() {
  vec2 d = gl_PointCoord * 2.0 - 1.0;
  float r = length(d);
  // A ring with a cross through it: reads at a glance, whatever is behind it.
  float ring = smoothstep(0.62, 0.72, r) * (1.0 - smoothstep(0.88, 1.0, r));
  float cross = max(1.0 - smoothstep(0.0, 0.09, abs(d.x)), 1.0 - smoothstep(0.0, 0.09, abs(d.y))) * (1.0 - smoothstep(0.5, 0.62, r));
  float a = max(ring, cross);
  if (a < 0.02) discard;
  outColor = vec4(uColor, a);
}`;

type Program = {
  program: WebGLProgram;
  uniforms: Record<string, WebGLUniformLocation | null>;
};

function compile(gl: WebGL2RenderingContext, vs: string, fs: string): WebGLProgram | null {
  const make = (type: number, src: string) => {
    const s = gl.createShader(type);
    if (!s) return null;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      // The message names the shader; nothing else here can fail this way.
      console.error("[scope] shader:", gl.getShaderInfoLog(s));
      gl.deleteShader(s);
      return null;
    }
    return s;
  };
  const v = make(gl.VERTEX_SHADER, vs);
  const f = make(gl.FRAGMENT_SHADER, fs);
  if (!v || !f) return null;
  const p = gl.createProgram();
  if (!p) return null;
  gl.attachShader(p, v);
  gl.attachShader(p, f);
  gl.linkProgram(p);
  gl.deleteShader(v);
  gl.deleteShader(f);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    console.error("[scope] program:", gl.getProgramInfoLog(p));
    gl.deleteProgram(p);
    return null;
  }
  return p;
}

function program(gl: WebGL2RenderingContext, vs: string, fs: string, names: string[]): Program | null {
  const p = compile(gl, vs, fs);
  if (!p) return null;
  const uniforms: Record<string, WebGLUniformLocation | null> = {};
  for (const n of names) uniforms[n] = gl.getUniformLocation(p, n);
  return { program: p, uniforms };
}

/** The distances the planes sit at, from the band's control values (0..1, 1 = nearest). */
export function planeGeometry(near: number, far: number, fade: number, dNear: number, dFar: number): PlaneGeometry {
  const nearM = metresOfControl(near, dNear, dFar);
  const farM = metresOfControl(far, dNear, dFar);
  // The far end of the band is the sky: put the plane at the scene's own far anchor, and the sky
  // ring a little beyond it, so the scale still reads.
  const farZ = Number.isFinite(farM) ? farM : dFar;
  const fadeC = Math.max(0, Math.min(1, fade));
  const fadeNearM = metresOfControl(Math.min(1, near + fadeC), dNear, dFar);
  const fadeFarM = metresOfControl(Math.max(0, far - fadeC), dNear, dFar);
  return {
    nearZ: Number.isFinite(nearM) ? nearM : dNear,
    farZ: Math.max(farZ, dNear * 0.05),
    fadeNearZ: Number.isFinite(fadeNearM) ? fadeNearM : dNear,
    fadeFarZ: Number.isFinite(fadeFarM) ? fadeFarM : dFar,
    skyZ: dFar * 1.15,
  };
}

/**
 * The vertical field of view the view uses. The photo's own frame is kept wholly inside the
 * canvas, so a panel that isn't the photo's shape still shows the whole picture (with a margin
 * one way): the larger of the photo's own vertical view and the one the canvas' width would give.
 */
export function viewFovY(fovX: number, photoAspect: number, canvasAspect: number): number {
  const photo = fovYFromFovX(fovX, photoAspect);
  const across = fovYFromFovX(fovX, canvasAspect);
  return Math.max(1, Math.min(170, Math.max(photo, across)));
}

export class ScopeRenderer {
  private canvas: HTMLCanvasElement;
  private gl: WebGL2RenderingContext | null;
  private lost = false;
  private cloud: ScopeCloud | null = null;

  private points: Program | null = null;
  private quad: Program | null = null;
  private line: Program | null = null;
  private marker: Program | null = null;

  private pointVao: WebGLVertexArrayObject | null = null;
  private pointBuf: WebGLBuffer | null = null;
  private pointIndex: WebGLBuffer | null = null;
  private indexCount = 0;
  private quadVao: WebGLVertexArrayObject | null = null;
  private quadBuf: WebGLBuffer | null = null;
  private lineVao: WebGLVertexArrayObject | null = null;
  private lineBuf: WebGLBuffer | null = null;
  private markerVao: WebGLVertexArrayObject | null = null;
  private markerBuf: WebGLBuffer | null = null;

  private pickFbo: WebGLFramebuffer | null = null;
  private pickTex: WebGLTexture | null = null;
  private pickDepth: WebGLRenderbuffer | null = null;
  private pickSize = { w: 0, h: 0 };

  /** The last frame's matrices, for picking and for the labels the component places. */
  private mvp: Mat4 = new Float32Array(16);

  private onLost = (e: Event) => {
    // Keep the context: the browser hands it back on `restored`, and every buffer comes back with it.
    e.preventDefault();
    this.lost = true;
  };
  private onRestored = () => {
    this.lost = false;
    this.init();
    if (this.cloud) this.setCloud(this.cloud);
  };

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    // No `preserveDrawingBuffer`: the scope draws every frame it needs, on demand.
    this.gl = canvas.getContext("webgl2", { antialias: true, alpha: true, premultipliedAlpha: false });
    canvas.addEventListener("webglcontextlost", this.onLost);
    canvas.addEventListener("webglcontextrestored", this.onRestored);
    if (this.gl) this.init();
  }

  /** Whether this machine's viewer has WebGL2 at all. */
  get ok(): boolean {
    return this.gl !== null;
  }

  private init() {
    const gl = this.gl;
    if (!gl) return;
    this.points = program(gl, POINT_VS, POINT_FS, [
      "uMVP", "uEye", "uSize", "uDpr", "uDNear", "uDFar", "uBandNear", "uBandFar", "uBandFade", "uTint", "uPick",
    ]);
    this.quad = program(gl, QUAD_VS, QUAD_FS, ["uMVP", "uColor", "uAlpha", "uEdge"]);
    this.line = program(gl, LINE_VS, LINE_FS, ["uMVP"]);
    this.marker = program(gl, MARKER_VS, MARKER_FS, ["uMVP", "uSize", "uDpr", "uColor"]);

    this.pointVao = gl.createVertexArray();
    this.pointBuf = gl.createBuffer();
    gl.bindVertexArray(this.pointVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.pointBuf);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 24, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 3, gl.FLOAT, false, 24, 12);
    gl.bindVertexArray(null);

    this.pointIndex = gl.createBuffer();
    this.quadVao = gl.createVertexArray();
    this.quadBuf = gl.createBuffer();
    gl.bindVertexArray(this.quadVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.quadBuf);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 20, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 20, 12);
    gl.bindVertexArray(null);

    this.lineVao = gl.createVertexArray();
    this.lineBuf = gl.createBuffer();
    gl.bindVertexArray(this.lineVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.lineBuf);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 28, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, 28, 12);
    gl.bindVertexArray(null);

    this.markerVao = gl.createVertexArray();
    this.markerBuf = gl.createBuffer();
    gl.bindVertexArray(this.markerVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.markerBuf);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 12, 0);
    gl.bindVertexArray(null);

    gl.disable(gl.CULL_FACE);
    gl.enable(gl.DEPTH_TEST);
  }

  /** Uploads a cloud: once, interleaved as it came. */
  setCloud(cloud: ScopeCloud | null) {
    this.cloud = cloud;
    const gl = this.gl;
    if (!gl || !cloud || !this.pointBuf) return;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.pointBuf);
    gl.bufferData(gl.ARRAY_BUFFER, cloud.data, gl.STATIC_DRAW);
    // Every other point, for while the view is moving: half the fill, no visible difference.
    const stride = 2;
    const n = Math.floor(cloud.count / stride);
    const idx = new Uint32Array(n);
    for (let i = 0; i < n; i++) idx[i] = i * stride;
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.pointIndex);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, idx, gl.STATIC_DRAW);
    this.indexCount = n;
  }

  /** The matrices and camera for a frame (also what the component projects labels with). */
  static camera(frame: ScopeFrame) {
    const cam: CameraFrame = orbitCamera({ yaw: frame.yaw, pitch: frame.pitch, distance: frame.distance, target: frame.target });
    // The canvas' own aspect, so the projection fills it; the field of view is the photo's, so the
    // picture's width is the same as on the photo.
    const aspect = Math.max(frame.width / Math.max(frame.height, 1), 1e-6);
    const fovY = viewFovY(frame.fovX, frame.photoAspect, aspect);
    const view = viewMatrix(cam.eye, cam.forward, cam.up);
    const proj = perspective(fovY, aspect, 0.02, Math.max(1000, frame.dFar * 60));
    return { cam, view, proj, mvp: multiply(proj, view), fovY };
  }

  /** Draws one frame. Cheap when nothing moved: the component only calls it when something did. */
  render(frame: ScopeFrame) {
    const gl = this.gl;
    if (!gl || this.lost || !this.cloud) return;
    const cloud = this.cloud;
    const { cam, mvp } = ScopeRenderer.camera(frame);
    this.mvp = mvp;

    const dpr = frame.dpr;
    const w = Math.max(1, Math.round(frame.width * dpr));
    const h = Math.max(1, Math.round(frame.height * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    gl.viewport(0, 0, w, h);
    gl.clearColor(0.02, 0.025, 0.03, 1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);

    const planes = planeGeometry(frame.near, frame.far, frame.fade, frame.dNear, frame.dFar);

    // --- The points ---
    const p = this.points;
    if (p && this.pointVao) {
      gl.useProgram(p.program);
      gl.bindVertexArray(this.pointVao);
      gl.uniformMatrix4fv(p.uniforms.uMVP, false, mvp);
      gl.uniform3fv(p.uniforms.uEye, cam.eye);
      gl.uniform1f(p.uniforms.uSize, 2.2 * cloud.medianZ);
      gl.uniform1f(p.uniforms.uDpr, dpr);
      gl.uniform1f(p.uniforms.uDNear, frame.dNear);
      gl.uniform1f(p.uniforms.uDFar, frame.dFar);
      gl.uniform1f(p.uniforms.uBandNear, frame.near);
      gl.uniform1f(p.uniforms.uBandFar, frame.far);
      gl.uniform1f(p.uniforms.uBandFade, frame.fade);
      gl.uniform1f(p.uniforms.uTint, frame.tint ? 1 : 0);
      gl.uniform1f(p.uniforms.uPick, 0);
      gl.enable(gl.DEPTH_TEST);
      gl.depthMask(true);
      gl.disable(gl.BLEND);
      if (frame.decimate && this.indexCount > 0) {
        gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.pointIndex);
        gl.drawElements(gl.POINTS, this.indexCount, gl.UNSIGNED_INT, 0);
      } else {
        gl.drawArrays(gl.POINTS, 0, cloud.count);
      }
    }

    // --- The planes: translucent, so no depth write, far to near ---
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.depthMask(false);
    const quads: { z: number; alpha: number; edge: number; color: [number, number, number] }[] = [];
    if (Number.isFinite(planes.farZ)) quads.push({ z: planes.farZ, alpha: 0.08, edge: 0, color: [0.24, 0.9, 1] });
    if (Number.isFinite(planes.nearZ)) quads.push({ z: planes.nearZ, alpha: 0.1, edge: 0, color: [0.24, 0.9, 1] });
    if (frame.focus !== null) {
      const fz = metresOfControl(frame.focus, frame.dNear, frame.dFar);
      if (Number.isFinite(fz)) quads.push({ z: fz, alpha: 0.14, edge: 0, color: [0.5, 0.95, 1] });
    }
    // Mist: the start plane, then a faint volume out to the scene's far anchor.
    if (frame.fogStart !== null) {
      const fz = metresOfControl(frame.fogStart, frame.dNear, frame.dFar);
      if (Number.isFinite(fz)) {
        quads.push({ z: fz, alpha: 0.1, edge: 0, color: [0.55, 0.82, 1] });
        for (const k of [0.25, 0.55, 0.85]) {
          const z = fz + (planes.skyZ - fz) * k;
          if (z > fz && Number.isFinite(z)) quads.push({ z, alpha: 0.035, edge: 0, color: [0.55, 0.82, 1] });
        }
      }
    }
    // The fade zones: a faint quad where the band has eased to nothing.
    if (frame.fade > 1e-3) {
      quads.push({ z: planes.fadeFarZ, alpha: 0.03, edge: 0.6, color: [0.24, 0.9, 1] });
      quads.push({ z: planes.fadeNearZ, alpha: 0.035, edge: 0.6, color: [0.24, 0.9, 1] });
    }
    quads.sort((a, b) => Math.abs(b.z - cam.eye[2]) - Math.abs(a.z - cam.eye[2]));
    const q = this.quad;
    if (q && this.quadVao) {
      gl.useProgram(q.program);
      gl.bindVertexArray(this.quadVao);
      gl.uniformMatrix4fv(q.uniforms.uMVP, false, mvp);
      for (const quad of quads) {
        const [hw, hh] = frameHalfSize(quad.z, frame.fovX, frame.photoAspect);
        this.uploadQuad(hw, hh, quad.z);
        gl.uniform3fv(q.uniforms.uColor, quad.color);
        gl.uniform1f(q.uniforms.uAlpha, quad.alpha);
        gl.uniform1f(q.uniforms.uEdge, quad.edge);
        gl.drawArrays(gl.TRIANGLES, 0, 6);
      }
    }

    // --- The outlines, the fade ends' frames, the ground grid and the sky ring ---
    const lines = this.lineData(frame, planes);
    const l = this.line;
    if (l && this.lineVao && lines.length) {
      gl.useProgram(l.program);
      gl.bindVertexArray(this.lineVao);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.lineBuf);
      gl.bufferData(gl.ARRAY_BUFFER, lines, gl.DYNAMIC_DRAW);
      gl.uniformMatrix4fv(l.uniforms.uMVP, false, mvp);
      gl.drawArrays(gl.LINES, 0, lines.length / 7);
    }

    // --- The marker (always on top) ---
    const m = this.marker;
    if (m && this.markerVao && frame.marker) {
      gl.disable(gl.DEPTH_TEST);
      gl.useProgram(m.program);
      gl.bindVertexArray(this.markerVao);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.markerBuf);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(frame.marker.pos), gl.DYNAMIC_DRAW);
      gl.uniformMatrix4fv(m.uniforms.uMVP, false, mvp);
      gl.uniform1f(m.uniforms.uSize, 16);
      gl.uniform1f(m.uniforms.uDpr, dpr);
      gl.uniform3fv(m.uniforms.uColor, frame.marker.color);
      gl.drawArrays(gl.POINTS, 0, 1);
      gl.enable(gl.DEPTH_TEST);
    }
    // Relight's lights: the marker's ring, in the light's own colour, the selected one larger.
    // Drawn whether or not the cursor marker is showing (they used to vanish when it wasn't).
    if (m && this.markerVao && frame.lights?.length) {
      gl.disable(gl.DEPTH_TEST);
      gl.useProgram(m.program);
      gl.bindVertexArray(this.markerVao);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.markerBuf);
      gl.uniformMatrix4fv(m.uniforms.uMVP, false, mvp);
      gl.uniform1f(m.uniforms.uDpr, dpr);
      for (const l of frame.lights) {
        gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(l.pos), gl.DYNAMIC_DRAW);
        gl.uniform1f(m.uniforms.uSize, l.selected ? 24 : 15);
        gl.uniform3fv(m.uniforms.uColor, l.color);
        gl.drawArrays(gl.POINTS, 0, 1);
      }
      gl.enable(gl.DEPTH_TEST);
    }

    gl.depthMask(true);
    gl.disable(gl.BLEND);
    gl.bindVertexArray(null);
  }

  private uploadQuad(hw: number, hh: number, z: number) {
    const gl = this.gl;
    if (!gl) return;
    const v = new Float32Array([
      -hw, -hh, z, 0, 1,
      hw, -hh, z, 1, 1,
      hw, hh, z, 1, 0,
      -hw, -hh, z, 0, 1,
      hw, hh, z, 1, 0,
      -hw, hh, z, 0, 0,
    ]);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.quadBuf);
    gl.bufferData(gl.ARRAY_BUFFER, v, gl.DYNAMIC_DRAW);
  }

  /** The line vertices (x, y, z, r, g, b, a) for the plane outlines, the grid and the sky ring. */
  private lineData(frame: ScopeFrame, planes: PlaneGeometry): Float32Array {
    const out: number[] = [];
    const cyan: [number, number, number] = [0.24, 0.9, 1];
    const add = (a: Vec3, b: Vec3, c: [number, number, number], alpha: number) => {
      out.push(a[0], a[1], a[2], c[0], c[1], c[2], alpha, b[0], b[1], b[2], c[0], c[1], c[2], alpha);
    };
    const rect = (z: number, hw: number, hh: number, c: [number, number, number], alpha: number) => {
      const p = (x: number, y: number): Vec3 => [x, y, z];
      add(p(-hw, -hh), p(hw, -hh), c, alpha);
      add(p(hw, -hh), p(hw, hh), c, alpha);
      add(p(hw, hh), p(-hw, hh), c, alpha);
      add(p(-hw, hh), p(-hw, -hh), c, alpha);
    };
    const [nhw, nhh] = frameHalfSize(planes.nearZ, frame.fovX, frame.photoAspect);
    const [fhw, fhh] = frameHalfSize(planes.farZ, frame.fovX, frame.photoAspect);
    rect(planes.nearZ, nhw, nhh, cyan, 0.85);
    rect(planes.farZ, fhw, fhh, cyan, 0.7);
    if (frame.fade > 1e-3) {
      const [anh, bnh] = frameHalfSize(planes.fadeNearZ, frame.fovX, frame.photoAspect);
      const [afh, bfh] = frameHalfSize(planes.fadeFarZ, frame.fovX, frame.photoAspect);
      rect(planes.fadeNearZ, anh, bnh, cyan, 0.22);
      rect(planes.fadeFarZ, afh, bfh, cyan, 0.22);
      // The fade's walls: the four corners joined to their outer ends.
      for (const [x, y] of [[-1, -1], [1, -1], [1, 1], [-1, 1]] as const) {
        add([x * nhw, y * nhh, planes.nearZ], [x * anh, y * bnh, planes.fadeNearZ], cyan, 0.18);
        add([x * fhw, y * fhh, planes.farZ], [x * afh, y * bfh, planes.fadeFarZ], cyan, 0.18);
      }
    }
    if (frame.focus !== null) {
      const fz = metresOfControl(frame.focus, frame.dNear, frame.dFar);
      if (Number.isFinite(fz)) {
        const [fwh, fhh] = frameHalfSize(fz, frame.fovX, frame.photoAspect);
        rect(fz, fwh, fhh, [0.5, 0.95, 1], 0.9);
      }
    }
    // Mist's start plane, and (with Height) the layers it pools in.
    let fogZ: number | null = null;
    if (frame.fogStart !== null) {
      const fz = metresOfControl(frame.fogStart, frame.dNear, frame.dFar);
      if (Number.isFinite(fz)) {
        fogZ = fz;
        const [fwh, fhh] = frameHalfSize(fz, frame.fovX, frame.photoAspect);
        rect(fz, fwh, fhh, [0.55, 0.82, 1], 0.8);
      }
    }
    if (fogZ !== null && frame.fogFar !== null) {
      const fz = metresOfControl(frame.fogFar, frame.dNear, frame.dFar);
      if (Number.isFinite(fz)) {
        const [fwh, fhh] = frameHalfSize(fz, frame.fovX, frame.photoAspect);
        rect(fz, fwh, fhh, [0.55, 0.82, 1], 0.45);
      }
    }
    if (fogZ !== null && frame.fogHeight > 0.01 && this.cloud) {
      // Height fog: a few faint horizontal layers above the ground it pools on.
      const floor = this.cloud.floorY;
      const [hw] = frameHalfSize(Math.min(fogZ * 1.5, planes.skyZ), frame.fovX, frame.photoAspect);
      for (const up of [0.4, 1.2, 3.0]) {
        const y = floor + up * (0.5 + frame.fogHeight);
        const alpha = 0.14 * frame.fogHeight * (1.0 - up / 4.0);
        for (let z = fogZ; z < Math.min(planes.skyZ, fogZ * 3); z *= 1.6) {
          add([-hw, y, z], [hw, y, z], [0.6, 0.85, 1], alpha);
        }
      }
    }

    if (frame.grid && this.cloud) {
      const floor = this.cloud.floorY;
      const zMax = Math.min(Math.max(this.cloud.zMax, planes.farZ), planes.skyZ * 1.05);
      const grid: [number, number, number] = [0.3, 0.75, 0.85];
      // Squares of a round size that suits the scene (about ten to its depth), every fifth line stronger.
      const step = [0.1, 0.2, 0.5, 1, 2, 5, 10, 20, 50].find((s) => zMax / s <= 14) ?? 100;
      const [halfW] = frameHalfSize(zMax, frame.fovX, frame.photoAspect);
      const across = Math.max(1, Math.ceil(halfW / step));
      const alpha = (i: number) => (i % 5 === 0 ? 0.85 : 0.5);
      // Across the picture (lines along x), then along it (lines along z) either side of the axis.
      for (let i = 0; i * step <= zMax; i++) add([-across * step, floor, i * step], [across * step, floor, i * step], grid, alpha(i));
      for (let i = -across; i <= across; i++) add([i * step, floor, 0], [i * step, floor, zMax], grid, alpha(i));
    }

    if (frame.grid) {
      // The sky ring at the far anchor: how far the scene's own distances reach.
      const z = planes.skyZ;
      const [hw, hh] = frameHalfSize(z, frame.fovX, frame.photoAspect);
      const r = (hw + hh) / 2;
      const seg = 72;
      const sky: [number, number, number] = [0.35, 0.8, 0.95];
      for (let i = 0; i < seg; i++) {
        const a0 = (i / seg) * Math.PI * 2;
        const a1 = ((i + 1) / seg) * Math.PI * 2;
        add([Math.cos(a0) * r, Math.sin(a0) * r, z], [Math.cos(a1) * r, Math.sin(a1) * r, z], sky, 0.22);
      }
    }
    return new Float32Array(out);
  }

  /**
   * The point under a pixel (CSS pixels within the canvas), or -1. Drawn into an offscreen buffer
   * with each point's own number as its colour, then read back: exact, and cheap at a few pixels.
   */
  pick(xCss: number, yCss: number, frame: ScopeFrame): number {
    const gl = this.gl;
    if (!gl || this.lost || !this.cloud || !this.points || !this.pointVao) return -1;
    const p = this.points;
    const dpr = frame.dpr;
    const w = Math.max(1, Math.round(frame.width * dpr));
    const h = Math.max(1, Math.round(frame.height * dpr));
    if (!this.ensurePick(w, h)) return -1;
    const { mvp, cam } = ScopeRenderer.camera(frame);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.pickFbo);
    gl.viewport(0, 0, w, h);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.enable(gl.DEPTH_TEST);
    gl.depthMask(true);
    gl.disable(gl.BLEND);
    gl.useProgram(p.program);
    gl.bindVertexArray(this.pointVao);
    gl.uniformMatrix4fv(p.uniforms.uMVP, false, mvp);
    gl.uniform3fv(p.uniforms.uEye, cam.eye);
    gl.uniform1f(p.uniforms.uSize, 2.2 * this.cloud.medianZ);
    gl.uniform1f(p.uniforms.uDpr, dpr);
    gl.uniform1f(p.uniforms.uDNear, frame.dNear);
    gl.uniform1f(p.uniforms.uDFar, frame.dFar);
    gl.uniform1f(p.uniforms.uBandNear, frame.near);
    gl.uniform1f(p.uniforms.uBandFar, frame.far);
    gl.uniform1f(p.uniforms.uBandFade, frame.fade);
    gl.uniform1f(p.uniforms.uTint, 0);
    gl.uniform1f(p.uniforms.uPick, 1);
    gl.drawArrays(gl.POINTS, 0, this.cloud.count);

    // A few pixels round the click: a point's own size leaves the exact pixel empty now and then.
    const cx = Math.round(xCss * dpr);
    const cy = Math.round(h - yCss * dpr);
    const half = 3;
    const x0 = Math.max(0, cx - half);
    const y0 = Math.max(0, cy - half);
    const rw = Math.min(w - x0, half * 2 + 1);
    const rh = Math.min(h - y0, half * 2 + 1);
    const px = new Uint8Array(rw * rh * 4);
    gl.readPixels(x0, y0, rw, rh, gl.RGBA, gl.UNSIGNED_BYTE, px);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.bindVertexArray(null);

    let best = -1;
    let bestD = Infinity;
    for (let y = 0; y < rh; y++) {
      for (let x = 0; x < rw; x++) {
        const at = (y * rw + x) * 4;
        const id = px[at] | (px[at + 1] << 8) | (px[at + 2] << 16);
        if (id === 0) continue;
        const d = Math.hypot(x0 + x - cx, y0 + y - cy);
        if (d < bestD) {
          bestD = d;
          best = id - 1;
        }
      }
    }
    return best < this.cloud.count ? best : -1;
  }

  private ensurePick(w: number, h: number): boolean {
    const gl = this.gl;
    if (!gl) return false;
    if (this.pickFbo && this.pickSize.w === w && this.pickSize.h === h) return true;
    if (this.pickFbo) gl.deleteFramebuffer(this.pickFbo);
    if (this.pickTex) gl.deleteTexture(this.pickTex);
    if (this.pickDepth) gl.deleteRenderbuffer(this.pickDepth);
    this.pickFbo = gl.createFramebuffer();
    this.pickTex = gl.createTexture();
    this.pickDepth = gl.createRenderbuffer();
    if (!this.pickFbo || !this.pickTex || !this.pickDepth) return false;
    gl.bindTexture(gl.TEXTURE_2D, this.pickTex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.bindRenderbuffer(gl.RENDERBUFFER, this.pickDepth);
    gl.renderbufferStorage(gl.RENDERBUFFER, gl.DEPTH_COMPONENT16, w, h);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.pickFbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.pickTex, 0);
    gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.RENDERBUFFER, this.pickDepth);
    const ok = gl.checkFramebufferStatus(gl.FRAMEBUFFER) === gl.FRAMEBUFFER_COMPLETE;
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    this.pickSize = { w, h };
    return ok;
  }

  /** The projection x view of the last frame (for placing the labels). */
  get lastMvp(): Mat4 {
    return this.mvp;
  }

  dispose() {
    const gl = this.gl;
    this.canvas.removeEventListener("webglcontextlost", this.onLost);
    this.canvas.removeEventListener("webglcontextrestored", this.onRestored);
    if (!gl) return;
    for (const p of [this.points, this.quad, this.line, this.marker]) if (p) gl.deleteProgram(p.program);
    for (const b of [this.pointBuf, this.pointIndex, this.quadBuf, this.lineBuf, this.markerBuf]) if (b) gl.deleteBuffer(b);
    for (const v of [this.pointVao, this.quadVao, this.lineVao, this.markerVao]) if (v) gl.deleteVertexArray(v);
    if (this.pickFbo) gl.deleteFramebuffer(this.pickFbo);
    if (this.pickTex) gl.deleteTexture(this.pickTex);
    if (this.pickDepth) gl.deleteRenderbuffer(this.pickDepth);
    this.gl = null;
  }
}
