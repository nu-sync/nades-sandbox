//! A scene without a window: `--scene file.json --time s --render out.png` draws one moment of it, and
//! `--render-video request.json` a clip (PNG frames on stdout, for the MCP server's encoder). Both read the pack, the
//! collider and the game's effects at once, simulate every throw on every core, and print what each throw did as JSON
//! lines on stdout (`--render`) or stderr (videos), for checks to read.

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use nades_control::scene::{Scene, ViewName};
use nades_render::map::{Map, Targets};
use nades_render::{Gpu, View as Eye};
use rayon::prelude::*;

use crate::sim::{self, Placed};
use crate::{app, resident, views};

/// A scene's map on the GPU with the game's effects, and its throws simulated.
pub struct Staged {
    pub gpu: Gpu,
    pub map: Map,
    pub targets: Targets,
    pub placed: Vec<Placed>,
    pub collider: Option<nades_scene::collider::Collider>,
    /// What the map draws, as a collider, for a scripted shot's camera (S36); built only for one.
    pub sight: Option<nades_scene::collider::Collider>,
}

/// Puts `scene`'s map on the GPU for frames of `size`, and simulates its throws against the map's collider.
pub fn stage(scene: &Scene, (width, height): (u32, u32), arcs: Option<Option<usize>>) -> Result<Staged> {
    let (pack, data) = resident::open_cached(&crate::pack_path(Some(&scene.map)))?;
    let gpu = Gpu::new(nades_render::instance(), None)?;
    if !gpu.capabilities.bindless {
        bail!("{} has no texture binding arrays, which the renderer draws every material through", gpu.name());
    }
    if width > gpu.device.limits().max_texture_dimension_2d || height > gpu.device.limits().max_texture_dimension_2d {
        bail!("{width}×{height} is more than the GPU draws");
    }
    let mut map = Map::new(&gpu.device, &gpu.queue, pack, &data, crate::OFFSCREEN_FORMAT, &|_| {});
    let targets = map.targets(&gpu.device, &gpu.queue, width, height);
    map.ready_clouds(&gpu.device, &targets);
    let (noise, _, books) = crate::assets::effects();
    map.composite.set_noise(&gpu.device, &nades_render::smoke::noise_texture(&gpu.device, &gpu.queue, &noise));
    if let Some(books) = books {
        map.throw.set_flipbooks(&gpu.device, &books.upload(&gpu.device, &gpu.queue));
    }
    let colliders = crate::assets::colliders(&scene.map).map_err(|e| eprintln!("collider: {e:#} (grenades fly through walls, no smoke or fire)")).ok();
    let sun = app::toward_sun(&map);
    let placed: Vec<Placed> = scene.throws.par_iter().map(|t| (Arc::new(sim::simulate(t, colliders.as_ref(), sun)), t.start)).collect();
    let collider = colliders.map(|c| c.sight);
    if let Some(strong) = arcs {
        let arcs: Vec<_> = placed.iter().enumerate().map(|(k, (b, _))| b.arc(strong == Some(k))).collect();
        map.throw.set_arcs(&gpu.device, &arcs);
    }
    Ok(Staged { gpu, map, targets, placed, collider, sight: None })
}

/// What a throw did, as one JSON line.
pub fn report(k: usize, b: &sim::Built, start: f64) -> String {
    serde_json::to_string(&b.report(k, start)).expect("a report serializes")
}

/// Simulates a scene file's throws against its map's collider, with no GPU, and prints what each did, one JSON line
/// each (`--scene f.json --report`).
pub fn simulate(path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let scene = Scene::parse(&text).map_err(|e| anyhow!("{}: {}", path.display(), e.message))?;
    let colliders = crate::assets::colliders(&scene.map).map_err(|e| eprintln!("collider: {e:#} (grenades fly through walls, no smoke or fire)")).ok();
    // the smoke's light is drawing's business: straight up will do for the report
    let built: Vec<_> = scene.throws.par_iter().map(|t| sim::simulate(t, colliders.as_ref(), glam::DVec3::Z)).collect();
    for (k, (b, t)) in built.iter().zip(&scene.throws).enumerate() {
        println!("{}", report(k, b, t.start));
    }
    Ok(())
}

/// The camera `view` of throw `k` shows at `t` on the scene's clock.
fn view_camera(s: &Staged, view: ViewName, k: usize, t: f64) -> Result<Eye> {
    let (b, start) = s.placed.get(k).with_context(|| format!("the scene has no throw {k} (it has {})", s.placed.len()))?;
    let camera = sim::views::camera(view, b, b.local(*start, t), None, s.collider.as_ref()).context("the free view needs --setpos and --setang")?;
    Ok(views::of(&camera))
}

/// Draws `t` of the staged scene from `eye`, its smokes averaged over the jittered frames a still view takes. A
/// device's first draws bloom unlike its later ones (S41), and a smoke's samples no longer run the bloom sixteen times
/// over (S46): the picture is drawn [`WARM_UP`] times before the one kept, as a video's first frame is.
fn draw(s: &mut Staged, eye: &Eye, t: f64, material_seconds: f32) {
    for _ in 0..WARM_UP {
        draw_with(s, eye, t, material_seconds, 1, None);
    }
    draw_with(s, eye, t, material_seconds, nades_render::smoke::ACCUMULATE, None);
}

/// Draws `t` from `eye`, a smoke averaged over `samples` jittered frames.
fn draw_with(s: &mut Staged, eye: &Eye, t: f64, material_seconds: f32, samples: u32, lineup_flash: Option<usize>) {
    let encoder = draw_into(s, eye, t, material_seconds, samples, lineup_flash);
    s.gpu.queue.submit([encoder.finish()]);
}

/// Records `t` from `eye`, a smoke averaged over `samples` jittered frames, and returns the frame's last commands
/// unsubmitted, for the caller's own passes to follow in the same submission. Only the smoke's pass differs from one
/// sample to the next, so the scene is drawn once, and the exposure and bloom once after the last sample (S46). A
/// sample's values go through the queue, which every command buffer of a submission reads alike: each sample but the
/// last is submitted here.
fn draw_into(s: &mut Staged, eye: &Eye, t: f64, material_seconds: f32, samples: u32, lineup_flash: Option<usize>) -> wgpu::CommandEncoder {
    let size = s.targets.size();
    let samples = if sim::smoking(&s.placed, t) { samples.max(1) } else { 1 };
    let effects = sim::show(&s.gpu, &mut s.map, &s.placed, t, eye, size, sim::FrameOptions { accumulated: 0, lineup_flash });
    let mut encoder = s.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scene") });
    s.map.render_scene(&s.gpu.queue, &mut encoder, &s.targets, eye, material_seconds, &effects);
    for k in 0..samples {
        if k > 0 {
            s.gpu.queue.submit([encoder.finish()]);
            encoder = s.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("smoke sample") });
            s.map.composite.set_sample(k);
        }
        s.map.render_composite(&s.gpu.queue, &mut encoder, &s.targets, eye, k);
    }
    s.map.render_post(&mut encoder, &s.targets);
    encoder
}

/// What `--scene` asks for.
pub struct Shot<'a> {
    pub scene: &'a Path,
    pub time: f64,
    pub view: Option<ViewName>,
    pub throw: Option<usize>,
    /// A CS2 console position and angles (and eye height), in place of a view.
    pub setpos: Option<([f32; 3], [f32; 2], f32)>,
    pub fov: Option<f32>,
    pub arcs: bool,
    pub size: (u32, u32),
    pub out: &'a Path,
}

/// One moment of a scene file, offscreen, written as a PNG; what each throw did, one JSON line each, on stdout.
pub fn render(shot: Shot<'_>) -> Result<()> {
    let text = std::fs::read_to_string(shot.scene).with_context(|| format!("reading {}", shot.scene.display()))?;
    let scene = Scene::parse(&text).map_err(|e| anyhow!("{}: {}", shot.scene.display(), e.message))?;
    let k = shot.throw.unwrap_or(0);
    let mut s = stage(&scene, shot.size, shot.arcs.then_some(shot.throw))?;
    for (i, (b, start)) in s.placed.iter().enumerate() {
        println!("{}", report(i, b, *start));
    }
    let mut eye = match shot.setpos {
        Some((setpos, [pitch, yaw], height)) => Eye::from_setpos(setpos, pitch, yaw, height),
        None => view_camera(&s, shot.view.unwrap_or(ViewName::Orbit), k, shot.time)?,
    };
    if let Some(fov) = shot.fov {
        eye.fov_y = fov.to_radians();
    }
    draw(&mut s, &eye, shot.time, 0.0);
    let active = sim::active(&s.placed, shot.time);
    crate::write_frame(&s.gpu, &s.map, &s.targets, shot.size, shot.out)?;
    eprintln!("{}: {} at {:.2} s, {} throws, showing {}", shot.out.display(), scene.map, shot.time, s.placed.len(), if active.is_empty() { "no effect".into() } else { active.join(", ") });
    Ok(())
}

/// A scene's steady frame offscreen (`--scene f.json --time s --bench-offscreen n`): `frames` frames of the moment `shot`
/// names at its size, as the window draws a moving view (one sample of a smoke a frame, the present pass included), each
/// finished on the GPU before the next, so a frame's time is its whole cost. One JSON line: the median and the 95th
/// percentile after the warm-up, and what showed.
pub fn bench(shot: Shot<'_>, frames: usize, warm_up: usize) -> Result<()> {
    let text = std::fs::read_to_string(shot.scene).with_context(|| format!("reading {}", shot.scene.display()))?;
    let scene = Scene::parse(&text).map_err(|e| anyhow!("{}: {}", shot.scene.display(), e.message))?;
    let (width, height) = shot.size;
    let mut s = stage(&scene, shot.size, None)?;
    let mut eye = match shot.setpos {
        Some((setpos, [pitch, yaw], height)) => Eye::from_setpos(setpos, pitch, yaw, height),
        None => view_camera(&s, shot.view.unwrap_or(ViewName::Orbit), shot.throw.unwrap_or(0), shot.time)?,
    };
    if let Some(fov) = shot.fov {
        eye.fov_y = fov.to_radians();
    }
    let output = s.gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let output = output.create_view(&Default::default());
    let mut ms = Vec::with_capacity(frames);
    for k in 0..frames {
        let started = std::time::Instant::now();
        // a little motion, as the frame bench has: nothing is cached between frames
        let mut moved = eye;
        moved.eye.x += (k % 2) as f32 * 0.01;
        let mut encoder = draw_into(&mut s, &moved, shot.time, k as f32 / 60.0, 1, None);
        s.map.present(
            &mut encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &output, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(crate::CLEAR), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            }),
            &s.targets,
        );
        s.gpu.queue.submit([encoder.finish()]);
        s.gpu.wait_idle();
        ms.push(started.elapsed().as_secs_f64() * 1e3);
    }
    let mut ms = ms.split_off(warm_up.min(frames.saturating_sub(1)));
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    println!("{}", serde_json::json!({"frames": frames, "width": width, "height": height, "time": shot.time, "showing": sim::active(&s.placed, shot.time), "median_ms": at(0.5), "p95_ms": at(0.95), "max_ms": at(1.0)}));
    Ok(())
}

/// CS2's crosshair, four green arms with a dark outline round the middle, as rectangles for a `width` × `height`
/// frame in painting order (the outlines, then the arms over them): `x0, y0, x1, y1` with the ends exclusive, and each
/// one's colour. They may reach past the frame.
pub fn crosshair_paints(width: u32, height: u32) -> [nades_render::yuv::Paint; 8] {
    let unit = (f64::from(height) / 720.0).max(0.5);
    let (gap, length, thick) = ((4.0 * unit).round() as i32, (9.0 * unit).round() as i32, (1.5 * unit).round().max(1.0) as i32);
    let (cx, cy) = (width as i32 / 2, height as i32 / 2);
    let arms = |o: i32| {
        [
            [cx + gap - o, cy - thick / 2 - o, cx + gap + length + o, cy + (thick + 1) / 2 + o],
            [cx - gap - length - o, cy - thick / 2 - o, cx - gap + o, cy + (thick + 1) / 2 + o],
            [cx - thick / 2 - o, cy + gap - o, cx + (thick + 1) / 2 + o, cy + gap + length + o],
            [cx - thick / 2 - o, cy - gap - length - o, cx + (thick + 1) / 2 + o, cy - gap + o],
        ]
    };
    let outline = (unit.round() as i32).max(1);
    let (dark, green) = (arms(outline), arms(0));
    std::array::from_fn(|k| if k < 4 { (dark[k], [0, 0, 0]) } else { (green[k - 4], [50, 250, 50]) })
}

/// Paints the crosshair on packed pixels of `channels` bytes each (RGB first).
pub fn crosshair(pixels: &mut [u8], width: u32, height: u32, channels: usize) {
    for ([x0, y0, x1, y1], color) in crosshair_paints(width, height) {
        for y in y0.max(0)..y1.min(height as i32) {
            for x in x0.max(0)..x1.min(width as i32) {
                let i = (y as usize * width as usize + x as usize) * channels;
                pixels[i..i + 3].copy_from_slice(&color);
            }
        }
    }
}

/// The process's CPU time so far, user and system, every thread.
#[cfg(target_os = "macos")]
fn cpu_seconds() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage fills the rusage it is given and reports failure with -1
    let usage = (unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0).then(|| unsafe { usage.assume_init() })?;
    let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    Some(seconds(usage.ru_utime) + seconds(usage.ru_stime))
}

#[cfg(not(target_os = "macos"))]
fn cpu_seconds() -> Option<f64> {
    None
}

/// Packed RGBA as packed RGB.
fn rgb_of_rgba(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
}

/// How many frames the video loop keeps in flight, drawn and waiting for the CPU to read them: three or four were no
/// faster and held 230-480 MB more (S41).
const AHEAD: usize = 2;

/// How many times the first frame is drawn before the one kept (S41).
const WARM_UP: u32 = 3;

/// How a clip's frames leave on stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frames {
    /// One PNG after another.
    Png,
    /// Packed RGBA, 4 bytes a pixel.
    Rgba,
    /// NV12 as FFmpeg would make it of the RGBA, converted on the GPU: 1.5 bytes a pixel, the encoder's own layout (S51).
    Nv12,
}

/// The rows of a readback without their padding: `row` bytes each, of which the first `packed` are the picture's.
fn unpadded(rows: &[u8], row: usize, packed: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(rows.len() / row * packed);
    for line in rows.chunks_exact(row) {
        bytes.extend_from_slice(&line[..packed]);
    }
    bytes
}

/// A scene's clip as a stream of frames on stdout (PNGs, packed RGBA or NV12: [`Frames`]), with JSON progress on
/// stderr; stills, when asked for, written as PNGs.
pub fn render_video(path: &Path, format: Frames) -> Result<()> {
    use nades_control::video::{FrameTimings, ProcessUse, Spread, StageTime, VideoRequest, VideoTimings};
    use std::time::Instant;
    let ms = |since: Instant| since.elapsed().as_secs_f64() * 1e3;
    let request: VideoRequest = serde_json::from_slice(&std::fs::read(path)?)?;
    let request = request.validated().map_err(|e| anyhow!("{}: {}", e.code, e.message))?;
    let (width, height) = (request.width, request.height);
    if format == Frames::Nv12 && height < nades_control::video::NV12_MIN_HEIGHT {
        bail!("NV12 frames need at least {} rows: ask for RGBA", nades_control::video::NV12_MIN_HEIGHT);
    }
    let strong = request.throw.or_else(|| request.shot.first().and_then(nades_control::video::ShotMove::throw));
    let mut setup = Vec::new();
    let mut lap = Instant::now();
    let mut mark = |stage: &str, setup: &mut Vec<StageTime>| {
        setup.push(StageTime { stage: stage.into(), ms: ms(lap) });
        lap = Instant::now();
    };
    // what the map draws, as a collider, is two thirds of the setup: built beside the staging, not after it (S48)
    let sight = (!request.shot.is_empty()).then(|| {
        let map = request.scene.map.clone();
        std::thread::spawn(move || crate::assets::sight(&map))
    });
    let mut s = stage(&request.scene, (width, height), request.arcs.then_some(strong))?;
    mark("stage: the map on the GPU, the throws simulated", &mut setup);
    for (i, (b, start)) in s.placed.iter().enumerate() {
        eprintln!("{}", report(i, b, *start));
    }
    if let Some(sight) = sight {
        s.sight = sight.join().map_err(|_| anyhow!("the sight collider's thread panicked"))?.map_err(|e| eprintln!("sight: {e:#} (the shot keeps clear of the map's physics only)")).ok();
        mark("sight: what the map draws, as a collider (the wait left after the staging)", &mut setup);
    }
    let mut shot = (!request.shot.is_empty()).then(|| crate::shot::Shot::new(&request.shot, &s.placed, request.start_seconds, s.sight.as_ref().or(s.collider.as_ref())));
    mark("shot: the moves planned", &mut setup);
    if let (Some(shot), Some(sight)) = (&shot, &s.sight) {
        let audit = shot.audit(sight, f64::from(request.fps));
        eprintln!("{}", serde_json::json!({"event": "camera_audit", "frames": audit.frames, "clipped": audit.clipped, "hidden": audit.hidden}));
        mark("audit: the camera checked against the map", &mut setup);
    }
    // each still's frame
    let stills: Vec<(u32, &str)> = request
        .stills
        .iter()
        .map(|still| {
            let seconds = match still.at {
                nades_control::video::StillAt::Seconds(at) => at,
                nades_control::video::StillAt::Moment(m) => shot.as_ref().and_then(|shot| shot.moment(m)).unwrap_or(0.0),
            };
            ((seconds * f64::from(request.fps)).round() as u32, still.path.as_str())
        })
        .collect();
    let samples = request.samples.unwrap_or(4);
    let end = request.end_seconds.unwrap_or_else(|| request.start_seconds + shot.as_ref().map_or(1.0, crate::shot::Shot::seconds));
    let frames = request.frames_until(end);
    if end - request.start_seconds > 60.0 || frames > 1800 {
        bail!("the clip would last {:.1} s, {frames} frames: at most 60 s and 1800 frames", end - request.start_seconds);
    }
    // its length, before the first frame: the encoder's bitrate follows from it
    eprintln!("{}", serde_json::json!({"event": "video_plan", "total": frames}));
    let output = s.gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("video frame"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | nades_render::yuv::Nv12::SOURCE_USAGE,
        view_formats: nades_render::yuv::Nv12::SOURCE_VIEW_FORMATS,
    });
    // as NV12 the frame is converted on the GPU, the crosshair painted there; a still's frame is read as RGBA besides
    let nv12 = (format == Frames::Nv12).then(|| nades_render::yuv::Nv12::new(&s.gpu.device, &output, &crosshair_paints(width, height)));
    let (row, packed) = (crate::row_bytes(width) as usize, width as usize * 4);
    let (stream_row, stream_rows, stream_packed) = nv12.as_ref().map_or((row, height as usize, packed), |nv12| (nv12.rows().0 as usize, nv12.rows().1 as usize, width as usize));
    // readbacks for the frames drawn ahead: the GPU draws the next ones while this one goes to the encoder
    let readback = |label: &str, bytes: usize| {
        let buffer = s.gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: bytes as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let (sender, receiver) = std::sync::mpsc::channel();
        (buffer, sender, receiver)
    };
    let readbacks: Vec<_> = (0..AHEAD).map(|_| readback("video readback", stream_row * stream_rows)).collect();
    let still_readbacks: Vec<_> = (0..if nv12.is_some() && !stills.is_empty() { AHEAD } else { 0 }).map(|_| readback("still readback", row * height as usize)).collect();
    let still_at = |frame: u32| stills.iter().any(|(at, _)| (*at).min(frames - 1) == frame);
    let mut times: [Vec<f64>; 6] = Default::default();
    let [camera_ms, record_ms, wait_ms, convert_ms, stills_ms, write_ms] = &mut times;
    let stdout = std::io::stdout();
    let mut stream = stdout.lock();
    let view = request.view.unwrap_or(ViewName::Follow);
    // the frames drawn and waiting to be read, oldest first: number, readback, submission, whether it shows the crosshair
    let mut pending: std::collections::VecDeque<(u32, usize, wgpu::SubmissionIndex, bool)> = Default::default();
    let mut writing_stills = Vec::new();
    for frame in 0..frames + AHEAD as u32 - 1 {
        let drawn = (frame < frames).then(|| -> Result<_> {
            let began = Instant::now();
            let elapsed = f64::from(frame) / f64::from(request.fps);
            let t = shot.as_ref().map_or(request.start_seconds + elapsed, |shot| shot.scene_time(elapsed));
            let aiming = shot.as_ref().is_some_and(|shot| shot.crosshair(elapsed));
            let lineup_flash = shot.as_ref().and_then(|shot| shot.lineup_throw(elapsed));
            let eye = match (&mut shot, request.camera_at(elapsed)) {
                (Some(shot), _) => views::of(&shot.camera(elapsed, &s.placed, request.start_seconds, s.collider.as_ref())),
                (None, Some(camera)) => {
                    let mut live = crate::camera::Camera::bench();
                    live.set_pose(&camera);
                    live.view()
                }
                (None, None) => view_camera(&s, view, request.throw.unwrap_or(0), t)?,
            };
            camera_ms.push(ms(began));
            let recording = Instant::now();
            // a device's first two draws bloom a little unlike every later one (up to 28 levels in 0.5% of the pixels, S41):
            // the first frame is drawn a few times over before the one kept
            for _ in 0..if frame == 0 { WARM_UP } else { 0 } {
                draw_with(&mut s, &eye, t, elapsed as f32, samples, lineup_flash);
            }
            let mut encoder = draw_into(&mut s, &eye, t, elapsed as f32, samples, lineup_flash);
            let slot = frame as usize % AHEAD;
            let (buffer, _, _) = &readbacks[slot];
            {
                let target = output.create_view(&Default::default());
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("video present"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(crate::CLEAR), store: wgpu::StoreOp::Store } })],
                    ..Default::default()
                });
                s.map.present(&mut pass, &s.targets);
            }
            let rgba_into = |encoder: &mut wgpu::CommandEncoder, buffer: &wgpu::Buffer| {
                encoder.copy_texture_to_buffer(output.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row as u32), rows_per_image: Some(height) } }, wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
            };
            let still = still_readbacks.get(slot).filter(|_| still_at(frame));
            match &nv12 {
                Some(nv12) => {
                    nv12.encode(&mut encoder, buffer, aiming);
                    if let Some((buffer, _, _)) = still {
                        rgba_into(&mut encoder, buffer);
                    }
                }
                None => rgba_into(&mut encoder, buffer),
            }
            let submitted = s.gpu.queue.submit([encoder.finish()]);
            for (buffer, mapped, _) in std::iter::once(&readbacks[slot]).chain(still) {
                let mapped = mapped.clone();
                buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| { let _ = mapped.send(result); });
            }
            record_ms.push(ms(recording));
            Ok((frame, slot, submitted, aiming))
        }).transpose()?;
        pending.extend(drawn);
        if pending.len() < AHEAD && frame < frames {
            continue;
        }
        if let Some((done, slot, submitted, aiming)) = pending.pop_front() {
            let (buffer, _, mapped) = &readbacks[slot];
            let waiting = Instant::now();
            s.gpu.device.poll(wgpu::PollType::Wait { submission_index: Some(submitted), timeout: Some(std::time::Duration::from_secs(10)) })?;
            mapped.recv_timeout(std::time::Duration::from_secs(1)).context("frame mapping timed out")??;
            let here: Vec<&str> = stills.iter().filter(|(at, _)| (*at).min(frames - 1) == done).map(|(_, path)| *path).collect();
            let still = still_readbacks.get(slot).filter(|_| !here.is_empty());
            if let Some((_, _, mapped)) = still {
                mapped.recv_timeout(std::time::Duration::from_secs(1)).context("still mapping timed out")??;
            }
            wait_ms.push(ms(waiting));
            let converting = Instant::now();
            let rows = buffer.slice(..).get_mapped_range()?;
            // the frame as RGBA with its crosshair: what a still shows, and the stream when it is not NV12
            let rgba_of = |rows: &[u8]| {
                let mut rgba = unpadded(rows, row, packed);
                if aiming {
                    crosshair(&mut rgba, width, height, 4);
                }
                rgba
            };
            // the rows go to the encoder as they are; a copy only for the crosshair, a still or padded rows
            let (copy, still_rgba) = match (&nv12, still) {
                (Some(_), still) => {
                    let still_rgba = still.map(|(buffer, _, _)| -> Result<_> {
                        let rgba = rgba_of(&buffer.slice(..).get_mapped_range()?);
                        buffer.unmap();
                        Ok(rgba)
                    }).transpose()?;
                    ((stream_row != stream_packed).then(|| unpadded(&rows, stream_row, stream_packed)), still_rgba)
                }
                (None, _) => ((aiming || !here.is_empty() || format == Frames::Png || row != packed).then(|| rgba_of(&rows)), None),
            };
            let bytes: &[u8] = copy.as_deref().unwrap_or(&rows);
            convert_ms.push(ms(converting));
            let stilling = Instant::now();
            // PNG encoding is a third of a second a still: off this thread, so the frames keep flowing
            for still in here {
                let rgba = if nv12.is_some() { still_rgba.clone().context("a still's frame was not read")? } else { bytes.to_vec() };
                let still = still.to_string();
                writing_stills.push(std::thread::spawn(move || -> Result<()> {
                    std::fs::write(&still, crate::png_of(&rgb_of_rgba(&rgba), width, height)?).with_context(|| format!("writing {still}"))
                }));
            }
            stills_ms.push(ms(stilling));
            let writing = Instant::now();
            if format == Frames::Png {
                stream.write_all(&crate::png_of(&rgb_of_rgba(bytes), width, height)?)?;
            } else {
                stream.write_all(bytes)?;
            }
            write_ms.push(ms(writing));
            drop(rows);
            buffer.unmap();
            eprintln!("{}", serde_json::json!({"event":"video_progress", "frames":done + 1, "total":frames}));
        }
    }
    stream.flush()?;
    for still in writing_stills {
        still.join().map_err(|_| anyhow!("a still's writer panicked"))??;
    }
    let frame = FrameTimings {
        frames,
        camera: Spread::of(camera_ms),
        record: Spread::of(record_ms),
        wait: Spread::of(wait_ms),
        convert: Spread::of(convert_ms),
        stills: Spread::of(stills_ms),
        write: Spread::of(write_ms),
    };
    let renderer = ProcessUse { cpu_seconds: cpu_seconds(), peak_footprint_bytes: nades_render::memory::footprint().map(|f| f.peak) };
    let timings = VideoTimings { setup, frame, renderer, ..Default::default() };
    let mut event = serde_json::to_value(&timings)?;
    event["event"] = "video_timings".into();
    eprintln!("{event}");
    Ok(())
}
