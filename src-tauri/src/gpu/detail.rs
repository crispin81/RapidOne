//! Sharpening and grain (`crate::detail::apply`) on the GPU, on the finished picture before it's read
//! back: Richardson-Lucy's twenty rounds of Gaussian blurs, the halo limit, and the grain, as the CPU
//! does them (the same taps, from `detail::gaussian_taps`; the same constants).

use super::{DefU, Detail, Engine};

/// The kernels (`shaders/detail.wgsl`).
pub(crate) const KERNELS: [&str; 7] = ["dt_unpack", "dt_gauss_h", "dt_gauss_v", "dt_final", "dt_noise", "dt_box3", "dt_grain"];

/// The finished picture (packed, as the main shader writes it) sharpened and grained: a new buffer.
pub(crate) fn encode(e: &Engine, enc: &mut wgpu::CommandEncoder, packed: &wgpu::Buffer, dims: (usize, usize, usize), sixteen: bool, d: Detail) -> wgpu::Buffer {
    let (n, w, h) = dims;
    let words = if sixteen { 2 } else { 1 };
    let u = DefU { w: w as u32, h: h as u32, n: n as u32, i: [sixteen as u32, 0, 0, 0, 0, 0, 0, 0], ..DefU::default() };
    let plane = |label| e.buffer(label, (n * 4) as u64);
    let mut cur = packed.clone();
    if d.sharpen > 0.0 && w >= 3 && h >= 3 {
        use crate::detail::{RL_G, RL_HALO, RL_MIN_SIGMA, RL_P, RL_ROUNDS, RL_SIGMA};
        let amount = d.sharpen.min(150.0);
        let sigma = (RL_SIGMA * d.scale).max(RL_MIN_SIGMA);
        let gain = RL_G * (amount / 100.0).powf(RL_P);
        let (taps, r) = crate::detail::gaussian_taps(sigma);
        let taps = e.storage("taps", bytemuck::cast_slice(&taps));
        let lin = e.buffer("linear", (n * 12) as u64);
        let y = plane("brightness");
        e.kernel(enc, "dt_unpack", n, u, &taps, [None, Some(&cur), None], [Some(&lin), Some(&y), None]);
        let ru = DefU { r: r as u32, ..u };
        let est = plane("estimate");
        enc.copy_buffer_to_buffer(&y, 0, &est, 0, (n * 4) as u64);
        let across = plane("across");
        let ratio = plane("ratio");
        // The rounds in one compute pass (eighty small dispatches).
        let times = DefU { i: [sixteen as u32, 1, 0, 0, 0, 0, 0, 0], ..ru };
        let round = [
            e.bound("dt_gauss_h", n, ru, &taps, [Some(&est), None, None], [Some(&across), None, None]),
            e.bound("dt_gauss_v", n, ru, &taps, [Some(&across), None, Some(&y)], [Some(&ratio), None, None]),
            e.bound("dt_gauss_h", n, ru, &taps, [Some(&ratio), None, None], [Some(&across), None, None]),
            e.bound("dt_gauss_v", n, times, &taps, [Some(&across), None, None], [Some(&est), None, None]),
        ];
        let list: Vec<_> = (0..RL_ROUNDS).flat_map(|_| round.iter().cloned()).collect();
        e.kernels(enc, &list);
        let out = e.buffer("sharpened", (n * words * 4) as u64);
        let fin = DefU { f: [RL_HALO, gain, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..u };
        e.kernel(enc, "dt_final", n, fin, &taps, [Some(&est), None, Some(&y)], [Some(&lin), None, Some(&out)]);
        cur = out;
    }
    if d.grain > 0.0 {
        let none = e.storage("no taps", bytemuck::cast_slice(&[0f32; 4]));
        let noise = plane("noise");
        e.kernel(enc, "dt_noise", n, u, &none, [None, None, None], [Some(&noise), None, None]);
        let rows = plane("noise rows");
        e.kernel(enc, "dt_box3", n, u, &none, [Some(&noise), None, None], [Some(&rows), None, None]);
        let soft = plane("noise soft");
        e.kernel(enc, "dt_box3", n, DefU { i: [sixteen as u32, 1, 0, 0, 0, 0, 0, 0], ..u }, &none, [Some(&rows), None, None], [Some(&soft), None, None]);
        let out = e.buffer("grained", (n * words * 4) as u64);
        let g = DefU { f: [d.grain / 100.0 * crate::detail::GRAIN_AMOUNT, crate::detail::GRAIN_MID[0], crate::detail::GRAIN_MID[1], 0.0, 0.0, 0.0, 0.0, 0.0], ..u };
        e.kernel(enc, "dt_grain", n, g, &none, [Some(&soft), Some(&cur), None], [None, None, Some(&out)]);
        cur = out;
    }
    cur
}
