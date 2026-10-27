// ---- detail.rs: sharpening (Richardson-Lucy) and grain on the finished picture ------------------
// The picture comes as packed words (8-bit: one RGBA word a pixel; 16-bit: two words), as the main
// shader writes it; `i0` is 1 for 16-bit. The Gaussian's taps (computed in Rust) are in `taps`.

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
@group(0) @binding(1) var<storage, read> taps: array<f32>;
@group(0) @binding(2) var<storage, read> fin0: array<f32>;
@group(0) @binding(3) var<storage, read> uin: array<u32>;
@group(0) @binding(4) var<storage, read> fin2: array<f32>;
@group(0) @binding(5) var<storage, read_write> fout0: array<f32>;
@group(0) @binding(6) var<storage, read_write> fout1: array<f32>;
@group(0) @binding(7) var<storage, read_write> uout: array<u32>;

fn index(gid: vec3u, nwg: vec3u) -> u32 { return gid.x + gid.y * nwg.x * 64u; }

fn full_scale() -> f32 {
    if u.i0 == 1u { return 65535.0; }
    return 255.0;
}

// A pixel's three channels as stored (0..255 or 0..65535).
fn stored(i: u32) -> vec3f {
    if u.i0 == 1u {
        let a = uin[i * 2u];
        let b = uin[i * 2u + 1u];
        return vec3f(f32(a & 0xffffu), f32(a >> 16u), f32(b & 0xffffu));
    }
    let v = uin[i];
    return vec3f(f32(v & 0xffu), f32((v >> 8u) & 0xffu), f32((v >> 16u) & 0xffu));
}

// detail::Channel::from_f32 for each channel, packed.
fn store(i: u32, c: vec3f) {
    let m = full_scale();
    // Rounded half away from zero, as Rust's f32::round (WGSL's round() goes to even).
    let q = vec3u(clamp(floor(c + 0.5), vec3f(0.0), vec3f(m)));
    if u.i0 == 1u {
        uout[i * 2u] = q.x | (q.y << 16u);
        uout[i * 2u + 1u] = q.z;
    } else {
        uout[i] = q.x | (q.y << 8u) | (q.z << 16u);
    }
}

fn pw(x: f32, y: f32) -> f32 {
    if x <= 0.0 { return 0.0; }
    return pow(x, y);
}

// detail::srgb_to_lin / lin_to_srgb
fn srgb_to_lin(v: f32) -> f32 {
    if v <= 0.04045 { return v / 12.92; }
    return pw((v + 0.055) / 1.055, 2.4);
}

fn lin_to_srgb(v0: f32) -> f32 {
    let v = max(v0, 0.0);
    if v <= 0.0031308 { return v * 12.92; }
    return 1.055 * pw(v, 1.0 / 2.4) - 0.055;
}

// sharpen_rl's start: linear colour (fout0, three a pixel) and its brightness (fout1).
@compute @workgroup_size(64)
fn dt_unpack(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let s = stored(i) / full_scale();
    let l = vec3f(srgb_to_lin(s.x), srgb_to_lin(s.y), srgb_to_lin(s.z));
    fout0[i * 3u] = l.x;
    fout0[i * 3u + 1u] = l.y;
    fout0[i * 3u + 2u] = l.z;
    fout1[i] = 0.2126 * l.x + 0.7152 * l.y + 0.0722 * l.z;
}

// detail::gaussian_rows' `mirror`
fn mirror(i: i32, n: u32) -> u32 {
    let m = i32(n);
    var j = i;
    if i < 0 { j = -i; } else if i >= m { j = 2 * m - 2 - i; }
    return u32(clamp(j, 0, m - 1));
}

// gaussian_rows' horizontal pass of fin0 into fout0 (radius r, the taps' count 2r + 1).
@compute @workgroup_size(64)
fn dt_gauss_h(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let x = i % u.w;
    let y = i / u.w;
    let r = i32(u.r);
    let base = y * u.w;
    let ntaps = 2u * u.r + 1u;
    var acc: f32;
    if u.w <= 2u * u.r || x < u.r || x >= u.w - u.r {
        // At the edges (mirrored): the taps' products summed in order.
        acc = taps[0] * fin0[base + mirror(i32(x) - r, u.w)];
        for (var j = 1u; j < ntaps; j++) { acc += taps[j] * fin0[base + mirror(i32(x) + i32(j) - r, u.w)]; }
    } else {
        acc = taps[0] * fin0[base + x - u.r];
        for (var j = 1u; j < ntaps; j++) { acc += taps[j] * fin0[base + x - u.r + j]; }
    }
    fout0[i] = acc;
}

// gaussian_rows' vertical pass of the horizontal one (fin0), and what sharpen_rl does with each blurred
// value: `i1` 0, ratio = brightness (fin2) / max(blur, 1e-5) into fout0; `i1` 1, fout0 x= blur.
@compute @workgroup_size(64)
fn dt_gauss_v(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let x = i % u.w;
    let y = i / u.w;
    let r = i32(u.r);
    let ntaps = 2u * u.r + 1u;
    var acc = 0.0;
    for (var j = 0u; j < ntaps; j++) { acc += taps[j] * fin0[mirror(i32(y) - r + i32(j), u.h) * u.w + x]; }
    if u.i1 == 0u {
        fout0[i] = fin2[i] / max(acc, 1e-5);
    } else {
        fout0[i] = fout0[i] * acc;
    }
}

// sharpen_rl's end: the change (u, fin0, against the brightness, fin2) held within the 3x3 range
// (halo limit f0), gain f1, on the linear colour (fout0), back to sRGB and stored (uout).
@compute @workgroup_size(64)
fn dt_final(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let x = i32(i % u.w);
    let yy = i32(i / u.w);
    var lo = 3.4e38;
    var hi = -3.4e38;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let nx = u32(clamp(x + dx, 0, i32(u.w) - 1));
            let ny = u32(clamp(yy + dy, 0, i32(u.h) - 1));
            let v = fin2[ny * u.w + nx];
            lo = min(lo, v);
            hi = max(hi, v);
        }
    }
    let limit = u.f0 * (hi - lo);
    let yv = fin2[i];
    let d = clamp(fin0[i] - yv, -limit, limit);
    var k = 1.0;
    if yv > 1e-6 { k = max((yv + u.f1 * d) / yv, 0.0); }
    let l = vec3f(fout0[i * 3u], fout0[i * 3u + 1u], fout0[i * 3u + 2u]) * k;
    store(i, vec3f(lin_to_srgb(l.x), lin_to_srgb(l.y), lin_to_srgb(l.z)) * full_scale());
}

// detail::hash_noise for every pixel, into fout0.
@compute @workgroup_size(64)
fn dt_noise(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let x = i % u.w;
    let y = i / u.w;
    var n = (x * 0x9E3779B1u) ^ (y * 0x85EBCA77u);
    n ^= n >> 15u;
    n = n * 0x2C1B3C6Du;
    n ^= n >> 12u;
    n = n * 0x297A2D39u;
    n ^= n >> 15u;
    fout0[i] = (f32(n) / 4294967295.0) * 2.0 - 1.0;
}

// detail::blur's 3x3 box, one direction (`i1` 0: along rows, 1: down columns), fin0 into fout0.
@compute @workgroup_size(64)
fn dt_box3(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let x = i % u.w;
    let y = i / u.w;
    if u.i1 == 0u {
        let row = y * u.w;
        fout0[i] = (fin0[row + max(x, 1u) - 1u] + fin0[i] + fin0[row + min(x + 1u, u.w - 1u)]) / 3.0;
    } else {
        fout0[i] = (fin0[(max(y, 1u) - 1u) * u.w + x] + fin0[i] + fin0[min(y + 1u, u.h - 1u) * u.w + x]) / 3.0;
    }
}

// detail::apply's grain: the softened noise (fin0) at strength f0 (grain / 100 x its scale), with
// the mid-tone share f1 + f2 x mid, moving every channel of the stored picture (uin) by the change
// in its brightness (Rec. 709, of the stored values), into uout.
@compute @workgroup_size(64)
fn dt_grain(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = index(gid, nwg);
    if i >= u.n { return; }
    let p = stored(i);
    let m = full_scale();
    let y0 = (0.2126 * p.x + 0.7152 * p.y + 0.0722 * p.z) / m;
    let yc = clamp(y0, 0.0, 1.0);
    let mid = 4.0 * yc * (1.0 - yc);
    let y = y0 + fin0[i] * u.f0 * (u.f1 + u.f2 * mid);
    let d = (y - y0) * m;
    if d == 0.0 {
        store(i, p);
    } else {
        store(i, p + d);
    }
}
