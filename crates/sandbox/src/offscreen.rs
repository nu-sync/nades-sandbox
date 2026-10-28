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
    let collider = crate::assets::collider(&scene.map).map_err(|e| eprintln!("collider: {e:#} (grenades fly through walls, no smoke or fire)")).ok();
    let sun = app::toward_sun(&map);
    let placed: Vec<Placed> = scene.throws.par_iter().map(|t| (Arc::new(sim::simulate(t, collider.as_ref(), sun)), t.start)).collect();
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
    let collider = crate::assets::collider(&scene.map).map_err(|e| eprintln!("collider: {e:#} (grenades fly through walls, no smoke or fire)")).ok();
    // the smoke's light is drawing's business: straight up will do for the report
    let built: Vec<_> = scene.throws.par_iter().map(|t| sim::simulate(t, collider.as_ref(), glam::DVec3::Z)).collect();
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

/// Draws `t` of the staged scene from `eye`, its smokes averaged over the jittered frames a still view takes.
fn draw(s: &mut Staged, eye: &Eye, t: f64, material_seconds: f32) {
    draw_with(s, eye, t, material_seconds, nades_render::smoke::ACCUMULATE);
}

/// Draws `t` from `eye`, a smoke averaged over `samples` jittered frames.
fn draw_with(s: &mut Staged, eye: &Eye, t: f64, material_seconds: f32, samples: u32) {
    let size = s.targets.size();
    let frames = if sim::smoking(&s.placed, t) { samples.max(1) } else { 1 };
    for k in 0..frames {
        let effects = sim::show(&s.gpu, &mut s.map, &s.placed, t, eye, size, k);
        let mut encoder = s.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scene") });
        s.map.render(&s.gpu.queue, &mut encoder, &s.targets, eye, material_seconds, &effects);
        s.gpu.queue.submit([encoder.finish()]);
    }
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

/// A scene's clip as a stream of PNG frames on stdout, with JSON progress on stderr.
/// CS2's crosshair drawn over a frame of packed RGB: four green arms with a dark outline round the middle, sized to
/// the frame (`width` × `height`).
/// Paints the crosshair on packed pixels of `channels` bytes each (RGB first).
pub fn crosshair(pixels: &mut [u8], width: u32, height: u32, channels: usize) {
    let unit = (f64::from(height) / 720.0).max(0.5);
    let (gap, length, thick) = ((4.0 * unit).round() as i64, (9.0 * unit).round() as i64, (1.5 * unit).round().max(1.0) as i64);
    let (cx, cy) = (i64::from(width) / 2, i64::from(height) / 2);
    let mut paint = |x0: i64, y0: i64, x1: i64, y1: i64, color: [u8; 3]| {
        for y in y0.max(0)..y1.min(i64::from(height)) {
            for x in x0.max(0)..x1.min(i64::from(width)) {
                let i = (y * i64::from(width) + x) as usize * channels;
                pixels[i..i + 3].copy_from_slice(&color);
            }
        }
    };
    let arms = |o: i64| {
        [
            (cx + gap - o, cy - thick / 2 - o, cx + gap + length + o, cy + (thick + 1) / 2 + o),
            (cx - gap - length - o, cy - thick / 2 - o, cx - gap + o, cy + (thick + 1) / 2 + o),
            (cx - thick / 2 - o, cy + gap - o, cx + (thick + 1) / 2 + o, cy + gap + length + o),
            (cx - thick / 2 - o, cy - gap - length - o, cx + (thick + 1) / 2 + o, cy - gap + o),
        ]
    };
    let outline = (unit.round() as i64).max(1);
    for (x0, y0, x1, y1) in arms(outline) {
        paint(x0, y0, x1, y1, [0, 0, 0]);
    }
    for (x0, y0, x1, y1) in arms(0) {
        paint(x0, y0, x1, y1, [50, 250, 50]);
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

/// A scene's clip as a stream of frames on stdout (PNGs, or with `raw` packed RGBA, which the encoder takes faster),
/// with JSON progress on stderr; stills, when asked for, written as PNGs.
pub fn render_video(path: &Path, raw: bool) -> Result<()> {
    use nades_control::video::{FrameTimings, ProcessUse, Spread, StageTime, VideoRequest, VideoTimings};
    use std::time::Instant;
    let ms = |since: Instant| since.elapsed().as_secs_f64() * 1e3;
    let request: VideoRequest = serde_json::from_slice(&std::fs::read(path)?)?;
    let request = request.validated().map_err(|e| anyhow!("{}: {}", e.code, e.message))?;
    let (width, height) = (request.width, request.height);
    let strong = request.throw.or_else(|| request.shot.first().and_then(nades_control::video::ShotMove::throw));
    let mut setup = Vec::new();
    let mut lap = Instant::now();
    let mut mark = |stage: &str, setup: &mut Vec<StageTime>| {
        setup.push(StageTime { stage: stage.into(), ms: ms(lap) });
        lap = Instant::now();
    };
    let mut s = stage(&request.scene, (width, height), request.arcs.then_some(strong))?;
    mark("stage: the map on the GPU, the throws simulated", &mut setup);
    for (i, (b, start)) in s.placed.iter().enumerate() {
        eprintln!("{}", report(i, b, *start));
    }
    if !request.shot.is_empty() {
        s.sight = crate::assets::sight(&request.scene.map).map_err(|e| eprintln!("sight: {e:#} (the shot keeps clear of the map's physics only)")).ok();
        mark("sight: what the map draws, as a collider", &mut setup);
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let (row, packed) = (crate::row_bytes(width) as usize, width as usize * 4);
    // two readbacks: the GPU draws the next frame while this one goes to the encoder
    let readbacks: [_; 2] = std::array::from_fn(|_| {
        let buffer = s.gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("video readback"), size: (row * height as usize) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let (sender, receiver) = std::sync::mpsc::channel();
        (buffer, sender, receiver)
    });
    let mut times: [Vec<f64>; 6] = Default::default();
    let [camera_ms, record_ms, wait_ms, convert_ms, stills_ms, write_ms] = &mut times;
    let stdout = std::io::stdout();
    let mut stream = stdout.lock();
    let view = request.view.unwrap_or(ViewName::Follow);
    // the frame drawn last, waiting to be read: its number, readback, submission and whether it shows the crosshair
    let mut pending: Option<(u32, usize, wgpu::SubmissionIndex, bool)> = None;
    let mut writing_stills = Vec::new();
    for frame in 0..=frames {
        let drawn = (frame < frames).then(|| -> Result<_> {
            let began = Instant::now();
            let elapsed = f64::from(frame) / f64::from(request.fps);
            let t = shot.as_ref().map_or(request.start_seconds + elapsed, |shot| shot.scene_time(elapsed));
            let aiming = shot.as_ref().is_some_and(|shot| shot.crosshair(elapsed));
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
            draw_with(&mut s, &eye, t, elapsed as f32, samples);
            let slot = frame as usize % 2;
            let (buffer, mapped, _) = &readbacks[slot];
            let mut encoder = s.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("video present") });
            {
                let target = output.create_view(&Default::default());
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("video present"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(crate::CLEAR), store: wgpu::StoreOp::Store } })],
                    ..Default::default()
                });
                s.map.present(&mut pass, &s.targets);
            }
            encoder.copy_texture_to_buffer(output.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row as u32), rows_per_image: Some(height) } }, wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
            let submitted = s.gpu.queue.submit([encoder.finish()]);
            let mapped = mapped.clone();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| { let _ = mapped.send(result); });
            record_ms.push(ms(recording));
            Ok((frame, slot, submitted, aiming))
        }).transpose()?;
        if let Some((done, slot, submitted, aiming)) = pending.take() {
            let (buffer, _, mapped) = &readbacks[slot];
            let waiting = Instant::now();
            s.gpu.device.poll(wgpu::PollType::Wait { submission_index: Some(submitted), timeout: Some(std::time::Duration::from_secs(10)) })?;
            mapped.recv_timeout(std::time::Duration::from_secs(1)).context("frame mapping timed out")??;
            wait_ms.push(ms(waiting));
            let converting = Instant::now();
            let rows = buffer.slice(..).get_mapped_range()?;
            let here: Vec<&str> = stills.iter().filter(|(at, _)| (*at).min(frames - 1) == done).map(|(_, path)| *path).collect();
            // the rows go to the encoder as they are; a copy only for the crosshair, a still or padded rows
            let copy = (aiming || !here.is_empty() || !raw || row != packed).then(|| {
                let mut rgba = Vec::with_capacity(packed * height as usize);
                for line in rows.chunks_exact(row) {
                    rgba.extend_from_slice(&line[..packed]);
                }
                if aiming {
                    crosshair(&mut rgba, width, height, 4);
                }
                rgba
            });
            let rgba: &[u8] = copy.as_deref().unwrap_or(&rows);
            convert_ms.push(ms(converting));
            let stilling = Instant::now();
            // PNG encoding is a third of a second a still: off this thread, so the frames keep flowing
            for still in here {
                let (rgba, still) = (rgba.to_vec(), still.to_string());
                writing_stills.push(std::thread::spawn(move || -> Result<()> {
                    std::fs::write(&still, crate::png_of(&rgb_of_rgba(&rgba), width, height)?).with_context(|| format!("writing {still}"))
                }));
            }
            stills_ms.push(ms(stilling));
            let writing = Instant::now();
            if raw {
                stream.write_all(rgba)?;
            } else {
                stream.write_all(&crate::png_of(&rgb_of_rgba(rgba), width, height)?)?;
            }
            write_ms.push(ms(writing));
            drop(rows);
            buffer.unmap();
            eprintln!("{}", serde_json::json!({"event":"video_progress", "frames":done + 1, "total":frames}));
        }
        pending = drawn;
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
