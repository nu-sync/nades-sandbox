//! nades-sandbox, M2: a map in CS2's look (its materials, baked light, sky, 3D skybox, fog and post-processing) in a
//! window with a fly camera and the egui shell, on screen as soon after the process starts as it can be.
//!
//! Start-up runs on two threads from `main`: this one has AppKit make the window (≈76 ms), while the loader maps the
//! pack, brings up the GPU, puts the map on it, records its draws and draws one frame offscreen. The first frame
//! needs both.
//!
//! - `--map <name>` draws `~/.cache/nades/packs/<name>.pack` (de_cache by default; `NADES_PACK` names a file).
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
mod clock;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Context as _, Result, anyhow, bail};
use camera::{Camera, Fly};
use nades_pack::Pack;
use nades_render::map::{Map, Targets};
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
            "--map" => args.map = Some(it.next().context("--map needs a map name")?),
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
    if let Err(failure) = run() {
        eprintln!("nades-sandbox: {failure:#}");
        std::process::exit(1);
    }
    // the GPU objects need no tearing down: the process is done
    std::process::exit(0);
}

fn run() -> Result<()> {
    let args = args()?;
    if let Some(out) = &args.render {
        return render_offscreen(&args, out);
    }
    if let Some(frames) = args.bench_offscreen {
        return bench_offscreen(&args, frames);
    }
    let instance = nades_render::instance();
    let loader = std::thread::Builder::new().name("loader".into()).spawn({
        let instance = instance.clone();
        let path = pack_path(args.map.as_deref());
        move || load(instance, &path)
    })?;
    let mut builder = EventLoop::builder();
    // no menu bar: building AppKit's default one costs the launch ≈8 ms (Cmd+Q is handled as a key instead)
    #[cfg(target_os = "macos")]
    winit::platform::macos::EventLoopBuilderExtMacOS::with_default_menu(&mut builder, false);
    let event_loop = builder.build()?;
    clock::mark("event_loop");
    event_loop.set_control_flow(if args.bench_frames.is_some() { ControlFlow::Poll } else { ControlFlow::Wait });
    let mut app = App { args, instance, loader: Some(loader), view: None, failure: None, bench: Vec::new(), visible: false };
    event_loop.run_app(&mut app)?;
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

fn load(instance: wgpu::Instance, path: &Path) -> Result<Loaded> {
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
    let (width, height) = ((WINDOW.0 * EXPECTED_SCALE) as u32, (WINDOW.1 * EXPECTED_SCALE) as u32);
    let targets = map.targets(&gpu.device, &gpu.queue, width, height);
    let mut painter = egui_wgpu::Renderer::new(&gpu.device, SURFACE_FORMAT, egui_wgpu::RendererOptions { depth_stencil_format: None, dithering: false, ..Default::default() });
    let name = pack.contents().source.map.clone();
    let egui = warm_egui(&gpu, &mut painter, &map, &name);
    clock::mark("load:egui");
    warm_up(&gpu, &map, &targets);
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
    let eye = Eye::from_setpos(setpos, pitch, yaw, args.eye.unwrap_or(64.0));
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
    map.render(&gpu.queue, &mut encoder, &targets, &eye, 0.0);
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
        map.render(&gpu.queue, &mut encoder, &targets, &view, k as f32 / 60.0);
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
    buffer.slice(..).map_async(wgpu::MapMode::Read, |mapped| mapped.expect("the frame maps"));
    gpu.wait_idle();
    let bytes = buffer.slice(..).get_mapped_range()?;
    let rgb: Vec<u8> = bytes.chunks_exact(row_bytes(width) as usize).flat_map(|row| row[..width as usize * 4].chunks_exact(4).flat_map(|t| order.map(|k| t[k]))).collect();
    let mut png = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?), width, height);
    png.set_color(png::ColorType::Rgb);
    png.set_depth(png::BitDepth::Eight);
    png.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    png.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

/// egui with the shell laid out once at the usual Retina scale: its fonts parsed, the glyphs it shows rasterized and
/// their atlas on the GPU before the window exists. (Another scale only costs the first frame that work again.)
fn warm_egui(gpu: &Gpu, painter: &mut egui_wgpu::Renderer, map: &Map, name: &str) -> egui::Context {
    let context = egui::Context::default();
    context.set_theme(egui::Theme::Dark);
    // the same atlas size egui-winit asks for, or the atlas is rebuilt
    let max_texture_side = Some(gpu.device.limits().max_texture_dimension_2d as usize);
    let mut input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WINDOW.0 as f32, WINDOW.1 as f32))), max_texture_side, ..Default::default() };
    input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(EXPECTED_SCALE as f32);
    let status = Status { map: name.to_string(), gpu: gpu.name(), draws: map.draws, triangles: map.triangles, marks: clock::marks(), pre_main: clock::pre_main(), camera: glam::Vec3::ZERO };
    let output = context.run_ui(input, |ui| shell(ui, &status));
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
fn warm_up(gpu: &Gpu, map: &Map, targets: &Targets) {
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
    let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("warm-up") });
    map.render(&gpu.queue, &mut encoder, targets, &Camera::bench().view(), 0.0);
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
}

impl App {
    /// `--exit-after-first-frame` reports once the map can be on screen: its first frame presented and the window
    /// visible, in either order.
    fn exit_once_on_screen(&self, event_loop: &ActiveEventLoop) {
        if self.args.exit_after_first_frame && self.visible && self.view.as_ref().is_some_and(|view| view.frames > 0) {
            println!("{}", clock::json());
            event_loop.exit();
        }
    }

    /// After a frame is presented: the flags' exits, and the bench's clock.
    fn presented(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let view = self.view.as_mut().expect("a view draws");
        if view.frames == 1 {
            clock::mark("frame:presented");
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
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(loader) = self.loader.take() else { return };
        clock::mark("resumed");
        let first = View::new(event_loop, &self.instance, loader, &self.args).and_then(|view| {
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
            WindowEvent::Occluded(false) => {
                view.window.request_redraw();
                if !self.visible {
                    clock::mark("window:visible");
                    self.visible = true;
                    self.exit_once_on_screen(event_loop);
                }
            }
            WindowEvent::RedrawRequested => {
                let capture = view.frames == 0 && self.args.screenshot.is_some();
                let result = match view.draw(capture) {
                    Ok(true) => self.presented(event_loop),
                    other => other.map(|_| ()),
                };
                if let Err(failure) = result {
                    self.failure = Some(failure);
                    event_loop.exit();
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
            view.window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(view) = &self.view
            && (view.fly.moving() || self.args.bench_frames.is_some())
        {
            view.window.request_redraw();
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
}

impl View {
    fn new(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, loader: JoinHandle<Result<Loaded>>, args: &Args) -> Result<Self> {
        let attributes = Window::default_attributes().with_title(TITLE).with_inner_size(LogicalSize::new(WINDOW.0, WINDOW.1));
        let window = Arc::new(event_loop.create_window(attributes)?);
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
        config.present_mode = if args.bench_frames.is_some() { wgpu::PresentMode::AutoNoVsync } else { wgpu::PresentMode::AutoVsync };
        if args.screenshot.is_some() {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&gpu.device, &config);
        // the loader made the targets for a Retina window of the usual size; any other size gets its own
        let targets = if targets.size() == (config.width, config.height) { targets } else { map.targets(&gpu.device, &gpu.queue, config.width, config.height) };
        clock::mark("configured");
        let max_texture = gpu.device.limits().max_texture_dimension_2d as usize;
        let egui = egui_winit::State::new(context, egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, Some(max_texture));
        clock::mark("egui");
        Ok(Self { window, surface, config, targets, gpu, map, name, started: Instant::now(), egui, painter, camera: Camera::bench(), fly: Fly::default(), modifiers: Default::default(), frames: 0, last_frame: Instant::now(), readback: None })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width, height) == (self.config.width, self.config.height) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.gpu.device, &self.config);
        self.targets = self.map.targets(&self.gpu.device, &self.gpu.queue, width, height);
        self.window.request_redraw();
    }

    fn input(&mut self, event: &WindowEvent) {
        let response = self.egui.on_window_event(&self.window, event);
        match event {
            WindowEvent::KeyboardInput { event, .. } if !response.consumed => {
                if let PhysicalKey::Code(code) = event.physical_key {
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
        let now = Instant::now();
        self.fly.step(&mut self.camera, (now - self.last_frame).as_secs_f32().min(0.05));
        self.last_frame = now;
        let (width, height) = (self.config.width, self.config.height);

        let input = self.egui.take_egui_input(&self.window);
        let status = Status { map: self.name.clone(), gpu: self.gpu.name(), draws: self.map.draws, triangles: self.map.triangles, marks: clock::marks(), pre_main: clock::pre_main(), camera: self.camera.position };
        let output = self.egui.egui_ctx().run_ui(input, |ui| shell(ui, &status));
        self.egui.handle_platform_output(&self.window, output.platform_output);
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
        self.map.render(&self.gpu.queue, &mut encoder, &self.targets, &self.camera.view(), self.started.elapsed().as_secs_f32());
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
            self.map.present(&mut pass, &self.targets);
            self.painter.render(&mut pass, &jobs, &screen);
        }
        for id in &output.textures_delta.free {
            self.painter.free_texture(id);
        }
        if capture {
            let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("screenshot"), size: u64::from(row_bytes(width) * height), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            encoder.copy_texture_to_buffer(
                frame.texture().as_image_copy(),
                wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row_bytes(width)), rows_per_image: Some(height) } },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
            self.readback = Some(buffer);
        }
        self.gpu.queue.submit(callbacks.into_iter().chain([encoder.finish()]));
        if first {
            clock::mark("frame:submitted");
            // the first frame is finished on the GPU before it is presented
            self.gpu.wait_idle();
            clock::mark("frame:gpu");
        }
        self.window.pre_present_notify();
        match frame {
            Frame::Surface(frame) => self.gpu.queue.present(frame),
            Frame::Early(frame) => frame.present(),
        }
        self.frames += 1;
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

/// A row of BGRA8 texels padded for a texture-to-buffer copy.
fn row_bytes(width: u32) -> u32 {
    (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

/// What the shell shows.
struct Status {
    map: String,
    gpu: String,
    draws: usize,
    triangles: u64,
    marks: Vec<(&'static str, f64)>,
    pre_main: f64,
    camera: glam::Vec3,
}

/// The shell: a 300-point panel with the map's size and how long this start took.
fn shell(ui: &mut egui::Ui, status: &Status) {
    egui::Panel::left("shell").exact_size(300.0).show(ui, |ui| {
        ui.heading(TITLE);
        ui.label(format!("{}: {} draws, {:.2} M triangles", status.map, status.draws, status.triangles as f64 / 1e6));
        ui.label(format!("GPU: {}", status.gpu));
        ui.separator();
        ui.strong("Start-up, ms since main");
        if status.pre_main.is_finite() {
            ui.label(format!("before main: {:.1}", status.pre_main));
        }
        egui::Grid::new("marks").num_columns(2).show(ui, |ui| {
            for (stage, at) in &status.marks {
                ui.label(*stage);
                ui.label(format!("{at:.1}"));
                ui.end_row();
            }
        });
        ui.separator();
        ui.label("WASD to move, Shift faster, E and Q up and down; hold the right mouse button to look.");
        let p = status.camera;
        ui.label(format!("camera at {:.2}, {:.2}, {:.2} m", p.x, p.y, p.z));
    });
}
