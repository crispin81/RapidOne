// ---- gradient.rs: masks and gradients (local adjustments) ---------------------------------------
// Each mask is a record in `lut` (gpu::params::gradients): hidden, shapes, invert, its 12 sliders,
// then its shapes (16 numbers each: kind, subtract, invert, full, zero, centre, rx, ry, angle,
// feather, and a grid's width, height and place).

// gradient::smooth
fn grad_smooth(t0: f32) -> f32 {
    let t = clamp(t0, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// gradient::Grid::at
fn grid_at(w: u32, h: u32, o: u32, x: f32, y: f32) -> f32 {
    let fx = clamp(x * f32(w) - 0.5, 0.0, f32(w - 1u));
    let fy = clamp(y * f32(h) - 0.5, 0.0, f32(h - 1u));
    let x0 = u32(fx);
    let y0 = u32(fy);
    let x1 = min(x0 + 1u, w - 1u);
    let y1 = min(y0 + 1u, h - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let top = lut[o + y0 * w + x0] * (1.0 - tx) + lut[o + y0 * w + x1] * tx;
    let bottom = lut[o + y1 * w + x0] * (1.0 - tx) + lut[o + y1 * w + x1] * tx;
    return top * (1.0 - ty) + bottom * ty;
}

// gradient::Comp::at (with Gradient::weight for the formulas, their own invert off)
fn comp_at(o: u32, x: f32, y: f32, aspect: f32) -> f32 {
    let kind = u32(lut[o]);
    if kind == 1u {
        let fx = lut[o + 3u] * aspect;
        let fy = lut[o + 4u];
        let dx = lut[o + 5u] * aspect - fx;
        let dy = lut[o + 6u] - fy;
        let len2 = max(dx * dx + dy * dy, 1e-8);
        let t = ((x * aspect - fx) * dx + (y - fy) * dy) / len2;
        return 1.0 - clamp(t, 0.0, 1.0);
    }
    if kind == 2u {
        let px = (x - lut[o + 7u]) * aspect;
        let py = y - lut[o + 8u];
        let ang = -to_radians(lut[o + 11u]);
        let s = sin(ang);
        let c = cos(ang);
        let ru = px * c - py * s;
        let rv = px * s + py * c;
        let rx = max(lut[o + 9u] * aspect, 1e-4);
        let ry = max(lut[o + 10u], 1e-4);
        let qu = ru / rx;
        let qv = rv / ry;
        let d = sqrt(qu * qu + qv * qv);
        let f = clamp(lut[o + 12u] / 100.0, 0.0, 1.0);
        if f <= 1e-3 {
            if d <= 1.0 { return 1.0; }
            return 0.0;
        }
        let k = P3(K_RADIAL_FADE);
        let a = 1.0 - k.x * powf(f, k.y);
        let b = 1.0 + k.z * powf(f, P(K_RADIAL_FADE + 3u));
        return 1.0 - grad_smooth((d - a) / (b - a));
    }
    if kind == 3u {
        return clamp(grid_at(u32(lut[o + 13u]), u32(lut[o + 14u]), u32(lut[o + 15u]), x, y), 0.0, 1.0);
    }
    return 0.0;
}

// The gradients' combined amounts at one place (gradient::Local).
struct Local {
    ev: f32,
    temp: f32,
    tint: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    saturation: f32,
}

// gradient::MaskShape::at for the mask record at `o` (x, y: fractions of the whole photo).
fn mask_at(o: u32, x: f32, y: f32, aspect: f32) -> f32 {
    let comps = u32(lut[o + 1u]);
    var c = o + 15u;
    var w = comp_at(c, x, y, aspect);
    for (var k = 1u; k < comps; k++) {
        c += 16u;
        var p = comp_at(c, x, y, aspect);
        if lut[c + 2u] > 0.5 { p = 1.0 - p; }
        if lut[c + 1u] > 0.5 { w = w * (1.0 - p); } else { w = max(w, p); }
    }
    if lut[o + 2u] > 0.5 { w = 1.0 - w; }
    return w;
}

// Where the mask record after the one at `o` starts.
fn mask_next(o: u32) -> u32 { return o + 15u + u32(lut[o + 1u]) * 16u; }

// gradient::Prepared::at_frac (x, y: fractions of the whole photo)
fn local_at(x: f32, y: f32) -> Local {
    var l = Local(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let count = u32(P(R_GRADS + 1u));
    let aspect = P(R_GRADS + 3u);
    var o = u32(P(R_GRADS + 2u));
    for (var g = 0u; g < count; g++) {
        let hidden = lut[o] > 0.5;
        let sl = o + 3u;
        let w = mask_at(o, x, y, aspect);
        o = mask_next(o);
        if hidden || w <= 0.0 { continue; }
        l.ev += w * lut[sl];
        l.temp += w * lut[sl + 1u] / 100.0;
        l.tint += w * lut[sl + 2u] / 100.0;
        l.contrast += w * lut[sl + 3u] / 100.0;
        l.highlights += w * lut[sl + 4u] / 100.0;
        l.shadows += w * lut[sl + 5u] / 100.0;
        l.whites += w * lut[sl + 6u] / 100.0;
        l.blacks += w * lut[sl + 7u] / 100.0;
        l.saturation += w * lut[sl + 8u] / 100.0;
    }
    return l;
}

// gradient::Prepared::at_frac's `definition`: the masks' Texture, Clarity and Dehaze there.
fn local_definition(x: f32, y: f32) -> vec3f {
    var d = vec3f(0.0);
    let count = u32(P(R_GRADS + 1u));
    let aspect = P(R_GRADS + 3u);
    var o = u32(P(R_GRADS + 2u));
    for (var g = 0u; g < count; g++) {
        let hidden = lut[o] > 0.5;
        let sl = o + 3u;
        let w = mask_at(o, x, y, aspect);
        o = mask_next(o);
        if hidden || w <= 0.0 { continue; }
        d += w * vec3f(lut[sl + 9u], lut[sl + 10u], lut[sl + 11u]);
    }
    return d;
}

// gradient::Local::scene
fn local_scene(l: Local, p: vec3f) -> vec3f {
    if l.ev == 0.0 && l.temp == 0.0 && l.tint == 0.0 { return p; }
    let k = P(K_LOCAL_WB);
    let tb = P(K_LOCAL_WB + 1u);
    let ti = P(K_LOCAL_WB + 2u);
    let tg = P(K_LOCAL_WB + 3u);
    let gain = vec3f(l.ev + k * l.temp + ti * l.tint, l.ev - tg * l.tint, l.ev - tb * l.temp + ti * l.tint);
    return p * exp2(gain);
}

// gradient::Local::display
fn local_display(l: Local, c0: vec3f) -> vec3f {
    if l.contrast == 0.0 && l.highlights == 0.0 && l.shadows == 0.0 && l.whites == 0.0 && l.blacks == 0.0 && l.saturation == 0.0 { return c0; }
    var c = c0;
    let y = max(0.2627 * c.x + 0.678 * c.y + 0.0593 * c.z, 0.0);
    if y > 1e-6 && (l.contrast != 0.0 || l.highlights != 0.0 || l.shadows != 0.0 || l.whites != 0.0 || l.blacks != 0.0) {
        var v = min(powf(y, 1.0 / 2.4), 1.2);
        if l.contrast != 0.0 {
            let d = v - P(K_LOCAL_CONTRAST);
            var k = P(K_LOCAL_CONTRAST + 2u);
            if l.contrast > 0.0 { k = P(K_LOCAL_CONTRAST + 1u); }
            v += l.contrast * k * d * (1.0 - min(abs(d) * P(K_LOCAL_CONTRAST + 3u), 1.0));
        }
        if l.highlights != 0.0 {
            let w = grad_smooth((v - P(K_LOCAL_HIGHLIGHTS)) / P(K_LOCAL_HIGHLIGHTS + 1u));
            var k = P(K_LOCAL_HIGHLIGHTS + 3u);
            var room = v;
            if l.highlights > 0.0 { k = P(K_LOCAL_HIGHLIGHTS + 2u); room = max(1.0 - v, 0.0); }
            v += l.highlights * k * w * room;
        }
        if l.shadows != 0.0 {
            let w = 1.0 - grad_smooth((v - P(K_LOCAL_SHADOWS)) / P(K_LOCAL_SHADOWS + 1u));
            var k = P(K_LOCAL_SHADOWS + 3u);
            var room = v;
            if l.shadows > 0.0 { k = P(K_LOCAL_SHADOWS + 2u); room = max(1.0 - v, 0.0); }
            v += l.shadows * k * w * room;
        }
        if l.whites != 0.0 {
            if l.whites > 0.0 {
                v += l.whites * P(K_LOCAL_WHITES) * v * grad_smooth((P(K_LOCAL_WHITES + 1u) - v) / P(K_LOCAL_WHITES + 2u));
            } else {
                v += l.whites * P(K_LOCAL_WHITES + 3u) * v;
            }
        }
        if l.blacks != 0.0 {
            var k = P(K_LOCAL_BLACKS + 1u);
            if l.blacks > 0.0 { k = P(K_LOCAL_BLACKS); }
            v += l.blacks * k * max(1.0 - v, 0.0);
        }
        let k = powf(max(v, 0.0), 2.4) / y;
        c = c * k;
    }
    if l.saturation != 0.0 {
        let yy = 0.2627 * c.x + 0.678 * c.y + 0.0593 * c.z;
        var k = P(K_LOCAL_SATURATION + 1u);
        if l.saturation > 0.0 { k = P(K_LOCAL_SATURATION); }
        let s = max(1.0 + l.saturation * k, 0.0);
        c = yy + (c - yy) * s;
    }
    return c;
}
