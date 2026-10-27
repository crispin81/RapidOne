//! Texture, Clarity and Dehaze on the GPU (`tools::apply_definition`, step for step): the toned
//! picture stays on the GPU from the tone pass to the finish pass. What is measured on small copies
//! (Clarity's networks on at most 512 / 1000 pixels, Dehaze's haze measures) is the CPU's own code,
//! run on what the GPU hands back; the full-size work (blurs, the per-pixel changes) is the GPU's.

use super::{DefU, Engine, Job, Output, Pass};
use crate::tools::{DefinitionMeasured, DefinitionPart, DehazeMeasured, MapGrid, ToolSettings, ToolTuning};

/// Whether the GPU does these settings' Texture, Clarity and Dehaze (else the CPU does, between
/// the GPU's tone and finish passes): the networks Clarity uses by default, not the older models.
pub fn covers(s: &ToolSettings, t: &ToolTuning) -> bool {
    (s.clarity <= 0.0 || t.clar_disp > 0.5 || t.clar_net > 0.5) && (s.clarity >= 0.0 || t.clar_net > 0.5)
}

/// A render with Texture, Clarity or Dehaze, all on the GPU: the finished picture, and what the
/// whole photo measured (for a part rendered later), or None (render it on the CPU).
pub fn render_definition(job: &Job, s: &ToolSettings, t: &ToolTuning, source_long: usize, part: DefinitionPart) -> Option<(Output, Option<DefinitionMeasured>)> {
    if super::mode() == super::RenderMode::Cpu || !covers(s, t) || super::job_unsupported(job).is_some() {
        return None;
    }
    let _p = crate::prof::span("  GPU render with Definition");
    match run(job, s, t, source_long, part) {
        Ok(out) => {
            super::note(true, None);
            Some(out)
        }
        Err(why) => {
            eprintln!("[gpu] rendering on the CPU: {why}");
            super::note(false, Some(format!("Rendered on the CPU: {why}")));
            None
        }
    }
}


/// Pixels a box pass's invocation runs along (a row or column in stretches, so a long one keeps
/// the whole GPU busy).
const BOX_STRETCH: usize = 512;

/// The pieces of a Definition render on one engine.
struct Def<'a> {
    e: &'a Engine,
    prm: wgpu::Buffer,
    w: usize,
    h: usize,
    n: usize,
    /// Two planes the row box passes go through turned on their side (reused: the passes run in
    /// order).
    turned: std::cell::OnceCell<[wgpu::Buffer; 2]>,
}

impl Def<'_> {
    fn u(&self) -> DefU {
        DefU { w: self.w as u32, h: self.h as u32, n: self.n as u32, ..Default::default() }
    }

    fn plane(&self, label: &str) -> wgpu::Buffer {
        self.e.buffer(label, (self.n * 4) as u64)
    }

    /// Planes nothing recorded later reads, for later steps to use again (`Engine::give_back`).
    fn done<const K: usize>(&self, planes: [wgpu::Buffer; K]) {
        for b in planes {
            self.e.give_back(b);
        }
    }

    fn k(&self, enc: &mut wgpu::CommandEncoder, name: &str, count: usize, u: DefU, ins: [Option<&wgpu::Buffer>; 3], outs: [Option<&wgpu::Buffer>; 3]) {
        self.e.kernel(enc, name, count, u, &self.prm, ins, outs);
    }

    /// tools::blur: three box passes each way (radius `r`), into a new plane.
    fn blur(&self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, r: usize) -> wgpu::Buffer {
        let b = self.plane("blur");
        if r == 0 {
            enc.copy_buffer_to_buffer(src, 0, &b, 0, (self.n * 4) as u64);
            return b;
        }
        let a = self.plane("blur pass");
        self.box_pass(enc, src, &a, r, true);
        self.box_pass(enc, &a, &b, r, false);
        for _ in 1..3 {
            self.box_pass(enc, &b, &a, r, true);
            self.box_pass(enc, &a, &b, r, false);
        }
        self.done([a]);
        b
    }

    /// One box pass (radius `r`) along the rows or down the columns, in stretches of `BOX_STRETCH`.
    /// Along the rows the plane is turned on its side, passed down its columns and turned back: the
    /// same sums in the same order, but neighbouring invocations read neighbouring memory (24 MP, M1
    /// Max: 8.2 ms -> about 3).
    fn box_pass(&self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, dst: &wgpu::Buffer, r: usize, rows: bool) {
        let u = DefU { r: r as u32, i: [BOX_STRETCH as u32, 0, 0, 0, 0, 0, 0, 0], ..self.u() };
        if rows {
            let [a, b] = self.turned.get_or_init(|| [self.plane("turned"), self.plane("turned box")]);
            let side = DefU { w: self.h as u32, h: self.w as u32, ..u };
            let tiles = |w: usize, h: usize| w.div_ceil(16) * h.div_ceil(16) * 64;
            self.k(enc, "transpose", tiles(self.w, self.h), u, [Some(src), None, None], [Some(a), None, None]);
            self.k(enc, "box_v", self.h * self.w.div_ceil(BOX_STRETCH), side, [Some(a), None, None], [Some(b), None, None]);
            self.k(enc, "transpose", tiles(self.h, self.w), side, [Some(b), None, None], [Some(dst), None, None]);
        } else {
            self.k(enc, "box_v", self.w * self.h.div_ceil(BOX_STRETCH), u, [Some(src), None, None], [Some(dst), None, None]);
        }
    }

    /// tools::min_filter: the minimum over a (2r + 1)-square window, cut at the edges. Along the rows
    /// then down the columns; a row pass is a column pass of the plane turned on its side.
    fn min_filter(&self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, r: usize) -> wgpu::Buffer {
        let [a, b] = self.turned.get_or_init(|| [self.plane("turned"), self.plane("turned box")]);
        let u = DefU { r: r as u32, ..self.u() };
        let side = DefU { w: self.h as u32, h: self.w as u32, ..u };
        let tiles = |w: usize, h: usize| w.div_ceil(16) * h.div_ceil(16) * 64;
        let down = |enc: &mut wgpu::CommandEncoder, u: DefU, src: &wgpu::Buffer, dst: &wgpu::Buffer| {
            let (w, h) = (u.w as usize, u.h as usize);
            let starts = self.plane("min from block starts");
            let ends = self.plane("min to block ends");
            self.k(enc, "min_runs", w * h.div_ceil(2 * r + 1), u, [Some(src), None, None], [Some(&starts), Some(&ends), None]);
            self.k(enc, "min_join", self.n, u, [Some(&starts), Some(&ends), None], [Some(dst), None, None]);
            self.done([starts, ends]);
        };
        self.k(enc, "transpose", tiles(self.w, self.h), u, [Some(src), None, None], [Some(a), None, None]);
        down(enc, side, a, b);
        let rows = self.plane("min rows");
        self.k(enc, "transpose", tiles(self.h, self.w), side, [Some(b), None, None], [Some(&rows), None, None]);
        let out = self.plane("eroded");
        down(enc, u, &rows, &out);
        self.done([rows]);
        out
    }

    /// tools::blur_f: the two nearest whole radii, blended.
    fn blur_f(&self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, r: f32) -> wgpu::Buffer {
        let lo = r.floor().max(0.0);
        let a = self.blur(enc, src, lo as usize);
        let f = r - lo;
        if f < 0.02 {
            return a;
        }
        let b = self.blur(enc, src, lo as usize + 1);
        let out = self.plane("blur_f");
        self.k(enc, "blend", self.n, DefU { f: [f, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..self.u() }, [Some(&a), Some(&b), None], [Some(&out), None, None]);
        self.done([a, b]);
        out
    }

    /// A MapGrid read at every pixel (where the pixel sits in the whole photo: `frame`).
    fn grid_at(&self, enc: &mut wgpu::CommandEncoder, grid: &MapGrid, frame: Option<&crate::pipeline::Frame>) -> wgpu::Buffer {
        let g = self.e.storage("grid", bytemuck::cast_slice(&grid.v));
        let out = self.plane("grid at");
        let mut u = DefU { i: [grid.w as u32, grid.h as u32, 0, 0, 0, 0, 0, 0], ..self.u() };
        if let Some(f) = frame {
            u.i[2] = 1;
            u.f[..4].copy_from_slice(&[f.x0, f.y0, f.full_w, f.full_h]);
        }
        self.k(enc, "grid_at", self.n, u, [Some(&g), None, None], [Some(&out), None, None]);
        out
    }

    /// Block averages of a plane by `f` (MapGrid::of, the networks' small copies), read back.
    fn small(&self, enc: wgpu::CommandEncoder, src: &wgpu::Buffer, f: usize, clamp: bool) -> Result<(Vec<f32>, usize, usize), String> {
        let mut enc = enc;
        let (sw, sh) = (self.w.div_ceil(f), self.h.div_ceil(f));
        let out = self.e.buffer("small", (sw * sh * 4) as u64);
        self.k(&mut enc, "block_avg", sw * sh, DefU { i: [f as u32, sw as u32, sh as u32, clamp as u32, 0, 0, 0, 0], ..self.u() }, [Some(src), None, None], [Some(&out), None, None]);
        let v = super::floats_of(&self.e.read(enc, &out, (sw * sh * 4) as u64)?);
        Ok((v, sw, sh))
    }
}

/// Clarity's U-Net weights on the GPU: every layer's weights and biases in one buffer, and where.
pub(crate) struct UnetWeights {
    buf: wgpu::Buffer,
    /// (weights, biases) for e0 (2), downs (5), encs (5), ups (5 x 2), out; and g1, g2.
    convs: Vec<(u32, u32)>,
    lins: [(u32, u32); 2],
}

impl UnetWeights {
    pub(crate) fn size(&self) -> u64 {
        self.buf.size()
    }

    fn new(e: &Engine) -> UnetWeights {
        let net = crate::clarity_unet::shared();
        let mut data = Vec::new();
        let mut put = |w: &[f32], b: &[f32]| {
            let at = (data.len() as u32, (data.len() + w.len()) as u32);
            data.extend_from_slice(w);
            data.extend_from_slice(b);
            at
        };
        let mut convs = Vec::new();
        for c in net.e0.iter().chain(&net.downs).chain(&net.encs).chain(net.ups.iter().flatten()).chain(std::iter::once(&net.out)) {
            if c.cout % 8 == 0 {
                // Blocks of eight output channels, each input's eight weights together (`unet_conv`).
                let kk = c.k * c.k;
                let mut w = Vec::with_capacity(c.w.len());
                for cb in 0..c.cout / 8 {
                    for ci in 0..c.cin {
                        for q in 0..kk {
                            for j in 0..8 {
                                w.push(c.w[((cb * 8 + j) * c.cin + ci) * kk + q]);
                            }
                        }
                    }
                }
                convs.push(put(&w, &c.b));
            } else {
                convs.push(put(&c.w, &c.b));
            }
        }
        let lins = [put(&net.g1.w, &net.g1.b), put(&net.g2.w, &net.g2.b)];
        UnetWeights { buf: e.storage("U-Net weights", bytemuck::cast_slice(&data)), convs, lins }
    }
}

/// A U-Net tensor on the GPU: planar channels of h x w.
struct Tensor {
    buf: wgpu::Buffer,
    c: usize,
    h: usize,
    w: usize,
}

impl Def<'_> {
    /// clarity_unet::conv
    fn conv(&self, enc: &mut wgpu::CommandEncoder, x: &Tensor, cv: &crate::clarity_unet::Conv, at: (u32, u32), relu: bool, weights: &wgpu::Buffer) -> Tensor {
        let (k, s) = (cv.k, cv.stride);
        let pad = k / 2;
        let (oh, ow) = ((x.h + 2 * pad - k) / s + 1, (x.w + 2 * pad - k) / s + 1);
        let n = cv.cout * oh * ow;
        let out = self.e.buffer("U-Net", (n * 4) as u64);
        // An invocation for each place and eight output channels (each value, for the last layer).
        let (kernel, jobs) = if cv.cout % 8 == 0 { ("unet_conv", cv.cout / 8 * oh * ow) } else { ("unet_conv1", n) };
        let u = DefU { w: x.w as u32, h: x.h as u32, r: s as u32, n: jobs as u32, i: [cv.cin as u32, cv.cout as u32, k as u32, relu as u32, oh as u32, ow as u32, at.0, at.1], f: [0.0; 8] };
        self.k(enc, kernel, jobs, u, [Some(&x.buf), Some(weights), None], [Some(&out), None, None]);
        Tensor { buf: out, c: cv.cout, h: oh, w: ow }
    }

    /// clarity_unet::ln_gain on the GPU: the network's ln gain for each pixel of the small copy
    /// (`small`, w x h), as a plane.
    fn unet(&self, enc: &mut wgpu::CommandEncoder, weights: &UnetWeights, small: &[f32], w: usize, h: usize) -> wgpu::Buffer {
        let net = crate::clarity_unet::shared();
        let (inp, pw, ph) = crate::clarity_unet::net_input(small, w, h);
        let wb = &weights.buf;
        let x = Tensor { buf: self.e.storage("U-Net input", bytemuck::cast_slice(&inp)), c: 2, h: ph, w: pw };
        let at = |i: usize| weights.convs[i];
        let a = self.conv(enc, &x, &net.e0[0], at(0), true, wb);
        let mut skips = vec![self.conv(enc, &a, &net.e0[1], at(1), true, wb)];
        for i in 0..5 {
            let t = self.conv(enc, skips.last().unwrap(), &net.downs[i], at(2 + i), true, wb);
            let e = self.conv(enc, &t, &net.encs[i], at(7 + i), true, wb);
            skips.push(e);
        }
        let mut hcur = skips.pop().unwrap();
        // The bottleneck: each channel's mean through two small layers, added back.
        let mean = self.e.buffer("U-Net mean", (hcur.c * 4) as u64);
        self.k(enc, "unet_mean", hcur.c, DefU { w: hcur.w as u32, h: hcur.h as u32, i: [hcur.c as u32, 0, 0, 0, 0, 0, 0, 0], ..DefU::default() }, [Some(&hcur.buf), None, None], [Some(&mean), None, None]);
        let lin = |enc: &mut wgpu::CommandEncoder, v: &wgpu::Buffer, l: &crate::clarity_unet::Lin, at: (u32, u32), relu: bool| {
            let out = self.e.buffer("U-Net lin", (l.n_out * 4) as u64);
            self.k(enc, "unet_lin", l.n_out, DefU { i: [l.n_in as u32, l.n_out as u32, 0, relu as u32, 0, 0, at.0, at.1], ..DefU::default() }, [Some(v), Some(wb), None], [Some(&out), None, None]);
            out
        };
        let g1 = lin(enc, &mean, &net.g1, weights.lins[0], true);
        let g2 = lin(enc, &g1, &net.g2, weights.lins[1], false);
        let n = hcur.c * hcur.h * hcur.w;
        self.k(enc, "unet_add", n, DefU { w: hcur.w as u32, h: hcur.h as u32, n: n as u32, ..DefU::default() }, [Some(&g2), None, None], [Some(&hcur.buf), None, None]);
        for i in (0..5).rev() {
            let skip = &skips[i];
            let plane = skip.h * skip.w;
            let cat = self.e.buffer("U-Net concat", ((hcur.c + skip.c) * plane * 4) as u64);
            let up_n = hcur.c * plane;
            let u = DefU { w: hcur.w as u32, h: hcur.h as u32, n: up_n as u32, i: [hcur.c as u32, 0, 0, 0, skip.h as u32, skip.w as u32, 0, 0], ..DefU::default() };
            self.k(enc, "unet_up", up_n, u, [Some(&hcur.buf), None, None], [Some(&cat), None, None]);
            enc.copy_buffer_to_buffer(&skip.buf, 0, &cat, (up_n * 4) as u64, (skip.c * plane * 4) as u64);
            let t = Tensor { buf: cat, c: hcur.c + skip.c, h: skip.h, w: skip.w };
            let first = self.conv(enc, &t, &net.ups[i][0], at(12 + i * 2), true, wb);
            hcur = self.conv(enc, &first, &net.ups[i][1], at(13 + i * 2), true, wb);
        }
        let o = self.conv(enc, &hcur, &net.out, at(22), false, wb);
        let out = self.e.buffer("U-Net out", (w * h * 4) as u64);
        self.k(enc, "unet_out", w * h, DefU { w: w as u32, h: h as u32, n: (w * h) as u32, i: [pw as u32, 0, 0, 0, 0, 0, 0, 0], ..DefU::default() }, [Some(&o.buf), None, None], [Some(&out), None, None]);
        out
    }
}

impl Def<'_> {
    /// tools::box_mean: one box pass each way.
    fn box_mean(&self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, r: usize) -> wgpu::Buffer {
        let a = self.plane("box");
        let b = self.plane("box mean");
        self.box_pass(enc, src, &a, r, true);
        self.box_pass(enc, &a, &b, r, false);
        self.done([a]);
        b
    }

    /// tools::guided_filter
    fn guided(&self, enc: &mut wgpu::CommandEncoder, guide: &wgpu::Buffer, input: &wgpu::Buffer, r: usize, eps: f32) -> wgpu::Buffer {
        let n = self.n;
        let mi = self.box_mean(enc, guide, r);
        let mp = self.box_mean(enc, input, r);
        let ii = self.plane("guide squared");
        self.k(enc, "mul", n, self.u(), [Some(guide), Some(guide), None], [Some(&ii), None, None]);
        let ip = self.plane("guide x input");
        self.k(enc, "mul", n, self.u(), [Some(guide), Some(input), None], [Some(&ip), None, None]);
        let mii = self.box_mean(enc, &ii, r);
        let mip = self.box_mean(enc, &ip, r);
        let both = self.e.buffer("means", (2 * n * 4) as u64);
        enc.copy_buffer_to_buffer(&mii, 0, &both, 0, (n * 4) as u64);
        enc.copy_buffer_to_buffer(&mip, 0, &both, (n * 4) as u64, (n * 4) as u64);
        let a = self.plane("a");
        let b = self.plane("b");
        self.k(enc, "gf_ab", n, DefU { f: [eps, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..self.u() }, [Some(&mi), Some(&mp), Some(&both)], [Some(&a), Some(&b), None]);
        self.done([mi, mp, ii, ip, mii, mip, both]);
        let ma = self.box_mean(enc, &a, r);
        let mb = self.box_mean(enc, &b, r);
        let out = self.plane("guided");
        self.k(enc, "gf_out", n, self.u(), [Some(&ma), Some(&mb), Some(guide)], [Some(&out), None, None]);
        self.done([a, b, ma, mb]);
        out
    }

    /// tools::dehaze_measure on the GPU: the measures, and the haze map at full size. The median
    /// brightness and the haziest pixels are picked on the CPU (in the CPU's order) from samples.
    fn dehaze_measure(&self, enc: &mut wgpu::CommandEncoder, toned: &wgpu::Buffer, t: &ToolTuning) -> Result<(DehazeMeasured, wgpu::Buffer), String> {
        let _p = crate::prof::span("    Dehaze measures (GPU)");
        let (w, h, n) = (self.w, self.h, self.n);
        let lum = self.plane("encoded luminance");
        let dark = self.plane("darkest channel");
        self.k(enc, "dz_planes", n, self.u(), [Some(toned), None, None], [Some(&lum), Some(&dark), None]);
        let every = |enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, k: usize| {
            let c = (n + k - 1) / k;
            let out = self.e.buffer("samples", (c * 4) as u64);
            self.k(enc, "gather_step", c, DefU { n: c as u32, i: [k as u32, 0, 0, 0, 0, 0, 0, 0], ..self.u() }, [Some(src), None, None], [Some(&out), None, None]);
            (out, c)
        };
        let (lum7, c7) = every(enc, &lum, 7);
        let (dark7, _) = every(enc, &dark, 7);
        let dcb = self.blur(enc, &dark, (w.max(h) / 60).max(1));
        let (dc3, c3) = every(enc, &dcb, 3);
        let lum7 = super::floats_of(&self.e.read(std::mem::replace(enc, self.e.encoder()), &lum7, (c7 * 4) as u64)?);
        let dark7 = super::floats_of(&self.e.read(self.e.encoder(), &dark7, (c7 * 4) as u64)?);
        let dc3 = super::floats_of(&self.e.read(self.e.encoder(), &dc3, (c3 * 4) as u64)?);
        self.done([dark, dcb]);
        // The photo's brightness: median encoded luminance and mean darkest channel (every 7th pixel).
        let mut sorted = lum7;
        {
            use rayon::prelude::*;
            sorted.par_sort_by(|a, b| a.total_cmp(b));
        }
        let nu = sorted[sorted.len() / 2];
        let delta = (dark7.iter().map(|&v| v as f64).sum::<f64>() / dark7.len() as f64) as f32;
        // The haze colour: the haziest `dz_a_frac` of every 3rd pixel (as a stable sort by the
        // blurred darkest channel, highest first, would order them), their encoded colours averaged.
        let take = ((c3 as f32 * t.dz_a_frac) as usize).max(1).min(c3);
        let mut vals = dc3.clone();
        vals.select_nth_unstable_by(take - 1, |a, b| b.total_cmp(a));
        let kth = vals[take - 1];
        let mut chosen: Vec<usize> = (0..c3).filter(|&j| dc3[j].total_cmp(&kth) == std::cmp::Ordering::Greater).collect();
        for j in 0..c3 {
            if chosen.len() >= take {
                break;
            }
            if dc3[j].total_cmp(&kth) == std::cmp::Ordering::Equal {
                chosen.push(j);
            }
        }
        chosen.sort_by(|&a, &b| dc3[b].total_cmp(&dc3[a]).then(a.cmp(&b)));
        let idx: Vec<usize> = chosen.iter().map(|&j| j * 3).collect();
        let split: Vec<f32> = idx.iter().flat_map(|&i| [(i / 4096) as f32, (i % 4096) as f32]).collect();
        let idx_buf = self.e.storage("haziest", bytemuck::cast_slice(&split));
        let cols = self.e.buffer("haziest colours", (idx.len() * 12) as u64);
        let mut e2 = self.e.encoder();
        self.k(&mut e2, "gather_enc", idx.len(), DefU { n: idx.len() as u32, ..self.u() }, [Some(toned), Some(&idx_buf), None], [Some(&cols), None, None]);
        let cols = super::floats_of(&self.e.read(e2, &cols, (idx.len() * 12) as u64)?);
        let mut av = [0.0f32; 3];
        for c in cols.chunks(3) {
            for k in 0..3 {
                av[k] += c[k] / take as f32;
            }
        }
        // The dark-channel map against A, eroded, smoothed.
        let long = w.max(h) as f32;
        let dcn = self.plane("dark channel");
        self.k(enc, "dz_dcn", n, DefU { f: [av[0], av[1], av[2], 0.0, 0.0, 0.0, 0.0, 0.0], ..self.u() }, [Some(toned), None, None], [Some(&dcn), None, None]);
        let r = ((t.dz_patch * long).round() as usize).max(1);
        let eroded = self.min_filter(enc, &dcn, r);
        self.done([dcn]);
        let rs = (t.dz_smooth * long).max(1.0);
        let blurred = (t.dz_gf < 1.0).then(|| self.blur_f(enc, &eroded, rs));
        let smooth = if t.dz_gf > 0.0 {
            let g = self.guided(enc, &lum, &eroded, rs.round() as usize, t.dz_eps);
            match blurred {
                Some(b) if t.dz_gf < 1.0 => {
                    let out = self.plane("haze map");
                    self.k(enc, "blend", n, DefU { f: [t.dz_gf, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..self.u() }, [Some(&b), Some(&g), None], [Some(&out), None, None]);
                    self.done([b, g]);
                    out
                }
                _ => g,
            }
        } else {
            blurred.unwrap()
        };
        self.done([eroded, lum]);
        if (t.dz_dpow - 1.0).abs() > 1e-3 {
            self.k(enc, "dz_pow", n, self.u(), [None, None, None], [Some(&smooth), None, None]);
        }
        let parts = n.div_ceil(1024);
        let sums = self.e.buffer("sums", (parts * 4) as u64);
        self.k(enc, "sum1024", parts, self.u(), [Some(&smooth), None, None], [Some(&sums), None, None]);
        let sums = super::floats_of(&self.e.read(std::mem::replace(enc, self.e.encoder()), &sums, (parts * 4) as u64)?);
        let mean_dc = (sums.iter().map(|&v| v as f64).sum::<f64>() / n.max(1) as f64).max(1e-3) as f32;
        let f = w.max(h).div_ceil(800).max(1);
        let (v, gw, gh) = self.small(std::mem::replace(enc, self.e.encoder()), &smooth, f, false)?;
        Ok((DehazeMeasured { nu, delta, av, mean_dc, smooth: MapGrid { v, w: gw, h: gh }, full: None }, smooth))
    }
}

fn run(job: &Job, s: &ToolSettings, t: &ToolTuning, source_long: usize, part: DefinitionPart) -> Result<(Output, Option<DefinitionMeasured>), String> {
    let (params, lut) = super::params::build(job);
    let header = params.header();
    super::with_engine(&header, |engine| {
        let (w, h) = (job.scene.width, job.scene.height);
        let n = w * h;
        // Whole pictures are bound here (the main passes go in strips): too large for this GPU, and
        // Texture / Clarity / Dehaze run on the CPU between the GPU's passes instead.
        if (n * 12) as u64 > engine.max_buffer.min(engine.max_binding) {
            return Err(format!("the picture ({n} pixels) is larger than the GPU can take whole"));
        }
        let scene = {
            let _p = crate::prof::span("  GPU upload scene");
            engine.scene(job.scene)
        };
        let mask_clarity = job.grads.is_some_and(|g| g.list.iter().any(|m| m.clarity > 0.0));
        if (s.clarity > 0.0 || mask_clarity) && t.clar_disp > 0.5 && engine.unet.is_none() {
            engine.unet = Some(UnetWeights::new(engine));
        }
        let engine = &*engine;
        let d = Def { e: engine, prm: engine.params(&params), w, h, n, turned: Default::default() };
        let lut_buf = engine.storage("lut", bytemuck::cast_slice(&lut));
        let offs = job.tone_map.filter(|m| !m.offset.is_empty()).map(|m| engine.storage("offsets", bytemuck::cast_slice(&m.offset)));
        let toned = engine.buffer("toned", (n * 12) as u64);
        let mut enc = engine.encoder();
        engine.encode_pass(&mut enc, Pass::Tone, (n, w, h), &d.prm, &lut_buf, &scene, offs.as_ref(), &toned, 3);
        // The sliders' Texture / Clarity / Dehaze, then the masks' (as render_core_q does them).
        let mut measured = if crate::tools::definition_active(s) { Some(d.definition(&mut enc, &toned, s, t, source_long, &part)?) } else { None };
        // What was measured here, for later renders (the sliders' when measured now, the masks').
        measured = measured.filter(|_| part.measured.is_none());
        if job.grads.is_some_and(|g| g.has_definition()) {
            let masks = d.local_definition(&mut enc, &toned, &lut_buf, t, source_long, &part)?;
            if !masks.is_empty() {
                measured.get_or_insert_with(Default::default).masks = masks;
            }
        }

        // The display tools and sRGB.
        let words = if job.sixteen { 2 } else { 1 };
        let out = engine.buffer("out", (n * words * 4) as u64);
        engine.encode_pass(&mut enc, Pass::Finish, (n, w, h), &d.prm, &lut_buf, &toned, None, &out, words);
        let out = match job.detail {
            Some(dt) => super::detail::encode(engine, &mut enc, &out, (n, w, h), job.sixteen, dt),
            None => out,
        };
        let back = engine.read(enc, &out, (n * words * 4) as u64)?;
        if job.detail.is_some() {
            super::DETAIL_DONE.with(|d| d.set(true));
        }
        Ok((super::unpack(&back, Pass::Finish, job.sixteen), measured))
    })
}

impl Def<'_> {
    /// tools::apply_definition on the GPU: Texture, Clarity and Dehaze (`s`) on the toned picture
    /// `toned` (three floats a pixel), in place. Returns what it measured (empty where `part` gave it).
    #[allow(clippy::too_many_arguments)]
    fn definition(&self, enc: &mut wgpu::CommandEncoder, toned: &wgpu::Buffer, s: &ToolSettings, t: &ToolTuning, source_long: usize, part: &DefinitionPart) -> Result<DefinitionMeasured, String> {
    let mut measured = DefinitionMeasured::default();
    let frame = part.frame;
    let (w, h, n) = (self.w, self.h, self.n);

    // Clarity+ on the display picture: the U-Net's gain on the luminance.
    let mut clarity = s.clarity;
    if s.clarity > 0.0 && t.clar_disp > 0.5 {
        let lum = self.plane("lum");
        self.k(enc, "lum", n, self.u(), [Some(toned), None, None], [Some(&lum), None, None]);
        let g = match part.measured.and_then(|m| m.clar_gain.as_ref()) {
            Some(grid) => self.grid_at(enc, grid, frame),
            None => {
                let f = w.max(h).div_ceil(crate::clarity_unet::NET_LONG).max(1);
                let (small, sw, sh) = self.small(std::mem::replace(enc, self.e.encoder()), &lum, f, false)?;
                let gsb = self.unet(enc, self.e.unet.as_ref().unwrap(), &small, sw, sh);
                let g = self.plane("gain");
                if f == 1 {
                    enc.copy_buffer_to_buffer(&gsb, 0, &g, 0, (n * 4) as u64);
                } else {
                    self.k(enc, "upsample_unet", n, DefU { i: [sw as u32, sh as u32, f as u32, 0, 0, 0, 0, 0], ..self.u() }, [Some(&gsb), None, None], [Some(&g), None, None]);
                }
                let f2 = w.max(h).div_ceil(crate::clarity_unet::NET_LONG).max(1);
                let (v, gw, gh) = self.small(std::mem::replace(enc, self.e.encoder()), &g, f2, false)?;
                measured.clar_gain = Some(MapGrid { v, w: gw, h: gh });
                g
            }
        };
        let a = (s.clarity / 100.0).powf(t.clar_pow);
        let amp: f32 = std::env::var("CLARDISP_AMP").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let tone: f32 = std::env::var("CLARDISP_TONE").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        self.k(enc, "clarity_disp", n, DefU { f: [a, amp, tone, 0.0, 0.0, 0.0, 0.0, 0.0], ..self.u() }, [Some(&g), Some(&lum), None], [Some(toned), None, None]);
        self.done([g, lum]);
        clarity = 0.0;
    }

    // Dehaze: the CPU's measures (of the whole photo, or as measured with it), applied here.
    if s.dehaze != 0.0 {
        let whole = part.measured.and_then(|m| m.dehaze.as_ref());
        let (m, smooth): (DehazeMeasured, wgpu::Buffer) = match whole {
            Some(m) => (m.clone(), self.grid_at(enc, &m.smooth, frame)),
            None => self.dehaze_measure(enc, toned, t)?,
        };
        let f = [s.dehaze / 100.0, m.nu, m.delta, m.av[0], m.av[1], m.av[2], m.mean_dc, 0.0];
        self.k(enc, "dehaze_apply", n, DefU { f, ..self.u() }, [Some(&smooth), None, None], [Some(toned), None, None]);
        self.done([smooth]);
        if part.measured.is_none() {
            measured.dehaze = Some(DehazeMeasured { full: None, ..m });
        }
    }

    // OKLab, and the lightness the rest changes.
    let lab = self.e.buffer("lab", (n * 12) as u64);
    let l = self.plane("L");
    let a_plane = self.plane("a");
    self.k(enc, "lab_of", n, self.u(), [Some(toned), None, None], [Some(&lab), Some(&l), Some(&a_plane)]);
    let new_l = self.plane("new L");
    enc.copy_buffer_to_buffer(&l, 0, &new_l, 0, (n * 4) as u64);

    // Texture: detail against a blur at a fixed size in the photo's own pixels.
    let long = frame.map_or(w.max(h) as f32, |f| f.full_w.max(f.full_h));
    let shrink = long / source_long.max(1) as f32;
    let mut tex = None;
    if s.texture != 0.0 {
        let r = (t.tex_radius * shrink).max(1.0);
        let b_plane = self.plane("b");
        self.k(enc, "plane_of", n, DefU { i: [2, 0, 0, 0, 0, 0, 0, 0], ..self.u() }, [Some(&lab), None, None], [Some(&b_plane), None, None]);
        let detail = |enc: &mut wgpu::CommandEncoder, v: &wgpu::Buffer| {
            let bl = self.blur_f(enc, v, r);
            let out = self.plane("detail");
            self.k(enc, "blend", n, DefU { i: [1, 0, 0, 0, 0, 0, 0, 0], ..self.u() }, [Some(v), Some(&bl), None], [Some(&out), None, None]);
            self.done([bl]);
            out
        };
        let (hl, ha, hb) = (detail(enc, &l), detail(enc, &a_plane), detail(enc, &b_plane));
        let a = (s.texture.abs() / 100.0).powf(t.tex_pow) * s.texture.signum() * if s.texture < 0.0 { t.tex_minus } else { 1.0 };
        let out = self.e.buffer("texture", (n * 12) as u64);
        self.k(enc, "texture", n, DefU { f: [a, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..self.u() }, [Some(&hl), Some(&ha), Some(&hb)], [Some(&lab), Some(&out), None]);
        self.done([hl, ha, hb, b_plane]);
        tex = Some(out);
    }

    // Clarity from the lightness networks (+ when the display network is off, and -).
    let net = if clarity > 0.0 && t.clar_net > 0.5 {
        Some((crate::tools::clarity_net_positive(), (clarity / 100.0).powf(t.clar_pow)))
    } else if clarity < 0.0 && t.clar_net > 0.5 {
        Some((crate::tools::clarity_net_negative(), (-clarity / 100.0).powf(t.clar_n_pow)))
    } else {
        None
    };
    if let Some((weights, a)) = net {
        let change = match part.measured.and_then(|m| m.clar_change.as_ref()) {
            Some(grid) => self.grid_at(enc, grid, frame),
            None => {
                let lum = self.plane("lum");
                self.k(enc, "lum", n, self.u(), [Some(toned), None, None], [Some(&lum), None, None]);
                let f = crate::tools::clarity_net_factor(w, h);
                let (ysm, lw, lh) = self.small(std::mem::replace(enc, self.e.encoder()), &lum, f, true)?;
                let small = {
                    let _p = crate::prof::span("    Clarity network");
                    crate::tools::clarity_net_small(&ysm, lw, lh, &weights)
                };
                let sb = self.e.storage("change small", bytemuck::cast_slice(&small));
                let change = self.plane("change");
                if f == 1 {
                    enc.copy_buffer_to_buffer(&sb, 0, &change, 0, (n * 4) as u64);
                } else {
                    self.k(enc, "upsample_net", n, DefU { i: [lw as u32, lh as u32, f as u32, 0, 0, 0, 0, 0], ..self.u() }, [Some(&sb), None, None], [Some(&change), None, None]);
                }
                let f2 = w.max(h).div_ceil(1000).max(1);
                let (v, gw, gh) = self.small(std::mem::replace(enc, self.e.encoder()), &change, f2, false)?;
                measured.clar_change = Some(MapGrid { v, w: gw, h: gh });
                change
            }
        };
        self.k(enc, "clarity_net", n, DefU { f: [a, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..self.u() }, [Some(&change), None, None], [Some(&new_l), None, None]);
        self.done([change]);
    }

    // The new lightness and Texture into the colour, back to the toned picture.
    let fin = DefU { i: [tex.is_some() as u32, 0, 0, 0, 0, 0, 0, 0], f: [s.dehaze / 100.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..self.u() };
    self.k(enc, "finish_lab", n, fin, [Some(&new_l), tex.as_ref(), None], [Some(&lab), Some(toned), None]);
        Ok(measured)
    }

    /// render_core_q's masks with Texture, Clarity or Dehaze: each tool, each way, run over a copy of
    /// the picture at the strongest amount the masks reach, and faded in pixel by pixel by the masks'
    /// amount there (`toned` changed in place).
    #[allow(clippy::too_many_arguments)]
    /// Each reuses what it measured on the whole photo when `part` has it; returns what was measured
    /// here instead (`DefinitionMeasured::masks`).
    fn local_definition(&self, enc: &mut wgpu::CommandEncoder, toned: &wgpu::Buffer, lut: &wgpu::Buffer, t: &ToolTuning, source_long: usize, part: &DefinitionPart) -> Result<Vec<(usize, DefinitionMeasured)>, String> {
        let (w, h, n) = (self.w, self.h, self.n);
        let amounts = self.e.buffer("mask amounts", (n * 12) as u64);
        self.e.encode_pass(enc, Pass::Amounts, (n, w, h), &self.prm, lut, toned, None, &amounts, 3);
        let parts = n.div_ceil(1024);
        let peaks = self.e.buffer("mask peaks", (parts * 24) as u64);
        self.k(enc, "peaks1024", parts, self.u(), [Some(&amounts), None, None], [Some(&peaks), None, None]);
        let peaks = super::floats_of(&self.e.read(std::mem::replace(enc, self.e.encoder()), &peaks, (parts * 24) as u64)?);
        let mut measured = Vec::new();
        for tool in 0..3 {
            for (way, sign) in [1.0f32, -1.0].into_iter().enumerate() {
                let peak = peaks.chunks(6).map(|p| p[tool * 2 + way]).fold(0.0f32, f32::max);
                if peak < 0.5 {
                    continue;
                }
                let amount = (peak * sign).clamp(-100.0, 100.0);
                let one = ToolSettings {
                    texture: if tool == 0 { amount } else { 0.0 },
                    clarity: if tool == 1 { amount } else { 0.0 },
                    dehaze: if tool == 2 { amount } else { 0.0 },
                    ..Default::default()
                };
                if !covers(&one, t) {
                    return Err("this Clarity model inside a mask".into());
                }
                let done = self.e.buffer("mask tool", (n * 12) as u64);
                enc.copy_buffer_to_buffer(toned, 0, &done, 0, (n * 12) as u64);
                let key = tool * 2 + way;
                let reuse = part.measured.and_then(|m| m.mask(key, &one, t));
                let m = self.definition(enc, &done, &one, t, source_long, &DefinitionPart { frame: part.frame, measured: reuse })?;
                if reuse.is_none() {
                    measured.push((key, m));
                }
                let u = DefU { i: [tool as u32, 0, 0, 0, 0, 0, 0, 0], f: [sign, peak, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..self.u() };
                self.k(enc, "local_blend", n, u, [Some(&done), Some(&amounts), None], [Some(toned), None, None]);
                self.done([done]);
            }
        }
        self.done([amounts]);
        Ok(measured)
    }
}
