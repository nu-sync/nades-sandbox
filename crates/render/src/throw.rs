//! What a throw adds to the map (`throw.wgsl`): the recorded flight as a tube with its marks and the grenade flying
//! along it, the lines to the players a flash blinded, a flash's or HE's burst, and a fire's sprites with the film and
//! scorch under its flames, as show-me-the-nades' web view draws them (`render/effects.ts`, `render/fire.ts`).
//!
//! Everything is in Hammer units with z up, as the database stores it; the frame's matrices take it to the scene's
//! frame. A stage's static geometry is built once when it is shown ([`ThrowLayer::set_stage`]); what moves (the
//! grenade, the fire's sprites, the bursts) is rewritten for each moment ([`ThrowLayer::set_time`]).

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Mat4, Vec3};
use nades_scene::fire::{self, Fire, FireKind};
use nades_scene::stage::{Stage, nade_color};
use wgpu::util::DeviceExt as _;

use crate::map::DEPTH_FORMAT;
use crate::post::HDR_FORMAT;
use crate::view::{INCH, View};

/// Hammer units (z up) into the scene's frame (glTF: metres, y up): x_scene = y·inch, y_scene = z·inch, z_scene = x·inch.
pub fn source_to_scene() -> Mat4 {
    Mat4::from_cols(glam::Vec4::new(0.0, 0.0, INCH, 0.0), glam::Vec4::new(INCH, 0.0, 0.0, 0.0), glam::Vec4::new(0.0, INCH, 0.0, 0.0), glam::Vec4::W)
}

/// A point in Hammer units in the scene's frame.
pub fn to_scene(p: DVec3) -> Vec3 {
    Vec3::new(p.y as f32, p.z as f32, p.x as f32) * INCH
}

/// A direction in the scene's frame in Hammer units' axes.
pub fn to_source_dir(d: Vec3) -> Vec3 {
    Vec3::new(d.z, d.x, d.y)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct FrameUniform {
    view_proj: [f32; 16],
    view: [f32; 16],
    projection: [f32; 16],
    eye: [f32; 4],
    sun: [f32; 4],
    screen: [f32; 4],
    flip: [f32; 4],
    grid: [f32; 4],
    grid2: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default, Debug)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    color: [f32; 4],
    params: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct GlowInstance {
    centre: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct SpriteInstance {
    pos: [f32; 4],
    shape: [f32; 4],
    look: [f32; 4],
    colour: [f32; 4],
    tone: [f32; 4],
    book: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct PatchInstance {
    pos: [f32; 4],
    normal: [f32; 4],
    data: [f32; 4],
}

/// A mesh's vertices as triangles, appended to one list.
#[derive(Default)]
struct Meshes {
    vertices: Vec<Vertex>,
}

impl Meshes {
    fn push(&mut self, p: [Vec3; 3], n: [Vec3; 3], color: [f32; 4], params: [f32; 2]) {
        for k in 0..3 {
            self.vertices.push(Vertex { position: p[k].into(), normal: n[k].into(), color, params });
        }
    }

    /// A ring-by-ring tube around `centres` (with their tangents), `sides` around.
    fn tube(&mut self, centres: &[Vec3], radius: f32, sides: usize, color: [f32; 4], params: [f32; 2]) {
        if centres.len() < 2 {
            return;
        }
        let frames = transport_frames(centres);
        let ring = |i: usize| -> Vec<(Vec3, Vec3)> {
            let (n, b) = frames[i];
            (0..=sides)
                .map(|k| {
                    let a = k as f32 / sides as f32 * std::f32::consts::TAU;
                    let normal = n * a.cos() + b * a.sin();
                    (centres[i] + normal * radius, normal)
                })
                .collect()
        };
        let mut previous = ring(0);
        for i in 1..centres.len() {
            let next = ring(i);
            for k in 0..sides {
                let (a, b, c, d) = (previous[k], previous[k + 1], next[k + 1], next[k]);
                self.push([a.0, b.0, c.0], [a.1, b.1, c.1], color, params);
                self.push([a.0, c.0, d.0], [a.1, c.1, d.1], color, params);
            }
            previous = next;
        }
    }

    /// A replay's player: a body and a head in their side's colour, and a line from the eye where they look; dead, a
    /// dark disc where they fell.
    fn player(&mut self, p: &ReplayPlayer, alpha: f32) {
        let feet = Vec3::from_array(p.origin);
        let [r, g, b] = side_color(p.ct);
        if !p.alive {
            let squash = Mat4::from_scale(Vec3::new(1.0, 1.0, 0.18));
            self.ellipsoid(feet + Vec3::Z * 2.0, 15.0, 0.0, squash, [r * 0.35, g * 0.35, b * 0.35, alpha], [0.0, 0.0]);
            return;
        }
        let lift = if p.followed { 1.2 } else { 1.0 };
        let colour = [r * lift, g * lift, b * lift, alpha];
        self.ellipsoid(feet + Vec3::Z * 32.0, 12.0, 30.0, Mat4::IDENTITY, colour, [0.0, 0.0]);
        let eye = feet + Vec3::Z * 64.0;
        self.sphere(eye, 7.5, colour, [0.0, 0.0]);
        let (pitch, yaw) = (p.pitch.to_radians(), p.yaw.to_radians());
        let forward = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin());
        let tip = eye + forward * 46.0;
        self.tube(&[eye + forward * 6.0, tip], 1.6, 6, [1.0, 1.0, 1.0, alpha], [0.0, 0.0]);
        self.sphere(tip, 2.6, [1.0, 1.0, 1.0, alpha], [0.0, 0.0]);
    }

    fn sphere(&mut self, centre: Vec3, radius: f32, color: [f32; 4], params: [f32; 2]) {
        self.ellipsoid(centre, radius, 0.0, Mat4::IDENTITY, color, params);
    }

    /// A sphere, or a capsule when `length` (along its z) is above 0, turned by `turn`.
    fn ellipsoid(&mut self, centre: Vec3, radius: f32, length: f32, turn: Mat4, color: [f32; 4], params: [f32; 2]) {
        let (rings, sides) = (12, 16);
        let point = |i: usize, k: usize| -> (Vec3, Vec3) {
            let polar = i as f32 / rings as f32 * std::f32::consts::PI;
            let a = k as f32 / sides as f32 * std::f32::consts::TAU;
            let normal = Vec3::new(polar.sin() * a.cos(), polar.sin() * a.sin(), polar.cos());
            let stretch = if normal.z >= 0.0 { length * 0.5 } else { -length * 0.5 };
            let local = normal * radius + Vec3::new(0.0, 0.0, stretch);
            (centre + turn.transform_vector3(local), turn.transform_vector3(normal))
        };
        for i in 0..rings {
            for k in 0..sides {
                let (a, b, c, d) = (point(i, k), point(i, k + 1), point(i + 1, k + 1), point(i + 1, k));
                self.push([a.0, c.0, b.0], [a.1, c.1, b.1], color, params);
                self.push([a.0, d.0, c.0], [a.1, d.1, c.1], color, params);
            }
        }
    }
}

/// A normal and binormal along a polyline, carried from point to point without twisting (three's
/// `computeFrenetFrames` does the same).
fn transport_frames(points: &[Vec3]) -> Vec<(Vec3, Vec3)> {
    let tangent = |i: usize| {
        let (a, b) = if i + 1 < points.len() { (points[i], points[i + 1]) } else { (points[i - 1], points[i]) };
        (b - a).normalize_or(Vec3::X)
    };
    let t0 = tangent(0);
    // the axis the tangent leans on least
    let least = if t0.x.abs() <= t0.y.abs() && t0.x.abs() <= t0.z.abs() {
        Vec3::X
    } else if t0.y.abs() <= t0.z.abs() {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let mut normal = t0.cross(least).normalize_or(Vec3::Y);
    let mut frames = Vec::with_capacity(points.len());
    let mut previous = t0;
    for i in 0..points.len() {
        let t = tangent(i);
        let axis = previous.cross(t);
        if axis.length() > 1e-6 {
            let angle = previous.dot(t).clamp(-1.0, 1.0).acos();
            normal = glam::Quat::from_axis_angle(axis.normalize(), angle) * normal;
        }
        let binormal = t.cross(normal).normalize_or(Vec3::Z);
        normal = binormal.cross(t).normalize_or(normal);
        frames.push((normal, binormal));
        previous = t;
    }
    frames
}

/// three.js's centripetal Catmull–Rom curve through `points`, sampled at `segments + 1` points evenly along its length.
fn catmull_rom(points: &[DVec3], segments: usize) -> Vec<Vec3> {
    let l = points.len();
    let at = |t: f64| -> DVec3 {
        let p = (l - 1) as f64 * t;
        let mut i = p.floor() as usize;
        let mut weight = p - i as f64;
        if weight == 0.0 && i == l - 1 {
            i = l - 2;
            weight = 1.0;
        }
        let p0 = if i > 0 { points[i - 1] } else { points[0] - points[1] + points[0] };
        let (p1, p2) = (points[i], points[i + 1]);
        let p3 = if i + 2 < l { points[i + 2] } else { points[l - 1] - points[l - 2] + points[l - 1] };
        let mut dt0 = p0.distance_squared(p1).powf(0.25);
        let mut dt1 = p1.distance_squared(p2).powf(0.25);
        let mut dt2 = p2.distance_squared(p3).powf(0.25);
        if dt1 < 1e-4 {
            dt1 = 1.0;
        }
        if dt0 < 1e-4 {
            dt0 = dt1;
        }
        if dt2 < 1e-4 {
            dt2 = dt1;
        }
        let axis = |x0: f64, x1: f64, x2: f64, x3: f64| {
            let t1 = ((x1 - x0) / dt0 - (x2 - x0) / (dt0 + dt1) + (x2 - x1) / dt1) * dt1;
            let t2 = ((x2 - x1) / dt1 - (x3 - x1) / (dt1 + dt2) + (x3 - x2) / dt2) * dt1;
            let (c0, c1, c2, c3) = (x1, t1, -3.0 * x1 + 3.0 * x2 - 2.0 * t1 - t2, 2.0 * x1 - 2.0 * x2 + t1 + t2);
            c0 + c1 * weight + c2 * weight * weight + c3 * weight * weight * weight
        };
        DVec3::new(axis(p0.x, p1.x, p2.x, p3.x), axis(p0.y, p1.y, p2.y, p3.y), axis(p0.z, p1.z, p2.z, p3.z))
    };
    // evenly along the length, as TubeGeometry's getPointAt places its rings
    let fine = 200.max(segments * 2);
    let samples: Vec<DVec3> = (0..=fine).map(|k| at(k as f64 / fine as f64)).collect();
    let mut lengths = vec![0.0];
    for w in samples.windows(2) {
        lengths.push(lengths.last().copied().unwrap_or(0.0) + w[0].distance(w[1]));
    }
    let total = lengths[fine];
    (0..=segments)
        .map(|k| {
            let target = total * k as f64 / segments as f64;
            let i = lengths.partition_point(|&d| d < target).clamp(1, fine);
            let (a, b) = (lengths[i - 1], lengths[i]);
            let f = if b > a { (target - a) / (b - a) } else { 0.0 };
            let p = samples[i - 1].lerp(samples[i], f);
            Vec3::new(p.x as f32, p.y as f32, p.z as f32)
        })
        .collect()
}

fn rgba(rgb: [f64; 3], a: f32) -> [f32; 4] {
    [rgb[0] as f32, rgb[1] as f32, rgb[2] as f32, a]
}

fn v3(p: DVec3) -> Vec3 {
    Vec3::new(p.x as f32, p.y as f32, p.z as f32)
}

/// Each sequence of a flipbook: its first frame and how many frames it plays.
type Sequences = Vec<(u32, u32)>;

/// One flipbook as decoded: its name, its atlas (RGBA8, its first row the top) and size, its grid, its sequences.
struct Book {
    name: String,
    pixels: Vec<u8>,
    size: (u32, u32),
    grid: (u32, u32),
    sequences: Sequences,
}

/// The fire's flipbooks, decoded on the CPU: CS2's own, from the shared effects pack.
pub struct Flipbooks {
    books: Vec<Book>,
}

/// The flipbooks on the GPU: one view each, and each one's name, grid and sequences. Made once ([`Flipbooks::upload`])
/// and shared by every map's layer: cloning it clones handles, not textures.
#[derive(Clone)]
pub struct GpuBooks {
    views: Vec<wgpu::TextureView>,
    layout: Vec<(String, (u32, u32), Sequences)>,
}

impl Flipbooks {
    /// The flipbooks of the shared effects pack (`nades-pack --effects`).
    pub fn from_effects(effects: &nades_pack::effects::Effects) -> Self {
        let books = effects
            .flipbooks
            .iter()
            .map(|f| Book { name: f.name.clone(), pixels: f.rgba.clone(), size: (f.columns * f.cell, f.rows * f.cell), grid: (f.columns, f.rows), sequences: f.sequences.clone() })
            .collect();
        Self { books }
    }

    /// The flipbooks on the GPU, with every mip.
    pub fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> GpuBooks {
        let views = self.books.iter().map(|b| mipmapped(device, queue, &b.name, &b.pixels, b.size, false).create_view(&Default::default())).collect();
        let layout = self.books.iter().map(|b| (b.name.clone(), b.grid, b.sequences.clone())).collect();
        GpuBooks { views, layout }
    }
}

/// A texture with every mip level, box-filtered from `pixels` (RGBA8).
fn mipmapped(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, pixels: &[u8], (width, height): (u32, u32), repeat: bool) -> wgpu::Texture {
    let levels = 32 - width.max(height).leading_zeros();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut level = pixels.to_vec();
    let (mut w, mut h) = (width, height);
    for mip in 0..levels {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: mip, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &level,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |xx: u32, yy: u32| {
                        let (xx, yy) = if repeat { (xx % w, yy % h) } else { (xx.min(w - 1), yy.min(h - 1)) };
                        u32::from(level[((yy * w + xx) * 4 + c) as usize])
                    };
                    let sum = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
                    next[((y * nw + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        level = next;
        (w, h) = (nw, nh);
    }
    texture
}

/// What a stage keeps on the GPU while it is shown.
struct Shown {
    stage_id: String,
    /// The static meshes: the opaque marks, then the tube, then what draws over everything.
    meshes: Option<wgpu::Buffer>,
    opaque: std::ops::Range<u32>,
    tube: std::ops::Range<u32>,
    overlay: std::ops::Range<u32>,
    fire: Option<(Fire, Option<[Sequences; 3]>)>,
}

/// The throw's pipelines, its textures, and what the shown stage puts on the GPU.
pub struct ThrowLayer {
    frame: wgpu::Buffer,
    group: wgpu::BindGroup,
    group_layout: wgpu::BindGroupLayout,
    depth_layout: wgpu::BindGroupLayout,
    mesh_opaque: wgpu::RenderPipeline,
    mesh_tube: wgpu::RenderPipeline,
    mesh_overlay: wgpu::RenderPipeline,
    glow: wgpu::RenderPipeline,
    sprite: wgpu::RenderPipeline,
    patch: wgpu::RenderPipeline,
    noise: wgpu::TextureView,
    palette: wgpu::TextureView,
    repeated: wgpu::Sampler,
    clamped: wgpu::Sampler,
    /// The flipbooks on the GPU, their names and grids, once loaded.
    books: Option<GpuBooks>,
    shown: Option<Shown>,
    /// What moves, rewritten for each moment: the grenade's mesh, the bursts, the sprites, the patches.
    moving: Option<(wgpu::Buffer, u32)>,
    glows: Option<(wgpu::Buffer, u32)>,
    sprites: Option<(wgpu::Buffer, u32)>,
    patches: Option<(wgpu::Buffer, u32)>,
    /// Whether the stage's flight shows (the result and orbit views).
    pub flight_visible: bool,
    /// Towards the map's sun (Hammer units' axes), which lights the grenade in flight.
    pub toward_sun: Vec3,
    /// Seconds after detonation of the moment written last: before it, only the grenade flies, not its tube.
    time: f64,
    /// A replay's incendiaries: their own frame values and flipbooks, beside the molotovs the main group binds.
    second: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    second_sprites: Option<(wgpu::Buffer, u32)>,
    /// A replay's living players again, faint, where walls hide them.
    ghosts: Option<(wgpu::Buffer, u32)>,
}

/// One player in a replay's moment, in Hammer units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplayPlayer {
    /// The pawn's origin (its feet).
    pub origin: [f32; 3],
    /// `m_angEyeAngles`, degrees, pitch positive looking down.
    pub pitch: f32,
    pub yaw: f32,
    pub ct: bool,
    pub alive: bool,
    /// The followed player: drawn brighter.
    pub followed: bool,
}

/// The sides' colours in a replay: T gold, CT blue.
pub fn side_color(ct: bool) -> [f32; 3] {
    if ct { [0.06, 0.24, 1.0] } else { [1.0, 0.55, 0.04] }
}

/// The throw's draws read the scene's depth through this, one per frame size.
pub struct ThrowTargets {
    depth: wgpu::BindGroup,
}

fn texture_2d(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, pixels: &[u8], (width, height): (u32, u32)) -> wgpu::TextureView {
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            pixels,
        )
        .create_view(&Default::default())
}

impl ThrowLayer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("throw"), source: wgpu::ShaderSource::Wgsl(include_str!("throw.wgsl").into()) });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None };
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("throw"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(std::mem::size_of::<FrameUniform>() as u64) },
                    count: None,
                },
                texture(1),
                texture(2),
                texture(3),
                texture(4),
                texture(5),
                sampler(6),
                sampler(7),
            ],
        });
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("throw depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("throw"), bind_group_layouts: &[Some(&group_layout)], immediate_size: 0 });
        let depth_reading = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("throw effects"), bind_group_layouts: &[Some(&group_layout), Some(&depth_layout)], immediate_size: 0 });
        let alpha = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
        let over = wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add }, alpha };
        let add = wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add }, alpha };
        let premultiplied = wgpu::BlendState { color: alpha, alpha };
        let mesh_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x2],
        })];
        let glow_buffers = [Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<GlowInstance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4] })];
        let sprite_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SpriteInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4],
        })];
        let patch_buffers = [Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<PatchInstance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4] })];
        let pipeline = |label: &str, layout: &wgpu::PipelineLayout, vertex: &str, fragment: &str, buffers: &[Option<wgpu::VertexBufferLayout>], blend: Option<wgpu::BlendState>, depth: (bool, wgpu::CompareFunction), cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vertex), compilation_options: Default::default(), buffers },
                primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(depth.0), depth_compare: Some(depth.1), stencil: Default::default(), bias: Default::default() }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fragment), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: HDR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })] }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction::{Always, GreaterEqual};
        let mesh_opaque = pipeline("throw meshes", &layout, "mesh_vertex", "mesh", &mesh_buffers, None, (true, GreaterEqual), Some(wgpu::Face::Back));
        let mesh_tube = pipeline("throw tube", &layout, "mesh_vertex", "mesh", &mesh_buffers, Some(over), (false, GreaterEqual), Some(wgpu::Face::Back));
        let mesh_overlay = pipeline("throw overlay", &layout, "mesh_vertex", "mesh", &mesh_buffers, Some(over), (false, Always), Some(wgpu::Face::Back));
        let glow = pipeline("throw glow", &layout, "glow_vertex", "glow", &glow_buffers, Some(add), (false, GreaterEqual), None);
        let sprite = pipeline("fire sprites", &depth_reading, "sprite_vertex", "sprite", &sprite_buffers, Some(premultiplied), (false, GreaterEqual), None);
        let patch = pipeline("fire ground", &depth_reading, "patch_vertex", "fire_patch", &patch_buffers, Some(premultiplied), (false, GreaterEqual), None);
        let noise = mipmapped(device, queue, "fire noise", &fire::fire_noise(), (128, 128), true).create_view(&Default::default());
        let palette = texture_2d(device, queue, "fire palette", &fire::palette(), (fire::PALETTE_WIDTH as u32, 6));
        let repeated = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("throw repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let clamped = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("throw clamp"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, mipmap_filter: wgpu::MipmapFilterMode::Linear, ..Default::default() });
        let frame = device.create_buffer(&wgpu::BufferDescriptor { label: Some("throw frame"), size: std::mem::size_of::<FrameUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let blank = texture_2d(device, queue, "no flipbook", &[0; 4], (1, 1));
        let group = Self::bind(device, &group_layout, &frame, &noise, &palette, [&blank, &blank, &blank], &repeated, &clamped);
        Self {
            frame,
            group,
            group_layout,
            depth_layout,
            mesh_opaque,
            mesh_tube,
            mesh_overlay,
            glow,
            sprite,
            patch,
            noise,
            palette,
            repeated,
            clamped,
            books: None,
            shown: None,
            moving: None,
            glows: None,
            sprites: None,
            patches: None,
            flight_visible: false,
            toward_sun: Vec3::Z,
            time: 0.0,
            second: None,
            second_sprites: None,
            ghosts: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, frame: &wgpu::Buffer, noise: &wgpu::TextureView, palette: &wgpu::TextureView, books: [&wgpu::TextureView; 3], repeated: &wgpu::Sampler, clamped: &wgpu::Sampler) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("throw"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: frame.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(noise) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(palette) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(books[0]) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(books[1]) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(books[2]) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(repeated) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(clamped) },
            ],
        })
    }

    /// The per-size bind group: the scene's depth, which the fire reads.
    pub fn targets(&self, device: &wgpu::Device, depth: &wgpu::TextureView) -> ThrowTargets {
        ThrowTargets { depth: device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("throw depth"), layout: &self.depth_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(depth) }] }) }
    }

    /// Plays the fire's flipbooks (on the GPU already, [`Flipbooks::upload`]) in the fires shown from now on.
    pub fn set_flipbooks(&mut self, device: &wgpu::Device, books: &GpuBooks) {
        self.books = Some(books.clone());
        // the shown fire's books, if any, take effect when its stage is set again
        if let Some(shown) = &mut self.shown
            && let Some((fire, sequences)) = &mut shown.fire
        {
            *sequences = Self::sequences_of(self.books.as_ref(), fire.kind);
        }
        self.rebind(device);
    }

    fn sequences_of(books: Option<&GpuBooks>, kind: FireKind) -> Option<[Sequences; 3]> {
        let layout = &books?.layout;
        let names = fire::books(kind);
        let find = |name: &str| layout.iter().find(|(n, _, _)| n == name).map(|(_, _, s)| s.clone());
        Some([find(names[0])?, find(names[1])?, find(names[2])?])
    }

    /// Binds the shown fire's three flipbooks (or the blank one).
    fn rebind(&mut self, device: &wgpu::Device) {
        let kind = self.shown.as_ref().and_then(|s| s.fire.as_ref()).map_or(FireKind::Molotov, |(f, _)| f.kind);
        self.group = self.group_for(device, &self.frame, kind);
        // the incendiaries' group follows the books too
        if let Some((frame, _)) = self.second.take() {
            let group = self.group_for(device, &frame, FireKind::Incendiary);
            self.second = Some((frame, group));
        }
    }

    /// A group over `frame` binding `kind`'s three flipbooks (or the blank one).
    fn group_for(&self, device: &wgpu::Device, frame: &wgpu::Buffer, kind: FireKind) -> wgpu::BindGroup {
        let blank;
        let views: [&wgpu::TextureView; 3] = match &self.books {
            Some(GpuBooks { views, layout }) => {
                let names = fire::books(kind);
                let pick = |k: usize| layout.iter().position(|(n, _, _)| n == names[k]).map_or(&views[0], |i| &views[i]);
                [pick(0), pick(1), pick(2)]
            }
            None => {
                blank = self.palette.clone();
                [&blank, &blank, &blank]
            }
        };
        Self::bind(device, &self.group_layout, frame, &self.noise, &self.palette, views, &self.repeated, &self.clamped)
    }

    /// Builds what `stage` shows that does not move: the flight's tube and marks, the blind lines; and readies its fire.
    pub fn set_stage(&mut self, device: &wgpu::Device, stage: Option<&Stage>) {
        // after a replay, its players and fires go
        if self.second_sprites.as_ref().is_some_and(|(_, n)| *n > 0) {
            self.second_sprites = self.second_sprites.take().map(|(b, _)| (b, 0));
        }
        let Some(stage) = stage else {
            self.shown = None;
            return;
        };
        if self.shown.as_ref().is_some_and(|s| s.stage_id == stage.throw_id) {
            return;
        }
        let mut opaque = Meshes::default();
        let mut tube = Meshes::default();
        let mut overlay = Meshes::default();
        if let Some(flight) = &stage.flight {
            let color = nade_color(stage.grenade);
            let curve = catmull_rom(&flight.points, (flight.points.len() * 6).min(600));
            tube.tube(&curve, 1.6, 8, rgba(color, 0.9), [0.0, 1.0]);
            // the same tube, faint, where walls hide it, so the arc still reads
            overlay.tube(&curve, 1.6, 8, rgba(color, 0.22), [0.0, 1.0]);
            for (at, size) in flight.marks() {
                opaque.sphere(v3(at), 4.0 * size as f32, rgba(color, 1.0), [0.0, 0.0]);
            }
        }
        for line in &stage.blinded {
            let color = if line.enemy {
                let s = line.strength as f32;
                [1.0, 0.85 + 0.15 * s, 0.3 + 0.7 * s, 0.85]
            } else {
                [1.0, 0.35, 0.3, 0.85]
            };
            overlay.tube(&[v3(line.from), v3(line.eye)], 0.9 + line.strength as f32 * 1.4, 6, color, [0.0, 0.0]);
            overlay.sphere(v3(line.eye), 7.0, [color[0], color[1], color[2], 0.9], [0.0, 0.0]);
        }
        let opaque_count = opaque.vertices.len() as u32;
        let tube_count = tube.vertices.len() as u32;
        let overlay_count = overlay.vertices.len() as u32;
        let all: Vec<Vertex> = opaque.vertices.into_iter().chain(tube.vertices).chain(overlay.vertices).collect();
        let meshes = (!all.is_empty()).then(|| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("throw meshes"), contents: bytemuck::cast_slice(&all), usage: wgpu::BufferUsages::VERTEX }));
        let fire = stage.fire.clone().map(|f| {
            let sequences = Self::sequences_of(self.books.as_ref(), f.kind);
            (f, sequences)
        });
        self.shown = Some(Shown {
            stage_id: stage.throw_id.clone(),
            meshes,
            opaque: 0..opaque_count,
            tube: opaque_count..opaque_count + tube_count,
            overlay: opaque_count + tube_count..opaque_count + tube_count + overlay_count,
            fire,
        });
        self.rebind(device);
    }

    /// The frame's values for the throw's shaders, the fire's flipbooks being `kind`'s.
    fn uniform(&self, t: f64, view: &View, size: (u32, u32), kind: Option<FireKind>) -> FrameUniform {
        let toward_sun = self.toward_sun;
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let s2s = source_to_scene();
        // the camera in Hammer units: the scene's view, scaled back to inches
        let view_h = Mat4::from_scale(Vec3::splat(1.0 / INCH)) * view.view() * s2s;
        let projection_h = view.projection(aspect) * Mat4::from_scale(Vec3::splat(INCH));
        let eye = to_source_dir(view.eye) / INCH;
        let books = self.books.as_ref().map(|b| b.layout.clone());
        let grid = |k: usize| -> (f32, f32) {
            let (Some(layout), Some(kind)) = (&books, kind) else { return (1.0, 1.0) };
            let name = fire::books(kind)[k];
            layout.iter().find(|(n, _, _)| n == name).map_or((1.0, 1.0), |(_, g, _)| (g.0 as f32, g.1 as f32))
        };
        let (g0, g1, g2) = (grid(0), grid(1), grid(2));
        FrameUniform {
            view_proj: (view.view_proj(aspect) * s2s).to_cols_array(),
            view: view_h.to_cols_array(),
            projection: projection_h.to_cols_array(),
            eye: [eye.x, eye.y, eye.z, t as f32],
            sun: [toward_sun.x, toward_sun.y, toward_sun.z, 0.9],
            screen: [size.0 as f32, size.1 as f32, view.near, view.far],
            flip: [if self.books.is_some() && kind.is_some() { 1.0 } else { 0.0 }, fire::Gradient::Film.row(), 0.0, 0.0],
            grid: [g0.0, g0.1, g1.0, g1.1],
            grid2: [g2.0, g2.1, 0.0, 0.0],
        }
    }

    /// Writes what the shown stage looks like `t` seconds after detonation, from `view` into targets of `size`.
    pub fn set_time(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, stage: Option<&Stage>, t: f64, view: &View, size: (u32, u32)) {
        self.time = t;
        self.second_sprites = self.second_sprites.take().map(|(b, _)| (b, 0));
        self.ghosts = self.ghosts.take().map(|(b, _)| (b, 0));
        let kind = self.shown.as_ref().and_then(|s| s.fire.as_ref()).map(|(f, _)| f.kind);
        let uniform = self.uniform(t, view, size, kind);
        queue.write_buffer(&self.frame, 0, bytemuck::bytes_of(&uniform));

        // the grenade in flight, before it goes off
        let mut moving = Meshes::default();
        if self.flight_visible
            && let Some((at, spin)) = stage.and_then(|s| s.flight.as_ref()).and_then(|f| f.grenade_at(t))
        {
            let color = stage.map_or([1.0; 3], |s| nade_color(s.grenade));
            let turn = Mat4::from_quat(glam::Quat::from_xyzw(spin.x as f32, spin.y as f32, spin.z as f32, spin.w as f32));
            moving.ellipsoid(v3(at), 1.7, 2.2, turn, rgba(color, 1.0), [1.0, 0.0]);
        }
        self.moving = upload(device, queue, self.moving.take(), "grenade", &moving.vertices);

        let mut glows = Vec::new();
        if let Some(burst) = stage.and_then(|s| s.burst) {
            let on = burst.on(t) as f32;
            if on > 0.0 {
                let (size, color) = if burst.flash { (260.0, [3.2, 3.2, 3.6]) } else { (300.0, [3.0, 1.2, 0.35]) };
                glows.push(GlowInstance { centre: [burst.at.x as f32, burst.at.y as f32, burst.at.z as f32, size], color: [color[0], color[1], color[2], on] });
            }
        }
        self.glows = upload(device, queue, self.glows.take(), "bursts", &glows);

        let (mut sprites, mut patches) = (Vec::new(), Vec::new());
        if let Some(shown) = &self.shown
            && let Some((fire, sequences)) = &shown.fire
        {
            sprites = fire::instances(fire.kind, &fire.sprites_at(t), sequences.as_ref())
                .into_iter()
                .map(|i| SpriteInstance { pos: [i.pos[0], i.pos[1], i.pos[2], 0.0], shape: i.shape, look: i.look, colour: i.colour, tone: i.tone, book: i.book })
                .collect();
            patches = fire
                .patches_at(t)
                .into_iter()
                .map(|p| PatchInstance { pos: [p.pos[0] as f32, p.pos[1] as f32, p.pos[2] as f32, 0.0], normal: [p.normal[0] as f32, p.normal[1] as f32, p.normal[2] as f32, 0.0], data: [p.radius as f32, p.glow as f32, p.scorch as f32, p.seed as f32] })
                .collect();
        }
        self.sprites = upload(device, queue, self.sprites.take(), "fire sprites", &sprites);
        self.patches = upload(device, queue, self.patches.take(), "fire ground", &patches);
    }

    /// A replay's moment: every player, and each throw still showing with its stage's own time (seconds after its
    /// detonation): the grenade while it flies, its burst, its fire. Nothing of a single shown stage stays.
    #[allow(clippy::too_many_arguments)]
    pub fn set_replay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, moment: &[(&Stage, f64)], players: &[ReplayPlayer], view: &View, size: (u32, u32)) {
        if self.shown.is_some() {
            self.shown = None;
            // the main group binds the molotov's flipbooks
            self.rebind(device);
        }
        self.time = 0.0;
        self.flight_visible = false;
        self.ghosts = self.ghosts.take().map(|(b, _)| (b, 0));
        queue.write_buffer(&self.frame, 0, bytemuck::bytes_of(&self.uniform(0.0, view, size, Some(FireKind::Molotov))));

        let mut moving = Meshes::default();
        let mut ghosts = Meshes::default();
        for p in players {
            moving.player(p, 1.0);
            if p.alive {
                ghosts.player(p, 0.35);
            }
        }
        self.ghosts = upload(device, queue, self.ghosts.take(), "replay players behind walls", &ghosts.vertices);
        let mut glows = Vec::new();
        let (mut molotov, mut incendiary, mut patches) = (Vec::new(), Vec::new(), Vec::new());
        for (stage, t) in moment {
            let t = *t;
            if t < 0.0
                && let Some((at, spin)) = stage.flight.as_ref().and_then(|f| f.grenade_at(t))
            {
                let turn = Mat4::from_quat(glam::Quat::from_xyzw(spin.x as f32, spin.y as f32, spin.z as f32, spin.w as f32));
                moving.ellipsoid(v3(at), 1.7 * 1.6, 2.2 * 1.6, turn, rgba(nade_color(stage.grenade), 1.0), [1.0, 0.0]);
            }
            if let Some(burst) = stage.burst {
                let on = burst.on(t) as f32;
                if on > 0.0 {
                    let (size, color) = if burst.flash { (260.0, [3.2, 3.2, 3.6]) } else { (300.0, [3.0, 1.2, 0.35]) };
                    glows.push(GlowInstance { centre: [burst.at.x as f32, burst.at.y as f32, burst.at.z as f32, size], color: [color[0], color[1], color[2], on] });
                }
            }
            if let Some(f) = &stage.fire {
                let sequences = Self::sequences_of(self.books.as_ref(), f.kind);
                let sprites = fire::instances(f.kind, &f.sprites_at(t), sequences.as_ref())
                    .into_iter()
                    .map(|i| SpriteInstance { pos: [i.pos[0], i.pos[1], i.pos[2], 0.0], shape: i.shape, look: i.look, colour: i.colour, tone: i.tone, book: i.book });
                if f.kind == FireKind::Incendiary { incendiary.extend(sprites) } else { molotov.extend(sprites) }
                patches.extend(f.patches_at(t).into_iter().map(|p| PatchInstance { pos: [p.pos[0] as f32, p.pos[1] as f32, p.pos[2] as f32, 0.0], normal: [p.normal[0] as f32, p.normal[1] as f32, p.normal[2] as f32, 0.0], data: [p.radius as f32, p.glow as f32, p.scorch as f32, p.seed as f32] }));
            }
        }
        self.moving = upload(device, queue, self.moving.take(), "replay meshes", &moving.vertices);
        self.glows = upload(device, queue, self.glows.take(), "bursts", &glows);
        self.sprites = upload(device, queue, self.sprites.take(), "fire sprites", &molotov);
        self.patches = upload(device, queue, self.patches.take(), "fire ground", &patches);
        if !incendiary.is_empty() && self.second.is_none() {
            let frame = device.create_buffer(&wgpu::BufferDescriptor { label: Some("throw frame (incendiary)"), size: std::mem::size_of::<FrameUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let group = self.group_for(device, &frame, FireKind::Incendiary);
            self.second = Some((frame, group));
        }
        if let Some((frame, _)) = &self.second {
            queue.write_buffer(frame, 0, bytemuck::bytes_of(&self.uniform(0.0, view, size, Some(FireKind::Incendiary))));
        }
        self.second_sprites = upload(device, queue, self.second_sprites.take(), "fire sprites (incendiary)", &incendiary);
    }

    /// The draws in the map's pass, over its depth: marks and the grenade, the tube, the bursts, then what draws over
    /// everything (the tube's faint copy, the blind lines).
    pub fn draw_scene(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.group, &[]);
        // the flight's tube and marks show once it has gone off (render/effects.ts): before, the grenade flies alone
        let flight = self.flight_visible && self.time >= 0.0;
        let meshes = self.shown.as_ref().and_then(|s| Some((s, s.meshes.as_ref()?)));
        if let Some((shown, meshes)) = meshes
            && flight
            && !shown.opaque.is_empty()
        {
            pass.set_pipeline(&self.mesh_opaque);
            pass.set_vertex_buffer(0, meshes.slice(..));
            pass.draw(shown.opaque.clone(), 0..1);
        }
        // the grenade in flight, and a replay's players
        if let Some((buffer, count)) = self.moving.as_ref().filter(|(_, n)| *n > 0) {
            pass.set_pipeline(&self.mesh_opaque);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..*count, 0..1);
        }
        if let Some((shown, meshes)) = meshes
            && flight
            && !shown.tube.is_empty()
        {
            pass.set_pipeline(&self.mesh_tube);
            pass.set_vertex_buffer(0, meshes.slice(..));
            pass.draw(shown.tube.clone(), 0..1);
        }
        if let Some((buffer, count)) = self.glows.as_ref().filter(|(_, n)| *n > 0) {
            pass.set_pipeline(&self.glow);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..*count);
        }
        if let Some(shown) = &self.shown
            && let Some(meshes) = &shown.meshes
        {
            // the tube's faint copy only with the tube; the blind lines always
            let tube_ghost = shown.overlay.start..shown.overlay.start + shown.tube.len() as u32;
            let lines = tube_ghost.end..shown.overlay.end;
            pass.set_pipeline(&self.mesh_overlay);
            pass.set_vertex_buffer(0, meshes.slice(..));
            if flight && !tube_ghost.is_empty() {
                pass.draw(tube_ghost, 0..1);
            }
            if !lines.is_empty() {
                pass.draw(lines, 0..1);
            }
        }
        if let Some((buffer, count)) = self.ghosts.as_ref().filter(|(_, n)| *n > 0) {
            pass.set_pipeline(&self.mesh_overlay);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..*count, 0..1);
        }
    }

    /// Whether the effects pass has anything of the throw's to draw.
    pub fn has_effects(&self) -> bool {
        [&self.sprites, &self.patches, &self.second_sprites].iter().any(|b| b.as_ref().is_some_and(|(_, n)| *n > 0))
    }

    /// The draws in the effects pass, which read the scene's depth: the fire's ground, then its sprites.
    pub fn draw_effects(&self, pass: &mut wgpu::RenderPass<'_>, targets: &ThrowTargets) {
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_bind_group(1, &targets.depth, &[]);
        if let Some((buffer, count)) = self.patches.as_ref().filter(|(_, n)| *n > 0) {
            pass.set_pipeline(&self.patch);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..*count);
        }
        if let Some((buffer, count)) = self.sprites.as_ref().filter(|(_, n)| *n > 0) {
            pass.set_pipeline(&self.sprite);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..*count);
        }
        // a replay's incendiaries, with their own flipbooks
        if let (Some((buffer, count)), Some((_, group))) = (self.second_sprites.as_ref().filter(|(_, n)| *n > 0), &self.second) {
            pass.set_bind_group(0, group, &[]);
            pass.set_pipeline(&self.sprite);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..*count);
        }
    }
}

/// `items` in a vertex buffer, reusing `previous` when it is big enough; None when there are none.
fn upload<T: Pod>(device: &wgpu::Device, queue: &wgpu::Queue, previous: Option<(wgpu::Buffer, u32)>, label: &str, items: &[T]) -> Option<(wgpu::Buffer, u32)> {
    if items.is_empty() {
        // kept for the next moment's items
        return previous.map(|(b, _)| (b, 0));
    }
    let bytes: &[u8] = bytemuck::cast_slice(items);
    let buffer = match previous {
        Some((b, _)) if b.size() >= bytes.len() as u64 => b,
        _ => device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: (bytes.len() as u64).next_power_of_two().max(256), usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
    };
    queue.write_buffer(&buffer, 0, bytes);
    Some((buffer, items.len() as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hammer_units_turn_into_the_scenes_frame() {
        let p = DVec3::new(100.0, 200.0, 300.0);
        let scene = source_to_scene().transform_point3(Vec3::new(100.0, 200.0, 300.0));
        assert!((scene - to_scene(p)).length() < 1e-5);
        assert!((scene - Vec3::new(200.0, 300.0, 100.0) * INCH).length() < 1e-5);
        assert_eq!(to_source_dir(Vec3::new(1.0, 2.0, 3.0)), Vec3::new(3.0, 1.0, 2.0));
    }

    #[test]
    fn the_curve_passes_through_its_points_and_samples_evenly() {
        let points = [DVec3::ZERO, DVec3::new(100.0, 0.0, 0.0), DVec3::new(200.0, 0.0, 0.0)];
        let curve = catmull_rom(&points, 10);
        assert_eq!(curve.len(), 11);
        assert!(curve[0].length() < 1e-3 && (curve[10] - Vec3::new(200.0, 0.0, 0.0)).length() < 1e-3);
        assert!((curve[5] - Vec3::new(100.0, 0.0, 0.0)).length() < 0.5);
        let mut meshes = Meshes::default();
        meshes.tube(&curve, 1.6, 8, [1.0; 4], [0.0; 2]);
        assert_eq!(meshes.vertices.len(), 10 * 8 * 6);
        // every vertex sits 1.6 from the line
        assert!(meshes.vertices.iter().all(|v| (Vec3::new(0.0, v.position[1], v.position[2]).length() - 1.6).abs() < 1e-3));
    }
}
