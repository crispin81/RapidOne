//! Relight: virtual lights shaped by the scene's own geometry, in scene-linear light before the tone
//! curve. Our own implementation of the ideas in `docs/creative/research` (RapidRAW's docs and
//! CalibRaw's notes were read for behaviour only; no code of theirs is used or copied).
//!
//! Every pixel is a small surface at a real distance: `P = dir · z` metres in camera space, with the
//! normal MoGe-2 gave it (or one derived from the depth when only Depth Anything ran). Each light's
//! gain is a wrapped Lambert plus a Blinn sheen, attenuated by a true inverse square in metres and
//! (optionally) by screen-space ray-marched shadows in the same metres. Light colour is a
//! blackbody's at a Kelvin setting, relative to the photo's own white balance, normalised by its
//! luminance: changing colour never changes exposure.
//!
//! Differences from the rivals, deliberately (see the brief):
//! - metres everywhere (falloff, shadow reach, penumbra), so a light moved from 2 m to 4 m really
//!   quarters, and a portrait and a landscape behave the same;
//! - an energy-preserving wrap (`max(0, N·L + s) / (1 + 2s)`, whose hemisphere mean is Lambert's),
//!   plus a second-scatter term so surfaces turned away keep some light;
//! - shadows ray-marched **in metres** on a coarse map, jittered by the light's size for penumbra,
//!   box-blurred by Shadow softness, upsamped bilinearly (cheap, and smooth);
//! - sky is never lit by point or spot lights (they can't reach infinity) and the sun only lights
//!   surfaces: Ambient is the sky's only control, so a relit sky doesn't grey out;
//! - `match_light` fits a sun to the photo's own shading, which neither rival offers.

use crate::creative::{Light, LightKind, Relight};
use crate::gradient::Grid;
use crate::pipeline::{Frame, SceneImage};
use crate::scene::Refined;

/// The ambient slider in stops: 100 is +3 stops, −100 is −3.
pub fn ambient_gain(ambient: f32) -> f32 {
    2f32.powf(ambient.clamp(-100.0, 100.0) / 33.3333)
}

/// A blackbody's colour at `kelvin`, in linear light, normalised to unit luminance. Kim et al.'s
/// piecewise fit of the Planckian locus (a standard approximation), then a small tint.
pub fn blackbody(kelvin: f32) -> [f32; 3] {
    let t = kelvin.clamp(1000.0, 40000.0) / 100.0;
    let r = if t <= 66.0 { 255.0 } else { 329.698727446 * (t - 60.0).powf(-0.1332047592) };
    let g = if t <= 66.0 {
        99.4708025861 * t.ln() - 161.1195681661
    } else {
        288.1221695283 * (t - 60.0).powf(-0.0755148492)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.5177312231 * (t - 10.0).ln() - 305.0447927307
    };
    // sRGB's tone curve is undone so the colour is linear light (the fit is display-referred).
    let lin = |v: f32| {
        let v = (v / 255.0).clamp(0.0, 1.0);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut rgb = [lin(r), lin(g), lin(b)];
    let l = luma(rgb).max(1e-5);
    rgb = rgb.map(|v| v / l);
    rgb
}

/// Rec. 2020 luminance, the working space's.
pub fn luma(p: [f32; 3]) -> f32 {
    (0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).max(0.0)
}

/// A light's linear colour: its blackbody at Kelvin relative to `white_kelvin` (daylight, 5500 K, in
/// the app: so Warm reads the same on every photo, 5500 white, lower warmer, whatever preset the
/// camera was on), tinted green↔magenta, unit luminance. The RAW panel's White Balance then moves
/// it with everything else, as it would in camera.
pub fn light_colour(light: &Light, white_kelvin: f32) -> [f32; 3] {
    let own = blackbody(light.kelvin);
    let white = blackbody(white_kelvin);
    let base = [own[0] / white[0].max(1e-4), own[1] / white[1].max(1e-4), own[2] / white[2].max(1e-4)];
    let tint = (light.tint / 100.0).clamp(-1.0, 1.0);
    let rgb = [base[0] * 2f32.powf(0.12 * tint), base[1] * 2f32.powf(-0.28 * tint), base[2] * 2f32.powf(0.12 * tint)];
    let l = luma(rgb).max(1e-5);
    rgb.map(|v| v / l)
}

/// The wrap's diffuse term: an energy-preserving wide Lambert. Its mean over the hemisphere is the
/// same as plain Lambert's for every softness, so widening the source never brightens the scene.
pub fn wrap(n_dot_l: f32, softness: f32) -> f32 {
    let s = (softness / 100.0).clamp(0.0, 1.0);
    (n_dot_l + s).max(0.0) / (1.0 + 2.0 * s)
}

/// The second-scatter term (a fraction of the wrap reaches round facing-away surfaces), so a soft
/// light doesn't leave a hard black in the shadow.
fn second_scatter(n_dot_l: f32, softness: f32) -> f32 {
    let s = (softness / 100.0).clamp(0.0, 1.0);
    0.18 * s * (1.0 - n_dot_l).max(0.0) / (1.0 + s)
}

/// The Blinn sheen: sharpness and strength follow Softness (a broad source has a broad sheen).
pub fn sheen(n_dot_h: f32, softness: f32) -> f32 {
    let s = (softness / 100.0).clamp(0.0, 1.0);
    let power = 8.0 + 56.0 * (1.0 - s);
    n_dot_h.max(0.0).powf(power) * (0.02 + 0.18 * (1.0 - s))
}

/// A modifier's size in metres, from the Size slider: a bare head's 14 cm reflector to a 1.5 m
/// octabox or a patch of bounced ceiling.
pub fn source_metres(light: &Light) -> f32 {
    0.05 + 1.45 * (light.size / 100.0).clamp(0.0, 1.0)
}

/// A point light's reach in metres: the distance at which half its light is left. Light falls off
/// as the inverse square of the distance, as a real flash's does, from a reference that scales
/// with the scene (a third of the way to the subject, at least a metre, so a lamp at a typical
/// key distance of 1–2 m from a sitter 3–4 m away lights them at about its own power, and moving
/// it in from 3 m to 1.5 m gains well over a stop). A big source is gentler close in (its area
/// spreads the near field), so its size widens the reach a little. `subject_m` is how far the
/// subject (what the lamp is aimed at) is from the camera. Never the modifier's size alone: a bare
/// flash is not dimmer than a softbox for being small.
pub fn reach_metres(light: &Light, subject_m: f32) -> f32 {
    let r0 = (0.35 * subject_m.abs()).max(1.0);
    let s = source_metres(light);
    (r0 * r0 + s * s).sqrt()
}

/// The inverse-square falloff, normalised so half the light is left at the reach.
pub fn falloff(distance: f32, reach: f32) -> f32 {
    let r = reach.max(1e-3);
    let d = distance.max(0.0);
    let rr = r * r;
    rr / (rr + d * d)
}

/// A spot's cone: 1 inside the inner cone, 0 outside, smooth between, from the cosine of the angle
/// between the light's axis and the direction to the surface.
pub fn cone(cos_axis: f32, half_angle_deg: f32, feather: f32) -> f32 {
    let outer = half_angle_deg.clamp(1.0, 89.0).to_radians().cos();
    let f = (feather / 100.0).clamp(0.0, 1.0);
    let inner = (half_angle_deg * (1.0 - 0.9 * f.max(0.02))).clamp(0.5, 89.0).to_radians().cos();
    if cos_axis >= inner {
        1.0
    } else if cos_axis <= outer {
        0.0
    } else {
        let t = (cos_axis - outer) / (inner - outer).max(1e-5);
        t * t * (3.0 - 2.0 * t)
    }
}

/// A point light's spread: a panel's lobe, full inside `cone` degrees and easing out to nothing a
/// feather beyond it (so 85° and a wide feather is effectively all round, and a smaller Cone with a
/// narrow Feather is a softbox turned to its aim).
pub fn panel_lobe(cos_axis: f32, cone_deg: f32, feather: f32) -> f32 {
    let inner = cone_deg.clamp(1.0, 89.5).to_radians().cos();
    let f = (feather / 100.0).clamp(0.0, 1.0);
    let outer_deg = (cone_deg + 2.0 + f * 60.0).min(89.9);
    let outer = outer_deg.to_radians().cos();
    if cos_axis >= inner {
        1.0
    } else if cos_axis <= outer {
        0.0
    } else {
        smoothstep(outer, inner, cos_axis)
    }
}

/// The ambient's character under the sun or the moon. Ambient stands for the photo's own light,
/// set as a shutter speed would (it dims everything alike, in linear light). Under the sky it is
/// also the sky's own light: blue as the sun goes down (the warm sun and cool shadows of golden
/// hour), and at night, as film graders do "day for night": the colour fades (our eyes lose it in
/// the dark), it cools, the highlights and the sky go down harder than the midtones, and the
/// deepest shadows keep a little detail. How strong all that is follows how far Ambient is down
/// and how high the sun or moon is; with only room lights it is plain dimming.
pub struct SkyAmbient {
    /// The ambient's colour (unit luminance) and the sky's extra gain.
    pub tint: [f32; 3],
    pub sky_gain: f32,
    /// Night: 0..1 colour taken out, the highlights' knee, the shadows' lift (linear light).
    pub desaturate: f32,
    pub knee: f32,
    pub lift: f32,
}

impl SkyAmbient {
    pub fn plain() -> Self {
        SkyAmbient { tint: [1.0; 3], sky_gain: 1.0, desaturate: 0.0, knee: 0.0, lift: 0.0 }
    }

    /// From the first sun or moon that is on (the sun first, if both are).
    pub fn of(relight: &Relight, white_kelvin: f32) -> Self {
        let on = |k: LightKind| relight.lights.iter().find(|l| l.on && l.kind == k && l.intensity > 0.0);
        let Some(l) = on(LightKind::Sun).or_else(|| on(LightKind::Moon)) else {
            return Self::plain();
        };
        // How far the ambient is down: 0 left as shot, 1 two stops down (or more).
        let dim = (-relight.ambient / 66.6).clamp(0.0, 1.0);
        // The sky's blue, relative to the photo's white.
        let sky = blackbody(10000.0);
        let white = blackbody(white_kelvin);
        let blue = [sky[0] / white[0].max(1e-4), sky[1] / white[1].max(1e-4), sky[2] / white[2].max(1e-4)];
        let tinted = |f: f32| {
            let t = [1.0 + (blue[0] - 1.0) * f, 1.0 + (blue[1] - 1.0) * f, 1.0 + (blue[2] - 1.0) * f];
            let y = luma(t).max(1e-5);
            t.map(|v| v / y)
        };
        if l.kind == LightKind::Sun {
            // Golden hour: the lower the sun, the bluer the sky's light against it.
            // Golden hour: the shadows lean a little blue only with the sun low (under ~20°) and the
            // ambient down; a hint, not a cast (it was strong enough to turn a beach photo blue).
            let low = 1.0 - smoothstep(4.0, 20.0, l.elevation);
            SkyAmbient { tint: tinted(0.18 * low * dim), ..Self::plain() }
        } else {
            // Night: stronger the further the ambient is down, a little less with the moon high.
            let night = dim * (1.0 - 0.3 * smoothstep(20.0, 60.0, l.elevation));
            SkyAmbient { tint: tinted(0.55 * night), sky_gain: 2f32.powf(-1.0 * night), desaturate: 0.75 * night, knee: 1.5 * night, lift: 0.0015 * night }
        }
    }

    /// Whether there is more than a tint to do.
    pub fn grades(&self) -> bool {
        self.desaturate > 1e-3 || self.knee > 1e-3 || self.lift > 1e-6
    }

    /// The night grade on the scene's own light: colour out, highlights down, deep shadows kept.
    pub fn grade(&self, p: [f32; 3]) -> [f32; 3] {
        let y = luma(p);
        let d = self.desaturate;
        let p = [p[0] + (y - p[0]) * d, p[1] + (y - p[1]) * d, p[2] + (y - p[2]) * d];
        // A soft knee: the brighter, the more it comes down (1/(1 + k·Y)); dark tones barely move.
        let k = 1.0 / (1.0 + self.knee * y);
        p.map(|v| v * k + self.lift)
    }
}

/// How much of the sun's (or moon's) light gets through the air at its height: a clear sky's
/// extinction over the Kasten–Young air mass (held at 15, a hazy horizon's), 1 overhead, almost a
/// stop down at 10° and two and a half on the horizon, as the low sun really is dimmer (its colour is the Warm slider's, set from
/// its height by the panel).
pub fn atmosphere(elevation_deg: f32) -> f32 {
    let el = elevation_deg.clamp(0.0, 90.0);
    let air_mass = (1.0 / (el.to_radians().sin() + 0.50572 * (el + 6.07995).powf(-1.6364))).min(15.0);
    (-0.12 * (air_mass - 1.0)).exp()
}

/// A sun's direction (from the surface toward the light) in camera space (x right, y down, z
/// forward). Azimuth runs around the vertical: 0° from the right of the frame, 90° from behind the
/// subject (backlight), 180° from the left, 270° from behind the camera (front light).
pub fn sun_direction(azimuth_deg: f32, elevation_deg: f32) -> [f32; 3] {
    let az = azimuth_deg.to_radians();
    let el = elevation_deg.clamp(-5.0, 89.0).to_radians();
    [el.cos() * az.cos(), -el.sin(), el.cos() * az.sin()]
}

/// The photo's luminance and distance on a coarse grid (128 cells on the long side), read back
/// edge-aware: how lit the surroundings at this pixel's own distance are.
struct ShadeGrid {
    gw: usize,
    gh: usize,
    lum: Vec<f32>,
    z: Vec<f32>,
}

impl ShadeGrid {
    fn new(lum: &[f32], w: usize, h: usize, metres_at: impl Fn(f32, f32) -> f32) -> Self {
        // Never finer than the picture itself (a cell with no pixels would read as black).
        let long = 128usize.min(w.max(h) / 4).max(1);
        let (gw, gh) = if w >= h { (long, ((long * h) / w.max(1)).max(1)) } else { (((long * w) / h.max(1)).max(1), long) };
        let mut sum = vec![0.0f32; gw * gh];
        let mut n = vec![0.0f32; gw * gh];
        for y in 0..h {
            let cy = (y * gh / h.max(1)).min(gh - 1);
            for x in 0..w {
                let cx = (x * gw / w.max(1)).min(gw - 1);
                sum[cy * gw + cx] += lum[y * w + x];
                n[cy * gw + cx] += 1.0;
            }
        }
        let lum_g = sum.iter().zip(&n).map(|(s, c)| s / c.max(1.0)).collect();
        let z = (0..gw * gh).map(|i| metres_at(((i % gw) as f32 + 0.5) / gw as f32, ((i / gw) as f32 + 0.5) / gh as f32)).collect();
        ShadeGrid { gw, gh, lum: lum_g, z }
    }

    fn at(&self, fx: f32, fy: f32, z_own: f32) -> f32 {
        let (gw, gh) = (self.gw as isize, self.gh as isize);
        let (x, y) = (fx * gw as f32 - 0.5, fy * gh as f32 - 0.5);
        let (x0, y0) = (x.round() as isize, y.round() as isize);
        let tol = (0.08 * z_own).max(0.05);
        let (mut s, mut ws) = (0.0f32, 0.0f32);
        for j in -3..=3isize {
            for i in -3..=3isize {
                let (cx, cy) = (x0 + i, y0 + j);
                if cx < 0 || cy < 0 || cx >= gw || cy >= gh {
                    continue;
                }
                let k = (cy * gw + cx) as usize;
                let (dx, dy) = (x - cx as f32, y - cy as f32);
                let wd = (-(dx * dx + dy * dy) / 8.0).exp();
                let dz = (self.z[k] - z_own) / tol;
                let wgt = wd * (-dz * dz).exp();
                s += wgt * self.lum[k];
                ws += wgt;
            }
        }
        if ws > 1e-6 {
            s / ws
        } else {
            let k = (y0.clamp(0, gh - 1) * gw + x0.clamp(0, gw - 1)) as usize;
            self.lum[k]
        }
    }
}

/// A separable box blur of a single-channel picture, radius `r` pixels (two running sums).
pub(crate) fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; w * h];
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (mut acc, mut n) = (0.0f32, 0.0f32);
        for x in 0..(r + 1).min(w) {
            acc += src[y * w + x];
            n += 1.0;
        }
        for x in 0..w {
            tmp[y * w + x] = acc / n;
            if x + r + 1 < w {
                acc += src[y * w + x + r + 1];
                n += 1.0;
            }
            if x >= r {
                acc -= src[y * w + x - r];
                n -= 1.0;
            }
        }
    }
    for x in 0..w {
        let (mut acc, mut n) = (0.0f32, 0.0f32);
        for y in 0..(r + 1).min(h) {
            acc += tmp[y * w + x];
            n += 1.0;
        }
        for y in 0..h {
            out[y * w + x] = acc / n;
            if y + r + 1 < h {
                acc += tmp[(y + r + 1) * w + x];
                n += 1.0;
            }
            if y >= r {
                acc -= tmp[(y - r) * w + x];
                n -= 1.0;
            }
        }
    }
    out
}

/// A backlight's rim, relative to the same light falling square on a surface: an edge seen against
/// the background catches it as a bright line.
const RIM_GAIN: f32 = 0.8;

/// How strongly a pixel sits on a silhouette lit from behind: the subject's edge where the
/// background falls away, on the side facing the light. Each of eight screen directions looks a
/// few control pixels out for a big step back in depth (the background); the directions towards
/// the light (`toward`, unit, in control pixels) count fully, the others less, and all of them
/// equally when the light hides right behind the subject (`hidden` 1). `width_px` is the rim's
/// reach in control pixels (a soft source makes a wider rim).
pub fn rim_edge(control: &Grid, fx: f32, fy: f32, own_m: f32, toward: [f32; 2], hidden: f32, width_px: f32, near: f32, far: f32) -> f32 {
    let (cw, ch) = (control.w.max(1) as f32, control.h.max(1) as f32);
    let mut best = 0.0f32;
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let d = [a.cos(), a.sin()];
        let w = (d[0] * toward[0] + d[1] * toward[1]).max(0.0) * (1.0 - hidden) + hidden;
        if w < 0.05 {
            continue;
        }
        // Only the subject's own pixels: a pixel in the soft band where the depth map blends the
        // subject into the background has something nearer just inside it, and lighting it would
        // draw a halo out on the background (in the background's colour).
        let (iu, iv) = ((fx - d[0] * width_px / cw).clamp(0.0, 1.0), (fy - d[1] * width_px / ch).clamp(0.0, 1.0));
        let inner = crate::scene::metres_of(control.at(iu, iv).clamp(0.0, 1.0), near, far);
        let solid = 1.0 - smoothstep(0.04, 0.14, (own_m - inner) / own_m.max(1e-3));
        if solid < 0.01 {
            continue;
        }
        // March outwards in small steps and find where the background starts, between samples
        // (the depth gap crossing a threshold, interpolated): the rim then fades smoothly with the
        // real distance to the edge. Three fixed samples gave three flat bands (24 Nov).
        const N: usize = 10;
        let mut prev_gap = 0.0f32;
        for i in 1..=N {
            let r = width_px * i as f32 / N as f32;
            let (u, v) = (fx + d[0] * r / cw, fy + d[1] * r / ch);
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                break;
            }
            let zn = crate::scene::metres_of(control.at(u, v).clamp(0.0, 1.0), near, far);
            let gap = (zn - own_m) / own_m.max(1e-3);
            const EDGE: f32 = 0.12;
            if gap >= EDGE {
                // Where between the last two samples the gap crossed it.
                let t = if gap > prev_gap { ((EDGE - prev_gap) / (gap - prev_gap)).clamp(0.0, 1.0) } else { 1.0 };
                let dist = width_px * ((i - 1) as f32 + t) / N as f32;
                let x = (1.0 - dist / width_px.max(1e-3)).clamp(0.0, 1.0);
                // Brightest right at the edge, easing off inwards (a rim is a thin line).
                let e = x * x * smoothstep(0.04, 0.25, gap);
                best = best.max(w * e * solid);
                break;
            }
            prev_gap = gap.max(0.0);
        }
    }
    best
}

/// A point or spot light's position in camera-space metres: its place on the image plane at a
/// distance from Depth. Depth 50 is the scene's own middle — the geometric mean of the robust
/// anchors, which is the control scale's halfway — and 0 is at the camera, 100 as far behind that
/// middle again. So half the slider is in front of the subject and half behind it, with room to
/// put a light right at the lens or well behind the sitter.
pub fn light_position(light: &Light, focal: f32, w: f32, h: f32, near: f32, far: f32) -> [f32; 3] {
    let t = (light.depth / 100.0).clamp(0.0, 1.0);
    let focus = (near.max(0.05) * far.max(near.max(0.05))).sqrt();
    let front = 0.02_f32;
    let half = (focus - front).max(0.02);
    let z = (focus + (t - 0.5) * 2.0 * half).max(front);
    let x = (light.u - 0.5) * w / focal;
    let y = (light.v - 0.5) * h / focal;
    [x * z, y * z, z]
}

/// A point or spot's direction from its own yaw and pitch, in camera space (x right, y down, z into
/// the scene): yaw 0 faces into the scene, 90° to the right, 360 wraps; pitch + is up. This is the
/// real 3D rotation the scope's rings set; it can point anywhere, including behind the light.
pub fn yaw_pitch_direction(yaw_deg: f32, pitch_deg: f32) -> [f32; 3] {
    let y = yaw_deg.to_radians();
    let p = pitch_deg.clamp(-90.0, 90.0).to_radians();
    normalise([p.cos() * y.sin(), -p.sin(), p.cos() * y.cos()])
}

/// Where a spot points: from the light's position to the scene point under (aim_u, aim_v), in
/// camera space. Falls back to the camera axis when the aim is on the light itself. A light with
/// its own yaw/pitch (the scope's rings) uses that direction instead, so it can face anywhere.
pub fn aim_axis(light: &Light, focal: f32, w: f32, h: f32, near: f32, far: f32, scene_metres_at: impl Fn(f32, f32) -> f32) -> [f32; 3] {
    if let (Some(yaw), Some(pitch)) = (light.yaw, light.pitch) {
        return yaw_pitch_direction(yaw, pitch);
    }
    let lp = light_position(light, focal, w, h, near, far);
    let z = scene_metres_at(light.aim_u, light.aim_v).max(near * 0.5);
    let d = ray(light.aim_u.clamp(0.0, 1.0), light.aim_v.clamp(0.0, 1.0), focal, w, h);
    let target = [d[0] * z, d[1] * z, d[2] * z];
    let v = [target[0] - lp[0], target[1] - lp[1], target[2] - lp[2]];
    if v[0] * v[0] + v[1] * v[1] + v[2] * v[2] < 1e-6 {
        return normalise([-lp[0], -lp[1], -lp[2]]);
    }
    normalise(v)
}

/// The camera-space ray direction of a pixel.
fn ray(u: f32, v: f32, focal: f32, w: f32, h: f32) -> [f32; 3] {
    let x = (u - 0.5) * w / focal;
    let y = (v - 0.5) * h / focal;
    let len = (x * x + y * y + 1.0).sqrt();
    [x / len, y / len, 1.0 / len]
}

fn normalise(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / len, v[1] / len, v[2] / len]
}

/// A smooth 0..1 ramp between two edges.
fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A normal derived from the depth where MoGe's normals are missing: the surface's tilt from the
/// control map's own gradient, in camera space, at fractions `u`, `v`.
fn normal_from_depth(depth: &Refined, u: f32, v: f32, focal: f32, w: f32, h: f32, near: f32, far: f32) -> [f32; 3] {
    let step = 1.0 / w.max(h) as f32;
    let z_at = |uu: f32, vv: f32| {
        let c = depth.control.at(uu.clamp(0.0, 1.0), vv.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        crate::scene::metres_of(c, near, far)
    };
    let (z0, zl, zr, zd, zu) = (z_at(u, v), z_at(u - step, v), z_at(u + step, v), z_at(u, v - step), z_at(u, v + step));
    // The surface point at each sample, then a cross product of two tangents.
    let p = |uu: f32, vv: f32, z: f32| {
        let d = ray(uu, vv, focal, w, h);
        [d[0] * z, d[1] * z, d[2] * z]
    };
    let du = {
        let a = p(u - step, v, zl);
        let b = p(u + step, v, zr);
        [b[0] - a[0], b[1] - a[1], b[2] - a[2]]
    };
    let dv = {
        let a = p(u, v - step, zu);
        let b = p(u, v + step, zd);
        [b[0] - a[0], b[1] - a[1], b[2] - a[2]]
    };
    let mut n = normalise([du[1] * dv[2] - du[2] * dv[1], du[2] * dv[0] - du[0] * dv[2], du[0] * dv[1] - du[1] * dv[0]]);
    // Face the camera (camera z is forward, so a visible surface's z is negative).
    if n[2] > 0.0 {
        n = n.map(|c| -c);
    }
    if !n.iter().all(|c| c.is_finite()) || dot(n, n) < 0.25 {
        n = [0.0, 0.0, -1.0];
    }
    // A little of the depth's own softness is fine; the light's relief pass adds the photo's texture.
    let _ = z0;
    n
}

/// A light's shadows on a coarse grid over the picture: how much of the light each cell's surface
/// is blocked from, and that surface's distance (so the grid is read back edge-aware: a cell on
/// the wall behind never darkens the face in front, and the reverse).
pub struct ShadowGrid {
    occ: Grid,
    z: Grid,
}

impl ShadowGrid {
    /// The occlusion at (`fx`, `fy`) for a surface `z_own` metres away: the nearby cells weighted
    /// by closeness and by how alike their distance is, so the shadow's edge follows the photo's
    /// own depth edges instead of the grid's blocks.
    pub fn at(&self, fx: f32, fy: f32, z_own: f32) -> f32 {
        let (gw, gh) = (self.occ.w as isize, self.occ.h as isize);
        let x = fx * gw as f32 - 0.5;
        let y = fy * gh as f32 - 0.5;
        let (x0, y0) = (x.floor() as isize, y.floor() as isize);
        let tol = (0.06 * z_own).max(0.03);
        let mut sum = 0.0;
        let mut wsum = 0.0;
        for j in -1..=2 {
            for i in -1..=2 {
                let (cx, cy) = ((x0 + i).clamp(0, gw - 1), (y0 + j).clamp(0, gh - 1));
                let (dx, dy) = (x - (x0 + i) as f32, y - (y0 + j) as f32);
                let wd = (1.0 - (dx * dx + dy * dy).sqrt() / 2.2).max(0.0);
                let idx = (cy * gw + cx) as usize;
                let dz = (self.z.data[idx] - z_own) / tol;
                let w = wd * (-dz * dz).exp();
                sum += w * self.occ.data[idx];
                wsum += w;
            }
        }
        if wsum > 1e-6 {
            sum / wsum
        } else {
            self.occ.at(fx, fy)
        }
    }
}

/// A light's shadows: from every cell's surface, rays to points spread over the light's own face
/// (a lamp's real size, a sun's tiny disc), so the penumbra is the physical one — sharp where the
/// shadow touches what casts it and softer further away — and Soft-ish (Feather for the sun)
/// widens the source as haze or a diffuser would. Only a light smoothing follows, edge-aware.
fn shadow_map(light: &Light, depth: &Refined, focal: f32, w: f32, h: f32, near: f32, far: f32, sun: Option<[f32; 3]>) -> Option<ShadowGrid> {
    if light.shadows <= 0.0 {
        return None;
    }
    let long = 256usize;
    let aspect = w / h;
    let (gw, gh) = if aspect >= 1.0 { (long, ((long as f32 / aspect).round() as usize).max(1)) } else { (((long as f32 * aspect).round() as usize).max(1), long) };
    // Seven points over the source and 28 steps a ray: the penumbra without the grain, at about
    // the cost of the old box-blurred map on a full preview.
    let rays = 7usize;
    let steps = 28usize;
    let light_z = light_position(light, focal, w, h, near, far)[2];
    // The sun's ray length.
    let reach = reach_metres(light, light_z);
    let soft = (light.shadow_soft / 100.0).clamp(0.0, 1.0);
    // A lamp's face (its radius, widened by Soft-ish), or the sun's disc as an angle (0.27°, up to
    // about 5° through haze).
    let source_r = source_metres(light) * 0.5 * (1.0 + 2.5 * soft);
    let sun_r = 0.0047 + 0.085 * soft;
    let mut data = vec![0.0f32; gw * gh];
    let mut zs = vec![far; gw * gh];
    let light_pos = if sun.is_none() {
        Some(light_position(light, focal, w, h, near, far))
    } else {
        None
    };
    // The sun's own frame, for its disc.
    let (sa, sb) = sun.map_or(([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), |d| {
        let r = if d[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
        let a = normalise([d[1] * r[2] - d[2] * r[1], d[2] * r[0] - d[0] * r[2], d[0] * r[1] - d[1] * r[0]]);
        let b = [d[1] * a[2] - d[2] * a[1], d[2] * a[0] - d[0] * a[2], d[0] * a[1] - d[1] * a[0]];
        (a, b)
    });
    for gy in 0..gh {
        for gx in 0..gw {
            let (u, v) = ((gx as f32 + 0.5) / gw as f32, (gy as f32 + 0.5) / gh as f32);
            if depth.sky.as_ref().is_some_and(|s| s.at(u, v) > 0.5) {
                continue;
            }
            let c = depth.control.at(u, v).clamp(0.0, 1.0);
            let z = crate::scene::metres_of(c, near, far);
            zs[gy * gw + gx] = z;
            let d = ray(u, v, focal, w, h);
            let p = [d[0] * z, d[1] * z, d[2] * z];
            let mut blocked = 0.0;
            for r in 0..rays {
                // Points over the source's face (a sunflower spiral: even, no rings).
                let rad = ((r as f32 + 0.5) / rays as f32).sqrt();
                let a = r as f32 * 2.399_963;
                let (ox, oy) = (a.cos() * rad, a.sin() * rad);
                let l = match sun {
                    Some(dir) => normalise([
                        dir[0] + (sa[0] * ox + sb[0] * oy) * sun_r,
                        dir[1] + (sa[1] * ox + sb[1] * oy) * sun_r,
                        dir[2] + (sa[2] * ox + sb[2] * oy) * sun_r,
                    ]),
                    None => {
                        let lp = light_pos.unwrap_or([0.0, 0.0, 1.0]);
                        normalise([lp[0] + ox * source_r - p[0], lp[1] + oy * source_r - p[1], lp[2] - p[2]])
                    }
                };
                let Some(lp) = light_pos else {
                    // Sun: a long ray, but only far enough to matter.
                    blocked += march(p, l, depth, focal, w, h, near, far, reach * 6.0, steps);
                    continue;
                };
                let dist = ((lp[0] - p[0]).powi(2) + (lp[1] - p[1]).powi(2) + (lp[2] - p[2]).powi(2)).sqrt();
                blocked += march(p, l, depth, focal, w, h, near, far, dist - 0.05, steps);
            }
            data[gy * gw + gx] = (blocked / rays as f32).clamp(0.0, 1.0);
        }
    }
    // A light, edge-aware smoothing of the rays' grain (never across a depth edge).
    let radius = 1 + (2.0 * soft).round() as isize;
    let mut blurred = data.clone();
    for y in 0..gh as isize {
        for x in 0..gw as isize {
            let z0 = zs[(y * gw as isize + x) as usize];
            let tol = (0.06 * z0).max(0.03);
            let mut sum = 0.0;
            let mut n = 0.0;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx >= 0 && yy >= 0 && (xx as usize) < gw && (yy as usize) < gh {
                        let i = yy as usize * gw + xx as usize;
                        let dz = (zs[i] - z0) / tol;
                        let wgt = (-dz * dz).exp();
                        sum += wgt * data[i];
                        n += wgt;
                    }
                }
            }
            blurred[(y * gw as isize + x) as usize] = if n > 0.0 { sum / n } else { 0.0 };
        }
    }
    Some(ShadowGrid { occ: Grid { w: gw, h: gh, data: blurred }, z: Grid { w: gw, h: gh, data: zs } })
}

/// March from `p` toward the light `l` (unit) up to `max_t` metres: the share of the ray that is
/// blocked by surfaces in front of it.
#[allow(clippy::too_many_arguments)]
fn march(p: [f32; 3], l: [f32; 3], depth: &Refined, focal: f32, w: f32, h: f32, near: f32, far: f32, max_t: f32, steps: usize) -> f32 {
    let mut occluded = 0.0;
    let mut alive = 1.0f32;
    for i in 0..steps {
        let t = max_t * (i as f32 + 0.5) / steps as f32;
        let q = [p[0] + l[0] * t, p[1] + l[1] * t, p[2] + l[2] * t];
        if q[2] <= 0.05 {
            break;
        }
        let u = 0.5 + q[0] / q[2] * focal / w;
        let v = 0.5 + q[1] / q[2] * focal / h;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            // Off the frame: the scene isn't known there; nothing to block with.
            continue;
        }
        let c = depth.control.at(u, v).clamp(0.0, 1.0);
        let z = crate::scene::metres_of(c, near, far);
        // The surface there is in front of this step by a real distance: it blocks. Only within a
        // few metres — a background far behind the ray is not an occluder for a near light.
        let behind = z - q[2];
        // A surface seen from the camera has a thickness behind it we can't see: about a hand's
        // width plus a sixth of its distance (a head, a tree trunk), not all the way back. Past it,
        // the ray goes behind the object and on — without this every pole and arm cast a shadow as
        // if it were a wall reaching back to the horizon (the blobby, plastic look).
        let thick = 0.25 + 0.16 * z;
        if behind > 0.02 && behind < thick {
            let weight = smoothstep(0.02, 0.12, behind) * (1.0 - smoothstep(thick * 0.6, thick, behind));
            occluded += alive * weight * (1.5 / steps as f32) * 6.0;
            alive *= 1.0 - weight.min(1.0);
        }
    }
    occluded.clamp(0.0, 1.0)
}

/// Relight: `None` when it is off (so a photo without it is bit-identical).
pub fn apply(image: &SceneImage, depth: &Refined, relight: &Relight, frame: Option<&Frame>) -> Option<SceneImage> {
    apply_balanced(image, depth, relight, frame, crate::pipeline::NEUTRAL_KELVIN)
}

/// As `apply`, for a scene light balanced to `white_kelvin` (the photo's as-shot white balance):
/// each light's colour is taken relative to it.
pub fn apply_balanced(image: &SceneImage, depth: &Refined, relight: &Relight, frame: Option<&Frame>, white_kelvin: f32) -> Option<SceneImage> {
    if !relight.enabled {
        return None;
    }
    let (w, h) = (image.width, image.height);
    if w < 4 || h < 4 {
        return None;
    }
    // Without metric depth there are no metres to place a light in: leave the photo as it is.
    let (Some((d_near, d_far)), Some(focal)) = (depth.anchors, depth.focal_px.filter(|f| *f > 1.0)) else {
        return Some(image.clone());
    };
    let (cw, ch) = (depth.control.w.max(1) as f32, depth.control.h.max(1) as f32);
    // Each light on: its colour and strength, its shape, and its shadows.
    struct Lit {
        light: Light,
        colour: [f32; 3],
        strength: f32,
        sun: Option<[f32; 3]>,
        pos: Option<[f32; 3]>,
        /// Spot: the cone's axis in camera space (from the lamp to its aim point).
        axis: Option<[f32; 3]>,
        shadows: Option<ShadowGrid>,
        reach: f32,
        /// Where the lamp falls on the picture (control fractions), for its rim from behind.
        screen: Option<[f32; 2]>,
        /// Sun / moon Spread: the patch of light breaking through (its centre in metres and its
        /// radius and soft edge); None lights the whole scene.
        patch: Option<([f32; 3], f32, f32)>,
    }
    let lit: Vec<Lit> = relight
        .lights
        .iter()
        .filter(|l| l.on && l.intensity > 0.0)
        .map(|l| {
            let celestial = matches!(l.kind, LightKind::Sun | LightKind::Moon);
            let sun = celestial.then(|| sun_direction(l.azimuth, l.elevation));
            let pos = if sun.is_some() { None } else { Some(light_position(l, focal, cw, ch, d_near, d_far)) };
            // What the lamp lights: the scene at its aim point (or at the middle, for a lamp
            // turned by its own yaw/pitch), which sets how far its light carries.
            let (su, sv) = if l.yaw.is_some() && l.pitch.is_some() { (0.5, 0.5) } else { (l.aim_u.clamp(0.0, 1.0), l.aim_v.clamp(0.0, 1.0)) };
            let subject_m = crate::scene::metres_of(depth.control.at(su, sv).clamp(0.0, 1.0), d_near, d_far);
            let reach = if pos.is_some() { reach_metres(l, subject_m) } else { reach_metres(l, d_far) };
            let axis = (!celestial).then(|| {
                aim_axis(l, focal, cw, ch, d_near, d_far, |u, v| {
                    let c = depth.control.at(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)).clamp(0.0, 1.0);
                    crate::scene::metres_of(c, d_near, d_far)
                })
            });
            let shadows = shadow_map(l, depth, focal, cw, ch, d_near, d_far, sun);
            Lit {
                light: l.clone(),
                colour: light_colour(l, white_kelvin),
                strength: 2f32.powf(4.0 * (l.intensity / 100.0).clamp(0.0, 1.0)) * if celestial { atmosphere(l.elevation) } else { 1.0 }
                    // Moonlight is a reflection of the sun: two stops under what the same
                    // Bright gives a lamp, so a night stays a night.
                    * if l.kind == LightKind::Moon { 0.25 } else { 1.0 },
                sun,
                pos,
                axis,
                shadows,
                reach,
                screen: pos.filter(|p| p[2] > 0.01).map(|p| [p[0] / p[2] * focal / cw + 0.5, p[1] / p[2] * focal / ch + 0.5]),
                patch: (celestial && l.cone < 99.5).then(|| {
                    // Centred where the sun is aimed (the subject), as wide as Spread says: from a
                    // narrow shaft (a tenth of the subject's distance) up to past the whole scene.
                    let (au, av) = (l.aim_u.clamp(0.0, 1.0), l.aim_v.clamp(0.0, 1.0));
                    let z = crate::scene::metres_of(depth.control.at(au, av).clamp(0.0, 1.0), d_near, d_far);
                    let r = ray(au, av, focal, cw, ch);
                    let centre = [r[0] * z, r[1] * z, r[2] * z];
                    let t = (l.cone / 100.0).clamp(0.0, 1.0);
                    let radius = z * (0.1 + 3.0 * t * t);
                    let soft = (l.feather / 100.0).clamp(0.05, 1.0);
                    (centre, radius, soft)
                }),
            }
        })
        .collect();
    let ambient = ambient_gain(relight.ambient);
    // The ambient's character, from the sun or moon in the scene (none for room lights: then it
    // only dims, as a shutter speed does).
    let sky_light = SkyAmbient::of(relight, white_kelvin);
    let amb = std::array::from_fn::<f32, 3, _>(|c| ambient * sky_light.tint[c]);
    // The sky keeps its own colour (it is the source of that blue already); only its level follows.
    let amb_sky = [ambient * sky_light.sky_gain; 3];
    // How much direct light lifts the photo's texture: most for the hard sun, less for soft lamps.
    let texture = {
        let hard = relight.lights.iter().filter(|l| l.on && l.intensity > 0.0).map(|l| 1.0 - (l.softness / 100.0).clamp(0.0, 1.0)).fold(0.0f32, f32::max);
        let sun = relight.lights.iter().any(|l| l.on && l.kind == LightKind::Sun);
        (if sun { 0.9 } else { 0.5 }) * (0.4 + 0.6 * hard)
    };
    let relief = (relight.relief / 100.0).clamp(0.0, 1.0);
    let sky_grid = depth.sky.as_deref();
    let normals = depth.normals.as_deref();
    // The photo's luminance gradient for Relief (a blurred difference, in the photo's own pixels).
    let luma_at = |x: isize, y: isize| -> f32 {
        let xx = x.clamp(0, w as isize - 1) as usize;
        let yy = y.clamp(0, h as isize - 1) as usize;
        luma(image.px[yy * w + xx])
    };
    // The photo's own texture, for direct light: a blurred luminance (a box of about 1/150 of the
    // picture) to measure each pixel against. Direct sun or flash makes texture stand out (every
    // leaf and stone casts its own little shadow); a soft ambient flattens it. Without this a light
    // only brightens what's there, and a lit landscape looks painted.
    let lum: Vec<f32> = image.px.iter().map(|p| luma(*p)).collect();
    let detail_blur = box_blur(&lum, w, h, ((w.max(h) as f32 / 150.0).round() as usize).max(2));
    // How lit each area already is: the photo's luminance over a broad area (1/30 of the picture),
    // against the photo's own average. A shadow is dim because little light reaches it, not
    // because the surface is dark; a lamp pointed into it adds its light to the surface's own
    // colour (the pixel over that dimness), so it lifts the shadow as a real light does, and adds
    // proportionally less where the photo is already bright.
    // Edge-aware: on a coarse grid of the photo's luminance and distance, each pixel reads the cells
    // round it weighted by how alike their distance is, so a bright subject never makes the dark
    // wall beside it look "lit" (or the wall the subject's edge look "in shadow"): that drew a halo
    // round every silhouette.
    let shade_grid = ShadeGrid::new(&lum, w, h, |u, v| crate::scene::metres_of(depth.control.at(u, v).clamp(0.0, 1.0), d_near, d_far));
    let shading_ref = (lum.iter().map(|v| *v as f64).sum::<f64>() / lum.len().max(1) as f64).max(1e-4) as f32;
    let out: Vec<[f32; 3]> = {
        use rayon::prelude::*;
        (0..w * h)
            .into_par_iter()
            .map(|i| {
                let base = image.px[i];
                let (fx, fy) = frame.map_or((((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32), |f| f.frac(i, w));
                // The sky mask, smoothed over a few of its own pixels: its stair steps along a ridge
                // line otherwise show as a jagged edge between the lit land and the unlit sky.
                // `sky_near` is how much sky is right beside this pixel (where normals are unreliable).
                let (sky, sky_near) = sky_grid.map_or((0.0, 0.0), |s| {
                    let (ox, oy) = (1.5 / s.w.max(1) as f32, 1.5 / s.h.max(1) as f32);
                    let mut sum = 0.0;
                    let mut most = 0.0f32;
                    for j in -1..=1 {
                        for i in -1..=1 {
                            let v = s.at((fx + i as f32 * ox).clamp(0.0, 1.0), (fy + j as f32 * oy).clamp(0.0, 1.0)).clamp(0.0, 1.0);
                            sum += v;
                            most = most.max(v);
                        }
                    }
                    (sum / 9.0, most)
                });
                let own = depth.control.at(fx, fy).clamp(0.0, 1.0);
                let step = 2isize;
                let relief_tilt = if relief > 0.0 {
                    let (x, y) = ((i % w) as isize, (i / w) as isize);
                    let gx = luma_at(x + step, y) - luma_at(x - step, y);
                    let gy = luma_at(x, y + step) - luma_at(x, y - step);
                    let k = relief * 2.5;
                    [gx * k, gy * k, 0.0]
                } else {
                    [0.0, 0.0, 0.0]
                };
                // One surface's gain: ambient plus every light that reaches it.
                // The gain at one surface, and the part of it the sun or moon gives (graded with the
                // scene's own light at night).
                let surface = |control: f32| -> ([f32; 3], [f32; 3], [f32; 3]) {
                    let z = crate::scene::metres_of(control.clamp(0.0, 1.0), d_near, d_far);
                    let dir = ray(fx, fy, focal, cw, ch);
                    let p = [dir[0] * z, dir[1] * z, dir[2] * z];
                    let mut n0 = normals.map_or_else(|| normal_from_depth(depth, fx, fy, focal, cw, ch, d_near, d_far), |m| m.at(fx, fy));
                    if relief_tilt != [0.0, 0.0, 0.0] {
                        n0 = normalise([n0[0] + relief_tilt[0], n0[1] + relief_tilt[1], n0[2]]);
                    }
                    let v = [-dir[0], -dir[1], -dir[2]];
                    // Beside the sky the depth model's normals bend wildly (the ridge line): ease them
                    // towards facing the camera, so the land's edge is lit like the land below it.
                    if sky_near > 0.05 {
                        // The land a little below the ridge (towards the ground, away from the
                        // sky): its normal, not the camera's direction, which lit every ridge line
                        // as a bright outline.
                        let below = normals.map(|m| {
                            let mut acc = [0.0f32; 3];
                            for i in 1..=3 {
                                let nb = m.at(fx, (fy + i as f32 * 3.0 / ch).min(1.0));
                                acc = [acc[0] + nb[0], acc[1] + nb[1], acc[2] + nb[2]];
                            }
                            normalise(acc)
                        });
                        let k = (sky_near * 0.9).min(0.9);
                        let to = below.unwrap_or(v);
                        let k = if below.is_some() { k } else { k * 0.5 };
                        n0 = normalise([n0[0] * (1.0 - k) + to[0] * k, n0[1] * (1.0 - k) + to[1] * k, n0[2] * (1.0 - k) + to[2] * k]);
                    }
                    // Silhouettes: a normal seen edge-on is the least reliable the model has, and a
                    // light catches it as a bright rim all round the subject (a "mask" look). Ease
                    // the normal towards the view direction as it turns edge-on, so shading there
                    // is steady, while surfaces facing the camera keep their own normal.
                    let facing = dot(n0, v).abs();
                    let k = smoothstep(0.08, 0.40, facing);
                    if k < 0.999 {
                        n0 = normalise([n0[0] * k + v[0] * (1.0 - k), n0[1] * k + v[1] * (1.0 - k), n0[2] * k + v[2] * (1.0 - k)]);
                    }
                    let mut gain = amb;
                    let mut from_sky = [0.0f32; 3];
                    let mut from_rim = [0.0f32; 3];
                    for l in &lit {
                        let (l_dir, atten, cone_f) = match (l.sun, l.pos) {
                            (Some(s), _) => {
                                // The sun: no falloff, and only the surfaces facing it; with a
                                // Spread, only inside its patch (parallel rays through a gap in the
                                // clouds: a cylinder along the sun's direction).
                                let inside = l.patch.map_or(1.0, |(c, r, soft)| {
                                    let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
                                    let along = dot(d, s);
                                    let off = [d[0] - s[0] * along, d[1] - s[1] * along, d[2] - s[2] * along];
                                    let dist = dot(off, off).sqrt();
                                    1.0 - smoothstep(r * (1.0 - soft), r, dist)
                                });
                                (s, 1.0, inside)
                            }
                            (None, Some(lp)) => {
                                let to = [lp[0] - p[0], lp[1] - p[1], lp[2] - p[2]];
                                let d = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt().max(1e-4);
                                let axis = l.axis.unwrap_or_else(|| normalise([-lp[0], -lp[1], -lp[2]]));
                                let cos_axis = -dot(normalise(to), axis);
                                // Both a spot and a point are panels: their aim turns the lobe, its
                                // Cone sets how wide and Feather how soft its edge is. A spot's edge
                                // is the tight cone; a point's spreads out to nothing past it.
                                let c = if l.light.kind == LightKind::Spot {
                                    cone(cos_axis, l.light.cone, l.light.feather)
                                } else {
                                    panel_lobe(cos_axis, l.light.cone, l.light.feather)
                                };
                                (normalise(to), falloff(d, l.reach), c)
                            }
                            _ => ([0.0, 0.0, 1.0], 0.0, 0.0),
                        };
                        let n_dot_l = dot(n0, l_dir);
                        // How far the lamp is behind this surface (towards the background, seen
                        // from the camera): 0 in front, 1 straight behind. A backlight doesn't
                        // spill round onto the side facing the camera, so its second scatter fades.
                        let back = smoothstep(0.0, 0.5, -dot(l_dir, v));
                        let diffuse = wrap(n_dot_l, l.light.softness) + second_scatter(n_dot_l, l.light.softness) * (1.0 - 0.85 * back);
                        // Far off, the depth model's normals are smooth blobs (a whole tree one
                        // rounded shape): lit by them, every tree turns into a painted ball. The
                        // further away (from 8 m, most by 80 m), the more the light just falls on the surface as it faces
                        // the light overall, and its form comes from the photo's own texture
                        // (below) instead.
                        let far_flat = 0.8 * smoothstep(8.0, 80.0, z);
                        let flat = wrap(0.55, l.light.softness);
                        let diffuse = diffuse + (flat * smoothstep(-0.2, 0.3, n_dot_l) - diffuse) * far_flat;
                        // The rim a light behind the subject draws round its silhouette, where the
                        // background falls away on the lamp's side (as a real backlight does).
                        // Where the light is on the picture: a lamp's own place, or for the sun the
                        // way its direction points across the frame (it's never "hidden").
                        let place = match (l.screen, l.sun) {
                            (Some(sc), _) => Some((sc[0] - fx, sc[1] - fy, true)),
                            (None, Some(sd)) => Some((sd[0], sd[1], false)),
                            _ => None,
                        };
                        let rim = match (back > 0.01, place) {
                            (true, Some((tx, ty, lamp))) => {
                                let t = if lamp { [tx * cw, ty * ch] } else { [tx, ty] };
                                let tl = (t[0] * t[0] + t[1] * t[1]).sqrt();
                                let toward = if tl > 1e-3 { [t[0] / tl, t[1] / tl] } else { [0.0, 0.0] };
                                // A lamp hidden right behind the head rims it all round; so does a
                                // sun straight behind the subject (its direction has no sideways part).
                                let hidden = if lamp { 1.0 - smoothstep(0.02, 0.25, tl / cw.max(ch)) } else { 1.0 - smoothstep(0.05, 0.4, tl) };
                                let soft = (l.light.softness / 100.0).clamp(0.0, 1.0);
                                // A rim's width is physical: the band of a head or a shoulder where the
                                // light grazes, about 1–3 cm (more for a soft source, and hair
                                // scatters it wider), seen at this surface's distance.
                                let width = (focal * (0.010 + 0.02 * soft) / z.max(0.05)).min(cw.max(ch) * 0.03).max(2.0);
                                back * rim_edge(&depth.control, fx, fy, z, toward, hidden, width, d_near, d_far)
                            }
                            _ => 0.0,
                        };
                        let h = normalise([l_dir[0] + v[0], l_dir[1] + v[1], l_dir[2] + v[2]]);
                        // The sheen fades where the surface is seen edge-on: a grazing normal would
                        // otherwise light every leaf tip and tree line with a bright rim.
                        let grazing = smoothstep(0.05, 0.35, dot(n0, v));
                        let spec = sheen(dot(n0, h), l.light.softness) * diffuse.clamp(0.0, 1.0) * grazing;
                        let shadow = l.shadows.as_ref().map_or(1.0, |g| {
                            let occ = g.at(fx, fy, z);
                            1.0 - (l.light.shadows / 100.0).clamp(0.0, 1.0) * occ.clamp(0.0, 1.0)
                        });
                        let k = l.strength * atten * cone_f * shadow;
                        // The rim is light grazing past the subject's edge: not blocked by it.
                        let k_rim = l.strength * atten * cone_f * RIM_GAIN;
                        for c in 0..3 {
                            let add = l.colour[c] * (k * (diffuse + spec) + k_rim * rim);
                            from_rim[c] += l.colour[c] * k_rim * rim;
                            gain[c] += add;
                            if l.sun.is_some() {
                                from_sky[c] += add;
                            }
                        }
                    }
                    (gain, from_sky, from_rim)
                };
                // Each pixel is lit at its **own** depth, not the pair the mask's edge maps give
                // Bokeh and Mist: a pixel of background beside a lit subject then keeps its own
                // light instead of borrowing the subject's (which drew a bright rim along every
                // bush and tree line). A soft depth edge still fades the light smoothly.
                let (gain, from_sky, from_rim) = surface(own);
                // Without a sky mask, the farthest depth stands in for it; with one, far land (a
                // mountain range) is land, and the sun lights it.
                let far_sky = if sky_grid.is_some() { 0.0 } else { 1.0 - smoothstep(0.01, 0.05, own) };
                let sky_w = smoothstep(0.45, 0.9, sky).max(far_sky);
                let gain = if sky_w > 0.001 {
                    std::array::from_fn::<f32, 3, _>(|c| gain[c] + (amb_sky[c] - gain[c]) * sky_w)
                } else {
                    gain
                };
                // Direct light brings out the photo's texture: the light's part of the gain is
                // raised where the pixel is brighter than its surroundings and lowered where darker
                // (a stronger effect for the sun's hard light than a soft lamp's).
                let scene_part: [f32; 3] = std::array::from_fn(|c| amb[c] + (amb_sky[c] - amb[c]) * sky_w);
                let detail = {
                    let b = detail_blur[i].max(1e-4);
                    ((luma(base) / b) - 1.0).clamp(-0.6, 0.6) * (1.0 - sky_w)
                };
                // The surface's own colour over the light already on it (see `shading`): 1 at the
                // photo's average, more in shadow (up to 3×), a little less where it's bright.
                let own_m = crate::scene::metres_of(own, d_near, d_far);
                let lit_here = shade_grid.at(fx, fy, own_m);
                // Square-rooted and capped: a dark area may be a shadow, or simply a dark surface
                // (the sea, a navy shirt) in full light, and the photo can't tell which; lifting
                // fully turned dark blue water bright cyan under a sun. Half way, in stops, is a
                // shadow lifted well and a dark surface left looking dark.
                let lift = (shading_ref / lit_here.max(0.06 * shading_ref)).sqrt().clamp(0.75, 3.0);
                let lift = 1.0 + (lift - 1.0) * (1.0 - sky_w);
                // A rim lifts in full (up to 3 stops): it falls only on an edge, where the "dark
                // surface, not a shadow" doubt is small, and a backlit edge in a dark, moody photo
                // has to come up bright to read as a rim at all.
                let rim_lift = (shading_ref / lit_here.max(0.06 * shading_ref)).clamp(1.0, 8.0);
                let rim_lift = 1.0 + (rim_lift - 1.0) * (1.0 - sky_w);
                let gain: [f32; 3] = std::array::from_fn(|c| {
                    let light = (gain[c] - scene_part[c]).max(0.0);
                    let rim = from_rim[c].min(light);
                    scene_part[c] + (light - rim) * lift * (1.0 + texture * detail).max(0.0) + rim * rim_lift
                });
                if !sky_light.grades() {
                    return std::array::from_fn(|c| (base[c] * gain[c]).max(0.0));
                }
                // The scene's own light (graded for the night) and the lamps' own, kept apart so the
                // grade never touches what a flash adds.
                // At night the moon's own light is seen by night-adapted eyes too: graded with it.
                let own: [f32; 3] = std::array::from_fn(|c| amb[c] + (amb_sky[c] - amb[c]) * sky_w + from_sky[c] * (1.0 - sky_w));
                let lit: [f32; 3] = std::array::from_fn(|c| (base[c] * (gain[c] - own[c])).max(0.0));
                let shown = sky_light.grade(std::array::from_fn(|c| (base[c] * own[c]).max(0.0)));
                std::array::from_fn(|c| shown[c] + lit[c])
            })
            .collect()
    };
    Some(SceneImage { width: w, height: h, px: out, source_long: image.source_long })
}

/// Match the light: fits a sun to the photo's own shading — the direction whose `N·L` best follows
/// the picture's luminance (a robust correlation over the non-sky pixels). Returns the light, or
/// None without normals or geometry.
pub fn match_light(image: &SceneImage, depth: &Refined) -> Option<Light> {
    let (w, h) = (image.width, image.height);
    let (_, far) = depth.anchors?;
    let focal = depth.focal_px.filter(|f| *f > 1.0)?;
    let normals = depth.normals.as_deref()?;
    let step = ((w * h) / 4000).max(1);
    let mut samples: Vec<([f32; 3], f32)> = Vec::new();
    for i in (0..w * h).step_by(step) {
        let (u, v) = (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32);
        if depth.sky.as_ref().is_some_and(|s| s.at(u, v) > 0.5) {
            continue;
        }
        let l = luma(image.px[i]);
        if !l.is_finite() || l <= 0.0 {
            continue;
        }
        samples.push((normals.at(u, v), l));
    }
    if samples.len() < 32 {
        return None;
    }
    // Rank-transform the luminance: the fit follows the shading's order, not exposure. Equal
    // values (flat surfaces) share their average rank, so they can't be read as an order.
    let mut order: Vec<usize> = (0..samples.len()).collect();
    order.sort_by(|&a, &b| samples[a].1.total_cmp(&samples[b].1));
    let mut rank = vec![0.0f32; samples.len()];
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && samples[order[j + 1]].1 == samples[order[i]].1 {
            j += 1;
        }
        let avg = (i + j) as f32 / 2.0 / (samples.len() - 1) as f32;
        for &k in &order[i..=j] {
            rank[k] = avg;
        }
        i = j + 1;
    }
    let mut best = (f32::NEG_INFINITY, 45.0f32, 35.0f32);
    for a in 0..16 {
        for e in 0..8 {
            let az = a as f32 * 22.5;
            let el = 5.0 + e as f32 * 11.0;
            let dir = sun_direction(az, el);
            let mut vals: Vec<f32> = samples.iter().map(|(n, _)| dot(*n, dir)).collect();
            // Pearson correlation between N·L and the luminance's rank.
            let mean_v = vals.iter().sum::<f32>() / vals.len() as f32;
            let mean_r = 0.5;
            let mut cov = 0.0;
            let mut va = 0.0;
            let mut vb = 0.0;
            for (v, r) in vals.iter_mut().zip(rank.iter()) {
                let dv = *v - mean_v;
                cov += dv * (*r - mean_r);
                va += dv * dv;
                vb += (*r - mean_r) * (*r - mean_r);
            }
            let corr = if va > 1e-9 && vb > 1e-9 { cov / (va * vb).sqrt() } else { 0.0 };
            if corr > best.0 {
                best = (corr, az, el);
            }
        }
    }
    let _ = (far, focal);
    let mut light = Light { kind: LightKind::Sun, on: true, intensity: 35.0, kelvin: 5600.0, softness: 20.0, shadows: 0.0, azimuth: best.1, elevation: best.2, ..Default::default() };
    light.id = 1000;
    Some(light)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creative::LightKind;
    use crate::edges::EdgeMaps;
    use std::sync::Arc;

    fn depth(w: usize, h: usize, control: f32) -> Refined {
        let grid = Grid { w, h, data: vec![control; w * h] };
        Refined {
            control: Arc::new(grid.clone()),
            sky: None,
            edges: Some(Arc::new(EdgeMaps::flat(&grid))),
            anchors: Some((1.0, 100.0)),
            focal_px: Some(120.0),
            ground: None,
            normals: Some(Arc::new(crate::scene::Normals { w, h, data: Arc::new(vec![[0.0, 0.0, -1.0]; w * h]) })),
        }
    }

    fn photo(w: usize, h: usize, level: f32) -> SceneImage {
        SceneImage { width: w, height: h, px: vec![[level, level, level]; w * h], source_long: w }
    }

    fn on(lights: Vec<Light>, ambient: f32) -> Relight {
        Relight { enabled: true, ambient, relief: 0.0, lights }
    }

    #[test]
    fn the_ambient_is_in_stops() {
        assert!((ambient_gain(0.0) - 1.0).abs() < 1e-5);
        assert!((ambient_gain(33.3333) - 2.0).abs() < 1e-3);
        assert!((ambient_gain(-33.3333) - 0.5).abs() < 1e-3);
        assert!(ambient_gain(100.0) <= 8.0 + 1e-3);
    }

    #[test]
    fn the_wrap_keeps_lamberts_energy() {
        // The hemisphere mean of the wrapped diffuse is Lambert's, whatever the softness.
        let mean = |s: f32| {
            let n = 2000;
            (0..n).map(|i| wrap((i as f32 + 0.5) / n as f32, s)).sum::<f32>() / n as f32
        };
        for s in [0.0f32, 25.0, 50.0, 100.0] {
            assert!((mean(s) - 0.5).abs() < 2e-3, "softness {s}: {}", mean(s));
        }
    }

    #[test]
    fn the_falloff_is_an_inverse_square_in_metres() {
        // At the reach half the light is left; twice the reach a fifth; four times a seventeenth.
        assert!((falloff(10.0, 10.0) - 0.5).abs() < 1e-5);
        assert!((falloff(20.0, 10.0) - 0.2).abs() < 1e-5);
        assert!((falloff(40.0, 10.0) - 1.0 / 17.0).abs() < 1e-5);
        // And the reach follows the scene's own far anchor.
        let light = Light { size: 50.0, ..Default::default() };
        assert!(reach_metres(&light, 20.0) > reach_metres(&light, 2.0));
        // And a bigger source reaches further at the same distance.
        assert!(reach_metres(&Light { size: 90.0, ..Default::default() }, 10.0) > reach_metres(&light, 10.0));
    }

    #[test]
    fn a_spot_cone_is_soft_at_its_edge() {
        // Inside the inner cone it is full, outside the outer one none, and in between it eases.
        assert!((cone(1.0, 30.0, 0.0) - 1.0).abs() < 1e-5);
        assert!(cone(5.0f32.to_radians().cos(), 30.0, 50.0) > 0.98);
        // Feather 50 runs from 16.5° (full) to 30° (none), so 24° is part way.
        let mid_soft = cone(24.0f32.to_radians().cos(), 30.0, 50.0);
        assert!(mid_soft > 0.01 && mid_soft < 0.99, "edge {mid_soft}");
        assert_eq!(cone(40.0f32.to_radians().cos(), 30.0, 50.0), 0.0);
        // A sharper feather is fuller at the same angle than a very soft one.
        let mid = 24.0f32.to_radians().cos();
        assert!(cone(mid, 30.0, 10.0) > cone(mid, 30.0, 90.0));
    }

    #[test]
    fn the_sun_direction_follows_azimuth_and_elevation() {
        // Azimuth 0 at the horizon: from the right. Elevation 90: straight down (camera y is down).
        let right = sun_direction(0.0, 0.0);
        assert!((right[0] - 1.0).abs() < 1e-5 && right[1].abs() < 1e-5);
        let above = sun_direction(0.0, 90.0);
        assert!(above[1] < -0.99, "{above:?}");
        // Azimuth 90: from behind the subject (forward, +z).
        let behind = sun_direction(90.0, 0.0);
        assert!(behind[2] > 0.99, "{behind:?}");
    }

    #[test]
    fn a_point_light_lights_a_facing_surface_and_not_sky() {
        let d = depth(8, 8, 0.5);
        let image = photo(8, 8, 0.25);
        let light = Light { kind: LightKind::Point, u: 0.5, v: 0.5, depth: 0.0, intensity: 90.0, ..Default::default() };
        let out = apply(&image, &d, &on(vec![light], 0.0), None).unwrap();
        let lit = luma(out.px[0]);
        assert!(lit > luma(image.px[0]), "the light did not light anything: {lit}");
        // Sky: ambient only.
        let sky = Grid { w: 8, h: 8, data: vec![1.0; 64] };
        let d_sky = Refined { sky: Some(Arc::new(sky)), ..Refined { control: d.control.clone(), sky: None, edges: d.edges.clone(), anchors: d.anchors, focal_px: d.focal_px, ground: d.ground, normals: d.normals.clone() } };
        let out_sky = apply(&image, &d_sky, &on(vec![Light { intensity: 100.0, ..Default::default() }], 0.0), None).unwrap();
        for p in &out_sky.px {
            assert!((luma(*p) - 0.25).abs() < 1e-5, "sky lit by a point light: {p:?}");
        }
    }

    #[test]
    fn ambient_dims_and_light_off_is_bit_identical() {
        let d = depth(8, 8, 0.5);
        let image = photo(8, 8, 0.3);
        let dim = apply(&image, &d, &on(vec![], -33.3333), None).unwrap();
        assert!((luma(dim.px[0]) - 0.15).abs() < 1e-3);
        assert!(apply(&image, &d, &Relight::default(), None).is_none());
        // On but with nothing on gives the photo back unchanged.
        let same = apply(&image, &d, &on(vec![], 0.0), None).unwrap();
        assert_eq!(image.px, same.px);
    }

    #[test]
    fn light_colour_keeps_its_luminance() {
        for k in [2000.0f32, 3400.0, 5600.0, 9000.0] {
            let c = blackbody(k);
            assert!((luma(c) - 1.0).abs() < 1e-3, "{k}: {c:?}");
        }
        // Warm is redder than cool.
        let warm = blackbody(2700.0);
        let cool = blackbody(9000.0);
        assert!(warm[0] / warm[2] > cool[0] / cool[2]);
    }

    #[test]
    fn a_points_spread_is_wide_by_default_and_turns_with_its_aim() {
        // 85° with a wide feather reaches almost everywhere; a narrow panel is dark behind.
        assert!(panel_lobe(0.1, 85.0, 90.0) > 0.05);
        assert!(panel_lobe(0.9, 85.0, 45.0) > 0.99);
        assert_eq!(panel_lobe(-0.5, 30.0, 10.0), 0.0);
        assert!(panel_lobe(0.95, 30.0, 10.0) > 0.9);
    }

    #[test]
    fn a_spots_axis_follows_its_aim() {
        // A spot above the frame pointing at the right half: its axis leans right; aimed left, left.
        let at = |aim_u: f32| {
            let light = Light { kind: LightKind::Spot, u: 0.5, v: 0.1, depth: 20.0, aim_u, aim_v: 0.6, ..Default::default() };
            aim_axis(&light, 120.0, 1600.0, 1000.0, 1.0, 20.0, |_, _| 5.0)
        };
        assert!(at(0.9)[0] > 0.05, "aimed right, the axis should lean right");
        assert!(at(0.1)[0] < -0.05, "aimed left, it should lean left");
        // Straight down the middle stays a sane unit vector.
        let mid = at(0.5);
        assert!((dot(mid, mid) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_lights_own_yaw_and_pitch_set_its_axis() {
        // The scope's rings: yaw 90 faces right, pitch + is up (camera y is down), and the pair wins
        // over the old aim point when both are present.
        let right = yaw_pitch_direction(90.0, 0.0);
        assert!(right[0] > 0.99 && right[2].abs() < 1e-3, "yaw 90 should face right: {right:?}");
        let up = yaw_pitch_direction(0.0, 90.0);
        assert!(up[1] < -0.99, "pitch 90 should face up (camera y is down): {up:?}");
        let turned = Light { kind: LightKind::Spot, yaw: Some(90.0), pitch: Some(0.0), aim_u: 0.1, aim_v: 0.1, ..Default::default() };
        let axis = aim_axis(&turned, 120.0, 1600.0, 1000.0, 1.0, 20.0, |_, _| 5.0);
        assert!(axis[0] > 0.99, "the own angles should win over the aim point: {axis:?}");
        // A full turn comes back to the start.
        let round = yaw_pitch_direction(360.0, 0.0);
        assert!((round[2] - 1.0).abs() < 1e-3 && round[0].abs() < 1e-3, "360 wraps to forward: {round:?}");
    }

    #[test]
    fn the_light_position_follows_depth() {
        let mut light = Light { kind: LightKind::Point, u: 0.5, v: 0.5, depth: 0.0, ..Default::default() };
        let front = light_position(&light, 120.0, 8.0, 8.0, 1.0, 100.0);
        light.depth = 100.0;
        let back = light_position(&light, 120.0, 8.0, 8.0, 1.0, 100.0);
        assert!(back[2] > front[2]);
        // Depth 50 is the scene's middle, with as much room in front as behind.
        light.depth = 50.0;
        let mid = light_position(&light, 120.0, 8.0, 8.0, 1.0, 9.0);
        assert!((mid[2] - 3.0).abs() < 1e-3, "geometric middle of 1 and 9 is 3: {mid:?}");
        light.depth = 0.0;
        let front = light_position(&light, 120.0, 8.0, 8.0, 1.0, 9.0);
        light.depth = 100.0;
        let back = light_position(&light, 120.0, 8.0, 8.0, 1.0, 9.0);
        assert!((mid[2] - front[2] - (back[2] - mid[2])).abs() < 1e-3, "symmetric about the middle: {front:?} {mid:?} {back:?}");
    }

    #[test]
    fn matching_the_light_recovers_its_direction() {
        // Surfaces facing round the vertical, lit by a sun at 30° azimuth: the fit should find it.
        let (w, h) = (48usize, 36usize);
        let truth = sun_direction(30.0, 20.0);
        let normals: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let a = (i % w) as f32 / w as f32 * std::f32::consts::TAU;
                [a.cos(), 0.0, a.sin()]
            })
            .collect();
        let px: Vec<[f32; 3]> = normals.iter().map(|n| [0.05 + 0.9 * dot(*n, truth).max(0.0); 3]).collect();
        let image = SceneImage { width: w, height: h, px, source_long: w };
        let grid = Grid { w, h, data: vec![0.6; w * h] };
        let d = Refined {
            control: Arc::new(grid.clone()),
            sky: None,
            edges: Some(Arc::new(EdgeMaps::flat(&grid))),
            anchors: Some((1.0, 100.0)),
            focal_px: Some(120.0),
            ground: None,
            normals: Some(Arc::new(crate::scene::Normals { w, h, data: Arc::new(normals) })),
        };
        let l = match_light(&image, &d).expect("a fit");
        assert_eq!(l.kind, LightKind::Sun);
        let dir = sun_direction(l.azimuth, l.elevation);
        assert!(dot(dir, truth) > 0.9, "azimuth {} elevation {} gives {dir:?}, wanted {truth:?}", l.azimuth, l.elevation);
    }

    #[test]
    fn a_light_behind_the_subject_rims_its_edge_and_leaves_its_front_dark() {
        // A near disc (the subject) on a far background, lit by a lamp right behind it, facing the
        // camera: the disc's edge catches a rim, its middle (facing the camera) gets next to none.
        let (w, h) = (64usize, 64usize);
        let data: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - 31.5, (i / w) as f32 - 31.5);
                if (x * x + y * y).sqrt() < 16.0 { 0.75 } else { 0.15 }
            })
            .collect();
        let grid = Grid { w, h, data };
        let d = Refined {
            control: Arc::new(grid.clone()),
            sky: None,
            edges: Some(Arc::new(EdgeMaps::flat(&grid))),
            anchors: Some((1.0, 100.0)),
            focal_px: Some(120.0),
            ground: None,
            normals: Some(Arc::new(crate::scene::Normals { w, h, data: Arc::new(vec![[0.0, 0.0, -1.0]; w * h]) })),
        };
        let image = photo(w, h, 0.25);
        let lamp = Light { kind: LightKind::Point, u: 0.5, v: 0.3, depth: 25.0, intensity: 80.0, softness: 30.0, yaw: Some(180.0), pitch: Some(0.0), ..Default::default() };
        let out = apply(&image, &d, &on(vec![lamp], 0.0), None).unwrap();
        let at = |x: usize, y: usize| luma(out.px[y * w + x]);
        let middle = at(32, 32);
        let top_edge = at(32, 32 - 15);
        assert!(middle < 0.25 * 1.1, "the front of a backlit subject should stay near its own level: {middle}");
        assert!(top_edge > middle * 1.3, "the edge towards the lamp should catch a rim: edge {top_edge}, middle {middle}");
    }

    #[test]
    fn the_sun_dims_low_in_the_sky_and_its_spread_is_a_patch() {
        assert!((atmosphere(90.0) - 1.0).abs() < 0.01);
        let low = atmosphere(10.0);
        assert!(low < 0.75 && low > 0.4, "10° should be about a stop down: {low}");
        assert!(atmosphere(0.0) < low && atmosphere(0.0) > 0.1);
        // A sun from the front-left lighting a flat wall: whole-scene versus a narrow patch on the
        // middle. The patch keeps the middle lit and leaves the corner as it was.
        let d = depth(32, 32, 0.5);
        let image = photo(32, 32, 0.25);
        let sun = Light { kind: LightKind::Sun, azimuth: 250.0, elevation: 20.0, intensity: 60.0, cone: 100.0, ..Default::default() };
        let all = apply(&image, &d, &on(vec![sun.clone()], 0.0), None).unwrap();
        let patch = apply(&image, &d, &on(vec![Light { cone: 5.0, feather: 20.0, aim_u: 0.5, aim_v: 0.5, ..sun }], 0.0), None).unwrap();
        let (mid, corner) = (16 * 32 + 16, 0);
        assert!(luma(all.px[corner]) > 0.26, "the whole-scene sun lights the corner");
        assert!((luma(patch.px[mid]) - luma(all.px[mid])).abs() < 1e-3, "the patch's middle is lit as fully");
        assert!((luma(patch.px[corner]) - 0.25).abs() < 1e-3, "outside the patch is left alone: {}", luma(patch.px[corner]));
        // The moon is a celestial light too: parallel, no falloff.
        let moon = apply(&image, &d, &on(vec![Light { kind: LightKind::Moon, azimuth: 250.0, elevation: 30.0, intensity: 40.0, cone: 100.0, ..Default::default() }], 0.0), None).unwrap();
        assert!((luma(moon.px[mid]) - luma(moon.px[corner])).abs() < 0.02, "moonlight is even across a flat wall");
    }

    #[test]
    fn the_ambient_takes_the_skys_character_from_the_sun_and_moon() {
        // Room lights only: plain dimming.
        let lamp = Light { kind: LightKind::Point, ..Default::default() };
        let plain = SkyAmbient::of(&on(vec![lamp], -66.0), 5500.0);
        assert!(!plain.grades() && plain.tint.iter().all(|t| (t - 1.0).abs() < 1e-6));
        // A low sun with the ambient down: the shadows go blue; a high one barely.
        let low = SkyAmbient::of(&on(vec![Light { kind: LightKind::Sun, elevation: 5.0, ..Default::default() }], -50.0), 5500.0);
        let high = SkyAmbient::of(&on(vec![Light { kind: LightKind::Sun, elevation: 60.0, ..Default::default() }], -50.0), 5500.0);
        assert!(low.tint[2] > low.tint[0] * 1.03 && low.tint[2] < low.tint[0] * 1.6, "a low sun's ambient should lean blue, a hint not a cast: {:?}", low.tint);
        assert!((high.tint[2] - high.tint[0]).abs() < 0.02, "a high sun's ambient stays neutral: {:?}", high.tint);
        // The moon with the ambient two stops down: night — colour out, highlights pulled down
        // harder than the midtones, the sky darker still, deep shadows kept off black.
        let night = SkyAmbient::of(&on(vec![Light { kind: LightKind::Moon, elevation: 30.0, ..Default::default() }], -66.0), 5500.0);
        assert!(night.grades() && night.sky_gain < 0.7);
        let red = night.grade([0.4, 0.1, 0.1]);
        assert!(red[0] - red[2] < 0.3 * (0.4 - 0.1), "colour should mostly fade: {red:?}");
        let (bright, mid) = (luma(night.grade([0.8; 3])) / 0.8, luma(night.grade([0.1; 3])) / 0.1);
        assert!(bright < mid, "highlights should come down harder than midtones: {bright} vs {mid}");
        assert!(luma(night.grade([0.0; 3])) > 0.0, "the deepest shadows keep a little");
    }

    #[test]
    fn a_light_into_a_shadow_lifts_it() {
        // Half the picture in shadow (a tenth of the light), half lit, the same surface: the same
        // lamp on both adds the same light, so the shadow comes up by far more than the lit half
        // (it was multiplied before, and a shadow stayed dark).
        let (w, h) = (64usize, 32usize);
        let d = depth(w, h, 0.5);
        let px: Vec<[f32; 3]> = (0..w * h).map(|i| if i % w < w / 2 { [0.02; 3] } else { [0.2; 3] }).collect();
        let image = SceneImage { width: w, height: h, px, source_long: w };
        let lamp = Light { kind: LightKind::Point, u: 0.5, v: 0.5, depth: 0.0, intensity: 60.0, softness: 50.0, ..Default::default() };
        let out = apply(&image, &d, &on(vec![lamp], 0.0), None).unwrap();
        let (dark_in, lit_in) = (0.02f32, 0.2f32);
        let dark_out = luma(out.px[(h / 2) * w + 4]);
        let lit_out = luma(out.px[(h / 2) * w + w - 5]);
        let (dark_gain, lit_gain) = (dark_out - dark_in, lit_out - lit_in);
        assert!(dark_gain > 0.0 && lit_gain > 0.0);
        assert!(dark_gain / dark_in > 2.0 * (lit_gain / lit_in), "the shadow should come up relatively much more: {dark_gain} on {dark_in} vs {lit_gain} on {lit_in}");
    }
}
