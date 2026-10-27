//! The render engine on the GPU (wgpu: Metal on a Mac, Vulkan on Linux, DirectX 12 or Vulkan on
//! Windows). The same per-pixel steps as the CPU engine (`pipeline::render_core_q` and the display
//! tools in `tools`), as compute shaders (`shaders/`), each named after the Rust function it mirrors.
//! Every table and fitted number reaches the shaders as data from the same Rust source (`params`).
//!
//! The CPU engine stays the reference and the complete fallback: `render` returns None whenever the
//! GPU can't do a render (no usable GPU, an error, a setting it doesn't cover yet), and the caller
//! renders it on the CPU. The Render setting (`set_mode`): Auto (the GPU if usable, else the CPU),
//! GPU (the same, but a fall back is reported in `status`), CPU. Until the app sets it, renders stay
//! on the CPU, so tests and the measuring tools are unchanged.
//!
//! GPU memory is let go when the engine has been idle a while (`IDLE_BUFFERS`, `IDLE_DEVICE`), so it
//! never sits on memory Remove's or Retouch's models want.

mod definition;
mod detail;
mod params;

use crate::pipeline::{Frame, RenderSettings, SceneImage, ToneCurve, ToneMap, Tuning};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where renders run (the Render setting).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderMode {
    /// The GPU if there is a usable one, else the CPU.
    Auto,
    /// The GPU; when it can't, the CPU (and `status` says so).
    Gpu,
    Cpu,
}

static MODE: AtomicU8 = AtomicU8::new(2);

pub fn set_mode(mode: RenderMode) {
    MODE.store(
        match mode {
            RenderMode::Auto => 0,
            RenderMode::Gpu => 1,
            RenderMode::Cpu => 2,
        },
        Ordering::Relaxed,
    );
    if mode == RenderMode::Cpu {
        // The device itself goes when idle (`IDLE_DEVICE`); switching back is then instant.
        release(false);
    } else {
        // A GPU that failed to start may work now (another try, once).
        *FAILED.lock().unwrap() = None;
    }
    let mut st = STATUS.lock().unwrap();
    st.mode = mode;
}

thread_local! {
    /// A Render setting for this thread only (tests: so a GPU test can't move the other tests,
    /// running alongside, onto the GPU).
    static THREAD_MODE: std::cell::Cell<Option<RenderMode>> = const { std::cell::Cell::new(None) };
}

/// Renders on this thread use `mode` (None: the Render setting).
pub fn set_thread_mode(mode: Option<RenderMode>) {
    THREAD_MODE.with(|m| m.set(mode));
}

pub fn mode() -> RenderMode {
    if let Some(m) = THREAD_MODE.with(|m| m.get()) {
        return m;
    }
    match MODE.load(Ordering::Relaxed) {
        0 => RenderMode::Auto,
        1 => RenderMode::Gpu,
        _ => RenderMode::Cpu,
    }
}

/// What the Render setting shows: the mode, the GPU found, and why the last render (if any) fell
/// back to the CPU.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub mode: RenderMode,
    /// The GPU's name, once one has been found.
    pub adapter: Option<String>,
    /// Whether the last render ran on the GPU.
    pub on_gpu: bool,
    /// Why the GPU wasn't used for the last render (no usable GPU, an error, a setting it doesn't do).
    pub note: Option<String>,
}

static STATUS: Mutex<Status> = Mutex::new(Status { mode: RenderMode::Cpu, adapter: None, on_gpu: false, note: None });

pub fn status() -> Status {
    STATUS.lock().unwrap().clone()
}

/// Told the new status when a render lands somewhere else than the last (GPU <-> CPU, or a new
/// reason): the title bar's GPU button follows along. Only on a change, so a slider drag's renders
/// cost nothing extra.
static ON_CHANGE: std::sync::OnceLock<Box<dyn Fn(Status) + Send + Sync>> = std::sync::OnceLock::new();

/// Set once, as the app starts.
pub fn on_change(f: impl Fn(Status) + Send + Sync + 'static) {
    let _ = ON_CHANGE.set(Box::new(f));
}

fn note(on_gpu: bool, why: Option<String>) {
    let mut st = STATUS.lock().unwrap();
    let was = (st.on_gpu, st.note.clone());
    st.on_gpu = on_gpu;
    if why.is_some() || on_gpu {
        st.note = why;
    }
    if was != (st.on_gpu, st.note.clone()) {
        let now = st.clone();
        drop(st);
        if let Some(f) = ON_CHANGE.get() {
            f(now);
        }
    }
}

/// Everything `render_core_q` has worked out for a render, for the GPU to do the same.
pub struct Job<'a> {
    pub scene: &'a SceneImage,
    /// The settings as rendered (the Mixer's Yellow Saturation extra added).
    pub settings: &'a RenderSettings,
    /// The tone sliders as the bands use them (`render_core_q`'s `bands`).
    pub bands: &'a RenderSettings,
    /// The tuning, with the photo's statistics where they're used.
    pub tuning: &'a Tuning,
    pub light: [[f32; 3]; 3],
    pub gain: f32,
    pub wb_gain: f32,
    pub camera: Option<&'a ([[f32; 3]; 3], [[f32; 3]; 3])>,
    pub curve: &'a ToneCurve,
    pub display_tools: &'a crate::tools::Prepared,
    /// `ColourAdjust`'s numbers (Shinobi's: Adobe's profile uses no look).
    pub has_look: bool,
    pub kc_full: f32,
    pub vibrance_v: f32,
    pub mix_scene: bool,
    pub mixer_moves: bool,
    pub frame: Option<Frame>,
    pub tone_map: Option<&'a ToneMap>,
    pub adobe: Option<&'a crate::dngcolour::ProfileRender>,
    /// Masks and gradients.
    pub grads: Option<&'a crate::gradient::Prepared<'a>>,
    /// 16 bits a channel (else 8).
    pub sixteen: bool,
    /// Sharpening and grain on the finished picture (`detail`), when the caller wants them done here.
    pub detail: Option<Detail>,
}

/// detail::apply's settings: Sharpening 0..150, Grain 0..100, and the picture's long edge over the
/// sensor's.
#[derive(Clone, Copy, Debug)]
pub struct Detail {
    pub sharpen: f32,
    pub grain: f32,
    pub scale: f32,
}

thread_local! {
    /// Sharpening and grain for the next render on this thread, if the GPU should do them
    /// (`commands::render_image` asks; it does them itself unless `detail_done` says so).
    static DETAIL_WANTED: std::cell::Cell<Option<Detail>> = const { std::cell::Cell::new(None) };
    static DETAIL_DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Asks for the next render on this thread to do sharpening and grain on the GPU.
pub fn want_detail(d: Option<Detail>) {
    DETAIL_WANTED.with(|w| w.set(d));
    DETAIL_DONE.with(|w| w.set(false));
}

/// What `want_detail` asked for (taken: only one render does it).
pub(crate) fn wanted_detail() -> Option<Detail> {
    DETAIL_WANTED.with(|w| w.take())
}

/// Whether the GPU did the sharpening and grain asked for (and forgets the request).
pub fn detail_done() -> bool {
    DETAIL_WANTED.with(|w| w.set(None));
    DETAIL_DONE.with(|w| w.replace(false))
}

/// Which part of a render.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Scene light to the finished picture.
    Full,
    /// Scene light to the toned colour (Texture, Clarity and Dehaze then run on the CPU).
    Tone,
    /// The toned colour to the finished picture.
    Finish,
    /// The masks' Texture / Clarity / Dehaze amounts at each pixel (inside `definition.rs` only).
    Amounts,
}

/// A render's result.
pub enum Output {
    /// RGB, 8 or 16 bits a channel (`Job::sixteen`).
    Bytes(Vec<u8>),
    Words(Vec<u16>),
    /// The toned colour (`Pass::Tone`).
    Toned(Vec<[f32; 3]>),
}

/// The pass of a render on the GPU, or None (render it on the CPU). `toned`: the input of
/// `Pass::Finish`.
pub fn render(job: &Job, pass: Pass, toned: Option<&[[f32; 3]]>) -> Option<Output> {
    if mode() == RenderMode::Cpu || pass == Pass::Amounts {
        return None;
    }
    if let Some(why) = job_unsupported(job) {
        note_cpu(why);
        return None;
    }
    let _p = crate::prof::span(match pass {
        Pass::Full => "  GPU render",
        Pass::Tone => "  GPU tone",
        Pass::Finish | Pass::Amounts => "  GPU finish",
    });
    match run(job, pass, toned) {
        Ok(out) => {
            note(true, None);
            Some(out)
        }
        Err(why) => {
            eprintln!("[gpu] rendering on the CPU: {why}");
            note(false, Some(format!("Rendered on the CPU: {why}")));
            None
        }
    }
}

/// Starts the GPU and builds its shader in the background (a tiny render), so the first photo
/// doesn't wait for it.
pub fn warm_up() {
    std::thread::spawn(|| {
        let scene = SceneImage { width: 2, height: 2, px: vec![[0.18; 3]; 4], source_long: 2 };
        let s = RenderSettings { tools: crate::tools::ToolSettings { mixer: [[1.0; 3]; 8], ..Default::default() }, ..Default::default() };
        let _ = crate::pipeline::render(&scene, &s, 0.0, None);
    });
}

/// What the GPU doesn't do yet, so the render stays on the CPU (said in `status`).
pub fn unsupported(tuning: &Tuning) -> Option<&'static str> {
    if tuning.tone_table > 0.5 || tuning.tone_adapt != 0.0 {
        return Some("the measured tone table");
    }
    if crate::pipeline::MIX_SOLVE_COMPARE.load(Ordering::Relaxed) != 1 {
        return Some("a comparison of the Mixer's solves");
    }
    if tuning.tools.mix_table > 0.5 || tuning.tools.grade_table > 0.5 {
        return Some("the measured Mixer / Grade tables");
    }
    None
}

/// What in a prepared render the GPU can't take (there's room for four of Adobe's look tables; a
/// profile and a look have two at most).
pub(crate) fn job_unsupported(job: &Job) -> Option<&'static str> {
    job.adobe.filter(|a| a.look_tables.len() > params::MAX_LOOKS).map(|_| "a profile with more than four look tables")
}

/// Say that a render stayed on the CPU, and why.
pub fn note_cpu(why: &str) {
    if mode() != RenderMode::Cpu {
        note(false, Some(format!("Rendered on the CPU: the GPU doesn't do {why} yet")));
    }
}

// ---- The device ---------------------------------------------------------------------------------

/// Free the picture buffers after this long unused, and the whole device after this.
const IDLE_BUFFERS: Duration = Duration::from_secs(20);
const IDLE_DEVICE: Duration = Duration::from_secs(600);
/// Scenes kept on the GPU between renders, and the memory they may take together.
const KEEP_SCENES: usize = 3;
const KEEP_SCENE_BYTES: u64 = 400 << 20;
/// What the buffer pool may keep between renders (an export's planes, so the next photo's don't
/// have to be made again: 0.16 s of a 24 MP export on the M1 Max).
const KEEP_POOL_BYTES: u64 = 1 << 30;
/// Pixels a dispatch at most (a strip of the picture): keeps every binding small.
const STRIP: usize = 1 << 22;

struct Engine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    /// The Definition kernels' layout (`definition.rs`).
    def_layout: wgpu::BindGroupLayout,
    /// The pipelines for the shaders built with this header (the parameters' places).
    header: String,
    pipelines: Option<Pipelines>,
    /// The scenes last rendered, newest last (a drag's small one and the photo's preview, kept
    /// both so going between them doesn't upload again).
    scenes: Vec<SceneBuf>,
    /// Clarity's U-Net weights, uploaded the first time they're needed.
    unet: Option<definition::UnetWeights>,
    /// Small buffers for a kernel's unused bindings.
    spare: Vec<wgpu::Buffer>,
    /// Picture-sized buffers to use again (`buffer` / `give_back`).
    pool: Mutex<Pool>,
    /// With RAPIDONE_GPU_TIMES: timestamps around each compute pass, and the passes' names.
    times: Option<Times>,
    /// Errors wgpu reported outside a scope, and whether the device was lost.
    errors: Arc<Mutex<Vec<String>>>,
    lost: Arc<AtomicBool>,
    last_used: Instant,
    max_buffer: u64,
    /// The largest buffer a shader can see whole (the Definition stage binds whole pictures).
    max_binding: u64,
}

struct Pipelines {
    /// render_full, render_tone, render_finish.
    main: [wgpu::ComputePipeline; 4],
    /// The Definition kernels, by entry point name.
    def: std::collections::HashMap<&'static str, wgpu::ComputePipeline>,
}

/// The scene light last uploaded (kept while a photo's sliders move).
struct SceneBuf {
    key: SceneKey,
    buf: wgpu::Buffer,
}

#[derive(Clone, Copy, PartialEq)]
struct SceneKey {
    ptr: usize,
    len: usize,
    width: usize,
    sum: [u32; 4],
}

impl SceneKey {
    /// The scene's place in memory, size and a sample of its pixels: a new scene in the same place
    /// with the same size and the same 256 sampled pixels would be taken for the old one.
    fn of(s: &SceneImage) -> SceneKey {
        let n = s.px.len();
        let mut sum = [0u32; 4];
        for k in 0..256usize {
            let p = s.px[(k * 7919 * 131 + k) % n.max(1)];
            for c in 0..3 {
                sum[c] = sum[c].wrapping_mul(31).wrapping_add(p[c].to_bits());
            }
            sum[3] = sum[3].wrapping_add(p[0].to_bits() ^ p[2].to_bits().rotate_left(7));
        }
        SceneKey { ptr: s.px.as_ptr() as usize, len: n, width: s.width, sum }
    }
}

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
/// Why the GPU couldn't be started (not tried again until the setting changes).
static FAILED: Mutex<Option<String>> = Mutex::new(None);
static IDLE_WATCH: AtomicBool = AtomicBool::new(false);

/// What the engine holds on the GPU between renders: (whether the device is up, bytes kept: the
/// last scene and Clarity's weights).
pub fn held() -> (bool, u64) {
    let e = ENGINE.lock().unwrap();
    match e.as_ref() {
        None => (false, 0),
        Some(e) => (true, e.scenes.iter().map(|s| s.buf.size()).sum::<u64>() + e.unet.as_ref().map_or(0, |u| u.size()) + e.pool.lock().unwrap().bytes()),
    }
}

/// Let go of the GPU's memory: the picture buffers, or (`all`) the device too.
pub fn release(all: bool) {
    let mut e = ENGINE.lock().unwrap();
    if all {
        *e = None;
    } else if let Some(engine) = e.as_mut() {
        engine.scenes.clear();
        engine.unet = None;
        engine.pool.lock().unwrap().clear();
    }
}

fn idle_watch() {
    if IDLE_WATCH.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(5));
        let mut e = ENGINE.lock().unwrap();
        let idle = e.as_ref().map(|e| e.last_used.elapsed());
        match idle {
            Some(t) if t > IDLE_DEVICE => *e = None,
            Some(t) if t > IDLE_BUFFERS => {
                if let Some(e) = e.as_mut() {
                    e.scenes.clear();
                    e.unet = None;
                    e.pool.lock().unwrap().clear();
                }
            }
            _ => {}
        }
    });
}

/// The graphics APIs to try, in order. Windows: Vulkan first (Intel Arc, NVIDIA and AMD all have
/// it), then DirectX 12, whose shader compiler without a shipped dxcompiler.dll is the old, slow FXC.
/// `RAPIDONE_GPU_BACKEND` (metal, vulkan or dx12) picks one, for testing.
fn backend_order() -> Vec<wgpu::Backends> {
    match std::env::var("RAPIDONE_GPU_BACKEND").map(|v| v.to_ascii_lowercase()).as_deref() {
        Ok("metal") => vec![wgpu::Backends::METAL],
        Ok("vulkan") => vec![wgpu::Backends::VULKAN],
        Ok("dx12") => vec![wgpu::Backends::DX12],
        _ if cfg!(windows) => vec![wgpu::Backends::VULKAN, wgpu::Backends::DX12],
        _ => vec![wgpu::Backends::PRIMARY],
    }
}

/// A hardware GPU on one of `backend_order`'s APIs.
fn adapter() -> Result<wgpu::Adapter, String> {
    let mut why = String::from("no usable GPU");
    for backends in backend_order() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })) {
            Ok(a) if a.get_info().device_type == wgpu::DeviceType::Cpu => why = format!("no usable GPU (only {}, a software one)", a.get_info().name),
            Ok(a) => return Ok(a),
            Err(e) => why = format!("no usable GPU ({e})"),
        }
    }
    Err(why)
}

fn start() -> Result<Engine, String> {
    let adapter = adapter()?;
    let info = adapter.get_info();
    // wgpu's default limits (what every Metal, Vulkan and DX12 GPU it supports gives), but buffers
    // as large as this one takes: whole pictures are bound (a 24 MP one is 288 MB); a picture
    // larger than these goes to the CPU (`render`, `definition::run` check).
    let have = adapter.limits();
    let limits = wgpu::Limits { max_buffer_size: have.max_buffer_size, max_storage_buffer_binding_size: have.max_storage_buffer_binding_size, ..wgpu::Limits::default() };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("RapidOne render"),
        required_features: if timing() && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) { wgpu::Features::TIMESTAMP_QUERY } else { wgpu::Features::empty() },
        required_limits: limits.clone(),
        ..Default::default()
    }))
    .map_err(|e| format!("the GPU wouldn't start ({e})"))?;
    let errors = Arc::new(Mutex::new(Vec::new()));
    let sink = errors.clone();
    device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| sink.lock().unwrap().push(e.to_string())));
    let lost = Arc::new(AtomicBool::new(false));
    let flag = lost.clone();
    device.set_device_lost_callback(move |_, why| {
        eprintln!("[gpu] device lost: {why}");
        flag.store(true, Ordering::Relaxed);
    });
    let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("render"),
        entries: &[
            storage(0, true),
            storage(1, true),
            storage(2, true),
            storage(3, true),
            storage(4, false),
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    });
    let def_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("definition"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            storage(1, true),
            storage(2, true),
            storage(3, true),
            storage(4, true),
            storage(5, false),
            storage(6, false),
            storage(7, false),
        ],
    });
    let name = format!("{} ({:?})", info.name, info.backend);
    STATUS.lock().unwrap().adapter = Some(name);
    idle_watch();
    let spare = (0..6)
        .map(|_| device.create_buffer(&wgpu::BufferDescriptor { label: Some("spare"), size: 16, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false }))
        .collect();
    let times = Times::new(&device);
    Ok(Engine {
        device,
        queue,
        layout,
        def_layout,
        header: String::new(),
        pipelines: None,
        scenes: Vec::new(),
        unet: None,
        spare,
        pool: Default::default(),
        times,
        errors,
        lost,
        last_used: Instant::now(),
        max_buffer: limits.max_buffer_size,
        max_binding: limits.max_storage_buffer_binding_size as u64,
    })
}

/// The main shader's text with the parameters' places.
fn shader_source(header: &str) -> String {
    [header, include_str!("shaders/common.wgsl"), include_str!("shaders/tone.wgsl"), include_str!("shaders/tools.wgsl"), include_str!("shaders/gradients.wgsl"), include_str!("shaders/main.wgsl")].join("\n")
}

/// The Definition kernels (`shaders/definition.wgsl`).
const DEF_KERNELS: [&str; 35] = [
    "lum", "block_avg", "upsample_unet", "upsample_net", "grid_at", "box_v", "transpose", "blend", "clarity_disp", "dehaze_apply", "lab_of", "plane_of", "texture", "clarity_net", "finish_lab",
    "unet_conv", "unet_conv1", "unet_up", "unet_mean", "unet_lin", "unet_add", "unet_out",
    "dz_planes", "gather_step", "gather_enc", "dz_dcn", "min_runs", "min_join", "peaks1024", "local_blend", "mul", "gf_ab", "gf_out", "dz_pow", "sum1024",
];

/// Buffers lent for the render under way, and ones free to lend again (newest last).
#[derive(Default)]
struct Pool {
    lent: Vec<wgpu::Buffer>,
    free: Vec<wgpu::Buffer>,
}

/// Buffers smaller than this aren't pooled.
const POOL_MIN_BYTES: u64 = 1 << 16;

impl Pool {
    /// The render is over: all it was lent is free, as much as `KEEP_POOL_BYTES` of it kept (the
    /// most recently used).
    fn end_render(&mut self) {
        let lent = std::mem::take(&mut self.lent);
        self.free.extend(lent);
        let mut total = 0;
        let mut keep = self.free.len();
        while keep > 0 && total + self.free[keep - 1].size() <= KEEP_POOL_BYTES {
            total += self.free[keep - 1].size();
            keep -= 1;
        }
        self.free.drain(..keep);
    }

    fn bytes(&self) -> u64 {
        self.free.iter().chain(&self.lent).map(|b| b.size()).sum()
    }

    fn clear(&mut self) {
        self.free.clear();
    }
}

/// A Definition kernel's numbers (`definition.wgsl`'s `U`).
#[derive(Clone, Copy, Default)]
pub(crate) struct DefU {
    pub w: u32,
    pub h: u32,
    pub r: u32,
    pub n: u32,
    pub i: [u32; 8],
    pub f: [f32; 8],
}

impl DefU {
    fn words(&self) -> [u32; 20] {
        let mut out = [0u32; 20];
        out[..4].copy_from_slice(&[self.w, self.h, self.r, self.n]);
        out[4..12].copy_from_slice(&self.i);
        for k in 0..8 {
            out[12 + k] = self.f[k].to_bits();
        }
        out
    }
}

/// Workgroups for `count` invocations of 64, laid out in two dimensions.
fn groups(count: usize) -> (u32, u32) {
    let g = count.div_ceil(64).max(1) as u32;
    let x = g.min(32768);
    (x, g.div_ceil(x))
}

impl Engine {
    fn pipelines(&mut self, header: &str) -> Result<(), String> {
        if self.header != header || self.pipelines.is_none() {
            let _p = crate::prof::span("  GPU shader build");
            let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("render"),
                source: wgpu::ShaderSource::Wgsl(shader_source(header).into()),
            });
            // RAPIDONE_GPU_DUMP=folder: the shaders as built, for checking them with other back ends.
            if let Some(dir) = std::env::var_os("RAPIDONE_GPU_DUMP") {
                let dir = std::path::PathBuf::from(dir);
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join("render.wgsl"), shader_source(header));
                let _ = std::fs::write(dir.join("definition.wgsl"), [header, include_str!("shaders/definition.wgsl")].join("\n"));
                let _ = std::fs::write(dir.join("detail.wgsl"), include_str!("shaders/detail.wgsl"));
            }
            let def_module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("definition"),
                source: wgpu::ShaderSource::Wgsl([header, include_str!("shaders/definition.wgsl")].join("\n").into()),
            });
            let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("render"), bind_group_layouts: &[Some(&self.layout)], immediate_size: 0 });
            let def_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("definition"), bind_group_layouts: &[Some(&self.def_layout)], immediate_size: 0 });
            let make = |module: &wgpu::ShaderModule, layout: &wgpu::PipelineLayout, entry: &str| {
                self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(layout),
                    module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            };
            let main = [make(&module, &layout, "render_full"), make(&module, &layout, "render_tone"), make(&module, &layout, "render_finish"), make(&module, &layout, "render_amounts")];
            let detail_module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("detail"),
                source: wgpu::ShaderSource::Wgsl(include_str!("shaders/detail.wgsl").into()),
            });
            let def = DEF_KERNELS
                .iter()
                .map(|&k| (k, make(&def_module, &def_layout, k)))
                .chain(detail::KERNELS.iter().map(|&k| (k, make(&detail_module, &def_layout, k))))
                .collect();
            if let Some(e) = pollster::block_on(scope.pop()) {
                return Err(format!("the GPU shader didn't build: {e}"));
            }
            self.pipelines = Some(Pipelines { main, def });
            self.header = header.to_string();
        }
        Ok(())
    }

    /// A buffer of `bytes` the shaders read and write, and that can be copied from and to.
    /// A buffer for this render: one handed back (`give_back`, or by an earlier render) if one is the
    /// same size, else a new one. What's in it is left over; every kernel writes all it reads.
    fn buffer(&self, label: &str, bytes: u64) -> wgpu::Buffer {
        let size = bytes.max(4);
        let mut p = self.pool.lock().unwrap();
        let b = match p.free.iter().rposition(|b| b.size() == size) {
            Some(i) => p.free.remove(i),
            None => self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        };
        // Small ones aren't worth keeping track of.
        if size >= POOL_MIN_BYTES {
            p.lent.push(b.clone());
        }
        b
    }

    /// The render's numbers. (A storage buffer: as a uniform one, read through vec4s, the main
    /// shader ran two to three times slower on Metal.)
    fn params(&self, p: &params::Params) -> wgpu::Buffer {
        self.storage("params", bytemuck::cast_slice(&p.data))
    }

    fn storage(&self, label: &str, bytes: &[u8]) -> wgpu::Buffer {
        use wgpu::util::DeviceExt;
        let contents: &[u8] = if bytes.is_empty() { &[0; 4] } else { bytes };
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        })
    }

    /// Hand a buffer from `buffer` back once nothing recorded after this needs what's in it: a later
    /// `buffer` of the same size may then get it (the GPU runs the commands in order).
    pub(crate) fn give_back(&self, b: wgpu::Buffer) {
        let mut p = self.pool.lock().unwrap();
        if let Some(i) = p.lent.iter().position(|l| *l == b) {
            p.lent.swap_remove(i);
            p.free.push(b);
        }
    }

    /// The scene in a GPU buffer: kept from a recent render when it's the same.
    fn scene(&mut self, s: &SceneImage) -> wgpu::Buffer {
        let key = SceneKey::of(s);
        if let Some(i) = self.scenes.iter().position(|b| b.key == key) {
            let b = self.scenes.remove(i);
            let buf = b.buf.clone();
            self.scenes.push(b);
            return buf;
        }
        let buf = self.storage("scene", bytemuck::cast_slice(&s.px));
        self.scenes.push(SceneBuf { key, buf: buf.clone() });
        // At most a few, and not much memory: the oldest go first.
        while self.scenes.len() > KEEP_SCENES || (self.scenes.len() > 1 && self.scenes.iter().map(|b| b.buf.size()).sum::<u64>() > KEEP_SCENE_BYTES) {
            self.scenes.remove(0);
        }
        buf
    }

    /// Runs what's been encoded, then copies the first `bytes` of `buf` back.
    fn read(&self, encoder: wgpu::CommandEncoder, buf: &wgpu::Buffer, bytes: u64) -> Result<Vec<u32>, String> {
        let mut encoder = encoder;
        let read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read"),
            size: bytes.max(4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(buf, 0, &read, 0, bytes.max(4));
        let stamps = self.times.as_ref().and_then(|t| t.resolve(&self.device, &mut encoder));
        let _p = crate::prof::span("  GPU run + read back");
        self.queue.submit([encoder.finish()]);
        let slice = read.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("GPU wait failed: {e}"))?;
        rx.recv().map_err(|_| "GPU readback never came".to_string())?.map_err(|e| format!("GPU readback failed: {e}"))?;
        drop(_p);
        let _q = crate::prof::span("  GPU copy out");
        let data = read.slice(..).get_mapped_range().map_err(|e| format!("GPU readback failed: {e}"))?;
        let words = bytemuck::cast_slice::<u8, u32>(&data[..bytes as usize]).to_vec();
        drop(data);
        read.unmap();
        if let (Some(t), Some(b)) = (&self.times, stamps) {
            t.print(&self.device, &self.queue, &b);
        }
        Ok(words)
    }

    fn encoder(&self) -> wgpu::CommandEncoder {
        self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render") })
    }

    /// One pass of the main shader over a picture of `n` pixels (`w` wide), a strip at a time.
    #[allow(clippy::too_many_arguments)]
    fn encode_pass(&self, encoder: &mut wgpu::CommandEncoder, pass: Pass, dims: (usize, usize, usize), prm: &wgpu::Buffer, lut: &wgpu::Buffer, input: &wgpu::Buffer, offs: Option<&wgpu::Buffer>, out: &wgpu::Buffer, words: usize) {
        let (n, w, h) = dims;
        let pipeline = &self.pipelines.as_ref().unwrap().main[match pass {
            Pass::Full => 0,
            Pass::Tone => 1,
            Pass::Finish => 2,
            Pass::Amounts => 3,
        }];
        let dummy = offs.is_none().then(|| self.storage("offsets", bytemuck::cast_slice(&[0f32; 64])));
        let mut base = 0usize;
        while base < n {
            let count = STRIP.min(n - base);
            use wgpu::util::DeviceExt;
            let u = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("job"),
                contents: bytemuck::cast_slice(&[base as u32, count as u32, w as u32, h as u32]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let offs_binding = match offs {
                Some(o) => strip_binding(o, base, count, 1),
                None => wgpu::BufferBinding { buffer: dummy.as_ref().unwrap(), offset: 0, size: None },
            };
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("render"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: prm.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: lut.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(strip_binding(input, base, count, 3)) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Buffer(offs_binding) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Buffer(strip_binding(out, base, count, words)) },
                    wgpu::BindGroupEntry { binding: 5, resource: u.as_entire_binding() },
                ],
            });
            let tw = self.stamp("render pass");
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("render"), timestamp_writes: tw });
            cp.set_pipeline(pipeline);
            cp.set_bind_group(0, &group, &[]);
            let (x, y) = groups(count);
            cp.dispatch_workgroups(x, y, 1);
            drop(cp);
            base += count;
        }
    }

    /// One Definition kernel over `count` invocations: inputs (read) and outputs (read and write).
    pub(crate) fn kernel(&self, encoder: &mut wgpu::CommandEncoder, name: &str, count: usize, u: DefU, prm: &wgpu::Buffer, ins: [Option<&wgpu::Buffer>; 3], outs: [Option<&wgpu::Buffer>; 3]) {
        let (pipeline, group, (x, y)) = self.bound(name, count, u, prm, ins, outs);
        let tw = self.stamp(name);
        let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(name), timestamp_writes: tw });
        cp.set_pipeline(&pipeline);
        cp.set_bind_group(0, &group, &[]);
        cp.dispatch_workgroups(x, y, 1);
    }

    /// A kernel's pipeline, its bindings and its workgroups, ready to record (several can go in one
    /// compute pass: `kernels`).
    pub(crate) fn bound(&self, name: &str, count: usize, u: DefU, prm: &wgpu::Buffer, ins: [Option<&wgpu::Buffer>; 3], outs: [Option<&wgpu::Buffer>; 3]) -> (wgpu::ComputePipeline, wgpu::BindGroup, (u32, u32)) {
        let pipeline = self.pipelines.as_ref().unwrap().def[name].clone();
        use wgpu::util::DeviceExt;
        let ub = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("u"), contents: bytemuck::cast_slice(&u.words()), usage: wgpu::BufferUsages::UNIFORM });
        // Unused bindings get a small buffer of their own each (a buffer can't be read and written in
        // one dispatch), made once.
        let spare = &self.spare;
        let all: [&wgpu::Buffer; 6] = std::array::from_fn(|k| if k < 3 { ins[k].unwrap_or(&spare[k]) } else { outs[k - 3].unwrap_or(&spare[k]) });
        let pick = |k: usize| all[k].as_entire_binding();
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(name),
            layout: &self.def_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: prm.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: pick(0) },
                wgpu::BindGroupEntry { binding: 3, resource: pick(1) },
                wgpu::BindGroupEntry { binding: 4, resource: pick(2) },
                wgpu::BindGroupEntry { binding: 5, resource: pick(3) },
                wgpu::BindGroupEntry { binding: 6, resource: pick(4) },
                wgpu::BindGroupEntry { binding: 7, resource: pick(5) },
            ],
        });
        (pipeline, group, groups(count))
    }

    /// Several kernels (from `bound`) recorded in one compute pass, in order.
    pub(crate) fn kernels(&self, encoder: &mut wgpu::CommandEncoder, list: &[(wgpu::ComputePipeline, wgpu::BindGroup, (u32, u32))]) {
        let tw = self.stamp("kernels (sharpening rounds)");
        let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("kernels"), timestamp_writes: tw });
        for (pipeline, group, (x, y)) in list {
            cp.set_pipeline(pipeline);
            cp.set_bind_group(0, group, &[]);
            cp.dispatch_workgroups(*x, *y, 1);
        }
    }
}

/// Runs `f` with the engine (started if it isn't), catching wgpu's errors; on any, the engine is
/// let go so the next render starts it afresh.
fn with_engine<R>(header: &str, f: impl FnOnce(&mut Engine) -> Result<R, String>) -> Result<R, String> {
    let mut guard = ENGINE.lock().unwrap();
    if guard.as_ref().is_some_and(|e| e.lost.load(Ordering::Relaxed)) {
        *guard = None;
    }
    if guard.is_none() {
        if let Some(why) = FAILED.lock().unwrap().clone() {
            return Err(why);
        }
        let _p = crate::prof::span("  GPU start");
        match start() {
            Ok(e) => *guard = Some(e),
            Err(why) => {
                *FAILED.lock().unwrap() = Some(why.clone());
                return Err(why);
            }
        }
    }
    let engine = guard.as_mut().unwrap();
    engine.last_used = Instant::now();
    engine.pipelines(header)?;
    let oom = engine.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let invalid = engine.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let result = f(engine);
    engine.pool.lock().unwrap().end_render();
    let invalid = pollster::block_on(invalid.pop());
    let oom = pollster::block_on(oom.pop());
    let late: Vec<String> = std::mem::take(&mut *engine.errors.lock().unwrap());
    let err = invalid.map(|e| format!("GPU validation error: {e}")).or(oom.map(|e| format!("GPU out of memory: {e}"))).or(late.first().map(|e| format!("GPU error: {e}")));
    if let Some(e) = err {
        // Something went wrong on the device: start again next time.
        *guard = None;
        return Err(e);
    }
    if result.is_err() && guard.as_ref().is_some_and(|e| e.lost.load(Ordering::Relaxed)) {
        *guard = None;
    }
    result
}

pub use definition::render_definition;

fn run(job: &Job, pass: Pass, toned: Option<&[[f32; 3]]>) -> Result<Output, String> {
    let (params, lut) = params::build(job);
    let header = params.header();
    with_engine(&header, |engine| {
        let (w, h) = (job.scene.width, job.scene.height);
        let n = w * h;
        let words = match pass {
            Pass::Tone => 3,
            _ if job.sixteen => 2,
            _ => 1,
        };
        let out_bytes = (n * words * 4) as u64;
        if out_bytes > engine.max_buffer || (n * 12) as u64 > engine.max_buffer {
            return Err(format!("the picture ({n} pixels) is larger than the GPU's buffers"));
        }
        let input = match (pass, toned) {
            (Pass::Finish, Some(t)) => engine.storage("toned", bytemuck::cast_slice(t)),
            (Pass::Finish, None) => return Err("no toned picture to finish".into()),
            _ => {
                let _p = crate::prof::span("  GPU upload scene");
                engine.scene(job.scene)
            }
        };
        let prm = engine.params(&params);
        let lut_buf = engine.storage("lut", bytemuck::cast_slice(&lut));
        let offs = match job.tone_map {
            Some(m) if pass != Pass::Finish && !m.offset.is_empty() => Some(engine.storage("offsets", bytemuck::cast_slice(&m.offset))),
            _ => None,
        };
        let out = engine.buffer("out", out_bytes);
        let mut encoder = engine.encoder();
        engine.encode_pass(&mut encoder, pass, (n, w, h), &prm, &lut_buf, &input, offs.as_ref(), &out, words);
        let out = match (pass, job.detail) {
            (Pass::Full | Pass::Finish, Some(d)) => detail::encode(engine, &mut encoder, &out, (n, w, h), job.sixteen, d),
            _ => out,
        };
        let words_back = engine.read(encoder, &out, out_bytes)?;
        if pass != Pass::Tone && job.detail.is_some() {
            DETAIL_DONE.with(|d| d.set(true));
        }
        Ok(unpack(&words_back, pass, job.sixteen))
    })
}

/// Whether to time the GPU's passes (RAPIDONE_GPU_TIMES), for speed work.
fn timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RAPIDONE_GPU_TIMES").is_some())
}

/// Timestamps at the start and end of each compute pass, printed with the passes' names at each
/// read back (RAPIDONE_GPU_TIMES).
struct Times {
    set: wgpu::QuerySet,
    names: Mutex<Vec<String>>,
}

const MAX_STAMPS: u32 = 1024;

impl Times {
    fn new(device: &wgpu::Device) -> Option<Times> {
        (timing() && device.features().contains(wgpu::Features::TIMESTAMP_QUERY)).then(|| Times {
            set: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("times"), ty: wgpu::QueryType::Timestamp, count: MAX_STAMPS }),
            names: Mutex::new(Vec::new()),
        })
    }

    fn resolve(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> Option<wgpu::Buffer> {
        let n = self.names.lock().unwrap().len() as u32 * 2;
        if n == 0 {
            return None;
        }
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("stamps"), size: n as u64 * 8, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let read = device.create_buffer(&wgpu::BufferDescriptor { label: Some("stamps read"), size: n as u64 * 8, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        encoder.resolve_query_set(&self.set, 0..n, &buf, 0);
        encoder.copy_buffer_to_buffer(&buf, 0, &read, 0, n as u64 * 8);
        Some(read)
    }

    fn print(&self, device: &wgpu::Device, queue: &wgpu::Queue, read: &wgpu::Buffer) {
        let slice = read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(data) = slice.get_mapped_range() else { return };
        let t: Vec<u64> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        read.unmap();
        let period = queue.get_timestamp_period() as f64;
        let mut names = self.names.lock().unwrap();
        let mut total = 0.0;
        for (k, name) in names.iter().enumerate() {
            let ms = t[2 * k + 1].saturating_sub(t[2 * k]) as f64 * period / 1e6;
            total += ms;
            eprintln!("[gpu time] {name:<34} {ms:>8.3} ms");
        }
        eprintln!("[gpu time] {:<34} {total:>8.3} ms", "(passes)");
        names.clear();
    }
}

impl Engine {
    /// Timestamp writes for the next compute pass (RAPIDONE_GPU_TIMES).
    fn stamp(&self, name: &str) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        let t = self.times.as_ref()?;
        let mut names = t.names.lock().unwrap();
        let k = names.len() as u32;
        if 2 * k + 1 >= MAX_STAMPS {
            return None;
        }
        names.push(name.to_string());
        Some(wgpu::ComputePassTimestampWrites { query_set: &t.set, beginning_of_pass_write_index: Some(2 * k), end_of_pass_write_index: Some(2 * k + 1) })
    }
}

/// Words read back as the floats they hold.
fn floats_of(words: &[u32]) -> Vec<f32> {
    words.iter().map(|&w| f32::from_bits(w)).collect()
}

/// The finished picture (or the toned colour) from the words the shader wrote.
fn unpack(w: &[u32], pass: Pass, sixteen: bool) -> Output {
    let _p = crate::prof::span("  GPU unpack");
    use rayon::prelude::*;
    match pass {
        Pass::Tone => Output::Toned(w.par_chunks(3).map(|c| [f32::from_bits(c[0]), f32::from_bits(c[1]), f32::from_bits(c[2])]).collect()),
        _ if sixteen => Output::Words(w.par_chunks(2).flat_map_iter(|c| [(c[0] & 0xffff) as u16, (c[0] >> 16) as u16, (c[1] & 0xffff) as u16]).collect()),
        _ => Output::Bytes(w.par_iter().flat_map_iter(|&v| [v as u8, (v >> 8) as u8, (v >> 16) as u8]).collect()),
    }
}

/// Pixels `base .. base + count` of a buffer of `per` words a pixel.
fn strip_binding(buffer: &wgpu::Buffer, base: usize, count: usize, per: usize) -> wgpu::BufferBinding<'_> {
    wgpu::BufferBinding { buffer, offset: (base * per * 4) as u64, size: std::num::NonZeroU64::new((count * per * 4) as u64) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{render, Profile, RenderSettings};

    /// Colours from black to six times white.
    fn grid() -> SceneImage {
        let levels = [0.0f32, 0.003, 0.01, 0.03, 0.08, 0.18, 0.4, 0.8, 1.5, 3.0, 6.0];
        let mut px = Vec::new();
        for &r in &levels {
            for &g in &levels {
                for &b in &levels {
                    px.push([r, g, b]);
                }
            }
        }
        let n = px.len();
        SceneImage { width: n, height: 1, px, source_long: 6000 }
    }

    /// The GPU engine renders as the CPU engine does, within 2 levels for nearly every value
    /// (the Mixer's solve may tip either way for a few colours held at the gamut edge). Runs only
    /// where there's a usable GPU.
    #[test]
    fn gpu_renders_as_the_cpu_does() {
        let scene = grid();
        let mut cases = vec![RenderSettings::default()];
        cases.push(RenderSettings { exposure: 0.6, contrast: 30.0, highlights: -50.0, shadows: 40.0, whites: 15.0, blacks: -20.0, vibrance: 25.0, saturation: 15.0, ..Default::default() });
        let mut tools = RenderSettings { profile: Profile::ShinobiStandard, ..Default::default() };
        tools.tools.mixer[3] = [-20.0, -30.0, 10.0];
        tools.tools.curve_rgb = vec![[0.0, 0.0], [128.0, 140.0], [255.0, 255.0]];
        tools.tools.grade.highs.sat = 20.0;
        tools.tools.grade.highs.hue = 50.0;
        tools.tools.vignette = -30.0;
        cases.push(tools);
        let mut definition = RenderSettings::default();
        definition.tools.texture = 30.0;
        definition.tools.clarity = 40.0;
        definition.tools.dehaze = 25.0;
        definition.tools.sharpen = 40.0;
        cases.push(definition);
        // Texture, Clarity and Dehaze inside masks, each way.
        use crate::gradient::{Gradient, Kind};
        let masks = vec![
            Gradient { kind: Kind::Linear, full: [0.1, 0.2], zero: [0.6, 0.7], texture: 35.0, clarity: 40.0, dehaze: 25.0, ..Default::default() },
            Gradient { kind: Kind::Radial, centre: [0.55, 0.5], rx: 0.3, ry: 0.25, feather: 50.0, texture: -30.0, clarity: -35.0, dehaze: -20.0, ..Default::default() },
        ];
        cases.push(RenderSettings { gradients: masks, ..Default::default() });
        for s in cases {
            set_thread_mode(Some(RenderMode::Cpu));
            let cpu = render(&scene, &s, 0.0, None);
            set_thread_mode(Some(RenderMode::Gpu));
            let gpu = render(&scene, &s, 0.0, None);
            set_thread_mode(None);
            if !status().on_gpu {
                if FAILED.lock().unwrap().is_some() {
                    eprintln!("no usable GPU here: {:?}", status().note);
                    return;
                }
                panic!("the GPU render fell back to the CPU: {:?}", status().note);
            }
            let over = cpu.as_raw().iter().zip(gpu.as_raw().iter()).filter(|(a, b)| (**a as i32 - **b as i32).abs() > 2).count();
            assert!(over * 500 <= cpu.as_raw().len(), "{over} values over 2 levels apart");
            // The same bytes again (a kernel writing past a buffer's end, or reading what it didn't
            // write, shows up here), with buffers left from the render before.
            set_thread_mode(Some(RenderMode::Gpu));
            let again = render(&scene, &s, 0.0, None);
            set_thread_mode(None);
            assert!(gpu.as_raw() == again.as_raw(), "the GPU gave different bytes the second time");
        }
    }
}
