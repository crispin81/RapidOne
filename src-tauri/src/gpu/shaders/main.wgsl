// ---- pipeline::render_core_q, per pixel ----------------------------------------------------------

// The colour as shown without the tone sliders, which the Mixer's solve inverts (render_core_q's
// `shown_of`): Shinobi's look and Vibrance on the profile curve, or Adobe's profile finish.
fn shown_of(x: vec3f) -> vec3f {
    if on(R_ADOBE) {
        return colour_adjust(adobe_fix(ap3(M(K_PP_FROM), pr_finish(ap3(M(K_PP_TO), x)))), false);
    }
    return colour_adjust(base_curve(x), on(R_ADJ_HAS_LOOK));
}

// pipeline::oklab_jacobian
fn oklab_jacobian(rgb: vec3f) -> M3 {
    let lms = ap3(M(K_OK_TO_LMS), max(rgb, vec3f(0.0)));
    let c = cbrt3(max(lms, vec3f(1e-6)));
    let d = 1.0 / (3.0 * c * c);
    let t = M(K_OK_TO_LMS);
    let mid = M3(t.r0 * d.x, t.r1 * d.y, t.r2 * d.z);
    return m3_mul(M(K_OK_LMS_TO_LAB), mid);
}

fn norm2(v: vec3f) -> f32 { return v.x * v.x + v.y * v.y + v.z * v.z; }

// pipeline::mixer_solve_with: the scene colour that shows as `want`, starting from `lit`.
fn mixer_solve(lit: vec3f, shown_lit: vec3f, want: vec3f) -> vec3f {
    let start = max(lit, vec3f(1e-4));
    let goal = to_lab(max(want, vec3f(0.0)));
    var now = shown_lit;
    if any(start != lit) { now = shown_of(start); }
    var r = to_lab(max(now, vec3f(0.0))) - goal;
    let tol = P(K_MIX_SOLVE_TOL);
    if max3(abs(r)) < tol { return start; }
    let u0 = log(start);
    var u = u0;
    var jac = oklab_jacobian(now);
    let nw = max(now, vec3f(0.0));
    jac = M3(jac.r0 * nw, jac.r1 * nw, jac.r2 * nw);
    var mu = -1.0;
    var nu = 2.0;
    var shown = now;
    var ratio_left = true;
    var use_ratio = false;
    let rounds = u32(P(K_MIX_SOLVE_ROUNDS));
    let step_max = P(K_MIX_SOLVE_STEP);
    let bound = P(K_MIX_SOLVE_BOX);
    for (var n = 0u; n < rounds; n++) {
        let jt = m3_t(jac);
        var a = m3_mul(jt, jac);
        if mu < 0.0 { mu = 1e-3 * max(a.r0.x, max(a.r1.y, a.r2.z)); }
        a.r0.x += mu;
        a.r1.y += mu;
        a.r2.z += mu;
        let g = ap3(jt, r);
        let inv = invert3(a);
        let ratio = use_ratio;
        use_ratio = false;
        var s0: vec3f;
        if ratio {
            for (var k = 0u; k < 3u; k++) {
                if shown[k] > 1e-6 { s0[k] = log(clamp(max(want[k], 0.0) / shown[k], 0.25, 4.0)); } else { s0[k] = 0.0; }
            }
        } else {
            s0 = clamp(-ap3(inv, g), vec3f(-step_max), vec3f(step_max));
        }
        let stp = clamp(u + s0, u0 - bound, u0 + bound) - u;
        let ss = norm2(stp);
        if !(ss >= 1e-12) { break; }
        let u2 = u + stp;
        let shown2 = shown_of(exp(u2));
        let r2 = to_lab(max(shown2, vec3f(0.0))) - goal;
        let pred = ap3(jac, stp);
        let before = norm2(r);
        let promised = before - norm2(r + pred);
        let gained = before - norm2(r2);
        let corr = (r2 - r - pred) / ss;
        jac = M3(jac.r0 + corr.x * stp, jac.r1 + corr.y * stp, jac.r2 + corr.z * stp);
        var rho = -1.0;
        if promised > 0.0 { rho = gained / promised; }
        var kept = rho > P(K_MIX_SOLVE_GAIN);
        if ratio { kept = gained > P(K_MIX_SOLVE_CLEAR) * before; }
        if kept {
            u = u2;
            r = r2;
            shown = shown2;
            if !ratio {
                let q = 2.0 * rho - 1.0;
                mu *= max(1.0 - q * q * q, 1.0 / 3.0);
                nu = 2.0;
            }
            if max3(abs(r)) < tol { break; }
        } else if !ratio {
            mu *= nu;
            nu *= 2.0;
        }
        if ratio && !kept { ratio_left = false; }
        if ratio_left && (!kept || gained < P(K_MIX_SOLVE_STALL) * before) && max3(abs(r)) > P(K_MIX_SOLVE_FAR) {
            use_ratio = true;
        }
    }
    return exp(u);
}

// render_core_q's `mixed_scene`
fn mixed_scene(lit: vec3f) -> vec3f {
    if !on(R_MIXER_MOVES) && all(lit >= vec3f(1e-4)) { return lit; }
    let shown_lit = shown_of(lit);
    let want = apply_mixer_part(shown_lit, vec3f(1.0, 1.0, 0.0));
    return mixer_solve(lit, shown_lit, want);
}

fn settings_tone() -> Tone {
    return Tone(P(S_CONTRAST), P(S_HIGHLIGHTS), P(S_SHADOWS), P(S_WHITES), P(S_BLACKS));
}

// Pixel i (of the whole picture) as fractions of the whole photo (render_core_q's `frac`).
fn frac(i: u32) -> vec2f {
    let w = job.width;
    let x = f32(i % w);
    let y = f32(i / w);
    if on(R_FRAME) {
        let f = P(R_FRAME + 1u);
        return vec2f((x + 0.5 + f) / P(R_FRAME + 3u), (y + 0.5 + P(R_FRAME + 2u)) / P(R_FRAME + 4u));
    }
    return vec2f((x + 0.5) / f32(w), (y + 0.5) / f32(job.height));
}

// render_core_q's `tone_with` with the render's tone sliders (`bands`), `offset` from the tone map.
fn tone_base(p: vec3f, offset: f32) -> vec3f {
    let s = settings_tone();
    let key = P(R_KEY);
    let span = P(R_SPAN);
    let st = P(S_SHADOW_TINT);
    if on(R_ADOBE) {
        let gain = P(R_GAIN);
        let pre = max(ap3(M(R_LIGHT), p), vec3f(0.0)) / gain;
        var lit = ap3(M(K_PP_FROM), pr_working(pre) * gain);
        lit = shadow_tint(lit, st);
        if on(R_MIX_SCENE) { lit = mixed_scene(lit); }
        let shaped = contrast_and_tone(lit, s, offset, key, span);
        return adobe_fix(ap3(M(K_PP_FROM), pr_finish(ap3(M(K_PP_TO), shaped))));
    }
    var lit = max(ap3(M(R_LIGHT), p), vec3f(0.0));
    lit = shadow_tint(lit, st);
    if on(R_MIX_SCENE) { lit = mixed_scene(lit); }
    let shaped = contrast_and_tone(lit, s, offset, key, span);
    return base_curve(shaped);
}

// render_core_q's `tone`: everything up to the rendered colour (the measured tone table is the
// CPU's alone: `gpu::supported`).
fn tone(i: u32, p0: vec3f, offset: f32) -> vec3f {
    var p = p0;
    if on(R_GRADS) {
        let fr = frac(i);
        p = local_scene(local_at(fr.x, fr.y), p0);
    }
    let shown = tone_base(p, offset);
    return colour_adjust(shown, !on(R_ADOBE) && on(R_ADJ_HAS_LOOK));
}

// render_core_q's `finish`: the display tools, then display-encoded sRGB (0..1).
fn finish(i: u32, toned0: vec3f) -> vec3f {
    var toned = toned0;
    if on(R_GRADS) {
        let fr = frac(i);
        toned = local_display(local_at(fr.x, fr.y), toned0);
    }
    if on(R_DISPLAY_TOOLS) {
        let fr = frac(i);
        let u = fr.x * 2.0 - 1.0;
        let v = fr.y * 2.0 - 1.0;
        if on(R_MIX_SCENE) {
            let lum = apply_mixer_part(toned, vec3f(0.0, 0.0, 1.0));
            toned = apply_after_mixer(lum, u, v);
        } else {
            toned = apply_after_mixer(apply_mixer_part(toned, vec3f(1.0)), u, v);
        }
    }
    var out = srgb_encode3(ap3(M(K_TO_SRGB), toned));
    if on(R_DISPLAY_CURVE) {
        out = vec3f(display_curve(out.x, T_DISPLAY_CURVE), display_curve(out.y, T_DISPLAY_CURVE), display_curve(out.z, T_DISPLAY_CURVE));
    }
    if on(R_DISPLAY_BLUE_CURVE) {
        out.z = monotone_curve3(out.z, P(T_DISPLAY_BLUE_CURVE), P(T_DISPLAY_BLUE_CURVE + 1u));
    }
    let blue_s = P(T_BLUE_S);
    if blue_s != 0.0 {
        let x = clamp(out.z, 0.0, 1.0);
        let which = i32(P(T_BLUE_S_HALF));
        var on_ = true;
        if which == 1 { on_ = x < 0.5; } else if which == 2 { on_ = x > 0.5; }
        let hi = max3(out);
        let lo = min3(out);
        var w = 1.0;
        if which >= 3 {
            if out.z <= lo && hi > 1e-4 { w = (hi - lo) / hi; } else { w = 0.0; }
        }
        if which == 4 && out.x > 1e-4 { w = w * clamp((out.y / out.x - 0.85) / 0.15, 0.0, 1.0); }
        if on_ && w > 0.0 {
            out.z = x - w * blue_s * sin(TAU * x) / TAU;
        }
    }
    return out;
}

// The output word(s) for one pixel: 8-bit RGBA in one word, or 16-bit in two (render_core's
// `(v * 255 + 0.5) as u8`, render16's `(v * 65535 + 0.5) as u16`).
fn store(k: u32, c: vec3f) {
    if on(R_DEPTH16) {
        let q = vec3u(clamp(c * 65535.0 + 0.5, vec3f(0.0), vec3f(65535.0)));
        outp[k * 2u] = q.x | (q.y << 16u);
        outp[k * 2u + 1u] = q.z;
    } else {
        let q = vec3u(clamp(c * 255.0 + 0.5, vec3f(0.0), vec3f(255.0)));
        outp[k] = q.x | (q.y << 8u) | (q.z << 16u);
    }
}

fn load(k: u32) -> vec3f { return vec3f(src[k * 3u], src[k * 3u + 1u], src[k * 3u + 2u]); }

// pipeline::ToneGrid::sample: one of the grid's maps (at `o` in `lut`) at grid place (x, y).
fn tm_sample(o: u32, x: f32, y: f32) -> f32 {
    let gw = u32(P(R_TM_GRID + 1u));
    let gh = u32(P(R_TM_GRID + 2u));
    let fx = clamp(x - 0.5, 0.0, f32(gw - 1u));
    let fy = clamp(y - 0.5, 0.0, f32(gh - 1u));
    let x0 = u32(fx);
    let y0 = u32(fy);
    let x1 = min(x0 + 1u, gw - 1u);
    let y1 = min(y0 + 1u, gh - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let top = lut[o + y0 * gw + x0] * (1.0 - tx) + lut[o + y0 * gw + x1] * tx;
    let bot = lut[o + y1 * gw + x0] * (1.0 - tx) + lut[o + y1 * gw + x1] * tx;
    return top * (1.0 - ty) + bot * ty;
}

// The tone map's offset for pixel k of the strip (pixel i of the picture, scene light p): uploaded,
// or pipeline::ToneGrid::offset_at from the grid (the whole picture's: `ToneGrid::offsets`).
fn offset_at(k: u32, i: u32, p: vec3f) -> f32 {
    if on(R_TONE_MAP) { return offs[k]; }
    if !on(R_TM_GRID) { return 0.0; }
    let lit = luma(ap3(M(R_LIGHT), p));
    let e = log2(max(lit, 1e-5) / 0.18);
    let scale = P(R_TM_GRID + 3u);
    let w = u32(P(R_TM_GRID + 4u));
    let x = f32(i % w) / scale;
    let y = f32(i / w) / scale;
    var guide = e;
    let tt = P(T_TONE_TEXTURE);
    if tt > 0.0 {
        let gs = tm_sample(u32(P(R_TM_GRID + 5u)), x, y);
        guide = e - clamp(e - gs, -tt, tt);
    }
    return tm_sample(u32(P(R_TM_GRID + 6u)), x, y) * guide + tm_sample(u32(P(R_TM_GRID + 7u)), x, y) - e;
}

// Workgroups are laid out two-dimensionally (a strip can need more than 65535 of them in a row).
fn pixel(gid: vec3u, nwg: vec3u) -> u32 { return gid.x + gid.y * nwg.x * 64u; }

// Scene light to the finished picture.
@compute @workgroup_size(64)
fn render_full(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let k = pixel(gid, nwg);
    if k >= job.count { return; }
    let i = job.base + k;
    let p = load(k);
    store(k, finish(i, tone(i, p, offset_at(k, i, p))));
}

// Scene light to the toned colour (for Texture, Clarity and Dehaze on the CPU between).
@compute @workgroup_size(64)
fn render_tone(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let k = pixel(gid, nwg);
    if k >= job.count { return; }
    let i = job.base + k;
    let p = load(k);
    let c = tone(i, p, offset_at(k, i, p));
    outp[k * 3u] = bitcast<u32>(c.x);
    outp[k * 3u + 1u] = bitcast<u32>(c.y);
    outp[k * 3u + 2u] = bitcast<u32>(c.z);
}

// The masks' Texture, Clarity and Dehaze amounts at each pixel, three floats a pixel (render_core_q's
// `amounts`, for gpu/definition.rs).
@compute @workgroup_size(64)
fn render_amounts(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let k = pixel(gid, nwg);
    if k >= job.count { return; }
    let fr = frac(job.base + k);
    let a = local_definition(fr.x, fr.y);
    outp[k * 3u] = bitcast<u32>(a.x);
    outp[k * 3u + 1u] = bitcast<u32>(a.y);
    outp[k * 3u + 2u] = bitcast<u32>(a.z);
}

// The toned colour to the finished picture.
@compute @workgroup_size(64)
fn render_finish(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let k = pixel(gid, nwg);
    if k >= job.count { return; }
    store(k, finish(job.base + k, load(k)));
}
