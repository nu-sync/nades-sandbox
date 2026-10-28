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
    Ok(Staged { gpu, map, targets, placed, collider })
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
    let size = s.targets.size();
    let frames = if sim::smoking(&s.placed, t) { nades_render::smoke::ACCUMULATE } else { 1 };
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
pub fn render_video(path: &Path) -> Result<()> {
    use nades_control::video::VideoRequest;
    let request: VideoRequest = serde_json::from_slice(&std::fs::read(path)?)?;
    let request = request.validated().map_err(|e| anyhow!("{}: {}", e.code, e.message))?;
    let (width, height) = (request.width, request.height);
    let strong = request.throw.or_else(|| request.shot.first().and_then(nades_control::video::ShotMove::throw));
    let mut s = stage(&request.scene, (width, height), Some(strong))?;
    for (i, (b, start)) in s.placed.iter().enumerate() {
        eprintln!("{}", report(i, b, *start));
    }
    let mut shot = (!request.shot.is_empty()).then(|| crate::shot::Shot::new(&request.shot, &s.placed, request.start_seconds));
    let end = request.end_seconds.unwrap_or_else(|| request.start_seconds + shot.as_ref().map_or(1.0, crate::shot::Shot::seconds));
    let frames = request.frames_until(end);
    if end - request.start_seconds > 60.0 || frames > 1800 {
        bail!("the clip would last {:.1} s, {frames} frames: at most 60 s and 1800 frames", end - request.start_seconds);
    }
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
    let row = crate::row_bytes(width);
    let buffer = s.gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("video readback"), size: u64::from(row * height), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let stdout = std::io::stdout();
    let mut stream = stdout.lock();
    let view = request.view.unwrap_or(ViewName::Follow);
    for frame in 0..frames {
        let elapsed = f64::from(frame) / f64::from(request.fps);
        let t = request.start_seconds + elapsed;
        let eye = match (&mut shot, request.camera_at(elapsed)) {
            (Some(shot), _) => views::of(&shot.camera(elapsed, &s.placed, request.start_seconds, s.collider.as_ref())),
            (None, Some(camera)) => {
                let mut live = crate::camera::Camera::bench();
                live.set_pose(&camera);
                live.view()
            }
            (None, None) => view_camera(&s, view, request.throw.unwrap_or(0), t)?,
        };
        draw(&mut s, &eye, t, elapsed as f32);
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
        encoder.copy_texture_to_buffer(output.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } }, wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
        s.gpu.queue.submit([encoder.finish()]);
        stream.write_all(&crate::encode_png(&s.gpu, &buffer, width, height, [0, 1, 2])?)?;
        eprintln!("{}", serde_json::json!({"event":"video_progress", "frames":frame + 1, "total":frames}));
    }
    stream.flush()?;
    Ok(())
}
