// ---- tools.rs: Texture, Clarity and Dehaze (`apply_definition`) on the GPU --------------------------
// Whole-picture kernels on planes (one float a pixel) and on the toned picture (three a pixel). The
// parts measured on small pictures (Clarity's networks, Dehaze's haze measures) are the CPU's; their
// results come here as planes and numbers. `prm` holds the tuning by name, as for the main shader.

struct U {
    w: u32,
    h: u32,
    r: u32,
    n: u32,
    i0: u32,
    i1: u32,
    i2: u32,
    i3: u32,
    i4: u32,
    i5: u32,
    i6: u32,
    i7: u32,
    f0: f32,
    f1: f32,
    f2: f32,
    f3: f32,
    f4: f32,
    f5: f32,
    f6: f32,
    f7: f32,
}

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> prm: array<f32>;
@group(0) @binding(2) var<storage, read> in0: array<f32>;
@group(0) @binding(3) var<storage, read> in1: array<f32>;
@group(0) @binding(4) var<storage, read> in2: array<f32>;
@group(0) @binding(5) var<storage, read_write> out0: array<f32>;
@group(0) @binding(6) var<storage, read_write> out1: array<f32>;
@group(0) @binding(7) var<storage, read_write> out2: array<f32>;

fn P(o: u32) -> f32 { return prm[o]; }
fn P3(o: u32) -> vec3f { return vec3f(prm[o], prm[o + 1u], prm[o + 2u]); }

struct M3 { r0: vec3f, r1: vec3f, r2: vec3f }
fn M(o: u32) -> M3 { return M3(P3(o), P3(o + 3u), P3(o + 6u)); }
fn ap3(m: M3, v: vec3f) -> vec3f {
    return vec3f(
        m.r0.x * v.x + m.r0.y * v.y + m.r0.z * v.z,
        m.r1.x * v.x + m.r1.y * v.y + m.r1.z * v.z,
        m.r2.x * v.x + m.r2.y * v.y + m.r2.z * v.z,
    );
}
fn cbrt(x: f32) -> f32 {
    if x == 0.0 { return 0.0; }
    return sign(x) * pow(abs(x), 1.0 / 3.0);
}
fn powf(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        if y == 0.0 { return 1.0; }
        return 0.0;
    }
    return pow(x, y);
}
fn to_lab(rgb: vec3f) -> vec3f {
    let l = ap3(M(K_OK_TO_LMS), rgb);
    return ap3(M(K_OK_LMS_TO_LAB), vec3f(cbrt(l.x), cbrt(l.y), cbrt(l.z)));
}
fn to_rgb(lab: vec3f) -> vec3f { let l = ap3(M(K_OK_LAB_TO_LMS), lab); return ap3(M(K_OK_FROM_LMS), l * l * l); }

fn rgb_at(i: u32) -> vec3f { return vec3f(out0[i * 3u], out0[i * 3u + 1u], out0[i * 3u + 2u]); }
fn put_rgb(i: u32, c: vec3f) {
    out0[i * 3u] = c.x;
    out0[i * 3u + 1u] = c.y;
    out0[i * 3u + 2u] = c.z;
}

fn index(gid: vec3u, nwg: vec3u) -> u32 { return gid.x + gid.y * nwg.x * 64u; }

// The luminance of the toned picture (in0), max 0 (`apply_definition`'s `lum`, Y of XYZ).
@compute @workgroup_size(64)
fn lum(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let y = P3(K_Y_ROW);
    let c = vec3f(in0[i * 3u], in0[i * 3u + 1u], in0[i * 3u + 2u]);
    out0[i] = max(y.x * c.x + y.y * c.y + y.z * c.z, 0.0);
}

// Block averages of a plane (w x h) by `i0` (the output is `i1` x `i2`), each value clamped to 0..1
// first when `i3` is 1: clarity_unet::gain_map's and clarity_net_change's small copies, MapGrid::of.
@compute @workgroup_size(64)
fn block_avg(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    let sw = u.i1;
    if i >= sw * u.i2 { return; }
    let f = u.i0;
    let x = i % sw;
    let y = i / sw;
    var s = 0.0;
    var n = 0.0;
    for (var yy = y * f; yy < min((y + 1u) * f, u.h); yy++) {
        for (var xx = x * f; xx < min((x + 1u) * f, u.w); xx++) {
            var v = in0[yy * u.w + xx];
            if u.i3 == 1u { v = clamp(v, 0.0, 1.0); }
            s += v;
            n += 1.0;
        }
    }
    out0[i] = s / max(n, 1.0);
}

// clarity_unet::gain_map's stretch of the network's small output (in0, `i0` x `i1`, factor `i2`) to
// full size, sample positions at the block centres.
@compute @workgroup_size(64)
fn upsample_unet(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let sw = u.i0;
    let sh = u.i1;
    let f = f32(u.i2);
    let ox = i % u.w;
    let oy = i / u.w;
    let sx = max((f32(ox) + 0.5) / f - 0.5, 0.0);
    let x0 = min(u32(floor(sx)), sw - 1u);
    let x1 = min(x0 + 1u, sw - 1u);
    let fx = sx - f32(x0);
    let sy = max((f32(oy) + 0.5) / f - 0.5, 0.0);
    let y0 = min(u32(floor(sy)), sh - 1u);
    let y1 = min(y0 + 1u, sh - 1u);
    let fy = sy - f32(y0);
    let top = in0[y0 * sw + x0] * (1.0 - fx) + in0[y0 * sw + x1] * fx;
    let bot = in0[y1 * sw + x0] * (1.0 - fx) + in0[y1 * sw + x1] * fx;
    out0[i] = top * (1.0 - fy) + bot * fy;
}

// clarity_net_change's stretch of its small output (in0, `i0` x `i1`, factor `i2`) to full size.
@compute @workgroup_size(64)
fn upsample_net(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let lw = u.i0;
    let lh = u.i1;
    let f = f32(u.i2);
    let x = i % u.w;
    let y = i / u.w;
    let fx = (f32(x) + 0.5) / f - 0.5;
    let fy = (f32(y) + 0.5) / f - 0.5;
    let x0 = u32(max(floor(fx), 0.0));
    let y0 = u32(max(floor(fy), 0.0));
    let x1 = min(x0 + 1u, lw - 1u);
    let y1 = min(y0 + 1u, lh - 1u);
    let tx = clamp(fx - f32(x0), 0.0, 1.0);
    let ty = clamp(fy - f32(y0), 0.0, 1.0);
    let a = in0[y0 * lw + x0] * (1.0 - tx) + in0[y0 * lw + x1] * tx;
    let b = in0[y1 * lw + x0] * (1.0 - tx) + in0[y1 * lw + x1] * tx;
    out0[i] = a * (1.0 - ty) + b * ty;
}

// tools::MapGrid::at for every pixel (the grid in0, `i0` x `i1`); the pixel's place in the whole
// photo from the frame (f0..f3: x0, y0, full_w, full_h) when `i2` is 1.
@compute @workgroup_size(64)
fn grid_at(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let gw = u.i0;
    let gh = u.i1;
    var fxy = vec2f((f32(i % u.w) + 0.5) / f32(u.w), (f32(i / u.w) + 0.5) / f32(u.h));
    if u.i2 == 1u {
        fxy = vec2f((f32(i % u.w) + 0.5 + u.f0) / u.f2, (f32(i / u.w) + 0.5 + u.f1) / u.f3);
    }
    let x = clamp(fxy.x * f32(gw) - 0.5, 0.0, f32(gw - 1u));
    let y = clamp(fxy.y * f32(gh) - 0.5, 0.0, f32(gh - 1u));
    let x0 = u32(x);
    let y0 = u32(y);
    let x1 = min(x0 + 1u, gw - 1u);
    let y1 = min(y0 + 1u, gh - 1u);
    let tx = x - f32(x0);
    let ty = y - f32(y0);
    let a = in0[y0 * gw + x0] * (1.0 - tx) + in0[y0 * gw + x1] * tx;
    let b = in0[y1 * gw + x0] * (1.0 - tx) + in0[y1 * gw + x1] * tx;
    out0[i] = a * (1.0 - ty) + b * ty;
}

// tools::box_pass_down (and box_pass_into, on a plane turned on its side): one invocation for each
// stretch of i0 rows of a column, a running sum down it as the CPU's (each stretch's first window
// summed afresh; the CPU carries one sum down the whole column, so the last bits can differ).
@compute @workgroup_size(64)
fn box_v(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let t = index(gid, nwg);
    let segs = (u.h + u.i0 - 1u) / u.i0;
    if t >= u.w * segs { return; }
    // Neighbouring invocations take neighbouring columns (memory read in order).
    let x = t % u.w;
    let y0 = i32((t / u.w) * u.i0);
    let y1 = min(y0 + i32(u.i0), i32(u.h));
    let h = i32(u.h);
    let r = i32(u.r);
    var sum = 0.0;
    for (var i = y0 - r; i <= y0 + r; i++) { sum += in0[u32(clamp(i, 0, h - 1)) * u.w + x]; }
    let n = f32(2 * r + 1);
    for (var y = y0; y < y1; y++) {
        out0[u32(y) * u.w + x] = sum / n;
        sum += in0[u32(clamp(y + r + 1, 0, h - 1)) * u.w + x] - in0[u32(clamp(y - r, 0, h - 1)) * u.w + x];
    }
}

// The w x h plane in0 turned on its side (out0 is h wide, w high), 16 x 16 pixels a workgroup
// through workgroup memory, so both the reads and the writes go along rows. Dispatched as 64
// invocations for each tile.
var<workgroup> tile: array<f32, 272>;

@compute @workgroup_size(64)
fn transpose(@builtin(workgroup_id) wid: vec3u, @builtin(num_workgroups) nwg: vec3u, @builtin(local_invocation_index) li: u32) {
    let across = (u.w + 15u) / 16u;
    let t = wid.x + wid.y * nwg.x;
    let tx = (t % across) * 16u;
    let ty = (t / across) * 16u;
    let lx = li % 16u;
    let ly = li / 16u;
    // A row of the tile is 17 apart, so a column's reads fall in different banks.
    for (var k = 0u; k < 16u; k += 4u) {
        let x = tx + lx;
        let y = ty + ly + k;
        if x < u.w && y < u.h { tile[(ly + k) * 17u + lx] = in0[y * u.w + x]; }
    }
    workgroupBarrier();
    for (var k = 0u; k < 16u; k += 4u) {
        // Out row = in column (tx + ly + k), out column = in row (ty + lx).
        let x = ty + lx;
        let y = tx + ly + k;
        if x < u.h && y < u.w { out0[y * u.h + x] = tile[lx * 17u + ly + k]; }
    }
}

// out0 = in0 + (in1 - in0) x f0 (blur_f's blend); with `i0` 1, out0 = in0 - in1 (detail).
@compute @workgroup_size(64)
fn blend(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    if u.i0 == 1u {
        out0[i] = in0[i] - in1[i];
    } else {
        out0[i] = in0[i] + (in1[i] - in0[i]) * u.f0;
    }
}

// Clarity+ on the display picture (out0, in place): the network's gain (in0) on the luminance (in1),
// as apply_definition: a = f0, amp = f1, tone = f2; tone_correction's table at K_TONE_CORR.
@compute @workgroup_size(64)
fn clarity_disp(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let yy = clamp(in1[i], 0.0, 1.0);
    var l: f32;
    if yy > 0.008856 { l = 116.0 * cbrt(yy) - 16.0; } else { l = 903.3 * yy; }
    let t = clamp((l - 5.0) / 10.0, 0.0, 9.0);
    let ti = u32(floor(t));
    let tf = t - floor(t);
    let corr = P(K_TONE_CORR + ti) * (1.0 - tf) + P(K_TONE_CORR + min(ti + 1u, 9u)) * tf;
    let k = exp(u.f0 * (in0[i] * u.f1 + u.f2 * corr * 3.0 / (l + 16.0)));
    put_rgb(i, rgb_at(i) * k);
}

// tools::dehaze's per-pixel part (out0, in place) with the haze map `smooth` (in0): d = f0, nu = f1,
// delta = f2, A = (f3, f4, f5), mean_dc = f6.
@compute @workgroup_size(64)
fn dehaze_apply(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let g = P(TT_DZ_GAMMA);
    let d = u.f0;
    let av = vec3f(u.f3, u.f4, u.f5);
    let am = max((av.x + av.y + av.z) / 3.0, 1e-3);
    let ratio = av / am;
    let m = abs(d);
    let c = rgb_at(i);
    let e = vec3f(powf(max(c.x, 0.0), 1.0 / g), powf(max(c.y, 0.0), 1.0 / g), powf(max(c.z, 0.0), 1.0 / g));
    let dcv = in0[i];
    var out: vec3f;
    if d < 0.0 {
        let k = clamp(P(TT_DZ_M_AMT) * powf(m, P(TT_DZ_M_POW)) * (1.0 + P(TT_DZ_M_MED) * (u.f1 - 0.45) + P(TT_DZ_M_DARK) * (u.f2 - 0.3)), 0.0, 0.95);
        let kx = clamp(k * (1.0 + P(TT_DZ_M_LOC) * (dcv / u.f6 - 1.0)), 0.0, 0.95);
        for (var j = 0u; j < 3u; j++) {
            let hz = (1.0 - P(TT_DZ_M_A)) * P(TT_DZ_HAZE) * (1.0 + P(TT_DZ_M_COL) * (ratio[j] - 1.0)) + P(TT_DZ_M_A) * av[j];
            out[j] = powf(max(e[j] * (1.0 - kx) + hz * kx, 0.0), g);
        }
    } else {
        let amount = P(TT_DZ_W) * powf(m, P(TT_DZ_P_POW));
        let tr = clamp(1.0 - amount * clamp(dcv, 0.0, 1.0), P(TT_DZ_T0), 1.0);
        for (var j = 0u; j < 3u; j++) {
            let ap = am + P(TT_DZ_P_COL) * (av[j] - am);
            out[j] = powf(max(ap + (e[j] - ap) / tr, 0.0), g);
        }
    }
    put_rgb(i, out);
}

// The toned picture (in0) in OKLab: interleaved (out0), and its L (out1) and a (out2) as planes
// (Texture takes b from out0 with plane_of).
@compute @workgroup_size(64)
fn lab_of(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let c = vec3f(in0[i * 3u], in0[i * 3u + 1u], in0[i * 3u + 2u]);
    let lab = to_lab(max(c, vec3f(0.0)));
    out0[i * 3u] = lab.x;
    out0[i * 3u + 1u] = lab.y;
    out0[i * 3u + 2u] = lab.z;
    out1[i] = lab.x;
    out2[i] = lab.y;
}

// apply_definition's Texture change for each pixel (out1, three a pixel) from the detail planes
// (in0: L, in1: a, in2: b) and the lightness (in the interleaved OKLab, out0); a = f0.
@compute @workgroup_size(64)
fn texture(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let a = u.f0;
    let x = 100.0 * in0[i];
    var dk = 1.0;
    if x < 0.0 { dk = P(TT_TEX_DARK); }
    let y = sign(x) * log(1.0 + abs(x) / P(TT_TEX_KNEE)) * dk;
    let lstar = 100.0 * out0[i * 3u];
    let tone_l = (1.0 - P(TT_TEX_TONE_LO) * (1.0 - sm(0.0, 25.0, lstar))) * (1.0 - P(TT_TEX_TONE_HI) * sm(75.0, 100.0, lstar));
    let tone_c = 1.0 - P(TT_TEX_CHROMA_HI) * sm(75.0, 100.0, lstar);
    out1[i * 3u] = a * P(TT_TEX_AMOUNT) * y * tone_l / 100.0;
    out1[i * 3u + 1u] = tex_c(in1[i], a, tone_c);
    out1[i * 3u + 2u] = tex_c(in2[i], a, tone_c);
}

fn sm(e0: f32, e1: f32, v: f32) -> f32 {
    let t = clamp((v - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn tex_c(h: f32, a: f32, tone_c: f32) -> f32 {
    let x = P(K_AB_UNITS) * h;
    return a * P(TT_TEX_CHROMA) * tone_c * x / (1.0 + abs(x) / P(TT_TEX_CHROMA_KNEE)) / P(K_AB_UNITS);
}

// Clarity's lightness change from a network (in0, L*), into the new lightness (out0): + a x c / 116.
@compute @workgroup_size(64)
fn clarity_net(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    out0[i] = out0[i] + u.f0 * in0[i] / 116.0;
}

// apply_definition's end: the new lightness (in0), colour strength kept in step (d = f0) and the
// Texture change (in1, when `i0` is 1) on the OKLab colour (out0), then back to the toned picture
// (out1).
@compute @workgroup_size(64)
fn finish_lab(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let d = u.f0;
    var p = vec3f(out0[i * 3u], out0[i * 3u + 1u], out0[i * 3u + 2u]);
    let lv = in0[i];
    var chroma_k = 1.0;
    if d > 0.0 { chroma_k = 1.0 + P(TT_DEHAZE_SAT) * d; }
    var scale = 1.0;
    if p.x > 1e-4 { scale = max(lv / p.x, 0.0); }
    let cs = powf(scale, P(TT_CLAR_CEXP));
    p = vec3f(max(lv, 0.0), p.y * chroma_k * cs, p.z * chroma_k * cs);
    if u.i0 == 1u {
        p = vec3f(max(p.x + in1[i * 3u], 0.0), p.y + in1[i * 3u + 1u], p.z + in1[i * 3u + 2u]);
    }
    let c = max(to_rgb(p), vec3f(0.0));
    out1[i * 3u] = c.x;
    out1[i * 3u + 1u] = c.y;
    out1[i * 3u + 2u] = c.z;
}

// Channel `i0` of a picture three floats a pixel (in0), as a plane (out0).
@compute @workgroup_size(64)
fn plane_of(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    out0[i] = in0[i * 3u + u.i0];
}

// ---- clarity_unet.rs: the Clarity+ U-Net (`ln_gain`), layer by layer ----------------------------
// Tensors are planar (channel, row, column). The weights and biases of every layer are in in1.

// clarity_unet::conv: input in0 (i0 channels, h x w), output out0 (i1 channels, i4 x i5), kernel
// i2, stride r, ReLU when i3 is 1, biases at i7 in in1. One invocation computes eight output
// channels at one place, the weights at i6 laid out (block of eight channels, input channel, row,
// column, then the eight) so each input value meets its eight weights together. Each output adds its
// terms in the CPU's order (input channel, kernel row, column) onto its bias.
@compute @workgroup_size(64)
fn unet_conv(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let cin = u.i0;
    let k = u.i2;
    let oh = u.i4;
    let ow = u.i5;
    let place = i % (oh * ow);
    let cb = i / (oh * ow);
    let co0 = cb * 8u;
    let oy = place / ow;
    let ox = place % ow;
    let pad = i32(k / 2u);
    let s = i32(u.r);
    let bo = u.i7 + co0;
    var a0 = vec4f(in1[bo], in1[bo + 1u], in1[bo + 2u], in1[bo + 3u]);
    var a1 = vec4f(in1[bo + 4u], in1[bo + 5u], in1[bo + 6u], in1[bo + 7u]);
    let kk = k * k;
    for (var ci = 0u; ci < cin; ci++) {
        let plane = ci * u.h * u.w;
        let wci = u.i6 + ((cb * cin + ci) * kk) * 8u;
        for (var ky = 0u; ky < k; ky++) {
            let iy = i32(oy) * s + i32(ky) - pad;
            if iy < 0 || iy >= i32(u.h) { continue; }
            let row = plane + u32(iy) * u.w;
            for (var kx = 0u; kx < k; kx++) {
                let ix = i32(ox) * s + i32(kx) - pad;
                if ix < 0 || ix >= i32(u.w) { continue; }
                let v = in0[row + u32(ix)];
                let wo = wci + (ky * k + kx) * 8u;
                a0 += vec4f(in1[wo], in1[wo + 1u], in1[wo + 2u], in1[wo + 3u]) * v;
                a1 += vec4f(in1[wo + 4u], in1[wo + 5u], in1[wo + 6u], in1[wo + 7u]) * v;
            }
        }
    }
    if u.i3 == 1u {
        a0 = max(a0, vec4f(0.0));
        a1 = max(a1, vec4f(0.0));
    }
    let po = oh * ow;
    out0[co0 * po + place] = a0.x;
    out0[(co0 + 1u) * po + place] = a0.y;
    out0[(co0 + 2u) * po + place] = a0.z;
    out0[(co0 + 3u) * po + place] = a0.w;
    out0[(co0 + 4u) * po + place] = a1.x;
    out0[(co0 + 5u) * po + place] = a1.y;
    out0[(co0 + 6u) * po + place] = a1.z;
    out0[(co0 + 7u) * po + place] = a1.w;
}

// clarity_unet::conv for a layer whose outputs aren't a multiple of eight (the last, one channel):
// an invocation a value, the weights as the CPU's (output channel, input channel, row, column).
@compute @workgroup_size(64)
fn unet_conv1(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let cin = u.i0;
    let k = u.i2;
    let oh = u.i4;
    let ow = u.i5;
    let co = i / (oh * ow);
    let rem = i % (oh * ow);
    let oy = rem / ow;
    let ox = rem % ow;
    let pad = i32(k / 2u);
    let s = i32(u.r);
    var acc = in1[u.i7 + co];
    for (var ci = 0u; ci < cin; ci++) {
        let plane = ci * u.h * u.w;
        for (var ky = 0u; ky < k; ky++) {
            let iy = i32(oy) * s + i32(ky) - pad;
            if iy < 0 || iy >= i32(u.h) { continue; }
            for (var kx = 0u; kx < k; kx++) {
                let ix = i32(ox) * s + i32(kx) - pad;
                if ix < 0 || ix >= i32(u.w) { continue; }
                acc += in1[u.i6 + ((co * cin + ci) * k + ky) * k + kx] * in0[plane + u32(iy) * u.w + u32(ix)];
            }
        }
    }
    if u.i3 == 1u && acc < 0.0 { acc = 0.0; }
    out0[i] = acc;
}

// clarity_unet::upsample of in0 (i0 channels, h x w) to i4 x i5, into the first channels of out0
// (the concatenation with the skip, copied in after).
@compute @workgroup_size(64)
fn unet_up(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let oh = u.i4;
    let ow = u.i5;
    let c = i / (oh * ow);
    let rem = i % (oh * ow);
    let oy = rem / ow;
    let ox = rem % ow;
    let sy = max((f32(oy) + 0.5) * (f32(u.h) / f32(oh)) - 0.5, 0.0);
    let y0 = min(u32(floor(sy)), u.h - 1u);
    let y1 = min(y0 + 1u, u.h - 1u);
    let fy = sy - f32(y0);
    let sx = max((f32(ox) + 0.5) * (f32(u.w) / f32(ow)) - 0.5, 0.0);
    let x0 = min(u32(floor(sx)), u.w - 1u);
    let x1 = min(x0 + 1u, u.w - 1u);
    let fx = sx - f32(x0);
    let p = c * u.h * u.w;
    let top = in0[p + y0 * u.w + x0] * (1.0 - fx) + in0[p + y0 * u.w + x1] * fx;
    let bot = in0[p + y1 * u.w + x0] * (1.0 - fx) + in0[p + y1 * u.w + x1] * fx;
    out0[i] = top * (1.0 - fy) + bot * fy;
}

// The bottleneck's mean of each channel of in0 (i0 channels of h x w) into out0.
@compute @workgroup_size(64)
fn unet_mean(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let c = index(gid, nwg);
    if c >= u.i0 { return; }
    let plane = u.h * u.w;
    var s = 0.0;
    for (var j = 0u; j < plane; j++) { s += in0[c * plane + j]; }
    out0[c] = s / f32(plane);
}

// clarity_unet::lin: out0 (i1 values) from in0 (i0 values), weights at i6 and biases at i7 in in1,
// ReLU when i3 is 1.
@compute @workgroup_size(64)
fn unet_lin(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let o = index(gid, nwg);
    if o >= u.i1 { return; }
    var acc = 0.0;
    for (var j = 0u; j < u.i0; j++) { acc += in1[u.i6 + o * u.i0 + j] * in0[j]; }
    var s = in1[u.i7 + o] + acc;
    if u.i3 == 1u && s < 0.0 { s = 0.0; }
    out0[o] = s;
}

// The bottleneck's result (in0, one value a channel) added to every value of its channel in out0
// (h x w planes).
@compute @workgroup_size(64)
fn unet_add(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    out0[i] = out0[i] + in0[i / (u.h * u.w)];
}

// ln_gain's end: the output plane (in0, padded width i0) cropped to w x h, halved and held to +-3.
@compute @workgroup_size(64)
fn unet_out(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let x = i % u.w;
    let y = i / u.w;
    out0[i] = clamp(in0[y * u.i0 + x] * 0.5, -3.0, 3.0);
}

// ---- tools::dehaze_measure on the GPU ----------------------------------------------------------

fn dz_enc(v: f32) -> f32 { return powf(max(v, 0.0), 1.0 / P(TT_DZ_GAMMA)); }

fn enc_at(i: u32) -> vec3f {
    return vec3f(dz_enc(in0[i * 3u]), dz_enc(in0[i * 3u + 1u]), dz_enc(in0[i * 3u + 2u]));
}

// The toned picture (in0) encoded (`dz_gamma`): its luminance (out0) and darkest channel (out1).
@compute @workgroup_size(64)
fn dz_planes(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let e = enc_at(i);
    let k = P3(K_LUMA);
    out0[i] = k.x * e.x + k.y * e.y + k.z * e.z;
    out1[i] = min(e.x, min(e.y, e.z));
}

// Every i0-th value of in0 (from the first), into out0 (n of them).
@compute @workgroup_size(64)
fn gather_step(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let j = index(gid, nwg);
    if j >= u.n { return; }
    out0[j] = in0[j * u.i0];
}

// The encoded colour (three a value, out0) of the pixels whose indices are in in1, each as two
// exact floats (index / 4096 and index % 4096: integer bits in a float could be flushed as denormal).
@compute @workgroup_size(64)
fn gather_enc(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let j = index(gid, nwg);
    if j >= u.n { return; }
    let e = enc_at(u32(in1[j * 2u]) * 4096u + u32(in1[j * 2u + 1u]));
    out0[j * 3u] = e.x;
    out0[j * 3u + 1u] = e.y;
    out0[j * 3u + 2u] = e.z;
}

// The dark-channel map against the haze colour A (f0, f1, f2): the smallest of each encoded channel
// over A's.
@compute @workgroup_size(64)
fn dz_dcn(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let e = enc_at(i);
    out0[i] = min(e.x / max(u.f0, 1e-3), min(e.y / max(u.f1, 1e-3), e.z / max(u.f2, 1e-3)));
}

// tools::min_filter down the columns by van Herk / Gil-Werman: each column cut in blocks of 2r + 1,
// the minimum from each block's start (out0) and to its end (out1), one invocation a block of a
// column. A window then needs two of them (min_join): the same minimum, in a few reads a pixel.
@compute @workgroup_size(64)
fn min_runs(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let t = index(gid, nwg);
    let len = 2u * u.r + 1u;
    let blocks = (u.h + len - 1u) / len;
    if t >= u.w * blocks { return; }
    let x = t % u.w;
    let y0 = (t / u.w) * len;
    let y1 = min(y0 + len, u.h);
    var m = in0[y0 * u.w + x];
    for (var y = y0; y < y1; y++) {
        m = min(m, in0[y * u.w + x]);
        out0[y * u.w + x] = m;
    }
    m = in0[(y1 - 1u) * u.w + x];
    for (var y = y1; y > y0; y--) {
        m = min(m, in0[(y - 1u) * u.w + x]);
        out1[(y - 1u) * u.w + x] = m;
    }
}

// The window's minimum from min_runs' two (in0: from each block's start, in1: to its end).
@compute @workgroup_size(64)
fn min_join(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let x = i % u.w;
    let y = i / u.w;
    let len = 2u * u.r + 1u;
    let lo = select(0u, y - u.r, y >= u.r);
    let hi = min(y + u.r, u.h - 1u);
    let from_end = in1[lo * u.w + x];
    let from_start = in0[hi * u.w + x];
    if lo / len != hi / len {
        out0[i] = min(from_end, from_start);
    } else if lo % len == 0u {
        // The window starts its block (cut at the top).
        out0[i] = from_start;
    } else {
        // It ends its block (cut at the bottom, where the last block ends).
        out0[i] = from_end;
    }
}

// The masks' strongest amounts (in0: Texture, Clarity, Dehaze, three a pixel) over each 1024 pixels:
// out0 six a part, each tool up then down (render_core_q's `peak`, from 0).
@compute @workgroup_size(64)
fn peaks1024(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let p = index(gid, nwg);
    if p >= (u.n + 1023u) / 1024u { return; }
    var m = array<f32, 6>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (var i = p * 1024u; i < min(p * 1024u + 1024u, u.n); i++) {
        for (var t = 0u; t < 3u; t++) {
            let v = in0[i * 3u + t];
            m[t * 2u] = max(m[t * 2u], v);
            m[t * 2u + 1u] = max(m[t * 2u + 1u], -v);
        }
    }
    for (var j = 0u; j < 6u; j++) { out0[p * 6u + j] = m[j]; }
}

// render_core_q's masks: the toned picture (out0, three a pixel) towards one tool's result (in0) by
// the masks' amount of tool i0 (in1, three a pixel) the way f0 (1 or -1), over its peak f1.
@compute @workgroup_size(64)
fn local_blend(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let k = clamp(in1[i * 3u + u.i0] * u.f0 / u.f1, 0.0, 1.0);
    if k > 0.0 {
        for (var j = 0u; j < 3u; j++) {
            let c = out0[i * 3u + j];
            out0[i * 3u + j] = c + (in0[i * 3u + j] - c) * k;
        }
    }
}

// Elementwise: out0 = in0 x in1.
@compute @workgroup_size(64)
fn mul(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    out0[i] = in0[i] * in1[i];
}

// tools::guided_filter's a (out0) and b (out1) from the means of the guide (in0), the input (in1),
// and in2: the guide squared then guide x input, one plane after the other; eps = f0.
@compute @workgroup_size(64)
fn gf_ab(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let mi = in0[i];
    let mp = in1[i];
    let mii = in2[i];
    let mip = in2[u.n + i];
    let a = (mip - mi * mp) / (max(mii - mi * mi, 0.0) + u.f0);
    out0[i] = a;
    out1[i] = mp - a * mi;
}

// tools::guided_filter's end: out0 = mean a (in0) x guide (in2) + mean b (in1).
@compute @workgroup_size(64)
fn gf_out(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    out0[i] = in0[i] * in2[i] + in1[i];
}

// The haze map raised to `dz_dpow` (out0, in place), each value first held to 0..1.
@compute @workgroup_size(64)
fn dz_pow(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    out0[i] = powf(clamp(out0[i], 0.0, 1.0), P(TT_DZ_DPOW));
}

// Sums of 1024 values each of in0 (n values) into out0.
@compute @workgroup_size(64)
fn sum1024(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let j = index(gid, nwg);
    let start = j * 1024u;
    if start >= u.n { return; }
    var s = 0.0;
    for (var k = start; k < min(start + 1024u, u.n); k++) { s += in0[k]; }
    out0[j] = s;
}
