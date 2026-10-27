// ---- tools.rs: Fine tuning's shadow tint, Mixer, Curves, Grade, Vignette ---------------------------

// tools::ramp (its own: the span held at 1e-3 at least)
fn tools_ramp(x: f32, lo: f32, hi: f32) -> f32 {
    let t = clamp((x - lo) / max(hi - lo, 1e-3), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// tools::shadow_tint (`amount` already scaled by the camera's strength)
fn shadow_tint(c: vec3f, amount: f32) -> vec3f {
    if amount == 0.0 { return c; }
    let l = luma(c);
    let ev = log2(max(l, 1e-6) / 0.18);
    let w = 1.0 - tools_ramp(ev, P(TT_CAL_TINT_FROM), P(TT_CAL_TINT_TO));
    let a = amount / 100.0 * P(TT_CAL_TINT) * w * powf(max(P(R_WB_GAIN) / 3.0, 0.1), P(TT_CAL_TINT_WB));
    let gain = vec3f(exp2(a * P(TT_CAL_TINT_RED)), exp2(-a), exp2(a * P(TT_CAL_TINT_BLUE)));
    var out: vec3f;
    if on(R_HAS_CAMERA) && on(TT_CAL_TINT_CAMERA) {
        out = max(ap3(M(R_FROM_CAM), ap3(M(R_TO_CAM), c) * gain), vec3f(0.0));
    } else {
        out = c * gain;
    }
    let l2 = luma(out);
    if l2 <= 1e-9 || P(TT_CAL_TINT_DARK) < 0.0 { return out; }
    let k = l / l2 * exp2(-abs(a) * P(TT_CAL_TINT_DARK));
    return out * k;
}

// tools::rgb_hue (degrees; -1 for no hue)
fn rgb_hue(c: vec3f) -> f32 {
    let mx = max3(c);
    let mn = min3(c);
    let d = mx - mn;
    if d <= 1e-6 { return -1.0; }
    var h: f32;
    if mx == c.x { h = rem_euclid((c.y - c.z) / d, 6.0); } else if mx == c.y { h = (c.z - c.x) / d + 2.0; } else { h = (c.x - c.y) / d + 4.0; }
    return h * 60.0;
}

// tools::rgb_to_hsv
fn rgb_to_hsv(c: vec3f) -> vec3f {
    let mx = max3(c);
    let mn = min3(c);
    var s = 0.0;
    if mx > 0.0 { s = (mx - mn) / mx; }
    return vec3f(max(rgb_hue(c), 0.0), s, mx);
}

// tools::hsv_to_rgb
fn hsv_to_rgb(c: vec3f) -> vec3f {
    let h = rem_euclid(c.x, 360.0) / 60.0;
    let s = c.y;
    let v = c.z;
    let f = h - floor(h);
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    switch i32(floor(h)) {
        case 0: { return vec3f(v, t, p); }
        case 1: { return vec3f(q, v, p); }
        case 2: { return vec3f(p, v, t); }
        case 3: { return vec3f(p, q, v); }
        case 4: { return vec3f(t, p, v); }
        default: { return vec3f(v, p, q); }
    }
}

// tools::rgb_to_hsl
fn rgb_to_hsl(c: vec3f) -> vec3f {
    let mx = max3(c);
    let mn = min3(c);
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    var s = 0.0;
    if d > 1e-6 { s = d / max(1.0 - abs(2.0 * l - 1.0), 1e-6); }
    return vec3f(max(rgb_hue(c), 0.0), min(s, 1.0), l);
}

// tools::hsl_to_rgb
fn hsl_to_rgb(c0: vec3f) -> vec3f {
    let h = c0.x;
    let s = c0.y;
    let l = c0.z;
    let c = (1.0 - abs(2.0 * l - 1.0)) * s;
    let hp = rem_euclid(h, 360.0) / 60.0;
    let x = c * (1.0 - abs(hp % 2.0 - 1.0));
    var rgb: vec3f;
    switch i32(floor(hp)) {
        case 0: { rgb = vec3f(c, x, 0.0); }
        case 1: { rgb = vec3f(x, c, 0.0); }
        case 2: { rgb = vec3f(0.0, c, x); }
        case 3: { rgb = vec3f(0.0, x, c); }
        case 4: { rgb = vec3f(x, 0.0, c); }
        default: { rgb = vec3f(c, 0.0, x); }
    }
    let m = l - c / 2.0;
    return rgb + m;
}

// tools::band_weights
fn band_weights(hue: f32, width: f32) -> array<f32, 8> {
    var w: array<f32, 8>;
    var sum = 0.0;
    for (var b = 0u; b < 8u; b++) {
        let c = P(K_BAND_CENTRES + b) + P(TT_MIX_CENTRE + b);
        let pb = (b + 7u) % 8u;
        let nb = (b + 1u) % 8u;
        let prev = P(K_BAND_CENTRES + pb) + P(TT_MIX_CENTRE + pb);
        let next = P(K_BAND_CENTRES + nb) + P(TT_MIX_CENTRE + nb);
        let d = rem_euclid(hue - c + 540.0, 360.0) - 180.0;
        var span: f32;
        if d >= 0.0 { span = rem_euclid(next - c + 360.0, 360.0); } else { span = rem_euclid(c - prev + 360.0, 360.0); }
        w[b] = max(1.0 - abs(d) / max(span * width, 1e-3), 0.0);
        sum += w[b];
    }
    if sum > 0.0 {
        for (var b = 0u; b < 8u; b++) { w[b] /= sum; }
    }
    return w;
}

// tools::lum_surface's `pos` on one axis (`levels`: n values at `o`)
fn surf_pos(o: u32, n: u32, x: f32) -> vec2f {
    let last = n - 1u;
    if x <= P(o) { return vec2f(0.0, 0.0); }
    if x >= P(o + last) { return vec2f(f32(last - 1u), 1.0); }
    var i = last - 1u;
    for (var k = 0u; k < last; k++) {
        if x < P(o + k + 1u) { i = k; break; }
    }
    return vec2f(f32(i), (x - P(o + i)) / (P(o + i + 1u) - P(o + i)));
}

// tools::lum_surface (`tab`: 4 rows of 8)
fn lum_surface(tab: u32, s: f32, v: f32) -> f32 {
    let sp = surf_pos(K_SURF_S, 8u, s);
    let vp = surf_pos(K_SURF_V, 4u, v);
    let si = u32(sp.x);
    let vi = u32(vp.x);
    let r0 = P(tab + vi * 8u + si) + (P(tab + vi * 8u + si + 1u) - P(tab + vi * 8u + si)) * sp.y;
    let r1 = P(tab + (vi + 1u) * 8u + si) + (P(tab + (vi + 1u) * 8u + si + 1u) - P(tab + (vi + 1u) * 8u + si)) * sp.y;
    return r0 + (r1 - r0) * vp.y;
}

// The mixer's settings for one band, with only the parts in `keep` (hue, saturation, luminance).
fn mix_band(b: u32, keep: vec3f) -> vec3f { return P3(R_MIX + b * 3u) * keep; }

// tools::mixer_prophoto
fn mixer_prophoto(c: vec3f, keep: vec3f) -> vec3f {
    let G = 1.8;
    let e = powf3(clamp(ap3(M(K_PP_TO), c), vec3f(0.0), vec3f(1.0)), 1.0 / G);
    let hsv = rgb_to_hsv(e);
    let h = hsv.x;
    let s = hsv.y;
    let v = hsv.z;
    if s <= 1e-4 { return c; }
    var w = band_weights(h, P(TT_MIX_WIDTH));
    var cs = s;
    var cv = v;
    if !on(TT_MIX_SURFACE_PP) {
        let e2 = srgb_encode3(clamp(ap3(M(K_TO_SRGB), c), vec3f(0.0), vec3f(1.0)));
        let h2 = rgb_to_hsv(e2);
        cs = h2.y;
        cv = h2.z;
    }
    var dh = 0.0;
    var ds = 0.0;
    var dl_up = 0.0;
    var dl_down = 0.0;
    for (var b = 0u; b < 8u; b++) {
        if w[b] == 0.0 { continue; }
        let m = mix_band(b, keep);
        let bh = m.x;
        let bs = m.y;
        let bl = m.z;
        var neg = 1.0;
        if bh < 0.0 { neg = P(TT_MIX_HUE_NEG + b); }
        dh += w[b] * bh / 100.0 * P(TT_MIX_HUE + b) * neg;
        if bs >= 0.0 { ds += w[b] * (powf(bs / 100.0, P(TT_MIX_SAT_CURVE)) * P(TT_MIX_SAT_PLUS)); } else { ds += w[b] * (bs / 100.0 * P(TT_MIX_SAT_MINUS)); }
        let l = w[b] * bl / 100.0 * P(TT_MIX_LUM_BAND + b);
        if bl >= 0.0 {
            dl_up += l * P(TT_MIX_LUM_PLUS) * by_value(TT_MIX_LUM_BAND_V_UP + b * 4u, cv);
        } else {
            dl_down += l * P(TT_MIX_LUM_MINUS) * by_value(TT_MIX_LUM_BAND_V_DOWN + b * 4u, cv);
        }
    }
    let amount = min(s / max(P(TT_MIX_CHROMA_FULL), 1e-3), 1.0);
    if ds != 0.0 {
        if ds > 0.0 { ds *= lum_surface(TT_MIX_SAT_SURFACE_UP, cs, cv); } else { ds *= lum_surface(TT_MIX_SAT_SURFACE_DOWN, cs, cv); }
        if ds < 0.0 {
            ds *= 1.0 + P(TT_MIX_SAT_STRONG) * s * s + P(TT_MIX_SAT_BRIGHT) * s * pow4(v);
        }
    }
    if dl_up > 0.0 && P(TT_MIX_LUM_PALE) != 0.0 {
        ds -= P(TT_MIX_LUM_PALE) * dl_up * s * pow4(v);
    }
    var turned = c;
    if dh != 0.0 {
        turned = max(ap3(M(K_PP_FROM), powf3(max(hsv_to_rgb(vec3f(h + dh * amount, s, v)), vec3f(0.0)), G)), vec3f(0.0));
    }
    if dl_up != 0.0 || dl_down != 0.0 {
        let e3 = powf3(clamp(ap3(M(K_PP_TO), turned), vec3f(0.0), vec3f(1.0)), 1.0 / G);
        let hsl = rgb_to_hsl(e3);
        let hh = hsl.x;
        let ss = hsl.y;
        let ll = hsl.z;
        let lin = ap3(M(K_PP_TO), c);
        let hi = max3(lin);
        let lo = max(min3(lin), 0.0);
        var hsv_sat = 0.0;
        if hi > 1e-6 { hsv_sat = 1.0 - powf(lo / hi, 1.0 / G); }
        let sat = hsv_sat + (ss - hsv_sat) * P(TT_MIX_LUM_HSL);
        let r = powf(min(sat / max(P(TT_MIX_LUM_SAT_FULL), 1e-3), 1.0), P(TT_MIX_LUM_SAT_POW));
        let up = P(TT_MIX_LUM_FLOOR) + (1.0 - P(TT_MIX_LUM_FLOOR)) * r;
        let down = P(TT_MIX_LUM_FLOOR_DOWN) + (1.0 - P(TT_MIX_LUM_FLOOR_DOWN)) * r;
        var fade = 1.0;
        if P(TT_MIX_LUM_DARK) > 0.0 { fade = tools_ramp(ll, 0.0, P(TT_MIX_LUM_DARK)); }
        let su = lum_surface(TT_MIX_LUM_SURFACE_UP, cs, cv);
        let sd = lum_surface(TT_MIX_LUM_SURFACE_DOWN, cs, cv);
        let stops = (dl_up * up * su + dl_down * down * sd) * fade;
        let gained = turned * exp2(stops);
        if P(TT_MIX_LUM_HSL) <= 0.0 {
            turned = gained;
        } else {
            let lit = hsl_to_rgb(vec3f(hh, ss, min(ll * exp2(stops / G), 1.0)));
            let hsl_c = max(ap3(M(K_PP_FROM), powf3(max(lit, vec3f(0.0)), G)), vec3f(0.0));
            let blend = clamp(P(TT_MIX_LUM_HSL) + (1.0 - P(TT_MIX_LUM_HSL)) * P(TT_MIX_LUM_HSL_STRONG) * s * s, 0.0, 1.0);
            turned = gained + (hsl_c - gained) * blend;
        }
    }
    if ds == 0.0 { return turned; }
    let lab = to_lab(turned);
    let k = max(1.0 + ds * amount, 0.0);
    let oklab = max(to_rgb(vec3f(lab.x, lab.y * k, lab.z * k)), vec3f(0.0));
    if P(TT_MIX_SAT_HSL) <= 0.0 { return oklab; }
    let lin = max(ap3(M(K_PP_TO), turned), vec3f(0.0));
    let top = max(max3(lin), 1.0);
    let hsl = rgb_to_hsl(powf3(lin / top, 1.0 / G));
    let s2 = sat_soft(hsl.y, k);
    var hsl_c = max(ap3(M(K_PP_FROM), powf3(max(hsl_to_rgb(vec3f(hsl.x, s2, hsl.z)), vec3f(0.0)), G) * top), vec3f(0.0));
    if k > 1.0 && P(TT_MIX_SAT_HSV) > 0.0 {
        let hsv3 = rgb_to_hsv(powf3(lin / top, 1.0 / P(TT_MIX_SAT_GAMMA)));
        let hsv_c = max(ap3(M(K_PP_FROM), powf3(max(hsv_to_rgb(vec3f(hsv3.x, sat_soft(hsv3.y, k), hsv3.z)), vec3f(0.0)), P(TT_MIX_SAT_GAMMA)) * top), vec3f(0.0));
        hsl_c = hsl_c + (hsv_c - hsl_c) * P(TT_MIX_SAT_HSV);
    }
    return oklab + (hsl_c - oklab) * P(TT_MIX_SAT_HSL);
}

// mixer_prophoto's `soft`
fn sat_soft(s: f32, k: f32) -> f32 {
    if k > 1.0 {
        let plain = min(s * k, 1.0);
        return plain + (1.0 - powf(1.0 - s, k) - plain) * P(TT_MIX_SAT_SOFT);
    }
    return s * k;
}

// mixer_prophoto's `by_value`
fn by_value(tab: u32, cv: f32) -> f32 {
    let x = clamp(cv, P(K_SURF_V), P(K_SURF_V + 3u));
    var i = 2u;
    for (var k = 0u; k < 3u; k++) {
        if x <= P(K_SURF_V + k + 1u) { i = k; break; }
    }
    return P(tab + i) + (P(tab + i + 1u) - P(tab + i)) * (x - P(K_SURF_V + i)) / (P(K_SURF_V + i + 1u) - P(K_SURF_V + i));
}

fn pow4(x: f32) -> f32 { let x2 = x * x; return x2 * x2; }
fn powf3(v: vec3f, y: f32) -> vec3f { return vec3f(powf(v.x, y), powf(v.y, y), powf(v.z, y)); }
fn srgb_encode3(v: vec3f) -> vec3f { return vec3f(srgb_encode(v.x), srgb_encode(v.y), srgb_encode(v.z)); }

// tools::display_hue
fn display_hue(c: vec3f) -> f32 {
    return rgb_hue(powf3(clamp(ap3(M(K_TO_SRGB), c), vec3f(0.0), vec3f(1.0)), 1.0 / 2.2));
}

// tools::mixer (the OKLab Mixer, `mix_prophoto` 0)
fn mixer_oklab(c: vec3f, keep: vec3f) -> vec3f {
    let hue = display_hue(c);
    if hue < 0.0 { return c; }
    var w = band_weights(hue, P(TT_MIX_WIDTH));
    var dh = 0.0;
    var ds = 0.0;
    var dl = 0.0;
    for (var b = 0u; b < 8u; b++) {
        if w[b] == 0.0 { continue; }
        let m = mix_band(b, keep);
        dh += w[b] * m.x / 100.0 * P(TT_MIX_HUE + b);
        if m.y >= 0.0 { ds += w[b] * m.y / 100.0 * P(TT_MIX_SAT_PLUS); } else { ds += w[b] * m.y / 100.0 * P(TT_MIX_SAT_MINUS); }
        if m.z >= 0.0 { dl += w[b] * m.z / 100.0 * P(TT_MIX_LUM_PLUS); } else { dl += w[b] * m.z / 100.0 * P(TT_MIX_LUM_MINUS); }
    }
    let lab = to_lab(c);
    let chroma = sqrt(lab.y * lab.y + lab.z * lab.z);
    let amount = min(chroma / max(P(TT_MIX_CHROMA_FULL), 1e-3), 1.0);
    let angle = atan2(lab.z, lab.y) + to_radians(dh * amount);
    let l = max(lab.x + dl * amount, 0.0);
    let lum_rel = dl * amount / max(lab.x, 0.05);
    let new_chroma = chroma * max(1.0 + ds * amount, 0.0) * max(1.0 - P(TT_MIX_LUM_CHROMA) * lum_rel, 0.0);
    let turned = max(to_rgb(vec3f(l, new_chroma * cos(angle), new_chroma * sin(angle))), vec3f(0.0));
    if dh == 0.0 || P(TT_MIX_HUE_VALUE) == 0.0 { return turned; }
    let hue_only = to_rgb(vec3f(lab.x, chroma * cos(angle), chroma * sin(angle)));
    let v_before = max3(c);
    let v_after = max3(hue_only);
    if v_before <= 1e-6 || v_after <= 1e-6 { return turned; }
    let k = powf(v_before / v_after, P(TT_MIX_HUE_VALUE));
    return turned * k;
}

// tools::Prepared::apply_mixer_part (`keep`: 1 for each of hue, saturation, luminance used)
fn apply_mixer_part(c: vec3f, keep: vec3f) -> vec3f {
    if !on(R_HAS_MIX) { return c; }
    var moved = false;
    for (var b = 0u; b < 8u; b++) {
        let m = mix_band(b, keep);
        if m.x != 0.0 || m.y != 0.0 || m.z != 0.0 { moved = true; }
    }
    if !moved { return c; }
    if on(TT_MIX_PROPHOTO) { return mixer_prophoto(c, keep); }
    return mixer_oklab(c, keep);
}

// tools::Encoding (the curves' encoding: R_ENC = g, a, x0, slope)
fn enc_encode(x: f32) -> f32 {
    if x < P(R_ENC + 2u) { return x * P(R_ENC + 3u); }
    return (1.0 + P(R_ENC + 1u)) * powf(x, 1.0 / P(R_ENC)) - P(R_ENC + 1u);
}

fn enc_decode(y: f32) -> f32 {
    if y < P(R_ENC + 2u) * P(R_ENC + 3u) { return y / P(R_ENC + 3u); }
    return powf(max((y + P(R_ENC + 1u)) / (1.0 + P(R_ENC + 1u)), 0.0), P(R_ENC));
}

// tools::range_strength (`o`: 5 knots)
fn range_strength(o: u32, l: f32) -> f32 {
    let x = clamp((l - 0.1) / 0.2, 0.0, 4.0);
    let i = min(u32(floor(x)), 3u);
    let f = x - f32(i);
    return max(P(o + i) + (P(o + i + 1u) - P(o + i)) * f, 0.0);
}

fn sgn_pow(x: f32, y: f32) -> f32 { return sign(x) * powf(abs(x), y); }

// tools::grade_prepared (the render's GradePrep at R_GRADE_*)
fn grade_prepared(c: vec3f) -> vec3f {
    var to_s = M(K_TO_SRGB);
    var from_s = M(K_FROM_SRGB);
    if on(TT_GRADE_RGB_PP) { to_s = M(K_PP_TO); from_s = M(K_PP_FROM); }
    var lab = to_lab(max(c, vec3f(0.0)));
    let chroma0 = sqrt(lab.y * lab.y + lab.z * lab.z);
    let bend = P(R_GRADE_BEND);
    let v = powf(clamp(lab.x, 0.0, 1.0), bend);
    let e = P(R_GRADE_E);
    var black = tools_ramp(lab.x, 0.0, P(TT_GRADE_BLACK));
    black = black * (1.0 - tools_ramp(lab.x, P(TT_GRADE_WHITE), 1.0));
    let es = srgb_encode3(clamp(ap3(M(K_TO_SRGB), c), vec3f(0.0), vec3f(1.0)));
    let hsv = rgb_to_hsv(es);
    let tint_surface = lum_surface(TT_GRADE_TINT_SURFACE, hsv.y, hsv.z);
    let gg = P(TT_GRADE_RGB_GAMMA);
    let rgb_mode = P(TT_GRADE_RGB) > 0.0;
    var e_rgb = vec3f(0.0);
    if rgb_mode {
        let t = ap3(to_s, c);
        e_rgb = vec3f(sgn_pow(t.x, 1.0 / gg), sgn_pow(t.y, 1.0 / gg), sgn_pow(t.z, 1.0 / gg));
    }
    let ref_e = P3(R_GRADE_REF_E);
    let l0 = lab.x;
    let chan_v = powf3(clamp(e_rgb, vec3f(0.0), vec3f(1.0)), bend);
    var k_sum = 0.0;
    var up = 0.0;
    var down = 0.0;
    for (var ri = 0u; ri < 4u; ri++) {
        let curve = TT_GRADE_CURVES + ri * 5u;
        let peak = P(R_GRADE_PEAKS + ri);
        let w = peak * powf(range_strength(curve, v) / peak, e) * black;
        let tp = R_GRADE_TINTS + ri * R_GRADE_TINT_SIZE;
        if !on(tp) { continue; }
        let dir = P2(tp + 1u);
        let light = P(tp + 3u);
        let k_unit = P(tp + 4u) * tint_surface / (1.0 + chroma0 * P(TT_GRADE_CHROMA_DAMP));
        let k = k_unit * w;
        k_sum += abs(k);
        let sat_shape = P(tp + 5u);
        up += abs(k) * P(TT_GRADE_LIFT_UP + ri) * light * sat_shape;
        down += abs(k) * P(TT_GRADE_LIFT_DOWN + ri) * light * sat_shape;
        lab.y += dir.x * k;
        lab.z += dir.y * k;
        lab.x += k * (P(TT_GRADE_LIFT) + P(TT_GRADE_LIFT + 1u) * dir.x + P(TT_GRADE_LIFT + 2u) * dir.y);
        if rgb_mode {
            let pushed = P3(tp + 6u);
            let pure = P3(tp + 9u);
            let mean = P(tp + 12u);
            for (var i = 0u; i < 3u; i++) {
                let along = (pushed[i] - ref_e[i]) / 0.05;
                let d = along + ((pure[i] - mean) * P(TT_GRADE_PURE_GAIN) - along) * P(TT_GRADE_PURE);
                var wi = w;
                if P(TT_GRADE_TINT_CHAN) > 0.0 {
                    let reach_i = peak * powf(range_strength(curve, chan_v[i]) / peak, e) * black;
                    wi = w + (reach_i - w) * P(TT_GRADE_TINT_CHAN);
                }
                e_rgb[i] += d * k_unit * wi * P(TT_GRADE_RGB_GAIN);
            }
        }
    }
    if rgb_mode {
        let dec = vec3f(sgn_pow(clamp(e_rgb.x, 0.0, 1.0), gg), sgn_pow(clamp(e_rgb.y, 0.0, 1.0), gg), sgn_pow(clamp(e_rgb.z, 0.0, 1.0), gg));
        let rgb_tinted = to_lab(max(ap3(from_s, dec), vec3f(0.0)));
        lab = lab + (rgb_tinted - lab) * P(TT_GRADE_RGB);
    }
    if k_sum > 0.0 {
        let dl = lab.x - l0;
        var g = down;
        if dl > 0.0 { g = up; }
        lab.x = l0 + dl * (g / k_sum);
    }
    let before_lum = lab;
    var stops = 0.0;
    var colour_stops = 0.0;
    for (var ri = 0u; ri < 4u; ri++) {
        let lum = P(R_GRADE_LUM + ri);
        if lum != 0.0 {
            let s = lum / 100.0 * P(TT_GRADE_LUM) * range_strength(TT_GRADE_LUM_CURVES + ri * 5u, v) * black;
            stops += s;
            colour_stops += s * P(TT_GRADE_LUM_CHROMA + ri);
        }
    }
    if stops != 0.0 {
        let k = exp2(stops / 3.0);
        let kc = exp2(colour_stops / 3.0);
        lab = vec3f(lab.x * k, lab.y * kc, lab.z * kc);
        if P(TT_GRADE_LUM_RGB) > 0.0 {
            let lin = ap3(M(K_PP_TO), max(to_rgb(before_lum), vec3f(0.0)));
            var moved: vec3f;
            for (var i = 0u; i < 3u; i++) {
                let level = powf(powf(clamp(lin[i], 0.0, 1.0), 1.0 / max(P(TT_GRADE_LUM_RGB_GAMMA), 0.5)), bend);
                var s = 0.0;
                for (var ri = 0u; ri < 4u; ri++) {
                    let lum = P(R_GRADE_LUM + ri);
                    if lum != 0.0 {
                        s += lum / 100.0 * P(TT_GRADE_LUM) * range_strength(TT_GRADE_LUM_CURVES + ri * 5u, level) * black;
                    }
                }
                moved[i] = lin[i] * exp2(s);
            }
            let per_channel = to_lab(max(ap3(M(K_PP_FROM), moved), vec3f(0.0)));
            lab = lab + (per_channel - lab) * P(TT_GRADE_LUM_RGB);
        }
    }
    return max(to_rgb(lab), vec3f(0.0));
}

// tools::vig_ease
fn vig_ease(y0: f32, highlights: f32) -> f32 {
    let y = clamp(y0, 0.0, 1.0 - 1e-6);
    var i = 0u;
    for (var k = 0u; k < 9u; k++) {
        if P(K_VIG_KNOTS + k) <= y { i = k; }
    }
    i = min(i, 7u);
    let f = (y - P(K_VIG_KNOTS + i)) / (P(K_VIG_KNOTS + i + 1u) - P(K_VIG_KNOTS + i));
    let g0 = P(K_VIG_EASE0 + i) + highlights * P(K_VIG_EASE1 + i);
    let g1 = P(K_VIG_EASE0 + i + 1u) + highlights * P(K_VIG_EASE1 + i + 1u);
    let g = g0 * (1.0 - f) + g1 * f;
    return min(2.0 / (1.0 + exp(3.0 - 3.0 * g)), 1.0);
}

// subject::VignetteMap::at (R_VIG_MAP: width, height, place in `lut`)
fn vig_map_at(u: f32, v: f32) -> f32 {
    let w = u32(P(R_VIG_MAP));
    let h = u32(P(R_VIG_MAP + 1u));
    let o = u32(P(R_VIG_MAP + 2u));
    let x = clamp((u + 1.0) * 0.5 * f32(w) - 0.5, 0.0, f32(w - 1u));
    let y = clamp((v + 1.0) * 0.5 * f32(h) - 0.5, 0.0, f32(h - 1u));
    let x0 = u32(x);
    let y0 = u32(y);
    let x1 = min(x0 + 1u, w - 1u);
    let y1 = min(y0 + 1u, h - 1u);
    let fx = x - f32(x0);
    let fy = y - f32(y0);
    let top = lut[o + y0 * w + x0] * (1.0 - fx) + lut[o + y0 * w + x1] * fx;
    let bottom = lut[o + y1 * w + x0] * (1.0 - fx) + lut[o + y1 * w + x1] * fx;
    return top * (1.0 - fy) + bottom * fy;
}

// tools::Prepared::apply_after_mixer
fn apply_after_mixer(c0: vec3f, u: f32, v: f32) -> vec3f {
    var c = c0;
    if on(R_PARAMETRIC) {
        let e = powf3(clamp(ap3(M(K_PP_TO), c), vec3f(0.0), vec3f(1.0)), 1.0 / 2.2);
        c = max(ap3(M(K_PP_FROM), powf3(max(curve_apply_rgb(u32(P(R_PARAMETRIC + 1u)), e), vec3f(0.0)), 2.2)), vec3f(0.0));
    }
    if on(R_CURVES) {
        let cv = R_CURVES + 1u;
        if on(cv) {
            let l = luma(c);
            if l > 1e-6 {
                let nl = enc_decode(max(curve_eval(u32(P(cv + 1u)), enc_encode(min(l, 1.0))), 0.0));
                c = c * (nl / l);
            }
        }
        let t = ap3(M(K_PP_TO), c);
        var e = vec3f(enc_encode(clamp(t.x, 0.0, 1.0)), enc_encode(clamp(t.y, 0.0, 1.0)), enc_encode(clamp(t.z, 0.0, 1.0)));
        if on(cv + 2u) {
            let o = u32(P(cv + 3u));
            let each = vec3f(curve_eval(o, e.x), curve_eval(o, e.y), curve_eval(o, e.z));
            if P(TT_CURVE_HUE_KEEP) > 0.0 {
                let kept = curve_apply_rgb(o, e);
                e = each + (kept - each) * P(TT_CURVE_HUE_KEEP);
            } else {
                e = each;
            }
        }
        for (var ch = 0u; ch < 3u; ch++) {
            if on(cv + 4u + ch * 2u) {
                e[ch] = curve_eval(u32(P(cv + 5u + ch * 2u)), e[ch]);
            }
        }
        c = max(ap3(M(K_PP_FROM), vec3f(enc_decode(max(e.x, 0.0)), enc_decode(max(e.y, 0.0)), enc_decode(max(e.z, 0.0)))), vec3f(0.0));
    }
    if on(R_GRADE) {
        c = grade_prepared(c);
    }
    var a = 0.0;
    let vignette = P(R_VIGNETTE);
    if vignette != 0.0 {
        let rnd = P(R_VIG_ROUND);
        let aspect = P(R_ASPECT);
        var r: f32;
        if rnd > 0.0 {
            let k = rnd * P(K_VIG_ROUND_POS);
            var u2 = u;
            var v2 = v;
            if aspect >= 1.0 { u2 = u * (1.0 + (aspect - 1.0) * k); } else { v2 = v * (1.0 + (1.0 / aspect - 1.0) * k); }
            r = sqrt(u2 * u2 + v2 * v2);
        } else if rnd < 0.0 {
            let x = -rnd;
            let p = 2.0 + P(K_VIG_ROUND_P) * x + P(K_VIG_ROUND_P + 1u) * x * x;
            let scale = 1.0 + P(K_VIG_ROUND_SCALE) * x + P(K_VIG_ROUND_SCALE + 1u) * x * x;
            r = powf(powf(abs(u), p) + powf(abs(v), p), 1.0 / p) / scale;
        } else {
            r = sqrt(u * u + v * v);
        }
        let f = powf(max(tools_ramp(r, P(R_VIG_CENTRE) - P(R_VIG_HALF), P(R_VIG_CENTRE) + P(R_VIG_HALF)), 1e-6), P(R_VIG_GAMMA));
        let s = min(abs(vignette) / 100.0, 0.999);
        let keep = max(1.0 - s * f, 1e-3);
        let lum = max(0.2627 * c.x + 0.678 * c.y + 0.0593 * c.z, 1e-6);
        if vignette < 0.0 {
            a += P(K_VIG_K) * log2(keep) * (1.0 - vig_ease(lum, P(R_VIG_HIGHLIGHTS)));
        } else {
            let y = min(lum, 0.999);
            a += log2((1.0 - (1.0 - y) * powf(keep, P(K_VIG_K_PLUS))) / y);
        }
    }
    if P(R_ADAPTIVE) != 0.0 && on(R_VIG_MAP + 3u) {
        a += P(R_ADAPTIVE) / 100.0 * P(TT_VIG_AMOUNT) * vig_map_at(u, v);
    }
    if a != 0.0 {
        c = c * exp2(a);
    }
    return c;
}

// ---- dngcolour.rs: Adobe's profiles (ProfileRender) -----------------------------------------------

// dngcolour::rgb_to_hsv (hue 0..6)
fn dng_rgb_to_hsv(c: vec3f) -> vec3f {
    let mx = max3(c);
    let mn = min3(c);
    let d = mx - mn;
    if mx <= 0.0 || d <= 0.0 { return vec3f(0.0, 0.0, mx); }
    var h: f32;
    if mx == c.x { h = (c.y - c.z) / d; } else if mx == c.y { h = 2.0 + (c.z - c.x) / d; } else { h = 4.0 + (c.x - c.y) / d; }
    return vec3f(rem_euclid(h, 6.0), d / mx, mx);
}

// dngcolour::hsv_to_rgb
fn dng_hsv_to_rgb(c: vec3f) -> vec3f {
    let s = c.y;
    let v = c.z;
    if s <= 0.0 { return vec3f(v); }
    let h = rem_euclid(c.x, 6.0);
    let i = floor(h);
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    switch i32(i) {
        case 0: { return vec3f(v, t, p); }
        case 1: { return vec3f(q, v, p); }
        case 2: { return vec3f(p, v, t); }
        case 3: { return vec3f(p, q, v); }
        case 4: { return vec3f(t, p, v); }
        default: { return vec3f(v, p, q); }
    }
}

fn hsv_entry(o: u32, hue_divs: u32, sat_divs: u32, vi: u32, hi: u32, si: u32) -> vec3f {
    let k = o + ((vi * hue_divs + hi) * sat_divs + si) * 3u;
    return vec3f(lut[k], lut[k + 1u], lut[k + 2u]);
}

// dngcolour::apply_table (`t`: the table's place in `prm`: lut offset, hue, sat, val divisions, sRGB)
fn apply_table(t: u32, rgb: vec3f) -> vec3f {
    let o = u32(P(t));
    let hue_divs = u32(P(t + 1u));
    let sat_divs = u32(P(t + 2u));
    let val_divs = max(u32(P(t + 3u)), 1u);
    let hsv = dng_rgb_to_hsv(rgb);
    let h = hsv.x;
    let s = hsv.y;
    let v = hsv.z;
    let hf = h * f32(hue_divs) / 6.0;
    let sf = s * f32(sat_divs - 1u);
    var vin = clamp(v, 0.0, 1.0);
    if on(t + 4u) { vin = srgb_encode(v); }
    let vf = vin * f32(val_divs - 1u);
    let h0f = floor(hf);
    let s0f = min(floor(sf), f32(sat_divs - 1u));
    let v0f = min(floor(vf), f32(val_divs - 1u));
    let ht = hf - h0f;
    let st = sf - s0f;
    let vt = vf - v0f;
    let h0 = u32(h0f) % hue_divs;
    let s0 = u32(s0f);
    let v0 = u32(v0f);
    let h1 = (h0 + 1u) % hue_divs;
    let s1 = min(s0 + 1u, sat_divs - 1u);
    let v1 = min(v0 + 1u, val_divs - 1u);
    let a0 = mix(hsv_entry(o, hue_divs, sat_divs, v0, h0, s0), hsv_entry(o, hue_divs, sat_divs, v0, h0, s1), st);
    let b0 = mix(hsv_entry(o, hue_divs, sat_divs, v0, h1, s0), hsv_entry(o, hue_divs, sat_divs, v0, h1, s1), st);
    let p0 = mix(a0, b0, ht);
    let a1 = mix(hsv_entry(o, hue_divs, sat_divs, v1, h0, s0), hsv_entry(o, hue_divs, sat_divs, v1, h0, s1), st);
    let b1 = mix(hsv_entry(o, hue_divs, sat_divs, v1, h1, s0), hsv_entry(o, hue_divs, sat_divs, v1, h1, s1), st);
    let p1 = mix(a1, b1, ht);
    let d = mix(p0, p1, vt);
    return dng_hsv_to_rgb(vec3f(h + d.x * 6.0 / 360.0, min(s * d.y, 1.0), v * d.z));
}

// dngcolour::ProfileRender::working
fn pr_working(p: vec3f) -> vec3f {
    let c = max(ap3(M(R_PR_TO_PP), p), vec3f(0.0));
    if on(R_PR_HSM) { return apply_table(R_PR_HSM + 1u, c); }
    return c;
}

// dngcolour::ProfileRender::finish
fn pr_finish(c0: vec3f) -> vec3f {
    var c = max(c0, vec3f(0.0));
    let n = u32(P(R_PR_LOOKS));
    for (var i = 0u; i < n; i++) {
        c = apply_table(R_PR_LOOKS + 1u + i * 5u, c);
    }
    c = curve_apply_rgb(u32(P(R_PR_TONE)), max(c, vec3f(0.0)));
    if on(R_PR_LOOK_CURVE) {
        let o = u32(P(R_PR_LOOK_CURVE + 1u));
        let e = curve_apply_rgb(o, srgb_encode3(c));
        c = vec3f(srgb_decode(e.x), srgb_decode(e.y), srgb_decode(e.z));
    }
    return c;
}
