//! Mist: depth fog with Koschmieder / Beer–Lambert transmittance, exponential height fog with an
//! analytic optical depth, sun glow (Henyey–Greenstein), seeded 3D noise banks and a subject
//! protection. Our own implementation of the ideas in `RESEARCH.md`; no other editor's code.
//!
//! It runs on the scene light before the tone curve, and before Bokeh, so the fog is part of what
//! Bokeh blurs. Distances come from the same refined control depth the masks and Bokeh use (log
//! distance between the scene's robust anchors), with the two surfaces at depth edges so a tree
//! line or hair has no fog rim.

use crate::creative::Mist;
use crate::gradient::Grid;
use crate::pipeline::{Frame, SceneImage};
use crate::scene::Refined;

/// The fog's airlight: the scene's own ambient level (a robust high-ish percentile of the
/// non-sky pixels' luminance), so fog in a dark photo isn't self-luminous.
pub fn ambient(image: &SceneImage, depth: &Refined) -> f32 {
    let step = ((image.width * image.height) / 20_000).max(1);
    let mut values: Vec<f32> = Vec::new();
    let mut all: Vec<f32> = Vec::new();
    let mut sky_values: Vec<f32> = Vec::new();
    for i in (0..image.px.len()).step_by(step) {
        let p = image.px[i];
        let l = (0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).max(0.0);
        if !l.is_finite() {
            continue;
        }
        all.push(l);
        if let Some(sky) = &depth.sky {
            let (u, v) = (((i % image.width) as f32 + 0.5) / image.width as f32, ((i / image.width) as f32 + 0.5) / image.height as f32);
            if sky.at(u, v) > 0.5 {
                sky_values.push(l);
                continue;
            }
        }
        values.push(l);
    }
    let total = all.len();
    if values.is_empty() {
        values = all;
    }
    if values.is_empty() {
        return 0.18;
    }
    values.sort_by(f32::total_cmp);
    let at = ((values.len() - 1) as f32 * 0.6) as usize;
    let land = values[at].max(1e-4);
    // Fog is lit by the sky it hangs under: where the photo shows sky, the fog is about as bright
    // as it (a little under, as the sky's light is spread through the fog), never darker than the
    // land's own light. Without it, fog came out a grey that darkened a bright day.
    if sky_values.len() >= total / 100 && sky_values.len() > 20 {
        sky_values.sort_by(f32::total_cmp);
        let sky = sky_values[sky_values.len() / 2];
        return (0.85 * sky).max(land);
    }
    land
}

/// A value noise's hash at an integer lattice point, deterministic.
fn hash3(x: i32, y: i32, z: i32, seed: i32) -> f32 {
    let mut h = (x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263) ^ z.wrapping_mul(2147483647) ^ seed.wrapping_mul(1274126177)) as u32;
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    h = h ^ (h >> 16);
    (h & 0xffffff) as f32 / 0xffffff as f32
}

/// Smooth 3D value noise, 0..1, at `p` (metres) over a cell size in metres.
pub fn noise3(p: [f32; 3], scale: f32, seed: i32) -> f32 {
    let q = p.map(|v| v / scale);
    let (x0, y0, z0) = (q[0].floor() as i32, q[1].floor() as i32, q[2].floor() as i32);
    let f = [q[0] - x0 as f32, q[1] - y0 as f32, q[2] - z0 as f32];
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy, sz) = (s(f[0]), s(f[1]), s(f[2]));
    let mut v = 0.0;
    for dz in 0..2 {
        for dy in 0..2 {
            for dx in 0..2 {
                let h = hash3(x0 + dx, y0 + dy, z0 + dz, seed);
                let w = (if dx == 1 { sx } else { 1.0 - sx }) * (if dy == 1 { sy } else { 1.0 - sy }) * (if dz == 1 { sz } else { 1.0 - sz });
                v += h * w;
            }
        }
    }
    v.clamp(0.0, 1.0)
}

/// The fog colour's gains from Temperature and Tint: warm/cool like the white balance sliders,
/// normalised to unit luminance so `ambient` alone sets the airlight's brightness.
/// The colour of the light the fog hangs in: the sky's average colour (or, without a sky, the
/// farthest tenth of the scene), at unit luminance and halfway to grey, as fog mutes it.
fn scene_tint(image: &SceneImage, depth: &Refined) -> [f32; 3] {
    let step = ((image.width * image.height) / 20_000).max(1);
    let (mut sky_sum, mut sky_n) = ([0.0f64; 3], 0usize);
    let (mut far_sum, mut far_n) = ([0.0f64; 3], 0usize);
    for i in (0..image.px.len()).step_by(step) {
        let p = image.px[i];
        if !(p[0].is_finite() && p[1].is_finite() && p[2].is_finite()) {
            continue;
        }
        let (u, v) = (((i % image.width) as f32 + 0.5) / image.width as f32, ((i / image.width) as f32 + 0.5) / image.height as f32);
        if depth.sky.as_ref().is_some_and(|s| s.at(u, v) > 0.5) {
            for c in 0..3 {
                sky_sum[c] += p[c] as f64;
            }
            sky_n += 1;
        } else if depth.control.at(u, v) < 0.1 {
            for c in 0..3 {
                far_sum[c] += p[c] as f64;
            }
            far_n += 1;
        }
    }
    let (sum, n) = if sky_n > 50 { (sky_sum, sky_n) } else { (far_sum, far_n) };
    if n < 20 {
        return [1.0; 3];
    }
    let c = sum.map(|v| (v / n as f64) as f32);
    let l = (0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]).max(1e-6);
    let c = c.map(|v| v / l);
    // Halfway to grey: fog mutes the sky's colour; and never a wild cast.
    c.map(|v| (0.5 + 0.5 * v).clamp(0.6, 1.6))
}

fn fog_colour(mist: &Mist) -> [f32; 3] {
    let t = (mist.temperature / 100.0).clamp(-1.0, 1.0);
    let g = (mist.tint / 100.0).clamp(-1.0, 1.0);
    let mut rgb = [2f32.powf(0.35 * t), 2f32.powf(-0.25 * g), 2f32.powf(-0.35 * t)];
    let l = 0.2627 * rgb[0] + 0.678 * rgb[1] + 0.0593 * rgb[2];
    if l > 1e-6 {
        rgb = rgb.map(|v| v / l);
    }
    rgb
}

/// The Henyey-Greenstein phase, normalised to 1 at `cos = 1` (the glow's own direction), so a
/// setting of 100 is a strong glow and the shape doesn't depend on the scattering constant.
fn henyey_greenstein(cos: f32) -> f32 {
    let g = 0.6f32;
    let base = 1.0 + g * g - 2.0 * g;
    let v = 1.0 + g * g - 2.0 * g * cos.clamp(-1.0, 1.0);
    (base / v.max(1e-4)).powf(1.5)
}

/// The analytic exponential height-fog optical depth along a ray: the uniform part and the
/// exponential part, integrated in closed form. `height` is the camera height above the plane,
/// `slope` the height lost per metre along the ray, `scale` the exponential scale height and `k`
/// how much of the fog follows the ground (0 uniform).
fn height_integral(h0: f32, slope: f32, scale: f32, z: f32) -> f32 {
    if z <= 0.0 {
        return 0.0;
    }
    let z = if slope < -1e-6 { z.min((h0 / -slope).max(0.0)) } else { z };
    if slope.abs() > 1e-5 {
        (1.0 - (-slope * z / scale).exp()) / (slope / scale)
    } else {
        z
    }
}

/// The mean 3D noise over fixed cells along this ray, 0..1.
fn mist_noise(mist: &Mist, dir: &[f32; 3]) -> f32 {
    let seed = if mist.seed >= 0.5 { 1 } else { 0 };
    let mut sum = 0.0;
    for frac in [0.25f32, 0.55, 0.9] {
        let t = frac * 120.0; // fixed 120 m reach: the same cells however near or far the surface is
        sum += noise3([dir[0] * t, dir[1] * t, dir[2] * t], 12.0, seed);
    }
    sum / 3.0
}

/// Mist: `None` when it is off (so a photo without it is bit-identical).
pub fn apply(image: &SceneImage, depth: &Refined, mist: &Mist, subject: Option<&Grid>, frame: Option<&Frame>, ambient_level: Option<f32>) -> Option<SceneImage> {
    if !mist.enabled {
        return None;
    }
    let (w, h) = (image.width, image.height);
    if w < 4 || h < 4 {
        return None;
    }
    // Without metric depth there is no distance to fog: the brief's model needs metres.
    let (Some((d_near, d_far)), Some(focal)) = (depth.anchors, depth.focal_px.filter(|f| *f > 1.0)) else {
        return Some(image.clone());
    };
    let (cw, ch) = (depth.control.w.max(1) as f32, depth.control.h.max(1) as f32);
    let ambient_level = ambient_level.unwrap_or_else(|| ambient(image, depth));
    // The fog's colour: the scene's own sky light (fog is lit by the sky it hangs under), softened
    // towards grey, then Warm on top.
    let scene_tint = scene_tint(image, depth);
    let fc = fog_colour(mist);
    let airlight: [f32; 3] = {
        let c = [fc[0] * scene_tint[0], fc[1] * scene_tint[1], fc[2] * scene_tint[2]];
        let l = (0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]).max(1e-6);
        c.map(|v| v / l * ambient_level)
    };
    // Fog scatters light sideways too: what's seen through it is softened as well as veiled.
    let soft_view: Option<Vec<[f32; 3]>> = (frame.is_none() && mist.density > 2.0).then(|| {
        let r = ((w.max(h) as f32 / 500.0).round() as usize).max(1);
        let ch_blur = |c: usize| crate::relight::box_blur(&image.px.iter().map(|p| p[c]).collect::<Vec<_>>(), w, h, r);
        let (b0, b1, b2) = (ch_blur(0), ch_blur(1), ch_blur(2));
        (0..w * h).map(|i| [b0[i], b1[i], b2[i]]).collect()
    });
    // Density: as before up to the middle (saved edits look the same), and much thicker at the
    // top, so dense fog veils what's near the camera too, as a real fog bank does (24 Nov, Chris).
    // Amount is the fog's visibility, on a log scale: from a thin haze (eight times the scene's
    // far distance) to a pea-souper whose visibility is twice the nearest distance. So the
    // middle of the slider is real fog, and near the top it closes in on the camera, whatever the
    // scene's size (it used to scale with the far distance alone: a landscape's foreground stayed
    // clear at any density).
    let dn = (mist.density / 100.0).clamp(0.0, 1.0);
    let visibility = {
        let (thin, thick) = ((8.0 * d_far.max(0.1)).ln(), (2.0 * d_near.max(0.05)).ln());
        (thin + (thick - thin) * dn.powf(1.7)).exp()
    };
    let beta = if dn > 0.0 { 3.0 / visibility } else { 0.0 };
    let start = (mist.start / 100.0).clamp(0.0, 1.0);
    let soft = (mist.softness / 100.0).clamp(0.0, 1.0);
    // Over the top of End it reaches on from the nearest anchor to the camera, so End at the top
    // covers everything, the subject too.
    // Reaching the camera starts earlier (from 70, not 90), so the fog comes right up to the lens
    // without End having to sit at the very top.
    let reach = {
        let t = ((start - 0.7) / 0.3).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let d_start = crate::scene::metres_of(start, d_near, d_far) * (1.0 - reach);
    let far = (mist.far / 100.0).clamp(0.0, 1.0);
    let d_stop = if far > 0.0 { crate::scene::metres_of(far, d_near, d_far) } else { f32::INFINITY };
    // The glow's direction: the image plane at the far anchor, which may be off frame.
    let glow_dir = {
        let gx = (mist.glow_u - 0.5) * cw / focal;
        let gy = (mist.glow_v - 0.5) * ch / focal;
        let len = (gx * gx + gy * gy + 1.0).sqrt();
        [gx / len, gy / len, 1.0 / len]
    };
    // The ground plane, tilted by Horizon (50 = as estimated).
    let tilt = ((mist.horizon - 50.0) / 50.0).clamp(-1.0, 1.0) * 10f32.to_radians();
    let (sn, cs) = tilt.sin_cos();
    let ground = depth.ground.map(|g| crate::scene::Ground {
        normal: [g.normal[0], g.normal[1] * cs - g.normal[2] * sn, g.normal[1] * sn + g.normal[2] * cs],
        offset: g.offset,
    });
    let height_scale = 150.0 * 0.02f32.powf((mist.height_falloff / 100.0).clamp(0.0, 1.0));
    let k = (mist.height / 100.0).clamp(0.0, 1.0);
    let texture = mist.texture;
    let warm = [1.0f32, 0.72, 0.42];
    let glow_strength = (mist.glow / 100.0).clamp(0.0, 1.0);
    let sky_grid = depth.sky.as_deref();
    let noise_of_ray = |dir: &[f32; 3]| -> f32 {
        if texture > 1e-4 {
            1.0 + (texture / 100.0) * 0.9 * mist_noise(mist, dir)
        } else {
            1.0
        }
    };
    let out: Vec<[f32; 3]> = {
        use rayon::prelude::*;
        (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (fx, fy) = frame.map_or((((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32), |f| f.frac(i, w));
                let (n, fr, alpha) = depth.edges.as_ref().map_or((depth.control.at(fx, fy), depth.control.at(fx, fy), 1.0), |e| e.at(fx, fy));
                let sky = sky_grid.map_or(0.0, |s| s.at(fx, fy)).clamp(0.0, 1.0);
                let (dx, dy) = ((fx - 0.5) * cw / focal, (fy - 0.5) * ch / focal);
                let dl = (dx * dx + dy * dy + 1.0).sqrt();
                let dir = [dx / dl, dy / dl, 1.0 / dl];
                let protect = subject.map_or(1.0, |s| 1.0 - (mist.protect_subject / 100.0).clamp(0.0, 1.0) * s.at(fx, fy).clamp(0.0, 1.0));
                let noise = noise_of_ray(&dir);
                // One surface's transmittance and the in-scattered light it gains.
                let surface = |control_value: f32| -> (f32, [f32; 3]) {
                    let z = crate::scene::metres_of(control_value.clamp(0.0, 1.0), d_near, d_far);
                    let onset = if soft <= 1e-4 {
                        if control_value <= start { 1.0 } else { 0.0 }
                    } else {
                        let t = ((start - control_value) / soft).clamp(0.0, 1.0);
                        t * t * (3.0 - 2.0 * t)
                    };
                    let onset = onset + (1.0 - onset) * reach;
                    // What's beyond the far limit is seen through all the fog before it.
                    let d_eff = (z.min(d_stop) - d_start).max(0.0);
                    let tau = if d_eff <= 0.0 {
                        0.0
                    } else {
                        let mut tau = beta * (1.0 - k) * d_eff;
                        if k > 1e-4 {
                            // The ground is measured from the start plane, where the fog begins.
                            if let Some(g) = ground {
                                let h0 = (g.height([0.0, 0.0, 0.0]) + (g.normal[0] * dir[0] + g.normal[1] * dir[1] + g.normal[2] * dir[2]) * d_start).max(0.01);
                                let slope = g.normal[0] * dir[0] + g.normal[1] * dir[1] + g.normal[2] * dir[2];
                                tau += beta * k * (-h0 / height_scale).exp() * height_integral(h0, slope, height_scale, d_eff);
                            } else {
                                tau += beta * k * d_eff;
                            }
                        }
                        tau * noise
                    };
                    let tau = tau * onset * protect;
                    let t = (-tau).exp();
                    let cos = dir[0] * glow_dir[0] + dir[1] * glow_dir[1] + dir[2] * glow_dir[2];
                    let glow = glow_strength * (1.0 - t) * henyey_greenstein(cos) * 1.5;
                    (t, std::array::from_fn(|c| airlight[c] * (1.0 - t) + warm[c] * glow))
                };
                let base = image.px[i];
                // Softened through the fog it's seen through (more the more fog), before the veil.
                let base = match &soft_view {
                    Some(b) => {
                        let (tn0, _) = surface(n);
                        let m = (1.0 - tn0) * 0.55 * (1.0 - sky);
                        std::array::from_fn(|c| base[c] + (b[i][c] - base[c]) * m)
                    }
                    None => base,
                };
                if sky > 1e-4 {
                    // Sky: the fog's value at the far anchor, so the horizon continues.
                    let (t_sky, light_sky) = surface(0.0);
                    let (tn, ln) = surface(n);
                    let (tf, lf) = surface(fr);
                    let a = alpha.clamp(0.0, 1.0);
                    let t = (a * tn + (1.0 - a) * tf) * (1.0 - sky) + t_sky * sky;
                    return std::array::from_fn(|c| base[c] * t + (a * ln[c] + (1.0 - a) * lf[c]) * (1.0 - sky) + light_sky[c] * sky);
                }
                let (tn, ln) = surface(n);
                let (tf, lf) = surface(fr);
                let a = alpha.clamp(0.0, 1.0);
                let t = a * tn + (1.0 - a) * tf;
                std::array::from_fn(|c| base[c] * t + a * ln[c] + (1.0 - a) * lf[c])
            })
            .collect()
    };
    Some(SceneImage { width: w, height: h, px: out, source_long: image.source_long })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edges::EdgeMaps;
    use crate::scene::Ground;
    use std::sync::Arc;

    fn depth(w: usize, h: usize, near_control: f32) -> Refined {
        let grid = Grid { w, h, data: vec![near_control; w * h] };
        Refined {
            control: Arc::new(grid.clone()),
            sky: None,
            edges: Some(Arc::new(EdgeMaps::flat(&grid))),
            anchors: Some((1.0, 100.0)),
            focal_px: Some(120.0),
            ground: Some(Ground { normal: [0.0, -1.0, 0.0], offset: 1.6 }),
            normals: None,
        }
    }

    fn photo(w: usize, h: usize, level: f32) -> SceneImage {
        SceneImage { width: w, height: h, px: vec![[level, level, level]; w * h], source_long: w }
    }

    #[test]
    fn transmittance_falls_with_distance() {
        let d = depth(8, 8, 0.5);
        let image = photo(8, 8, 0.5);
        let mist = Mist { enabled: true, density: 60.0, start: 20.0, softness: 2.0, height: 0.0, glow: 0.0, texture: 0.0, ..Default::default() };
        let out = apply(&image, &d, &mist, None, None, Some(0.2)).unwrap();
        // Nearer (higher control) is less fogged than farther: with a uniform fog the near pixel
        // keeps more of its own light than the far one.
        let d2 = Refined {
            control: Arc::new(Grid { w: 2, h: 1, data: vec![0.9, 0.1] }),
            edges: Some(Arc::new(EdgeMaps::flat(&Grid { w: 2, h: 1, data: vec![0.9, 0.1] }))),
            ..clone_depth(&d)
        };
        let image2 = photo(4, 4, 0.5);
        let out2 = apply(&image2, &d2, &mist, None, None, Some(0.2)).unwrap();
        let far = out2.px[3][0];
        let near = out2.px[0][0];
        assert!(far < near, "far {far} should be closer to the airlight than near {near}");
        assert!(out.px[0][0].is_finite());
    }

    fn clone_depth(d: &Refined) -> Refined {
        Refined { control: d.control.clone(), sky: d.sky.clone(), edges: d.edges.clone(), anchors: d.anchors, focal_px: d.focal_px, ground: d.ground, normals: d.normals.clone() }
    }

    #[test]
    fn density_and_start_do_what_they_say() {
        let d = depth(4, 4, 0.3);
        let image = photo(4, 4, 0.1);
        let base = Mist { enabled: true, density: 60.0, start: 50.0, softness: 5.0, height: 0.0, ..Default::default() };
        let more = apply(&image, &d, &Mist { density: 90.0, ..base.clone() }, None, None, Some(0.3)).unwrap();
        let less = apply(&image, &d, &base, None, None, Some(0.3)).unwrap();
        assert!(more.px[0][0] > less.px[0][0], "more density, more airlight");
        // Fog starting farther away (a lower control start) leaves more of the photo clear.
        let far_start = apply(&image, &d, &Mist { start: 5.0, ..base.clone() }, None, None, Some(0.3)).unwrap();
        assert!(far_start.px[0][0] < less.px[0][0]);
    }

    #[test]
    fn the_two_points_bound_the_fog() {
        let image = photo(4, 4, 0.1);
        let at = |control: f32, start: f32, far: f32| {
            let mist = Mist { enabled: true, density: 60.0, start, far, softness: 5.0, ..Default::default() };
            apply(&image, &depth(4, 4, control), &mist, None, None, Some(0.5)).unwrap().px[0][0]
        };
        // End at the top covers the subject (the nearest thing too); moved down it leaves it clear.
        assert!(at(0.8, 100.0, 0.0) > 0.102 && at(1.0, 100.0, 0.0) > 0.1005);
        assert!((at(0.8, 60.0, 0.0) - 0.1).abs() < 1e-6);
        assert!(at(0.2, 60.0, 0.0) > 0.2);
        // A far limit thins what's beyond it, and leaves what's before it alone.
        assert!(at(0.1, 80.0, 40.0) < at(0.1, 80.0, 0.0));
        assert!((at(0.5, 80.0, 40.0) - at(0.5, 80.0, 0.0)).abs() < 1e-6);
        // The two together are no fog.
        assert!((at(0.1, 50.0, 50.0) - 0.1).abs() < 1e-6);
    }

    #[test]
    fn the_height_integral_matches_a_numeric_one() {
        // The analytic exponential height fog integral against 2000 steps, for a level ray and a
        // sloping one.
        let scale = 12.0f32;
        for (h0, slope) in [(1.6f32, 0.0f32), (2.0, -0.15), (0.8, 0.1)] {
            let z = 80.0f32;
            let z_eff = if slope < 0.0 { z.min(h0 / -slope) } else { z };
            let analytic = (-h0 / scale).exp() * if slope.abs() > 1e-5 { (1.0 - (-slope * z_eff / scale).exp()) / (slope / scale) } else { z_eff };
            let mut numeric = 0.0;
            let steps = 2000;
            for i in 0..steps {
                let t = z_eff * (i as f32 + 0.5) / steps as f32;
                let h = (h0 + slope * t).max(0.0);
                numeric += (-h / scale).exp() * z_eff / steps as f32;
            }
            assert!((analytic - numeric).abs() / numeric.max(1e-6) < 0.01, "{analytic} vs {numeric}");
        }
    }

    #[test]
    fn the_airlight_scales_with_the_scene_ambient() {
        let d = depth(4, 4, 0.5);
        let dim = photo(4, 4, 0.05);
        let bright = photo(4, 4, 0.5);
        let mist = Mist { enabled: true, density: 100.0, start: 90.0, softness: 1.0, height: 0.0, texture: 0.0, ..Default::default() };
        let a = ambient(&dim, &d);
        let b = ambient(&bright, &d);
        assert!((b - 0.5).abs() < 1e-3 && (a - 0.05).abs() < 1e-3);
        let out_dim = apply(&dim, &d, &mist, None, None, Some(a)).unwrap();
        let out_bright = apply(&bright, &d, &mist, None, None, Some(b)).unwrap();
        assert!(out_bright.px[0][0] > out_dim.px[0][0]);
    }

    #[test]
    fn texture_only_adds_fog_and_never_takes_it_away() {
        let d = depth(4, 4, 0.5);
        let image = photo(4, 4, 0.1);
        let clear = Mist { enabled: true, density: 50.0, start: 90.0, softness: 1.0, texture: 0.0, ..Default::default() };
        let textured = Mist { texture: 100.0, ..clear.clone() };
        let a = apply(&image, &d, &clear, None, None, Some(0.4)).unwrap();
        let b = apply(&image, &d, &textured, None, None, Some(0.4)).unwrap();
        assert!(b.px[0][0] >= a.px[0][0]);
    }

    #[test]
    fn texture_keeps_fog_growing_with_distance() {
        // Three distances along one ray, height fog and texture on: the fog only ever grows with
        // distance (the pixel moves further toward the airlight).
        let grid = Grid { w: 3, h: 1, data: vec![0.9, 0.5, 0.1] };
        let d = Refined {
            control: Arc::new(grid.clone()),
            sky: None,
            edges: Some(Arc::new(EdgeMaps::flat(&grid))),
            anchors: Some((1.0, 100.0)),
            focal_px: Some(120.0),
            ground: Some(Ground { normal: [0.0, -1.0, 0.0], offset: 1.6 }),
            normals: None,
        };
        let image = photo(4, 4, 0.1);
        let mist = Mist { enabled: true, density: 60.0, start: 95.0, softness: 2.0, height: 60.0, height_falloff: 40.0, texture: 100.0, glow: 0.0, ..Default::default() };
        let out = apply(&image, &d, &mist, None, None, Some(0.4)).unwrap();
        let near = out.px[0][0];
        let mid = out.px[1][0];
        let far = out.px[3][0];
        assert!(mid >= near - 1e-5 && far >= mid - 1e-5, "fog went backwards: {near} {mid} {far}");
    }

    #[test]
    fn mist_off_and_on_without_metric_depth_are_unchanged() {
        let image = photo(8, 8, 0.2);
        let d = depth(8, 8, 0.5);
        assert!(apply(&image, &d, &Mist::default(), None, None, None).is_none());
        let no_metric = Refined { anchors: None, ..clone_depth(&d) };
        let on = Mist { enabled: true, ..Default::default() };
        let out = apply(&image, &no_metric, &on, None, None, Some(0.2)).unwrap();
        assert_eq!(image.px, out.px);
    }
}
