// The render engine on the GPU: the same steps as the CPU engine, function by function, each named
// after the Rust function it mirrors (pipeline.rs, tools.rs, dngcolour.rs). Every measured table,
// curve and fitted number comes in `prm` / `lut` as data from the Rust source (`gpu/params.rs`);
// the `T_*`, `TT_*`, `K_*`, `S_*` and `R_*` names are their places there, written above this file
// when the shader is built. Only the plain formula constants (sRGB, CIELAB, mid-grey 0.18) are text.

@group(0) @binding(0) var<storage, read> prm: array<f32>;
@group(0) @binding(1) var<storage, read> lut: array<f32>;
// Scene light, linear Rec.2020, three floats a pixel (or, for the finish pass, the toned colour).
@group(0) @binding(2) var<storage, read> src: array<f32>;
// The tone map's offset for each pixel (`ToneMap::offset`), or one unused value.
@group(0) @binding(3) var<storage, read> offs: array<f32>;
// The finished picture (8-bit: one RGBA word a pixel; 16-bit: two words), or the toned colour as
// three float words a pixel.
@group(0) @binding(4) var<storage, read_write> outp: array<u32>;
@group(0) @binding(5) var<uniform> job: Job;

struct Job {
    // First pixel of this strip in the whole picture, and how many pixels it has.
    base: u32,
    count: u32,
    // Pixels a row of the picture (for positions), and its height.
    width: u32,
    height: u32,
}

fn P(o: u32) -> f32 { return prm[o]; }
fn P2(o: u32) -> vec2f { return vec2f(P(o), P(o + 1u)); }
fn P3(o: u32) -> vec3f { return vec3f(P(o), P(o + 1u), P(o + 2u)); }
fn on(o: u32) -> bool { return P(o) > 0.5; }

// A 3x3 matrix by rows, as the Rust `[[f32; 3]; 3]`.
struct M3 { r0: vec3f, r1: vec3f, r2: vec3f }

fn M(o: u32) -> M3 { return M3(P3(o), P3(o + 3u), P3(o + 6u)); }

// pipeline::apply3
fn ap3(m: M3, v: vec3f) -> vec3f {
    return vec3f(
        m.r0.x * v.x + m.r0.y * v.y + m.r0.z * v.z,
        m.r1.x * v.x + m.r1.y * v.y + m.r1.z * v.z,
        m.r2.x * v.x + m.r2.y * v.y + m.r2.z * v.z,
    );
}

fn m3_mul(a: M3, b: M3) -> M3 {
    let c0 = vec3f(b.r0.x, b.r1.x, b.r2.x);
    let c1 = vec3f(b.r0.y, b.r1.y, b.r2.y);
    let c2 = vec3f(b.r0.z, b.r1.z, b.r2.z);
    return M3(
        vec3f(dot(a.r0, c0), dot(a.r0, c1), dot(a.r0, c2)),
        vec3f(dot(a.r1, c0), dot(a.r1, c1), dot(a.r1, c2)),
        vec3f(dot(a.r2, c0), dot(a.r2, c1), dot(a.r2, c2)),
    );
}

fn m3_t(m: M3) -> M3 {
    return M3(vec3f(m.r0.x, m.r1.x, m.r2.x), vec3f(m.r0.y, m.r1.y, m.r2.y), vec3f(m.r0.z, m.r1.z, m.r2.z));
}

// whitebalance::invert3
fn invert3(m: M3) -> M3 {
    let det = m.r0.x * (m.r1.y * m.r2.z - m.r1.z * m.r2.y) - m.r0.y * (m.r1.x * m.r2.z - m.r1.z * m.r2.x)
        + m.r0.z * (m.r1.x * m.r2.y - m.r1.y * m.r2.x);
    let inv = 1.0 / det;
    return M3(
        vec3f((m.r1.y * m.r2.z - m.r1.z * m.r2.y) * inv, (m.r0.z * m.r2.y - m.r0.y * m.r2.z) * inv, (m.r0.y * m.r1.z - m.r0.z * m.r1.y) * inv),
        vec3f((m.r1.z * m.r2.x - m.r1.x * m.r2.z) * inv, (m.r0.x * m.r2.z - m.r0.z * m.r2.x) * inv, (m.r0.z * m.r1.x - m.r0.x * m.r1.z) * inv),
        vec3f((m.r1.x * m.r2.y - m.r1.y * m.r2.x) * inv, (m.r0.y * m.r2.x - m.r0.x * m.r2.y) * inv, (m.r0.x * m.r1.y - m.r0.y * m.r1.x) * inv),
    );
}

const PI: f32 = 3.14159265358979323846;
const TAU: f32 = 6.28318530717958647692;

// f32::cbrt
fn cbrt(x: f32) -> f32 {
    if x == 0.0 { return 0.0; }
    return sign(x) * pow(abs(x), 1.0 / 3.0);
}

fn cbrt3(v: vec3f) -> vec3f { return vec3f(cbrt(v.x), cbrt(v.y), cbrt(v.z)); }

// f32::powf for a base that may be zero (pow(0, y) is undefined in WGSL).
fn powf(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        if y == 0.0 { return 1.0; }
        return 0.0;
    }
    return pow(x, y);
}

// f32::rem_euclid
fn rem_euclid(x: f32, m: f32) -> f32 { return x - m * floor(x / m); }

fn max3(c: vec3f) -> f32 { return max(c.x, max(c.y, c.z)); }
fn min3(c: vec3f) -> f32 { return min(c.x, min(c.y, c.z)); }
fn to_radians(d: f32) -> f32 { return d * (PI / 180.0); }
fn to_degrees(r: f32) -> f32 { return r * (180.0 / PI); }
fn signum(x: f32) -> f32 { if x < 0.0 { return -1.0; } return 1.0; }

// pipeline::Oklab
fn to_lab(rgb: vec3f) -> vec3f { return ap3(M(K_OK_LMS_TO_LAB), cbrt3(ap3(M(K_OK_TO_LMS), rgb))); }
fn to_rgb(lab: vec3f) -> vec3f { let l = ap3(M(K_OK_LAB_TO_LMS), lab); return ap3(M(K_OK_FROM_LMS), l * l * l); }

// pipeline::srgb_encode
fn srgb_encode(v0: f32) -> f32 {
    let v = clamp(v0, 0.0, 1.0);
    if v <= 0.0031308 { return 12.92 * v; }
    return 1.055 * powf(v, 1.0 / 2.4) - 0.055;
}

// dngcolour::srgb_decode
fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 { return v / 12.92; }
    return powf((v + 0.055) / 1.055, 2.4);
}

fn luma(c: vec3f) -> f32 { let k = P3(K_LUMA); return k.x * c.x + k.y * c.y + k.z * c.z; }

// dngcolour::Curve::eval: a 4096-point table at `o` in `lut`.
fn curve_eval(o: u32, x: f32) -> f32 {
    let n = 4096u;
    let f = clamp(x, 0.0, 1.0) * f32(n - 1u);
    let i = min(u32(f), n - 2u);
    let t = f - f32(i);
    return lut[o + i] + (lut[o + i + 1u] - lut[o + i]) * t;
}

// dngcolour::Curve::apply_rgb
fn curve_apply_rgb(o: u32, c0: vec3f) -> vec3f {
    let c = clamp(c0, vec3f(0.0), vec3f(1.0));
    var hi = 0u;
    var lo = 0u;
    for (var i = 1u; i < 3u; i++) {
        if c[i] > c[hi] { hi = i; }
        if c[i] < c[lo] { lo = i; }
    }
    if hi == lo {
        return vec3f(curve_eval(o, c.x), curve_eval(o, c.y), curve_eval(o, c.z));
    }
    let mid = 3u - hi - lo;
    let nh = curve_eval(o, c[hi]);
    let nl = curve_eval(o, c[lo]);
    var out = vec3f(0.0);
    out[hi] = nh;
    out[lo] = nl;
    out[mid] = nl + (nh - nl) * (c[mid] - c[lo]) / (c[hi] - c[lo]);
    return out;
}
