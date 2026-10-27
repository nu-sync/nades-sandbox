//! nades-sandbox, M3: the lineup sandbox. A map in CS2's look with the lineups of the throws in cs2-throws files: pick one
//! and see it from the thrower's eye (Lineup), zoomed on the aim point (Aim), from an opponent's eye (Result) or from
//! anywhere around it (Orbit), with the flight, the smoke, fire, flash or HE as the round had it, a time scrubber, and the
//! practice command to copy (`lineups.rs`, `shell.rs`). The throws are facts read through a `ThrowSource`; grouping,
//! technique, callouts and viewpoints are nades-model's rules.
//!
//! Start-up runs on threads from `main`: this one has AppKit make the window (≈76 ms), while the loader maps the pack,
//! brings up the GPU, puts the map on it and draws one frame offscreen, and the data thread reads the lineup list
//! (`library.rs`). The first frame needs only the window and the map: it opens on the camera the last run left
//! (`state.rs`), and the throw's effects follow when they are built. The collider and the game's flipbooks and smoke
//! noise are read after it.
//!
//! - `--data <file or folder>` (repeatable) names the cs2-throws files to read; else `NADES_SANDBOX_DATA`, else
//!   `[data] paths` in the config file, else the demo adapter's cache (nades-config).
//! - `check <file>...` prints each cs2-throws file's problems and which features its facts allow (`check.rs`).
//! - `--map <name>` draws `~/.cache/nades/packs/<name>.pack` (the last map, else de_cache; `NADES_PACK` names a file).
//! - `--throw <id> [--view result] [--time s]` with `--render`: one throw's view as the sandbox shows it, its smoke
//!   averaged over 16 jittered frames. `--cameras <cases.json>` prints the cameras each `{"throw": id}`'s views open
//!   with (bench/cameras.py compares them with the web view's). `--bench-switch <n>` switches lineups every frame, n
//!   times, once their stages are built, and prints what each switching frame cost.
//! - `--exit-after-first-frame` prints the start-up marks as one JSON line once the first frame is presented and macOS
//!   reports the window visible (`window:visible`), whichever comes last, and exits.
//! - `--screenshot <file.png>` writes that frame (the bench camera) as an 8-bit sRGB PNG at the window's physical size.
//! - `--bench-frames <n>` draws n frames with vsync off and prints the median and 95th percentile wall time per frame
//!   after the first 30, with the display the window is on: macOS sometimes paces a window to its display's refresh
//!   anyway (8.3 ms at 120 Hz, 16.7 at 60), and then the number is the display's, not the frame's.
//! - `--render <file.png> --setpos x,y,z --setang pitch,yaw [--eye 64] [--size 1138x640]` draws one frame offscreen,
//!   with no window, from a CS2 console position (the player's feet) and angles, and writes it as a PNG: the in-game
//!   lineup shots' cameras, for the realism check (bench/realism.py).
//! - `--bench-offscreen <n> [--size 2880x1800]` draws n frames offscreen from the bench camera, each finished on the GPU
//!   before the next, and prints the median and 95th percentile per frame after the first 30: the steady frame where
//!   no window can be shown.

mod camera;
mod check;
mod clock;
mod control;
mod library;
mod lineups;
mod resident;
mod shell;
mod state;
mod views;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::collections::VecDeque;

use anyhow::{Context as _, Result, anyhow, bail};
use camera::{Camera, Fly};
use base64::Engine as _;
use nades_control::{AppCommand, AppMessage, AppState, ControlError, ControlOutput, FrameMetrics, Viewport};
use control::{Control, UserEvent};
use nades_config as config;
use nades_pack::Pack;
use nades_render::map::{FrameEffects, Map, Targets};
use nades_render::{Gpu, View as Eye};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Window, WindowId};

const TITLE: &str = "nades sandbox";
/// Where nothing is drawn: CS2's loading-screen grey (linear), as in M0 and the Bevy shell.
const CLEAR: wgpu::Color = wgpu::Color { r: 0.035, g: 0.040, b: 0.050, a: 1.0 };
/// The window's size in points, and the scale the loader prepares the first frame's targets for (Retina).
const WINDOW: (f64, f64) = (1440.0, 900.0);
const EXPECTED_SCALE: f64 = 2.0;
/// Offscreen renders are 8-bit sRGB, like the screenshots.
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// The swap chain's format (sRGB: the hardware encodes), fixed before the window exists so the loader can build the
/// pipelines for it. Every Metal surface offers it.
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
/// Frames left out of `--bench-frames` statistics.
const WARM_UP_FRAMES: usize = 30;

#[derive(Default)]
struct Args {
    map: Option<String>,
    exit_after_first_frame: bool,
    screenshot: Option<PathBuf>,
    bench_frames: Option<usize>,
    render: Option<PathBuf>,
    bench_offscreen: Option<usize>,
    setpos: Option<[f32; 3]>,
    setang: Option<[f32; 2]>,
    eye: Option<f32>,
    size: Option<(u32, u32)>,
    control_stdio: bool,
    instance_id: Option<String>,
    render_time: Option<f32>,
    fov: Option<f32>,
    bench_switch: Option<usize>,
    throw: Option<String>,
    view: Option<nades_scene::stage::ViewMode>,
    time: Option<f64>,
    cameras: Option<PathBuf>,
    bench_maps: Option<Vec<String>>,
    budget_mb: Option<u64>,
    data: Vec<PathBuf>,
}

/// Numbers separated by commas: `1,2,3`.
fn numbers<const N: usize>(text: Option<String>, flag: &str) -> Result<[f32; N]> {
    let text = text.with_context(|| format!("{flag} needs {N} numbers"))?;
    let values: Vec<f32> = text.split(',').map(|v| v.trim().parse::<f32>()).collect::<Result<_, _>>().with_context(|| format!("{flag} {text}"))?;
    values.try_into().map_err(|_| anyhow!("{flag} needs {N} numbers, comma-separated"))
}

fn args() -> Result<Args> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data" => args.data.push(it.next().context("--data needs a cs2-throws file or a folder of them")?.into()),
            "--control-stdio" => args.control_stdio = true,
            "--instance-id" => args.instance_id = Some(it.next().context("--instance-id needs an ID")?),
            "--render-time" => args.render_time = Some(numbers::<1>(it.next(), "--render-time")?[0]),
            "--fov" => args.fov = Some(numbers::<1>(it.next(), "--fov")?[0]),
            "--bench-switch" => args.bench_switch = Some(it.next().and_then(|n| n.parse().ok()).filter(|&n| n > 0).context("--bench-switch needs a count")?),
            "--throw" => args.throw = Some(it.next().context("--throw needs a throw id")?),
            "--view" => args.view = Some(it.next().as_deref().and_then(lineups::view_mode).context("--view needs lineup, aim, result or orbit")?),
            "--time" => args.time = Some(it.next().and_then(|t| t.parse().ok()).context("--time needs seconds after detonation")?),
            "--cameras" => args.cameras = Some(it.next().context("--cameras needs a file")?.into()),
            "--bench-maps" => args.bench_maps = Some(it.next().context("--bench-maps needs maps, comma-separated")?.split(',').map(str::to_string).collect()),
            "--map" => args.map = Some(it.next().context("--map needs a map name")?),
            "--budget-mb" => args.budget_mb = Some(it.next().and_then(|n| n.parse().ok()).context("--budget-mb needs megabytes")?),
            "--exit-after-first-frame" => args.exit_after_first_frame = true,
            "--screenshot" => args.screenshot = Some(it.next().context("--screenshot needs a file")?.into()),
            "--bench-frames" => args.bench_frames = Some(it.next().and_then(|n| n.parse().ok()).filter(|&n| n > WARM_UP_FRAMES).context("--bench-frames needs a count above 30")?),
            "--render" => args.render = Some(it.next().context("--render needs a file")?.into()),
            "--bench-offscreen" => args.bench_offscreen = Some(it.next().and_then(|n| n.parse().ok()).filter(|&n| n > WARM_UP_FRAMES).context("--bench-offscreen needs a count above 30")?),
            "--setpos" => args.setpos = Some(numbers::<3>(it.next(), "--setpos")?),
            "--setang" => args.setang = Some(numbers::<2>(it.next(), "--setang")?),
            "--eye" => args.eye = Some(numbers::<1>(it.next(), "--eye")?[0]),
            "--size" => {
                let size = it.next().context("--size needs WIDTHxHEIGHT")?;
                let (w, h) = size.split_once('x').context("--size needs WIDTHxHEIGHT")?;
                args.size = Some((w.parse()?, h.parse()?));
            }
            other => bail!("unknown argument {other}"),
        }
    }
    if args.control_stdio && (args.instance_id.as_deref().is_none_or(str::is_empty) || args.exit_after_first_frame || args.screenshot.is_some() || args.bench_frames.is_some() || args.render.is_some() || args.bench_offscreen.is_some()) {
        bail!("--control-stdio needs --instance-id and cannot be combined with render, screenshot, exit or benchmark modes");
    }
    if args.render_time.is_some_and(|time| !time.is_finite() || time < 0.0) {
        bail!("--render-time must be finite and nonnegative");
    }
    if args.fov.is_some_and(|fov| !fov.is_finite() || fov <= 1.0 || fov >= 179.0) {
        bail!("--fov must be strictly between 1 and 179 degrees");
    }
    Ok(args)
}

/// The pack to draw: `NADES_PACK`, else `~/.cache/nades/packs/<map>.pack`.
fn pack_path(map: Option<&str>) -> PathBuf {
    if let Some(path) = std::env::var_os("NADES_PACK") {
        return PathBuf::from(path);
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(format!(".cache/nades/packs/{}.pack", map.unwrap_or("de_cache")))
}

fn main() {
    clock::init();
    clock::mark("main");
    let words: Vec<String> = std::env::args().skip(1).collect();
    if words.first().map(String::as_str) == Some("check") {
        std::process::exit(if check::run(&words[1..]) { 0 } else { 1 });
    }
    if let Err(failure) = run() {
        eprintln!("nades-sandbox: {failure:#}");
        std::process::exit(1);
    }
    // the GPU objects need no tearing down: the process is done
    std::process::exit(0);
}

/// The data source for a run that reads throws without a window: one that does not open is an error.
fn cli_source(args: &Args) -> Result<Arc<dyn nades_model::ThrowSource>> {
    let data = config::data(&args.data)?;
    match config::open(&data) {
        (_, Some(notice)) => bail!("{notice}"),
        (source, None) => Ok(source),
    }
}

fn run() -> Result<()> {
    let args = args()?;
    if args.control_stdio { clock::enable_logs(); }
    if let Some(cases) = &args.cameras {
        return lineups::print_cameras(cli_source(&args)?.as_ref(), cases);
    }
    if let (Some(out), Some(throw)) = (&args.render, &args.throw) {
        return lineups::render_throw(cli_source(&args)?.as_ref(), throw, args.map.as_deref(), args.view, args.time, args.size.unwrap_or((1138, 640)), out);
    }
    if let Some(out) = &args.render {
        return render_offscreen(&args, out);
    }
    if let Some(frames) = args.bench_offscreen {
        return bench_offscreen(&args, frames);
    }
    // the map and the camera the last run left, unless told otherwise (benches start from the bench camera)
    let bench = args.screenshot.is_some() || args.bench_frames.is_some();
    let remembered = state::load().filter(|_| !bench);
    let map = args.map.clone().or_else(|| remembered.as_ref().map(|r| r.map.clone())).unwrap_or_else(|| "de_cache".into());
    let instance = nades_render::instance();
    // the loader warms the map up from the camera the first frame is drawn from
    let eye = remembered.as_ref().and_then(|r| r.camera_on(&map)).map(|(eye, pitch, yaw, fov)| Eye::from_cs(eye.map(|v| v as f32), pitch as f32, yaw as f32, fov as f32));
    let loader = std::thread::Builder::new().name("loader".into()).spawn({
        let instance = instance.clone();
        let path = pack_path(Some(&map));
        move || load(instance, &path, eye)
    })?;
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    // no menu bar: building AppKit's default one costs the launch ≈8 ms (Cmd+Q is handled as a key instead)
    #[cfg(target_os = "macos")]
    winit::platform::macos::EventLoopBuilderExtMacOS::with_default_menu(&mut builder, false);
    let event_loop = builder.build()?;
    clock::mark("event_loop");
    event_loop.set_control_flow(if args.bench_frames.is_some() || args.bench_switch.is_some() { ControlFlow::Poll } else { ControlFlow::Wait });
    let control = args.control_stdio.then(|| Control::start(event_loop.create_proxy())).transpose()?;
    // the data thread starts now: the lineup list is read while the window and the map come up
    let proxy = event_loop.create_proxy();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = proxy.send_event(UserEvent::Wake);
    });
    let (source, notice) = config::open(&config::data(&args.data)?);
    if let Some(notice) = &notice {
        eprintln!("data: {notice}");
    }
    let library = {
        let wake = wake.clone();
        library::Library::start(source, move || wake())
    };
    let mut sandbox = lineups::Sandbox::start(&map, remembered, library);
    sandbox.data_notice = notice;
    let mut lineups = lineups::Lineups::new(sandbox, bench, egui::Rect::NOTHING);
    // the other maps open in the background after the first frame, except in runs that time one map
    // (NADES_SANDBOX_PRELOAD: 0 never, 1 always)
    lineups.preload = match std::env::var("NADES_SANDBOX_PRELOAD").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => !bench && !args.exit_after_first_frame && args.bench_switch.is_none(),
    };
    let mut app = App { args, instance, loader: Some(loader), view: None, failure: None, bench: Vec::new(), visible: false, control, ready_sent: false, lineups: Some(lineups), wake, switching: None, maps_bench: None, exit_deadline: None };
    app.maps_bench = app.args.bench_maps.clone().map(lineups::MapsBench::new);
    event_loop.run_app(&mut app)?;
    if let Some(view) = &mut app.view
        && !view.lineups.bench
    {
        view.lineups.sandbox.remember();
    }
    app.failure.map_or(Ok(()), Err)
}

/// What the loader thread hands the window: the GPU with the map on it, the targets for the size the window is
/// expected to have, and egui's renderer.
struct Loaded {
    gpu: Gpu,
    map: Map,
    name: String,
    targets: Targets,
    painter: egui_wgpu::Renderer,
    egui: egui::Context,
}

/// The pack, mapped for as long as the process runs (the GPU reads its own pages), and its scene.
fn open_pack(path: &Path) -> Result<(&'static Pack, nades_pack::scene::Scene)> {
    let pack: &'static Pack = Box::leak(Box::new(Pack::open(path).with_context(|| format!("opening {} (build it with nades-pack <map>)", path.display()))?));
    let scene = pack.scene()?;
    Ok((pack, scene))
}

fn load(instance: wgpu::Instance, path: &Path, eye: Option<Eye>) -> Result<Loaded> {
    let (pack, scene) = open_pack(path)?;
    clock::mark("load:pack");
    let gpu = Gpu::new(instance, None)?;
    if !gpu.capabilities.bindless {
        bail!("{} has no texture binding arrays, which the renderer draws every material through", gpu.name());
    }
    clock::mark("load:device");
    let map = Map::new(&gpu.device, &gpu.queue, pack, &scene, SURFACE_FORMAT, &|stage| {
        clock::mark(stage);
    });
    // the 3D view sits between the shell's panels
    let (width, height) = lineups::view_size(WINDOW, EXPECTED_SCALE);
    let targets = map.targets(&gpu.device, &gpu.queue, width, height);
    let mut painter = egui_wgpu::Renderer::new(&gpu.device, SURFACE_FORMAT, egui_wgpu::RendererOptions { depth_stencil_format: None, dithering: false, ..Default::default() });
    let name = pack.contents().source.map.clone();
    let egui = warm_egui(&gpu, &mut painter, &name);
    clock::mark("load:egui");
    warm_up(&gpu, &map, &targets, &eye.unwrap_or_else(|| Camera::bench().view()));
    clock::mark("load:warm");
    Ok(Loaded { gpu, map, name, targets, painter, egui })
}

/// One frame offscreen from a CS2 console position and angles, written as a PNG; no window.
fn render_offscreen(args: &Args, out: &Path) -> Result<()> {
    let (setpos, [pitch, yaw]) = (args.setpos.context("--render needs --setpos x,y,z")?, args.setang.context("--render needs --setang pitch,yaw")?);
    let (width, height) = args.size.unwrap_or((1138, 640));
    let (pack, scene) = open_pack(&pack_path(args.map.as_deref()))?;
    clock::mark("load:pack");
    let gpu = Gpu::new(nades_render::instance(), None)?;
    if !gpu.capabilities.bindless {
        bail!("{} has no texture binding arrays, which the renderer draws every material through", gpu.name());
    }
    let map = Map::new(&gpu.device, &gpu.queue, pack, &scene, OFFSCREEN_FORMAT, &|stage| {
        clock::mark(stage);
    });
    let targets = map.targets(&gpu.device, &gpu.queue, width, height);
    let mut eye = Eye::from_setpos(setpos, pitch, yaw, args.eye.unwrap_or(64.0));
    if let Some(fov) = args.fov { eye.fov_y = fov.to_radians(); }
    let output = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render") });
    map.render(&gpu.queue, &mut encoder, &targets, &eye, args.render_time.unwrap_or(0.0), &FrameEffects::default());
    {
        let view = output.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(CLEAR), store: wgpu::StoreOp::Store } })],
            ..Default::default()
        });
        map.present(&mut pass, &targets);
    }
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("render"), size: u64::from(row_bytes(width) * height), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    encoder.copy_texture_to_buffer(
        output.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row_bytes(width)), rows_per_image: Some(height) } },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()]);
    clock::mark("render:submitted");
    gpu.wait_idle();
    clock::mark("render:gpu");
    write_png(&gpu, &buffer, width, height, out, [0, 1, 2])?;
    clock::mark("render:written");
    println!("{}", clock::json());
    Ok(())
}

/// The steady frame offscreen: `frames` frames from the bench camera into a target of the window's size, each submitted
/// and finished on the GPU before the next (so a frame's time is its whole cost, CPU and GPU, without pipelining).
fn bench_offscreen(args: &Args, frames: usize) -> Result<()> {
    let (width, height) = args.size.unwrap_or(((WINDOW.0 * EXPECTED_SCALE) as u32, (WINDOW.1 * EXPECTED_SCALE) as u32));
    let (pack, scene) = open_pack(&pack_path(args.map.as_deref()))?;
    let gpu = Gpu::new(nades_render::instance(), None)?;
    let map = Map::new(&gpu.device, &gpu.queue, pack, &scene, SURFACE_FORMAT, &|_| {});
    let targets = map.targets(&gpu.device, &gpu.queue, width, height);
    let output = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SURFACE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let output = output.create_view(&Default::default());
    let camera = Camera::bench();
    let mut times = Vec::with_capacity(frames);
    for k in 0..frames {
        let started = Instant::now();
        let mut view = camera.view();
        // a little motion, so nothing is cached between frames
        view.eye.x += (k % 2) as f32 * 0.01;
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bench") });
        map.render(&gpu.queue, &mut encoder, &targets, &view, k as f32 / 60.0, &FrameEffects::default());
        map.present(
            &mut encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &output, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(CLEAR), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            }),
            &targets,
        );
        gpu.queue.submit([encoder.finish()]);
        gpu.wait_idle();
        times.push(started.elapsed().as_secs_f64() * 1e3);
    }
    let mut ms: Vec<f64> = times[WARM_UP_FRAMES..].to_vec();
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    println!("{{\"frames\":{frames},\"width\":{width},\"height\":{height},\"median_ms\":{:.3},\"p95_ms\":{:.3}}}", at(0.5), at(0.95));
    Ok(())
}

/// Writes a copied frame (rows padded to 256 bytes, four bytes a texel) as an 8-bit RGB PNG; `order` picks red, green
/// and blue from each texel.
fn write_png(gpu: &Gpu, buffer: &wgpu::Buffer, width: u32, height: u32, path: &Path, order: [usize; 3]) -> Result<()> {
    let bytes = encode_png(gpu, buffer, width, height, order)?;
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn encode_png(gpu: &Gpu, buffer: &wgpu::Buffer, width: u32, height: u32, order: [usize; 3]) -> Result<Vec<u8>> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |mapped| { let _ = sender.send(mapped); });
    gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(Duration::from_secs(10)) })?;
    receiver.recv_timeout(Duration::from_secs(1)).context("frame mapping timed out")??;
    let bytes = buffer.slice(..).get_mapped_range()?;
    let rgb: Vec<u8> = bytes.chunks_exact(row_bytes(width) as usize).flat_map(|row| row[..width as usize * 4].chunks_exact(4).flat_map(|t| order.map(|k| t[k]))).collect();
    drop(bytes);
    buffer.unmap();
    let mut output = Vec::new();
    {
        let mut png = png::Encoder::new(&mut output, width, height);
        png.set_color(png::ColorType::Rgb);
        png.set_depth(png::BitDepth::Eight);
        png.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        png.write_header()?.write_image_data(&rgb)?;
    }
    if output.len() > nades_control::MAX_PNG_BYTES { bail!("PNG exceeds the 16 MiB image limit"); }
    Ok(output)
}

/// egui with the shell laid out once at the usual Retina scale: its fonts parsed, the glyphs it shows rasterized and
/// their atlas on the GPU before the window exists. (Another scale only costs the first frame that work again.)
fn warm_egui(gpu: &Gpu, painter: &mut egui_wgpu::Renderer, name: &str) -> egui::Context {
    let context = egui::Context::default();
    context.set_theme(egui::Theme::Dark);
    // the same atlas size egui-winit asks for, or the atlas is rebuilt
    let max_texture_side = Some(gpu.device.limits().max_texture_dimension_2d as usize);
    let mut input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WINDOW.0 as f32, WINDOW.1 as f32))), max_texture_side, ..Default::default() };
    input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(EXPECTED_SCALE as f32);
    let maps = [name.to_string()];
    let mut search = String::new();
    let output = context.run_ui(input, |ui| {
        shell::show(ui, shell::Status::empty(name, &maps, &mut search));
    });
    for (id, deltas) in &output.textures_delta.set {
        for delta in deltas {
            painter.update_texture(&gpu.device, &gpu.queue, *id, delta);
        }
    }
    context
}

/// Draws a whole frame off screen and waits for it, while AppKit is still making the window: the first frame on
/// screen then finds the GPU's first-use work (the pack's pages mapped for it, the pipelines' first runs, the texture
/// copies) done.
fn warm_up(gpu: &Gpu, map: &Map, targets: &Targets, eye: &Eye) {
    let (width, height) = targets.size();
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("warm-up"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SURFACE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT,
        view_formats: &[],
    });
    let target = target.create_view(&Default::default());
    // twice: a map's second frame still costs the GPU several times a steady one (S21)
    for k in 0..2 {
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("warm-up") });
        map.render(&gpu.queue, &mut encoder, targets, eye, k as f32 / 60.0, &FrameEffects::default());
        map.present(
            &mut encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("warm-up"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(CLEAR), store: wgpu::StoreOp::Discard } })],
                ..Default::default()
            }),
            targets,
        );
        gpu.queue.submit([encoder.finish()]);
        gpu.wait_idle();
    }
}

struct App {
    args: Args,
    instance: wgpu::Instance,
    loader: Option<JoinHandle<Result<Loaded>>>,
    view: Option<View>,
    failure: Option<anyhow::Error>,
    /// When each frame was presented, for `--bench-frames`.
    bench: Vec<Instant>,
    /// Whether macOS has reported the window visible: the first frame may be presented before it is.
    visible: bool,
    control: Option<Control>,
    ready_sent: bool,
    /// The lineups, until the window takes them.
    lineups: Option<lineups::Lineups>,
    /// Wakes the event loop from the data, collider, effects and map threads.
    wake: Arc<dyn Fn() + Send + Sync>,
    /// `--bench-switch`: each switching frame's CPU work and its whole time, GPU included (ms).
    switching: Option<Vec<(f64, f64)>>,
    /// `--bench-maps`: the maps still to pick and how long each took.
    maps_bench: Option<lineups::MapsBench>,
    /// `--exit-after-first-frame`: when to stop waiting for the frame's time on the display.
    exit_deadline: Option<Instant>,
}

impl App {
    /// `--exit-after-first-frame` reports once the map can be on screen: its first frame presented and the window
    /// visible, in either order.
    ///
    /// It waits up to half a second more for the window server to say when the frame reached the display
    /// (`frame:on_screen`), polled from `about_to_wait`.
    fn exit_once_on_screen(&mut self, event_loop: &ActiveEventLoop) {
        if self.args.exit_after_first_frame && self.visible && !event_loop.exiting() && self.view.as_ref().is_some_and(|view| view.frames > 0) {
            let deadline = *self.exit_deadline.get_or_insert_with(|| Instant::now() + Duration::from_millis(500));
            if clock::marks().iter().any(|(stage, _)| matches!(*stage, "frame:on_screen" | "frame:skipped")) || Instant::now() >= deadline {
                println!("{}", clock::json());
                event_loop.exit();
            }
        }
    }

    /// After a frame is presented: the flags' exits, and the bench's clock.
    fn presented(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let view = self.view.as_mut().expect("a view draws");
        if view.frames == 1 {
            let at = clock::mark("frame:presented");
            view.lineups.sandbox.first_frame_ms = Some(at + clock::pre_main().max(0.0));
            // what the first frame did not need: the collider, the game's flipbooks and smoke noise, the other maps
            if !view.lineups.bench {
                view.lineups.after_first_frame(&view.map, &view.name, self.wake.clone());
            }
            if self.visible {
                view.lineups.open_other_maps(&view.gpu, &view.name);
            }
            if let Some(path) = &self.args.screenshot {
                view.save_screenshot(path)?;
                event_loop.exit();
                return Ok(());
            }
            // the panel shows the marks from the next frame on
            view.window.request_redraw();
            self.exit_once_on_screen(event_loop);
        }
        if let Some(frames) = self.args.bench_frames {
            self.bench.push(Instant::now());
            if self.bench.len() == frames {
                let mut ms: Vec<f64> = self.bench.windows(2).skip(WARM_UP_FRAMES - 1).map(|w| (w[1] - w[0]).as_secs_f64() * 1e3).collect();
                ms.sort_by(f64::total_cmp);
                let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
                // the display the window is on: a window macOS paces to its refresh shows that rate, not the frame's cost
                let monitor = self.view.as_ref().and_then(|view| view.window.current_monitor());
                let refresh = monitor.as_ref().and_then(|m| m.refresh_rate_millihertz()).map_or(0.0, |r| f64::from(r) / 1000.0);
                let display = monitor.and_then(|m| m.name()).unwrap_or_default();
                println!("{{\"frames\":{frames},\"median_ms\":{:.3},\"p95_ms\":{:.3},\"display\":{display:?},\"refresh_hz\":{refresh}}}", at(0.5), at(0.95));
                event_loop.exit();
            }
        }
        self.notify_ready(event_loop);
        Ok(())
    }
}

impl App {
    /// `--bench-switch`: once the game's effects and the stages are in, one lineup switch per frame, each drawn and
    /// finished on the GPU; then the medians as one JSON line.
    fn bench_switch(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(n), Some(view)) = (self.args.bench_switch, self.view.as_mut()) else { return };
        if !view.lineups.ready_for(n) {
            return;
        }
        let record = self.switching.get_or_insert_with(Vec::new);
        if record.len() == n {
            let stats = |k: usize| {
                let mut v: Vec<f64> = record.iter().map(|r| if k == 0 { r.0 } else { r.1 }).collect();
                v.sort_by(f64::total_cmp);
                (v[v.len() / 2], v[((v.len() - 1) as f64 * 0.95).round() as usize], v[v.len() - 1])
            };
            let ((m0, p0, x0), (m1, p1, x1)) = (stats(0), stats(1));
            println!(
                "{{\"switches\":{n},\"switch_cpu_ms\":{{\"median\":{m0:.3},\"p95\":{p0:.3},\"max\":{x0:.3}}},\"frame_ms\":{{\"median\":{m1:.3},\"p95\":{p1:.3},\"max\":{x1:.3}}},\"size\":{:?},\"view\":{:?}}}",
                view.targets.size(),
                view.lineups.sandbox.view.name()
            );
            event_loop.exit();
            return;
        }
        let started = Instant::now();
        let switched = view.lineups.switch_next(&view.gpu, &mut view.map);
        let cpu = started.elapsed().as_secs_f64() * 1e3;
        view.finish_frames = true;
        let drawn = view.draw(false);
        view.finish_frames = false;
        if switched && matches!(drawn, Ok(true)) {
            record.push((cpu, started.elapsed().as_secs_f64() * 1e3));
        }
    }

    fn detach(&mut self, event_loop: &ActiveEventLoop) {
        let ready = self.ready_sent;
        self.control = None;
        if let Some(view) = &mut self.view {
            view.collect_metrics = false;
            view.observed = None;
            view.fly = Fly::default();
            let _ = view.window.set_cursor_grab(CursorGrabMode::None);
            view.window.set_cursor_visible(true);
        }
        if !ready { event_loop.exit(); }
    }

    fn send(&mut self, event_loop: &ActiveEventLoop, message: AppMessage) {
        if self.control.as_ref().is_some_and(Control::connected) && !control::write(&message) {
            self.detach(event_loop);
        }
    }

    fn notify_ready(&mut self, event_loop: &ActiveEventLoop) {
        if self.control.is_some() && !self.ready_sent && self.visible && self.view.as_ref().is_some_and(|view| view.frames > 0) {
            if !self.control.as_ref().expect("control readiness").mark_ready() {
                self.detach(event_loop);
                return;
            }
            self.ready_sent = true;
            let state = self.view.as_ref().expect("ready view").snapshot(self.args.instance_id.as_deref().expect("control instance"));
            self.send(event_loop, AppMessage::Ready { protocol_version: nades_control::PROTOCOL_VERSION, state });
        }
    }

    fn command(&mut self, event_loop: &ActiveEventLoop, command: AppCommand) -> std::result::Result<ControlOutput, ControlError> {
        if !self.ready_sent { return Err(ControlError::new("not_ready", "The window has not presented a visible frame yet.", true)); }
        let view = self.view.as_mut().ok_or_else(|| ControlError::new("not_ready", "The view is unavailable.", true))?;
        view.refresh_revision();
        let instance = self.args.instance_id.as_deref().expect("control instance");
        match command {
            AppCommand::Cancel { .. } => Err(ControlError::new("invalid_request", "Cancellation is handled by the control reader.", false)),
            AppCommand::GetState => Ok(ControlOutput::State { state: view.snapshot(instance) }),
            AppCommand::Shutdown => {
                event_loop.exit();
                Ok(ControlOutput::Stopped)
            }
            AppCommand::SetCamera { camera, expected_revision } => {
                view.check_revision(expected_revision)?;
                let camera = camera.validated()?;
                view.camera.set_pose(&camera);
                // the camera is the controller's until a lineup or a view is picked
                view.lineups.free = true;
                view.fly = Fly::default();
                let _ = view.window.set_cursor_grab(CursorGrabMode::None);
                view.window.set_cursor_visible(true);
                view.revision += 1;
                view.window.request_redraw();
                match view.draw(false) {
                    Ok(true) => Ok(ControlOutput::State { state: view.snapshot(instance) }),
                    Ok(false) => Err(ControlError::new("capture_unavailable", "Camera applied, but the window cannot currently present a frame. Inspect app_status or make the window visible.", true)),
                    Err(error) => Err(ControlError::new("render_failed", error.to_string(), true)),
                }
            }
            AppCommand::Capture { include_ui, expected_revision } => {
                view.check_revision(expected_revision)?;
                view.capture_include_ui = include_ui;
                let drawn = view.draw(true);
                view.capture_include_ui = true;
                match drawn {
                    Ok(true) => {
                        let state = view.snapshot(instance);
                        let buffer = view.readback.take().ok_or_else(|| ControlError::new("capture_unavailable", "No frame readback was produced.", true))?;
                        let (width, height) = view.readback_size;
                        let png = control::capture_at_revision(expected_revision, state.revision, || {
                            encode_png(&view.gpu, &buffer, width, height, [2, 1, 0])
                                .map_err(|error| ControlError::new("capture_failed", error.to_string(), true))
                        })?;
                        Ok(ControlOutput::Capture { state, png_base64: base64::prelude::BASE64_STANDARD.encode(png), include_ui, width, height })
                    }
                    Ok(false) => Err(ControlError::new("capture_unavailable", "The window surface is occluded or unavailable; use view_render for headless capture.", true)),
                    Err(error) => Err(ControlError::new("capture_failed", error.to_string(), true)),
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                if let Some(view) = self.view.as_mut()
                    && view.lineups.receive(&view.gpu, &mut view.map, &mut view.name)
                {
                    view.window.request_redraw();
                }
            }
            UserEvent::Disconnected => self.detach(event_loop),
            UserEvent::Command(request) => {
                if !self.control.as_ref().is_some_and(Control::connected) { self.detach(event_loop); return; }
                let allowed = self.control.as_ref().expect("connected control").begin(&request);
                let result = allowed.and_then(|()| self.command(event_loop, request.command));
                self.send(event_loop, AppMessage::Reply { id: request.id, result });
            }
        }
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(loader) = self.loader.take() else { return };
        clock::mark("resumed");
        let lineups = self.lineups.take().expect("the lineups start with the app");
        let first = View::new(event_loop, &self.instance, loader, &self.args, lineups).and_then(|view| {
            let capture = self.args.screenshot.is_some();
            let view = self.view.insert(view);
            // the first frame is drawn as soon as the window exists, not on a later turn of the event loop
            if view.draw(capture)? {
                self.presented(event_loop)
            } else {
                view.window.request_redraw();
                Ok(())
            }
        });
        if let Err(failure) = first {
            self.failure = Some(failure);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(view) = self.view.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event: ref key, .. } if view.modifiers.super_key() && key.physical_key == PhysicalKey::Code(winit::keyboard::KeyCode::KeyQ) => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => {
                view.modifiers = modifiers.state();
                view.input(&WindowEvent::ModifiersChanged(modifiers));
            }
            WindowEvent::Resized(size) => view.resize(size.width, size.height),
            // the first frame may have been drawn before the window was on screen
            WindowEvent::Occluded(true) => view.window_visible = false,
            WindowEvent::Occluded(false) => {
                view.window_visible = true;
                view.window.request_redraw();
                if !self.visible {
                    clock::mark("window:visible");
                    self.visible = true;
                    // the other maps wait until the window is on screen, so that nothing competes with its showing
                    if view.frames > 0 {
                        view.lineups.open_other_maps(&view.gpu, &view.name);
                    }
                    self.exit_once_on_screen(event_loop);
                    self.notify_ready(event_loop);
                }
            }
            WindowEvent::RedrawRequested => {
                if self.switching.is_some() {
                    self.bench_switch(event_loop);
                    return;
                }
                let capture = view.frames == 0 && self.args.screenshot.is_some();
                let frame_started = Instant::now();
                let drawn = view.draw(capture);
                let frame_ms = frame_started.elapsed().as_secs_f64() * 1e3;
                let drew = matches!(&drawn, Ok(true));
                let result = match drawn {
                    Ok(true) => self.presented(event_loop),
                    other => other.map(|_| ()),
                };
                if let Err(failure) = result {
                    self.failure = Some(failure);
                    event_loop.exit();
                    return;
                }
                if self.args.bench_switch.is_some() {
                    self.bench_switch(event_loop);
                }
                if let (Some(bench), Some(view)) = (self.maps_bench.as_mut(), self.view.as_mut()) {
                    match bench.step(&mut view.lineups, &view.name, drew, frame_ms) {
                        Some(report) => {
                            println!("{report}");
                            event_loop.exit();
                        }
                        None => view.window.request_redraw(),
                    }
                }
            }
            event => view.input(&event),
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        if let (Some(view), DeviceEvent::MouseMotion { delta: (dx, dy) }) = (self.view.as_mut(), event)
            && view.fly.looking
        {
            view.fly.look(&mut view.camera, dx, dy);
            view.lineups.free = true;
            view.revision += 1;
            view.window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.control.as_ref().is_some_and(|control| !control.connected()) { self.detach(event_loop); }
        if self.exit_deadline.is_some() {
            self.exit_once_on_screen(event_loop);
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(5)));
            return;
        }
        if let Some(view) = &mut self.view {
            if view.fly.moving() || self.args.bench_frames.is_some() || self.args.bench_switch.is_some() || view.lineups.animating() {
                view.window.request_redraw();
            }
            // an idle window still notices an ingest, and reads the lineups again
            view.lineups.sandbox.check_data();
            if self.args.bench_frames.is_none() && self.args.bench_switch.is_none() {
                event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(5)));
            }
        }
    }
}

/// The window and what draws into it.
struct View {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    targets: Targets,
    gpu: Gpu,
    map: Map,
    name: String,
    started: Instant,
    egui: egui_winit::State,
    painter: egui_wgpu::Renderer,
    camera: Camera,
    fly: Fly,
    modifiers: winit::keyboard::ModifiersState,
    frames: u64,
    last_frame: Instant,
    /// The first frame's pixels, copied out for `--screenshot`.
    readback: Option<wgpu::Buffer>,
    readback_size: (u32, u32),
    capture_include_ui: bool,
    revision: u64,
    rendered_revision: u64,
    rendered_time: f64,
    frame_cpu_ms: VecDeque<f64>,
    window_visible: bool,
    collect_metrics: bool,
    observed: Option<ViewSignature>,
    /// The lineups, their stages and the shell (lineups.rs).
    lineups: lineups::Lineups,
    /// The map the targets were made for ([`Map::id`]): another map's post-processing is its own.
    targets_map: u64,
    /// Wait for the GPU after each frame (the switch benches measure whole frames).
    finish_frames: bool,
}

#[derive(PartialEq)]
struct ViewSignature {
    map: String,
    camera: nades_control::CameraPose,
    scene: nades_control::SceneSummary,
    viewport: nades_control::ViewportRect,
    window_size: (u32, u32),
}

impl ViewSignature {
    fn observe(self, previous: &mut Option<Self>, revision: &mut u64) {
        if previous.as_ref().is_some_and(|previous| previous != &self) { *revision += 1; }
        *previous = Some(self);
    }
}

impl View {
    fn new(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, loader: JoinHandle<Result<Loaded>>, args: &Args, mut lineups: lineups::Lineups) -> Result<Self> {
        let attributes = Window::default_attributes().with_title(TITLE).with_inner_size(LogicalSize::new(WINDOW.0, WINDOW.1));
        let window = Arc::new(event_loop.create_window(attributes)?);
        if args.control_stdio { window.focus_window(); }
        clock::mark("window");
        let surface = instance.create_surface(window.clone())?;
        clock::mark("surface");
        let Loaded { gpu, map, name, targets, painter, egui: context } = loader.join().map_err(|_| anyhow!("the loader thread panicked"))??;
        clock::mark("loaded");
        if !surface.get_capabilities(&gpu.adapter).formats.contains(&SURFACE_FORMAT) {
            bail!("the window's surface cannot show {SURFACE_FORMAT:?}");
        }
        let size = window.inner_size();
        let mut config = surface.get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1)).context("the window's surface does not work with this GPU")?;
        config.format = SURFACE_FORMAT;
        config.present_mode = if args.bench_frames.is_some() || args.bench_switch.is_some() || args.bench_maps.is_some() { wgpu::PresentMode::AutoNoVsync } else { wgpu::PresentMode::AutoVsync };
        if args.screenshot.is_some() || args.control_stdio {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&gpu.device, &config);
        // the loader made the targets for the 3D view in a Retina window of the usual size; any other size gets its own
        let scale = window.scale_factor();
        let (width, height) = lineups::view_size((f64::from(config.width) / scale, f64::from(config.height) / scale), scale);
        let targets = if targets.size() == (width, height) { targets } else { map.targets(&gpu.device, &gpu.queue, width, height) };
        lineups.rect = egui::Rect::from_min_size(egui::pos2(shell::LEFT, shell::TOP), egui::vec2((f64::from(width) / scale) as f32, (f64::from(height) / scale) as f32));
        lineups.sandbox.toward_sun = lineups::toward_sun(&map);
        lineups.resident.budget = resident::budget(&gpu, args.budget_mb);
        lineups.shown_bytes = map.gpu_bytes;
        clock::mark("configured");
        let max_texture = gpu.device.limits().max_texture_dimension_2d as usize;
        let egui = egui_winit::State::new(context, egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, Some(max_texture));
        clock::mark("egui");
        let targets_map = map.id;
        Ok(Self { window, surface, config, targets, gpu, map, name, started: Instant::now(), egui, painter, camera: Camera::bench(), fly: Fly::default(), modifiers: Default::default(), frames: 0, last_frame: Instant::now(), readback: None, readback_size: (0, 0), capture_include_ui: true, revision: 0, rendered_revision: 0, rendered_time: 0.0, frame_cpu_ms: VecDeque::new(), window_visible: false, collect_metrics: args.control_stdio, observed: None, targets_map, lineups, finish_frames: args.bench_maps.is_some() })
    }

    fn check_revision(&self, expected: Option<u64>) -> std::result::Result<(), ControlError> {
        control::check_revision(expected, self.revision)
    }

    fn scene_rect(&self) -> nades_control::ViewportRect {
        let scale = self.window.scale_factor() as f32;
        let x = (self.lineups.rect.min.x * scale).round().max(0.0) as u32;
        let y = (self.lineups.rect.min.y * scale).round().max(0.0) as u32;
        let (width, height) = self.targets.size();
        nades_control::ViewportRect { x, y, width: width.min(self.config.width.saturating_sub(x)), height: height.min(self.config.height.saturating_sub(y)) }
    }

    fn refresh_revision(&mut self) {
        if !self.collect_metrics { return; }
        let current = ViewSignature {
            map: self.name.clone(), camera: self.camera.pose(), scene: self.lineups.summary(),
            viewport: self.scene_rect(), window_size: (self.config.width, self.config.height),
        };
        current.observe(&mut self.observed, &mut self.revision);
    }

    fn snapshot(&self, instance_id: &str) -> AppState {
        let mut ms: Vec<f64> = self.frame_cpu_ms.iter().copied().collect();
        ms.sort_by(f64::total_cmp);
        let at = |fraction: f64| ms.get(((ms.len().saturating_sub(1)) as f64 * fraction).round() as usize).copied();
        let pre_main = clock::pre_main();
        AppState {
            instance_id: instance_id.into(), map: self.name.clone(), camera: self.camera.pose(),
            viewport: Viewport { width: self.config.width, height: self.config.height, scale_factor: self.window.scale_factor(), visible: self.window_visible },
            scene_viewport: self.scene_rect(),
            frame_id: self.frames, revision: self.revision, presented_revision: self.rendered_revision, render_time_seconds: self.rendered_time,
            gpu: self.gpu.name(), draws: self.map.draws as u32, triangles: self.map.triangles, scene: self.lineups.summary(),
            startup_marks: clock::marks().into_iter().map(|(name, ms)| (name.into(), ms)).collect(),
            pre_main_ms: pre_main.is_finite().then_some(pre_main),
            metrics: FrameMetrics {
                last_cpu_ms: self.frame_cpu_ms.back().copied(), median_cpu_ms: at(0.5), p95_cpu_ms: at(0.95), samples: ms.len(), gpu_ms: None,
                memory: nades_render::memory::footprint().map(|value| nades_control::MemoryUsage {
                    physical_footprint_bytes: value.now, peak_physical_footprint_bytes: value.peak, graphics_ledger_bytes: value.graphics,
                }),
            },
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width, height) == (self.config.width, self.config.height) {
            return;
        }
        self.revision += 1;
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.gpu.device, &self.config);
        // the 3D view's targets follow its rectangle between the panels, in the next frame
        self.window.request_redraw();
    }

    fn input(&mut self, event: &WindowEvent) {
        let response = self.egui.on_window_event(&self.window, event);
        let over_shell = self.egui.egui_ctx().is_pointer_over_egui();
        if self.lineups.input(event, over_shell, self.window.scale_factor()) {
            self.revision += 1;
            self.window.request_redraw();
            return;
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } if !response.consumed && !self.egui.egui_ctx().egui_wants_keyboard_input() => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    // the fly keys take the camera from the lineup's view
                    let fly = matches!(code, winit::keyboard::KeyCode::KeyW | winit::keyboard::KeyCode::KeyA | winit::keyboard::KeyCode::KeyS | winit::keyboard::KeyCode::KeyD | winit::keyboard::KeyCode::KeyE | winit::keyboard::KeyCode::KeyQ);
                    if fly && event.state.is_pressed() && !self.lineups.free {
                        if let Some(shown) = self.lineups.camera() {
                            self.camera.set_pose(&lineups::pose_of(&shown));
                        }
                        self.lineups.free = true;
                    }
                    self.fly.key(code, event.state.is_pressed());
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => {
                self.fly.looking = state.is_pressed() && !self.egui.egui_ctx().is_pointer_over_egui();
                let grab = if self.fly.looking { CursorGrabMode::Locked } else { CursorGrabMode::None };
                let _ = self.window.set_cursor_grab(grab);
                self.window.set_cursor_visible(!self.fly.looking);
            }
            WindowEvent::Focused(false) => self.fly = Fly::default(),
            _ => {}
        }
        if response.repaint || self.fly.moving() {
            self.window.request_redraw();
        }
    }

    /// Draws a frame and presents it; false when there was no surface texture to draw into. The first frame waits
    /// for the GPU before it is presented, and `capture` copies it out for the screenshot.
    fn draw(&mut self, capture: bool) -> Result<bool> {
        let first = self.frames == 0;
        let mut laps = Laps::new();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Frame::Surface(frame),
            wgpu::CurrentSurfaceTexture::Occluded if first => match nades_render::EarlyFrame::acquire(&self.gpu.device, &self.surface, &self.config) {
                Some(frame) => Frame::Early(frame),
                None => return Ok(false),
            },
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                self.window.request_redraw();
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Validation => return Ok(false),
        };
        laps.lap("acquire");
        let now = Instant::now();
        let cpu_started = now;
        self.fly.step(&mut self.camera, (now - self.last_frame).as_secs_f32().min(0.05));
        self.last_frame = now;
        let (width, height) = (self.config.width, self.config.height);

        if !first {
            self.lineups.receive(&self.gpu, &mut self.map, &mut self.name);
        }
        laps.lap("receive");
        let input = self.egui.take_egui_input(&self.window);
        let timings = self.lineups.timings();
        let lineups = &mut self.lineups;
        let output = self.egui.egui_ctx().run_ui(input, |ui| lineups.ui(ui, timings.clone()));
        self.egui.handle_platform_output(&self.window, output.platform_output);
        let dt = (now - self.started).as_secs_f64() - self.rendered_time;
        laps.lap("shell");
        self.lineups.update(&self.gpu, &mut self.map, &mut self.name, self.egui.egui_ctx(), dt.clamp(0.0, 0.1));
        laps.lap("update");
        // the 3D view: between the panels, at the window's scale, from the lineup's view (or the camera taken by hand)
        let scale = self.window.scale_factor() as f32;
        let rect = self.lineups.rect;
        let size = ((rect.width() * scale).round().max(1.0) as u32, (rect.height() * scale).round().max(1.0) as u32);
        if size != self.targets.size() || self.targets_map != self.map.id {
            self.targets = self.map.targets(&self.gpu.device, &self.gpu.queue, size.0, size.1);
            self.targets_map = self.map.id;
            laps.lap("targets");
        }
        let shown = self.lineups.camera();
        if let Some(shown) = shown {
            self.camera.set_pose(&lineups::pose_of(&shown));
        }
        // a map with no lineup's view yet stays clear, rather than drawn from another map's camera
        let show_map = shown.is_some() || self.lineups.bench || self.lineups.free;
        let eye = self.camera.view();
        let Some(effects) = self.lineups.frame(&self.gpu, &mut self.map, &eye, size, first || capture) else {
            // the picture is final (its smoke averaged): nothing to draw
            return Ok(false);
        };
        self.refresh_revision();
        let jobs = self.egui.egui_ctx().tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [width, height], pixels_per_point: output.pixels_per_point };
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.painter.update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
            }
        }

        let target = frame.texture().create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let callbacks = self.painter.update_buffers(&self.gpu.device, &self.gpu.queue, &mut encoder, &jobs, &screen);
        self.rendered_time = self.started.elapsed().as_secs_f32() as f64;
        if show_map {
            self.map.render(&self.gpu.queue, &mut encoder, &self.targets, &eye, self.rendered_time as f32, &effects);
        }
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("present and shell"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(CLEAR), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            let (x, y) = ((rect.min.x * scale).round().max(0.0), (rect.min.y * scale).round().max(0.0));
            let (w, h) = ((size.0 as f32).min(width as f32 - x), (size.1 as f32).min(height as f32 - y));
            if show_map && w >= 1.0 && h >= 1.0 {
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                self.map.present(&mut pass, &self.targets);
                pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
            }
            if !capture || self.capture_include_ui { self.painter.render(&mut pass, &jobs, &screen); }
        }
        if capture {
            let rect = self.scene_rect();
            let (x, y, copy_width, copy_height) = if self.capture_include_ui {
                (0, 0, width, height)
            } else { (rect.x, rect.y, rect.width, rect.height) };
            if copy_width == 0 || copy_height == 0 { bail!("the scene viewport has no drawable area"); }
            let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("screenshot"), size: u64::from(row_bytes(copy_width) * copy_height), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            let mut source = frame.texture().as_image_copy();
            source.origin = wgpu::Origin3d { x, y, z: 0 };
            encoder.copy_texture_to_buffer(
                source,
                wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row_bytes(copy_width)), rows_per_image: Some(copy_height) } },
                wgpu::Extent3d { width: copy_width, height: copy_height, depth_or_array_layers: 1 },
            );
            self.readback = Some(buffer);
            self.readback_size = (copy_width, copy_height);
        }
        if capture && !self.capture_include_ui {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shell after scene capture"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })], ..Default::default()
            }).forget_lifetime();
            self.painter.render(&mut pass, &jobs, &screen);
        }
        for id in &output.textures_delta.free {
            self.painter.free_texture(id);
        }
        laps.lap("encode");
        self.gpu.queue.submit(callbacks.into_iter().chain([encoder.finish()]));
        laps.lap("submit");
        if first {
            clock::mark("frame:submitted");
            // the first frame is finished on the GPU before it is presented
            self.gpu.wait_idle();
            clock::mark("frame:gpu");
        } else if self.finish_frames {
            self.gpu.wait_idle();
            laps.lap("gpu");
        }
        self.window.pre_present_notify();
        match frame {
            Frame::Surface(frame) => self.gpu.queue.present(frame),
            Frame::Early(frame) => {
                frame.when_on_screen(|ago| match ago {
                    Some(ago) => {
                        clock::mark_ago("frame:on_screen", ago);
                    }
                    None => {
                        clock::mark("frame:skipped");
                    }
                });
                frame.present();
            }
        }
        laps.lap("present");
        self.lineups.frame_parts = laps.parts;
        self.frames += 1;
        self.rendered_revision = self.revision;
        if !first && self.collect_metrics {
            if self.frame_cpu_ms.len() == 120 { self.frame_cpu_ms.pop_front(); }
            self.frame_cpu_ms.push_back(cpu_started.elapsed().as_secs_f64() * 1000.0);
        }
        Ok(true)
    }

    /// Writes the captured first frame as an 8-bit sRGB PNG (the surface is BGRA).
    fn save_screenshot(&self, path: &Path) -> Result<()> {
        let buffer = self.readback.as_ref().context("no frame was captured")?;
        write_png(&self.gpu, buffer, self.config.width, self.config.height, path, [2, 1, 0])
    }
}

/// What a frame draws into: the surface's next texture, or for a new window's first frame, its layer's
/// ([`nades_render::EarlyFrame`]).
enum Frame {
    Surface(wgpu::SurfaceTexture),
    Early(nades_render::EarlyFrame),
}

impl Frame {
    fn texture(&self) -> &wgpu::Texture {
        match self {
            Frame::Surface(frame) => &frame.texture,
            Frame::Early(frame) => &frame.texture,
        }
    }
}

/// Where a frame's time goes on the window's thread: each part's ms, in order.
struct Laps {
    last: Instant,
    parts: Vec<(&'static str, f64)>,
}

impl Laps {
    fn new() -> Self {
        Self { last: Instant::now(), parts: Vec::new() }
    }

    fn lap(&mut self, part: &'static str) {
        let now = Instant::now();
        self.parts.push((part, (now - self.last).as_secs_f64() * 1e3));
        self.last = now;
    }
}

/// A row of BGRA8 texels padded for a texture-to-buffer copy.
fn row_bytes(width: u32) -> u32 {
    (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

#[cfg(test)]
mod control_view_tests {
    use super::*;

    fn signature(map: &str) -> ViewSignature {
        ViewSignature {
            map: map.into(), camera: Camera::bench().pose(), scene: nades_control::SceneSummary::default(),
            viewport: nades_control::ViewportRect { x: 0, y: 0, width: 100, height: 100 }, window_size: (100, 100),
        }
    }

    #[test]
    fn a_map_only_change_invalidates_the_scene_revision() {
        let mut observed = None;
        let mut revision = 7;
        signature("de_cache").observe(&mut observed, &mut revision);
        signature("de_cache").observe(&mut observed, &mut revision);
        assert_eq!(revision, 7);
        signature("de_mirage").observe(&mut observed, &mut revision);
        assert_eq!(revision, 8);
        assert_eq!(observed.unwrap().map, "de_mirage");
    }
}
