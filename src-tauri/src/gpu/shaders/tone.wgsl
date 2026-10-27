// ---- pipeline.rs: the profile curve, tone sliders, colour adjustments ---------------------------

// pipeline::filmic
fn filmic(x0: f32, o: u32) -> f32 {
    let x = max(x0, 0.0);
    return (x * (P(o) * x + P(o + 1u))) / (x * (P(o + 2u) * x + P(o + 3u)) + P(o + 4u));
}

// pipeline::ToneCurve::apply (the render's curve: R_CURVE_*)
fn tone_curve_apply(x: f32) -> f32 {
    if on(R_CURVE_IDENTITY) { return clamp(x, 0.0, 1.0); }
    return min(filmic(x * P(R_CURVE_PRE_GAIN), R_CURVE_K), 1.0);
}

// pipeline::tone_rgb
fn tone_rgb(c: vec3f) -> vec3f {
    var i0 = 0u;
    var i1 = 1u;
    var i2 = 2u;
    if c[i0] > c[i1] { let t = i0; i0 = i1; i1 = t; }
    if c[i1] > c[i2] {
        let t = i1; i1 = i2; i2 = t;
        if c[i0] > c[i1] { let u = i0; i0 = i1; i1 = u; }
    }
    let lo = c[i0];
    let mid = c[i1];
    let hi = c[i2];
    let new_lo = tone_curve_apply(lo);
    let new_hi = tone_curve_apply(hi);
    var new_mid = new_hi;
    if hi > lo { new_mid = new_lo + (new_hi - new_lo) * (mid - lo) / (hi - lo); }
    var out = vec3f(0.0);
    out[i0] = new_lo;
    out[i1] = new_mid;
    out[i2] = new_hi;
    return out;
}

// The profile curve (render_core_q's `base_curve`): on linear ProPhoto channels or Rec.2020's.
fn base_curve(x: vec3f) -> vec3f {
    if on(T_CURVE_PP) {
        return ap3(M(K_PP_FROM), tone_rgb(max(ap3(M(K_PP_TO), x), vec3f(0.0))));
    }
    return tone_rgb(x);
}

// pipeline::bump
fn bump(ev: f32, centre: f32, width: f32) -> f32 {
    let d = (ev - centre) / width;
    return exp(-0.5 * (d * d));
}

// pipeline::ramp
fn ramp(ev: f32, lo: f32, hi: f32) -> f32 {
    let t = clamp((ev - lo) / (hi - lo), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// pipeline::whites_curve
fn whites_curve(e: f32, amount: f32) -> f32 {
    let a = min(abs(amount) / 100.0, 1.0);
    if a == 0.0 { return e; }
    if amount < 0.0 {
        let o = T_WHITES_MINUS_CURVE;
        let k = P(o); let p = P(o + 1u); let b = P(o + 2u); let c0 = P(o + 3u); let c1 = P(o + 4u);
        return e - k * powf(a, p) * exp(b * e) / (1.0 + exp(b * (e - c0 - c1 * a)));
    }
    let o = T_WHITES_PLUS_CURVE;
    let k = P(o); let p = P(o + 1u); let b = P(o + 2u); let b2 = P(o + 3u); let c0 = P(o + 4u); let c1 = P(o + 5u);
    return e + k * powf(a, p) * exp(b * e) / (1.0 + exp(b2 * (e - c0 - c1 * a)));
}

// pipeline::blacks_curve
fn blacks_curve(e: f32, amount: f32) -> f32 {
    let a = min(abs(amount) / 100.0, 1.0);
    if a == 0.0 { return e; }
    let x = 0.18 * exp2(e);
    var y: f32;
    if amount < 0.0 {
        let o = T_BLACKS_MINUS_CURVE;
        let k = P(o); let p = P(o + 1u); let soft = P(o + 2u);
        let y0 = x - k * powf(a, p);
        y = (y0 + sqrt(y0 * y0 + soft * soft)) / 2.0;
    } else {
        let o = T_BLACKS_PLUS_CURVE;
        let k = P(o); let p = P(o + 1u); let q = P(o + 2u); let x0 = P(o + 3u); let w = P(o + 4u);
        let s = powf(x, q) / (powf(x, q) + powf(x0, q));
        y = x + k * powf(a, p) * s * max(1.0 - x / w, 0.0);
    }
    return log2(max(y, 1e-9) / 0.18);
}

// pipeline::hs_curve (`m`: the place of an 11-number model)
fn hs_curve(e: f32, amount: f32, m: u32, rising: bool) -> f32 {
    if amount == 0.0 { return 0.0; }
    let a = min(abs(amount) / 100.0, 1.0);
    let k = P(m); let p = P(m + 1u); let b = P(m + 2u); let b2 = P(m + 3u); let c = P(m + 4u);
    let w1 = P(m + 5u); let w2 = P(m + 6u); let w3 = P(m + 7u); let w4 = P(m + 8u); let m1 = P(m + 9u); let m2 = P(m + 10u);
    let st = T_PHOTO_STATS;
    let anchor = w1 * P(st) + w2 * P(st + 1u) + w3 * P(st + 2u) + w4 * P(st + 3u);
    var u = anchor - e;
    if rising { u = e - anchor; }
    var b2a = b + abs(b2);
    if rising { b2a = abs(b2); }
    if rising && b > 0.0 && b2a > b { u = min(u, c + log(b / (b2a - b)) / b2a); }
    let amp = abs(k) * powf(a, abs(p)) * exp2(m1 * (P(st + 3u) - P(st + 1u)) + m2 * P(st));
    let g = amp * exp(b * u) / (1.0 + exp(b2a * (u - c)));
    return signum(amount) * min(g, 4.0);
}

// pipeline::highlights_weight
fn highlights_weight(p: f32) -> f32 {
    let b = bump(p, P(T_HIGHLIGHTS_CENTRE), P(T_HIGHLIGHTS_WIDTH));
    if P(T_HIGHLIGHTS_TAIL) > 0.0 && p < P(T_HIGHLIGHTS_CENTRE) {
        return max(b, P(T_HIGHLIGHTS_TAIL) * exp((p - P(T_HIGHLIGHTS_CENTRE)) / max(P(T_HIGHLIGHTS_TAIL_WIDTH), 0.1)));
    }
    return b;
}

// pipeline::monotone_curve through (0, 0), (x1, y1), (1, 1)
fn monotone_curve3(v: f32, x1: f32, y1: f32) -> f32 {
    let xs = vec3f(0.0, x1, 1.0);
    let ys = vec3f(0.0, y1, 1.0);
    let x = clamp(v, 0.0, 1.0);
    let d0 = (ys[1] - ys[0]) / (xs[1] - xs[0]);
    let d1 = (ys[2] - ys[1]) / (xs[2] - xs[1]);
    var m = vec3f(d0, 0.0, d1);
    if d0 * d1 <= 0.0 { m[1] = 0.0; } else { m[1] = 2.0 / (1.0 / d0 + 1.0 / d1); }
    var i = 1u;
    if x < xs[1] { i = 0u; }
    let h = xs[i + 1u] - xs[i];
    let t = (x - xs[i]) / h;
    let t2 = t * t;
    let t3 = t * t * t;
    return (2.0 * t3 - 3.0 * t2 + 1.0) * ys[i] + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * ys[i + 1u] + (t3 - t2) * h * m[i + 1u];
}

// pipeline::display_curve
fn display_curve(v: f32, o: u32) -> f32 {
    let xs = vec4f(0.0, P(o), P(o + 2u), 1.0);
    let ys = vec4f(0.0, P(o + 1u), P(o + 3u), 1.0);
    let x = clamp(v, 0.0, 1.0);
    let d = vec3f((ys[1] - ys[0]) / (xs[1] - xs[0]), (ys[2] - ys[1]) / (xs[2] - xs[1]), (ys[3] - ys[2]) / (xs[3] - xs[2]));
    var m = vec4f(d[0], 0.0, 0.0, d[2]);
    for (var i = 1u; i < 3u; i++) {
        if d[i - 1u] * d[i] <= 0.0 { m[i] = 0.0; } else { m[i] = 2.0 / (1.0 / d[i - 1u] + 1.0 / d[i]); }
    }
    var i = 2u;
    if x < xs[1] { i = 0u; } else if x < xs[2] { i = 1u; }
    let h = xs[i + 1u] - xs[i];
    let t = (x - xs[i]) / h;
    let t2 = t * t;
    let t3 = t * t * t;
    return (2.0 * t3 - 3.0 * t2 + 1.0) * ys[i] + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * ys[i + 1u] + (t3 - t2) * h * m[i + 1u];
}

// The four tone sliders and Contrast, as one render reads them (`RenderSettings`).
struct Tone {
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
}

fn strength(amount: f32, plus: f32, minus: f32) -> f32 {
    if amount >= 0.0 { return amount / 100.0 * plus; }
    return amount / 100.0 * minus;
}

// What pipeline::tone_bands_span's `chain` needs about the pixel and photo.
struct Chain {
    at_hi: f32,
    at_lo: f32,
    wb_hi: f32,
    wb_lo: f32,
    stretch: f32,
    key: f32,
    wf: f32,
    wt: f32,
}

fn band_pos(c: Chain, e: f32) -> f32 { return P(T_TONE_KEY_REF) + (e - P(T_TONE_KEY_REF)) * c.stretch; }

// pipeline::tone_bands_span's `chain`: the end brightness and the brightness before Blacks.
fn tone_chain(s: Tone, c: Chain, e0: f32) -> vec2f {
    var e = e0;
    if on(T_HS_ADAPTIVE) {
        var hm = T_HIGHLIGHTS_PLUS_MODEL;
        if s.highlights < 0.0 { hm = T_HIGHLIGHTS_MINUS_MODEL; }
        e += hs_curve(e, s.highlights, hm, true);
        var link = 1.0;
        if s.shadows > 0.0 { link = max(1.0 - P(T_SHADOWS_HIGHLIGHTS_LINK) * max(-s.highlights, 0.0) / 100.0, 0.0); }
        var sm = T_SHADOWS_MINUS_MODEL;
        if s.shadows > 0.0 { sm = T_SHADOWS_PLUS_MODEL; }
        e += link * hs_curve(e, s.shadows, sm, false);
    } else {
        e += strength(s.highlights, P(T_HIGHLIGHTS_PLUS), P(T_HIGHLIGHTS_MINUS))
            * (highlights_weight(band_pos(c, e + c.at_hi)) + P(T_HIGHLIGHTS_GLOBAL) * ramp(c.key, P(T_HIGHLIGHTS_GLOBAL_FROM), P(T_HIGHLIGHTS_GLOBAL_FROM) + max(P(T_HIGHLIGHTS_GLOBAL_WIDTH), 0.1)));
        var sh = s.shadows;
        if s.shadows > 0.0 && P(T_SHADOWS_POW) != 1.0 { sh = 100.0 * powf(s.shadows / 100.0, P(T_SHADOWS_POW)); }
        let sp = band_pos(c, e + c.at_lo);
        var sw = P(T_SHADOWS_WIDTH);
        if sh > 0.0 && P(T_SHADOWS_PLUS_WIDTH_LO) > 0.0 && sp < P(T_SHADOWS_CENTRE) { sw = P(T_SHADOWS_PLUS_WIDTH_LO); }
        var link = 1.0;
        if sh > 0.0 { link = max(1.0 - P(T_SHADOWS_HIGHLIGHTS_LINK) * max(-s.highlights, 0.0) / 100.0, 0.0); }
        e += link * strength(sh, P(T_SHADOWS_PLUS), P(T_SHADOWS_MINUS))
            * (bump(sp, P(T_SHADOWS_CENTRE), sw) + P(T_SHADOWS_GLOBAL) * (1.0 - ramp(c.key, P(T_SHADOWS_GLOBAL_FROM), P(T_SHADOWS_GLOBAL_FROM) + max(P(T_SHADOWS_GLOBAL_WIDTH), 0.1))));
    }
    if on(T_WB_CURVE) {
        let ew = whites_curve(e, s.whites);
        return vec2f(blacks_curve(ew, s.blacks), ew);
    }
    var wr = ramp(band_pos(c, e + c.wb_hi + P(T_WHITES_KEY) * (c.key - P(T_TONE_KEY_REF))), c.wf, c.wt);
    if s.whites < 0.0 && P(T_WHITES_MINUS_POW) != 1.0 { wr = powf(wr, P(T_WHITES_MINUS_POW)); }
    let wc = 1.0 + P(T_WHITES_CONTRAST) * max(s.contrast, 0.0) / 100.0;
    e += strength(s.whites, P(T_WHITES_PLUS), P(T_WHITES_MINUS)) * wc * wr;
    let before_blacks = e;
    var bc = P(T_BLACKS_CENTRE);
    var bw = P(T_BLACKS_WIDTH);
    if s.blacks > 0.0 && P(T_BLACKS_PLUS_WIDTH) > 0.0 { bc = P(T_BLACKS_PLUS_CENTRE); bw = P(T_BLACKS_PLUS_WIDTH); }
    e += strength(s.blacks, P(T_BLACKS_PLUS), P(T_BLACKS_MINUS)) * bump(band_pos(c, e + c.wb_lo), bc, bw);
    return vec2f(e, before_blacks);
}

// pipeline::tone_bands_span
fn tone_bands_span(c: vec3f, s: Tone, offset: f32, key: f32, span: f32) -> vec3f {
    if s.highlights == 0.0 && s.shadows == 0.0 && s.whites == 0.0 && s.blacks == 0.0 { return c; }
    let l = luma(c);
    if l <= 1e-7 { return c; }
    let start = log2(l / 0.18);
    var below = min(key - P(T_TONE_KEY_REF), 0.0);
    if P(T_TONE_KEY_DARK_SPAN) > 0.0 { below = max(below, -P(T_TONE_KEY_DARK_SPAN)); }
    var extra = 0.0;
    if P(T_TONE_KEY_DARK) != 0.0 { extra = P(T_TONE_KEY_DARK) - P(T_TONE_KEY); }
    let key_shift = P(T_TONE_KEY) * (key - P(T_TONE_KEY_REF)) + extra * below;
    let at = P(T_TONE_LOCAL) * offset - key_shift;
    let at_wb = P(T_TONE_LOCAL_WB) * offset - key_shift;
    var ch: Chain;
    ch.stretch = powf(P(T_TONE_SPAN_REF) / max(span, 1.0), P(T_TONE_SPAN));
    ch.key = key;
    if on(T_TONE_MINMAX) {
        let o = P(T_TONE_LOCAL) * offset;
        let ow = P(T_TONE_LOCAL_WB) * offset;
        ch.at_hi = min(o, 0.0) - key_shift;
        ch.at_lo = max(o, 0.0) - key_shift;
        ch.wb_hi = min(ow, 0.0) - key_shift;
        ch.wb_lo = max(ow, 0.0) - key_shift;
    } else {
        ch.at_hi = at;
        ch.at_lo = at;
        ch.wb_hi = at_wb;
        ch.wb_lo = at_wb;
    }
    if on(T_HS_OWN) {
        ch.at_hi = P(T_HIGHLIGHTS_LOCAL) * offset - P(T_HIGHLIGHTS_KEY) * (key - P(T_TONE_KEY_REF));
        ch.at_lo = P(T_SHADOWS_LOCAL) * offset - P(T_SHADOWS_KEY) * (key - P(T_TONE_KEY_REF));
    }
    if s.whites >= 0.0 {
        ch.wf = P(T_WHITES_FROM);
        ch.wt = P(T_WHITES_TO);
    } else {
        ch.wf = P(T_WHITES_MINUS_FROM);
        ch.wt = P(T_WHITES_MINUS_TO);
    }
    if on(T_TONE_PP) {
        let p = max(ap3(M(K_PP_TO), c), vec3f(0.0));
        let hi = max3(p);
        let lo = min3(p);
        if hi <= 1e-7 { return c; }
        var nh = hi;
        if hi > 1e-7 { let e = log2(hi / 0.18); nh = hi * exp2(tone_chain(s, ch, e).x - e); }
        var nl = lo;
        if lo > 1e-7 { let e = log2(lo / 0.18); nl = lo * exp2(tone_chain(s, ch, e).x - e); }
        var out: vec3f;
        if hi - lo < 1e-9 { out = vec3f(nh); } else { out = nl + (nh - nl) * (p - lo) / (hi - lo); }
        return ap3(M(K_PP_FROM), out);
    }
    let r = tone_chain(s, ch, start);
    let ev = r.x;
    let scale = exp2(ev - start);
    if P(T_TONE_CHROMA) == 0.0 { return c * scale; }
    let keep = exp2(r.y - start) * powf(exp2(ev - r.y), 1.0 - P(T_TONE_CHROMA));
    return max(l * scale + (c - l) * keep, vec3f(0.0));
}

// pipeline::contrast_at's `map`
fn contrast_map(v: f32, k: f32, pivot: f32) -> f32 {
    if v <= 1e-6 { return v; }
    let ev = log2(v / 0.18);
    let d = ev - pivot;
    if d > 0.0 {
        let fade = 1.0 - ramp(ev, P(T_CONTRAST_FADE_FROM), P(T_CONTRAST_FADE_TO));
        let kk = 1.0 + (k - 1.0) * P(T_CONTRAST_BRIGHT) * fade;
        return v * exp2(pivot + d * kk - ev);
    }
    let a = -d;
    let f = P(T_CONTRAST_TOE_FROM);
    let to = P(T_CONTRAST_TOE_TO);
    var off: f32;
    if to <= f || a <= f {
        off = k * a;
    } else if a <= to {
        off = k * f + (a - f) * (k - (k - 1.0) * (a - f) / (2.0 * (to - f)));
    } else {
        off = k * f + (to - f) * (k + 1.0) / 2.0 + (a - to);
    }
    return v * exp2(pivot - off - ev);
}

// pipeline::contrast_at
fn contrast_at(c0: vec3f, amount: f32, key: f32) -> vec3f {
    if amount == 0.0 { return c0; }
    var k: f32;
    if amount > 0.0 { k = 1.0 + amount / 100.0 * (P(T_CONTRAST_PLUS) - 1.0); } else { k = 1.0 + amount / 100.0 * (1.0 - P(T_CONTRAST_MINUS)); }
    let pivot = P(T_CONTRAST_PIVOT) + P(T_CONTRAST_KEY) * (key - P(T_TONE_KEY_REF));
    if P(T_CONTRAST_RGB) > 0.5 {
        let pp = P(T_CONTRAST_RGB) > 1.5;
        var c = c0;
        if pp { c = ap3(M(K_PP_TO), c0); }
        let hi = max3(c);
        let lo = min3(c);
        if hi <= 1e-6 {
            if pp { return ap3(M(K_PP_FROM), c); }
            return c;
        }
        let nh = contrast_map(hi, k, pivot);
        let nl = contrast_map(max(lo, 0.0), k, pivot);
        var out: vec3f;
        if P(T_CONTRAST_RGB) > 2.5 {
            out = vec3f(contrast_map(max(c.x, 0.0), k, pivot), contrast_map(max(c.y, 0.0), k, pivot), contrast_map(max(c.z, 0.0), k, pivot));
        } else if hi - lo < 1e-9 {
            out = vec3f(nh);
        } else {
            out = nl + (nh - nl) * (c - lo) / (hi - lo);
        }
        if pp { return ap3(M(K_PP_FROM), out); }
        return out;
    }
    let l = luma(c0);
    if l <= 1e-6 { return c0; }
    let scale = contrast_map(l, k, pivot) / l;
    return c0 * scale;
}

// pipeline::contrast_and_tone
fn contrast_and_tone(lit: vec3f, s: Tone, offset: f32, key: f32, span: f32) -> vec3f {
    if P(T_CONTRAST_AFTER_TONE) > 1.5 {
        var wb = s;
        wb.highlights = 0.0;
        wb.shadows = 0.0;
        var hs = s;
        hs.whites = 0.0;
        hs.blacks = 0.0;
        return tone_bands_span(contrast_at(tone_bands_span(lit, wb, offset, key, span), s.contrast, key), hs, offset, key, span);
    } else if P(T_CONTRAST_AFTER_TONE) > 0.5 {
        return contrast_at(tone_bands_span(lit, s, offset, key, span), s.contrast, key);
    }
    return tone_bands_span(contrast_at(lit, s.contrast, key), s, offset, key, span);
}

// pipeline::look_hsv
fn look_hsv(c: vec3f) -> vec3f {
    let p = max(ap3(M(K_PP_TO), c), vec3f(0.0));
    let mx = max3(p);
    let mn = min3(p);
    let d = mx - mn;
    if mx <= 1e-7 || d <= 1e-7 { return c; }
    var h: f32;
    if mx == p.x { h = 60.0 * rem_euclid((p.y - p.z) / d, 6.0); } else if mx == p.y { h = 60.0 * ((p.z - p.x) / d + 2.0); } else { h = 60.0 * ((p.x - p.y) / d + 4.0); }
    let s = d / mx;
    let v = mx;
    let pos = h / 30.0;
    let i0 = u32(floor(pos)) % 12u;
    let f = pos - trunc(pos);
    let i1 = (i0 + 1u) % 12u;
    var e = P3(T_LOOK_HSV + i0 * 3u) + (P3(T_LOOK_HSV + i1 * 3u) - P3(T_LOOK_HSV + i0 * 3u)) * f;
    if on(R_LOOK_HSV_DARK) {
        let dark = P3(T_LOOK_HSV_DARK + i0 * 3u) + (P3(T_LOOK_HSV_DARK + i1 * 3u) - P3(T_LOOK_HSV_DARK + i0 * 3u)) * f;
        let m = powf(clamp(v, 0.0, 1.0), 1.0 / 2.2);
        e = dark + (e - dark) * m;
    }
    let h2 = h + e.x;
    let s2 = clamp(s * (1.0 + e.y), 0.0, 1.0);
    let v2 = v * max(1.0 + e.z * s, 0.0);
    let hp = rem_euclid(h2, 360.0) / 60.0;
    let fr = hp - floor(hp);
    let a = v2 * (1.0 - s2);
    let b = v2 * (1.0 - s2 * fr);
    let cc = v2 * (1.0 - s2 * (1.0 - fr));
    var out: vec3f;
    switch i32(hp) {
        case 0: { out = vec3f(v2, cc, a); }
        case 1: { out = vec3f(b, v2, a); }
        case 2: { out = vec3f(a, v2, cc); }
        case 3: { out = vec3f(a, b, v2); }
        case 4: { out = vec3f(cc, a, v2); }
        default: { out = vec3f(v2, a, b); }
    }
    return ap3(M(K_PP_FROM), out);
}

// The colour at boost `kk` in pipeline::limit_boost's two uses: on linear ProPhoto (`sat_pp`,
// `a` the ProPhoto colour, `y` its luminance) or on OKLab chroma (`a` the OKLab colour).
fn boost_at(sat_pp: bool, a: vec3f, y: f32, kk: f32) -> vec3f {
    if sat_pp { return ap3(M(K_PP_FROM), y + (a - y) * kk); }
    return to_rgb(vec3f(a.x, a.y * kk, a.z * kk));
}

// pipeline::limit_boost
fn limit_boost(sat_pp: bool, a: vec3f, y: f32, k: f32) -> vec3f {
    let full = boost_at(sat_pp, a, y, k);
    let at_one = boost_at(sat_pp, a, y, 1.0);
    if min3(full) >= 0.0 || k <= 1.0 || min3(at_one) < 0.0 { return max(full, vec3f(0.0)); }
    var lo = 1.0;
    var hi = k;
    let steps = 24;
    for (var n = 1; n <= steps; n++) {
        let kk = 1.0 + (k - 1.0) * f32(n) / f32(steps);
        if min3(boost_at(sat_pp, a, y, kk)) >= 0.0 { lo = kk; } else { hi = kk; break; }
    }
    for (var i = 0; i < 16; i++) {
        let mid = 0.5 * (lo + hi);
        if min3(boost_at(sat_pp, a, y, mid)) >= 0.0 { lo = mid; } else { hi = mid; }
    }
    return max(boost_at(sat_pp, a, y, lo), vec3f(0.0));
}

// pipeline::hue_weight_of
fn hue_weight_of(s1: f32, c1: f32, s2: f32, c2: f32, o: u32) -> f32 {
    return max(1.0 + P(o) * c1 + P(o + 1u) * s1 + P(o + 2u) * c2 + P(o + 3u) * s2, 0.0);
}

// pipeline::hue_weight
fn hue_weight(hue: f32, o: u32) -> f32 {
    return hue_weight_of(sin(hue), cos(hue), sin(2.0 * hue), cos(2.0 * hue), o);
}

// pipeline::ColourAdjust::apply. `has_look`: the render's (R_ADJ_HAS_LOOK) or none (the plain
// tuning used with Adobe's profile).
fn colour_adjust(rgb0: vec3f, has_look: bool) -> vec3f {
    var rgb = rgb0;
    if on(T_LOOK_PP) { rgb = look_hsv(rgb0); }
    let vibrance = P(S_VIBRANCE);
    let saturation = P(S_SATURATION);
    if vibrance == 0.0 && saturation == 0.0 && !has_look && P(S_CONTRAST) == 0.0 { return rgb; }
    var lab = to_lab(rgb);
    var chroma = sqrt(lab.y * lab.y + lab.z * lab.z);
    if chroma < 1e-6 { return rgb; }
    var hue = atan2(lab.z, lab.y);
    var soft_c = 1.0;
    if P(T_CONTRAST_SAT_SOFT) > 0.0 { soft_c = P(T_CONTRAST_SAT_SOFT) / (P(T_CONTRAST_SAT_SOFT) + chroma); }
    let kc = max(1.0 + P(R_ADJ_KC_FULL) * soft_c, 0.0);
    if has_look {
        let s1 = sin(hue);
        let co1 = cos(hue);
        let s2 = sin(2.0 * hue);
        let co2 = cos(2.0 * hue);
        chroma *= P(T_LOOK_CHROMA) * hue_weight_of(s1, co1, s2, co2, T_LOOK_CHROMA + 1u);
        var light = lab.x + chroma * (P(T_LOOK_LIGHT) * co1 + P(T_LOOK_LIGHT + 1u) * s1 + P(T_LOOK_LIGHT + 2u) * co2 + P(T_LOOK_LIGHT + 3u) * s2);
        let pos = rem_euclid(hue, TAU) / TAU * 12.0;
        let i0 = u32(floor(pos)) % 12u;
        let f = pos - trunc(pos);
        let i1 = (i0 + 1u) % 12u;
        let entry = P3(T_LOOK_LUT + i0 * 3u) + (P3(T_LOOK_LUT + i1 * 3u) - P3(T_LOOK_LUT + i0 * 3u)) * f;
        light += chroma * entry.z;
        if on(R_LOOK_WARM) {
            let d = (rem_euclid(to_degrees(hue) - 85.0 + 540.0, 360.0) - 180.0) / 55.0;
            let w = max(1.0 - d * d, 0.0);
            if w > 0.0 {
                let p = clamp(lab.x * 4.0, 0.0, 4.0);
                let i = min(u32(floor(p)), 3u);
                let fr = p - f32(i);
                let row = P2(T_LOOK_WARM + i * 2u) + (P2(T_LOOK_WARM + (i + 1u) * 2u) - P2(T_LOOK_WARM + i * 2u)) * fr;
                light += chroma * w * row.x;
                chroma *= max(1.0 + w * row.y, 0.0);
            }
        }
        chroma *= max(1.0 + entry.y, 0.0);
        hue += to_radians(P(T_LOOK_HUE) * co1 + P(T_LOOK_HUE + 1u) * s1 + P(T_LOOK_HUE + 2u) * co2 + P(T_LOOK_HUE + 3u) * s2 + entry.x);
        chroma /= 1.0 + P(T_LOOK_COMP) * chroma;
        chroma *= kc;
        lab = vec3f(max(light, 0.0), chroma * cos(hue), chroma * sin(hue));
        if vibrance == 0.0 && saturation == 0.0 {
            return max(to_rgb(lab), vec3f(0.0));
        }
    }
    var scale: f32;
    if saturation >= 0.0 {
        scale = 1.0 + saturation / 100.0 * P(T_SATURATION_PLUS) * hue_weight(hue, T_SATURATION_HUE);
    } else {
        scale = 1.0 + saturation / 100.0 * P(T_SATURATION_MINUS);
    }
    if vibrance > 0.0 {
        let soft = P(T_VIBRANCE_SOFT) / (P(T_VIBRANCE_SOFT) + chroma);
        scale *= 1.0 + P(R_ADJ_VIBRANCE_V) * soft * 2.0 * hue_weight(hue, T_VIBRANCE_HUE);
    } else if vibrance < 0.0 {
        scale *= 1.0 + vibrance / 100.0 * P(T_VIBRANCE_MINUS);
    }
    var kk = 1.0;
    if !has_look { kk = kc; }
    let k = max(scale, 0.0) * kk;
    if on(T_SAT_PP) {
        let p = ap3(M(K_PP_TO), to_rgb(lab));
        let y = 0.2880 * p.x + 0.7119 * p.y + 0.0001 * p.z;
        return limit_boost(true, p, y, k);
    }
    return limit_boost(false, lab, 0.0, k);
}

// pipeline::lightness_to_y / lightness / cielab_chroma are only used by the measured tone table,
// which the GPU leaves to the CPU (`gpu::supported`).

// pipeline::adobe_fix
fn adobe_fix(c: vec3f) -> vec3f {
    if !on(R_ADOBE_FIX) { return c; }
    let y = max(0.2627 * c.x + 0.6780 * c.y + 0.0593 * c.z, 0.0);
    if y <= 1e-6 { return c; }
    var fy0: f32;
    if y > 0.008856 { fy0 = cbrt(y); } else { fy0 = 7.787 * y + 16.0 / 116.0; }
    let l = 116.0 * fy0 - 16.0;
    let p = clamp(l / 10.0, 0.0, 10.0);
    let i = min(u32(floor(p)), 9u);
    let fr = p - f32(i);
    let at_l = P(T_ADOBE_FIX_L + i) + (P(T_ADOBE_FIX_L + i + 1u) - P(T_ADOBE_FIX_L + i)) * fr;
    let at_c = P(T_ADOBE_FIX_C + i) + (P(T_ADOBE_FIX_C + i + 1u) - P(T_ADOBE_FIX_C + i)) * fr;
    let l2 = max(l + at_l, 0.0);
    let fy = (l2 + 16.0) / 116.0;
    var y2: f32;
    if fy > 0.206893 { y2 = fy * fy * fy; } else { y2 = (fy - 16.0 / 116.0) / 7.787; }
    let k = y2 / y;
    let g = 1.0 + at_c;
    return y2 + (c * k - y2) * g;
}
