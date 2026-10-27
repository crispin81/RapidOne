//! What the GPU engine reads, as data from the same Rust source the CPU engine uses: the profile's
//! tuning (`Tuning`, `ToolTuning`), the tools' measured constants, and what `render_core_q` works out
//! once per render (its matrices, curves, the Mixer, Grade and Vignette settings, Adobe's tables).
//!
//! Numbers go in `Params` by name, in a fixed order; the shader is built with each name's place
//! (`Params::header`), so it never carries a copy of a number. Larger tables (curves, Adobe's
//! HueSatMap and LookTables, the subject vignette's map) go in `lut`, their places in `Params`.

use super::Job;
use crate::pipeline::{mul3, Oklab, REC2020_TO_XYZ, XYZ_TO_SRGB};
use crate::whitebalance::invert3;

/// Numbers by name, in the order put.
#[derive(Default)]
pub struct Params {
    pub data: Vec<f32>,
    names: Vec<(&'static str, usize)>,
}

/// A value as the f32s it is in `Params` (arrays row by row).
pub trait Flat {
    fn flat(&self, out: &mut Vec<f32>);
}

impl Flat for f32 {
    fn flat(&self, out: &mut Vec<f32>) {
        out.push(*self);
    }
}

impl Flat for bool {
    fn flat(&self, out: &mut Vec<f32>) {
        out.push(if *self { 1.0 } else { 0.0 });
    }
}

impl Flat for usize {
    fn flat(&self, out: &mut Vec<f32>) {
        out.push(*self as f32);
    }
}

impl<T: Flat, const N: usize> Flat for [T; N] {
    fn flat(&self, out: &mut Vec<f32>) {
        for v in self {
            v.flat(out);
        }
    }
}

impl Params {
    pub fn put(&mut self, name: &'static str, v: impl Flat) {
        self.names.push((name, self.data.len()));
        v.flat(&mut self.data);
    }

    /// The shader's text for each name's place.
    pub fn header(&self) -> String {
        let mut s = String::new();
        for (name, at) in &self.names {
            s += &format!("const {name}: u32 = {at}u;\n");
        }
        s += &format!("const R_GRADE_TINT_SIZE: u32 = {TINT_SIZE}u;\n");
        s
    }
}

/// Numbers per Grade tint in `R_GRADE_TINTS`: present, direction (2), light, k_pre, sat_shape,
/// pushed (3), pure (3), mean.
const TINT_SIZE: usize = 13;
/// Adobe look tables there is room for (a profile's own and a look's).
pub(crate) const MAX_LOOKS: usize = 4;

include!("tuning_fields.rs");

/// A table's place in `lut`.
fn add(lut: &mut Vec<f32>, v: &[f32]) -> usize {
    let at = lut.len();
    lut.extend_from_slice(v);
    at
}

/// An Adobe HSV table: its place in `lut`, divisions and encoding.
fn table(lut: &mut Vec<f32>, t: &crate::dcp::HsvTable) -> [f32; 5] {
    let flat: Vec<f32> = t.data.iter().flatten().copied().collect();
    let at = add(lut, &flat);
    [at as f32, t.hue_divs as f32, t.sat_divs as f32, t.val_divs as f32, if t.srgb_gamma { 1.0 } else { 0.0 }]
}

/// Everything a render needs, and its tables.
pub fn build(job: &Job) -> (Params, Vec<f32>) {
    let mut w = Params::default();
    let mut lut = Vec::new();
    let t = job.tuning;
    let s = job.settings;

    // The engine's constants.
    let ok = Oklab::new();
    w.put("K_OK_TO_LMS", ok.to_lms);
    w.put("K_OK_FROM_LMS", ok.from_lms);
    w.put("K_OK_LMS_TO_LAB", ok.lms_to_lab);
    w.put("K_OK_LAB_TO_LMS", ok.lab_to_lms);
    w.put("K_LUMA", crate::pipeline::LUMA);
    let to_pp = crate::tools::rec2020_to_prophoto();
    w.put("K_PP_TO", to_pp);
    w.put("K_PP_FROM", invert3(&to_pp));
    let to_srgb = mul3(&XYZ_TO_SRGB, &REC2020_TO_XYZ);
    w.put("K_TO_SRGB", to_srgb);
    w.put("K_FROM_SRGB", invert3(&to_srgb));
    w.put("K_BAND_CENTRES", crate::tools::BAND_CENTRES);
    w.put("K_SURF_S", crate::tools::SURF_S);
    w.put("K_SURF_V", crate::tools::SURF_V);
    w.put("K_VIG_KNOTS", crate::tools::VIG_KNOTS);
    w.put("K_VIG_EASE0", crate::tools::VIG_EASE0);
    w.put("K_VIG_EASE1", crate::tools::VIG_EASE1);
    w.put("K_VIG_ROUND_POS", crate::tools::VIG_ROUND_POS);
    w.put("K_VIG_ROUND_P", crate::tools::VIG_ROUND_P);
    w.put("K_VIG_ROUND_SCALE", crate::tools::VIG_ROUND_SCALE);
    w.put("K_VIG_K", crate::tools::VIG_K);
    w.put("K_VIG_K_PLUS", crate::tools::VIG_K_PLUS);
    w.put("K_Y_ROW", REC2020_TO_XYZ[1]);
    w.put("K_TONE_CORR", crate::clarity_unet::TONE_CORR);
    w.put("K_AB_UNITS", crate::tools::AB_UNITS);
    let solve = crate::pipeline::MIX_SOLVE_CONSTANTS;
    w.put("K_MIX_SOLVE_ROUNDS", solve.rounds);
    w.put("K_MIX_SOLVE_TOL", solve.tol);
    w.put("K_MIX_SOLVE_STEP", solve.step);
    w.put("K_MIX_SOLVE_BOX", solve.bound.ln());
    w.put("K_MIX_SOLVE_GAIN", solve.gain);
    w.put("K_MIX_SOLVE_STALL", solve.stall);
    w.put("K_MIX_SOLVE_FAR", solve.far);
    w.put("K_MIX_SOLVE_CLEAR", solve.clear);

    // The profile's tuning, every field.
    put_tuning(&mut w, t);

    // The sliders (the tone ones as `render_core_q`'s `bands` holds them).
    let b = job.bands;
    w.put("S_VIBRANCE", s.vibrance);
    w.put("S_SATURATION", s.saturation);
    w.put("S_CONTRAST", b.contrast);
    w.put("S_HIGHLIGHTS", b.highlights);
    w.put("S_SHADOWS", b.shadows);
    w.put("S_WHITES", b.whites);
    w.put("S_BLACKS", b.blacks);
    w.put("S_SHADOW_TINT", s.tools.cal_shadow_tint * s.shadow_tint_scale.unwrap_or(1.0));

    // What render_core_q works out once.
    w.put("R_DEPTH16", job.sixteen);
    w.put("R_LIGHT", job.light);
    w.put("R_GAIN", job.gain);
    w.put("R_WB_GAIN", job.wb_gain);
    w.put("R_HAS_CAMERA", job.camera.is_some());
    let none = [[0.0f32; 3]; 3];
    w.put("R_TO_CAM", job.camera.map_or(none, |c| c.0));
    w.put("R_FROM_CAM", job.camera.map_or(none, |c| c.1));
    w.put("R_CURVE_IDENTITY", job.curve.identity);
    w.put("R_CURVE_PRE_GAIN", job.curve.pre_gain);
    w.put("R_CURVE_K", job.curve.k);
    w.put("R_LOOK_HSV_DARK", t.look_hsv_dark != [[0.0; 3]; 12]);
    w.put("R_LOOK_WARM", t.look_warm != [[0.0; 2]; 5]);
    w.put("R_ADJ_HAS_LOOK", job.has_look);
    w.put("R_ADJ_KC_FULL", job.kc_full);
    w.put("R_ADJ_VIBRANCE_V", job.vibrance_v);
    w.put("R_ADOBE_FIX", t.adobe_fix_l != [0.0; 11] || t.adobe_fix_c != [0.0; 11]);
    w.put("R_MIX_SCENE", job.mix_scene);
    w.put("R_MIXER_MOVES", job.mixer_moves);
    let f = job.frame;
    w.put("R_FRAME", [f.is_some() as u8 as f32, f.map_or(0.0, |f| f.x0), f.map_or(0.0, |f| f.y0), f.map_or(1.0, |f| f.full_w), f.map_or(1.0, |f| f.full_h)]);
    // The tone map: each pixel's offset uploaded (worked out on the CPU), or its grid, the shader
    // working out the offsets (ToneGrid::offset_at).
    let tm = job.tone_map;
    w.put("R_TONE_MAP", tm.is_some_and(|m| !m.offset.is_empty()));
    let grid = tm.filter(|m| m.offset.is_empty()).map(|m| &m.grid);
    let places = grid.map(|g| (add(&mut lut, &g.g), add(&mut lut, &g.ma), add(&mut lut, &g.mb)));
    w.put("R_TM_GRID", [
        grid.is_some() as u8 as f32,
        grid.map_or(1.0, |g| g.gw as f32),
        grid.map_or(1.0, |g| g.gh as f32),
        grid.map_or(1.0, |g| g.scale),
        grid.map_or(1.0, |g| g.w as f32),
        places.map_or(0.0, |p| p.0 as f32),
        places.map_or(0.0, |p| p.1 as f32),
        places.map_or(0.0, |p| p.2 as f32),
    ]);
    w.put("R_KEY", job.tone_map.map_or(t.tone_key_ref, |m| m.key));
    w.put("R_SPAN", job.tone_map.map_or(t.tone_span_ref, |m| m.span));
    w.put("R_DISPLAY_CURVE", t.display_curve != [0.0; 4]);
    w.put("R_DISPLAY_BLUE_CURVE", t.display_blue_curve != [0.0; 2]);

    // Adobe's profile (dngcolour::ProfileRender).
    let pr = job.adobe;
    w.put("R_ADOBE", pr.is_some());
    w.put("R_PR_TO_PP", pr.map_or(none, |p| p.to_prophoto));
    let hsm = pr.and_then(|p| p.hue_sat_map.as_ref()).map(|t| table(&mut lut, t));
    w.put("R_PR_HSM", [hsm.is_some() as u8 as f32, hsm.map_or(0.0, |h| h[0]), hsm.map_or(0.0, |h| h[1]), hsm.map_or(0.0, |h| h[2]), hsm.map_or(0.0, |h| h[3]), hsm.map_or(0.0, |h| h[4])]);
    let mut looks = [0.0f32; 1 + 5 * MAX_LOOKS];
    if let Some(p) = pr {
        looks[0] = p.look_tables.len().min(MAX_LOOKS) as f32;
        for (i, lt) in p.look_tables.iter().take(MAX_LOOKS).enumerate() {
            looks[1 + i * 5..6 + i * 5].copy_from_slice(&table(&mut lut, lt));
        }
    }
    w.put("R_PR_LOOKS", looks);
    let tone_at = pr.map_or(0, |p| add(&mut lut, &p.tone.lut));
    w.put("R_PR_TONE", tone_at);
    let look_curve = pr.and_then(|p| p.look_curve.as_ref()).map(|c| add(&mut lut, &c.lut));
    w.put("R_PR_LOOK_CURVE", [look_curve.is_some() as u8 as f32, look_curve.unwrap_or(0) as f32]);

    // The display tools (tools::Prepared).
    let d = job.display_tools;
    w.put("R_DISPLAY_TOOLS", !d.is_identity());
    w.put("R_HAS_MIX", d.mixer.is_some());
    w.put("R_MIX", d.mixer.unwrap_or([[0.0; 3]; 8]));
    let mut curves = [0.0f32; 11];
    if let Some(cs) = &d.curves {
        curves[0] = 1.0;
        for (k, c) in cs.iter().enumerate() {
            if let Some(c) = c {
                curves[1 + k * 2] = 1.0;
                curves[2 + k * 2] = add(&mut lut, &c.lut) as f32;
            }
        }
    }
    w.put("R_CURVES", curves);
    let param = d.parametric.as_ref().map(|c| add(&mut lut, &c.lut));
    w.put("R_PARAMETRIC", [param.is_some() as u8 as f32, param.unwrap_or(0) as f32]);
    let enc = crate::tools::Encoding::new(t.tools.curve_gamma, t.tools.curve_toe);
    w.put("R_ENC", [enc.g, enc.a, enc.x0, enc.slope]);
    let grade = d.grade.as_ref().and(d.grade_full.as_ref());
    w.put("R_GRADE", grade.is_some());
    w.put("R_GRADE_BEND", grade.map_or(1.0, |g| g.bend));
    w.put("R_GRADE_E", grade.map_or(1.0, |g| g.e));
    w.put("R_GRADE_REF_E", grade.map_or([0.0; 3], |g| g.ref_e));
    w.put("R_GRADE_PEAKS", grade.map_or([1.0; 4], |g| g.peaks));
    w.put("R_GRADE_LUM", grade.map_or([0.0; 4], |g| [g.g.shadows.lum, g.g.mids.lum, g.g.highs.lum, g.g.global.lum]));
    let mut tints = [0.0f32; 4 * TINT_SIZE];
    if let Some(g) = grade {
        for (ri, tp) in g.tints.iter().enumerate() {
            if let Some(tp) = tp {
                let o = ri * TINT_SIZE;
                tints[o] = 1.0;
                tints[o + 1..o + 3].copy_from_slice(&tp.dir);
                tints[o + 3] = tp.light;
                tints[o + 4] = tp.k_pre;
                tints[o + 5] = tp.sat_shape;
                tints[o + 6..o + 9].copy_from_slice(&tp.pushed);
                tints[o + 9..o + 12].copy_from_slice(&tp.pure);
                tints[o + 12] = tp.mean;
            }
        }
    }
    w.put("R_GRADE_TINTS", tints);
    w.put("R_VIGNETTE", d.vignette);
    w.put("R_ADAPTIVE", d.adaptive);
    w.put("R_VIG_CENTRE", d.vig_centre);
    w.put("R_VIG_HALF", d.vig_half);
    w.put("R_VIG_GAMMA", d.vig_gamma);
    w.put("R_VIG_ROUND", d.vig_round);
    w.put("R_VIG_HIGHLIGHTS", d.vig_highlights);
    w.put("R_ASPECT", d.aspect);
    let map = d.vig_map.as_ref().map(|m| (m.w, m.h, add(&mut lut, &m.data)));
    w.put("R_VIG_MAP", [map.map_or(1.0, |m| m.0 as f32), map.map_or(1.0, |m| m.1 as f32), map.map_or(0.0, |m| m.2 as f32), map.is_some() as u8 as f32]);

    // Masks and gradients (gradient::Prepared): each mask's record in `lut`.
    let grads = job.grads.map(|g| gradients(&mut lut, g));
    w.put("R_GRADS", [grads.is_some() as u8 as f32, grads.map_or(0.0, |g| g.0 as f32), grads.map_or(0.0, |g| g.1 as f32), job.grads.and_then(|g| g.masks.first()).map_or(1.0, |m| m.aspect)]);
    w.put("K_RADIAL_FADE", crate::gradient::RADIAL_FADE);
    w.put("K_LOCAL_WB", crate::gradient::LOCAL_WB);
    w.put("K_LOCAL_CONTRAST", crate::gradient::LOCAL_CONTRAST);
    w.put("K_LOCAL_HIGHLIGHTS", crate::gradient::LOCAL_HIGHLIGHTS);
    w.put("K_LOCAL_SHADOWS", crate::gradient::LOCAL_SHADOWS);
    w.put("K_LOCAL_WHITES", crate::gradient::LOCAL_WHITES);
    w.put("K_LOCAL_BLACKS", crate::gradient::LOCAL_BLACKS);
    w.put("K_LOCAL_SATURATION", crate::gradient::LOCAL_SATURATION);

    if lut.is_empty() {
        lut.push(0.0);
    }
    (w, lut)
}

/// Numbers a mask shape takes in its record: kind, subtract, its own invert, full (2), zero (2),
/// centre (2), rx, ry, angle, feather, and a grid's width, height and place.
const COMP_SIZE: usize = 16;

/// The masks as records in `lut`: hidden, how many shapes, invert, the 12 sliders, then the shapes.
/// Returns how many masks and where the first starts.
fn gradients(lut: &mut Vec<f32>, g: &crate::gradient::Prepared) -> (usize, usize) {
    use crate::gradient::{Comp, Kind};
    // The grids first (subject masks, brush strokes), then the records.
    let mut grid_at = Vec::new();
    for m in &g.masks {
        for c in std::iter::once(&m.base).chain(m.parts.iter().map(|p| &p.2)) {
            grid_at.push(match c {
                Comp::Map(grid) => Some((grid.w, grid.h, add(lut, &grid.data))),
                _ => None,
            });
        }
    }
    let start = lut.len();
    let mut grids = grid_at.into_iter();
    for (gr, m) in g.list.iter().zip(&g.masks) {
        let sliders = [gr.exposure, gr.temperature, gr.tint, gr.contrast, gr.highlights, gr.shadows, gr.whites, gr.blacks, gr.saturation, gr.texture, gr.clarity, gr.dehaze];
        lut.extend_from_slice(&[gr.hidden as u8 as f32, (1 + m.parts.len()) as f32, m.invert as u8 as f32]);
        lut.extend_from_slice(&sliders);
        let base = (false, false, &m.base);
        for (subtract, invert, c) in std::iter::once(base).chain(m.parts.iter().map(|p| (p.0, p.1, &p.2))) {
            let mut rec = [0.0f32; COMP_SIZE];
            rec[1] = subtract as u8 as f32;
            rec[2] = invert as u8 as f32;
            match c {
                Comp::Formula(f) => {
                    rec[0] = match f.kind {
                        Kind::Linear => 1.0,
                        Kind::Radial => 2.0,
                        _ => 0.0,
                    };
                    rec[3..13].copy_from_slice(&[f.full[0], f.full[1], f.zero[0], f.zero[1], f.centre[0], f.centre[1], f.rx, f.ry, f.angle, f.feather]);
                }
                Comp::Map(_) => rec[0] = 3.0,
                Comp::Nothing => {}
            }
            if let Some((gw, gh, at)) = grids.next().flatten() {
                rec[13..16].copy_from_slice(&[gw as f32, gh as f32, at as f32]);
            }
            lut.extend_from_slice(&rec);
        }
    }
    (g.list.len(), start)
}
