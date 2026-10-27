//! A pack's map on the GPU, drawn in CS2's look (`map.wgsl`, then `post.wgsl` through [`crate::post`]).
//!
//! - **Uploads.** The pack's Geometry and Textures sections become GPU buffers. On Metal they are the pack's own
//!   mapped pages (`newBufferWithBytesNoCopy`): nothing is copied or zero-filled on the CPU. The GPU copies each
//!   texture's mips out of that buffer as the pack lays them out. Elsewhere each section is copied once.
//! - **One bind group for the scene.** Every texture is in one binding array, every material in one table, every
//!   drawn instance in one slot table; `map.wgsl` pulls the vertices from the Geometry section. A draw is one
//!   `draw_indexed` whose instances index the slots, so the only state that changes between draws is the pipeline.
//! - **The draws are recorded once** into render bundles (the map's, the 3D skybox's), which every frame executes;
//!   csgo_effects surfaces, which read the depth drawn before them, draw in passes of their own.
//! - **A frame** is: the sky and the 3D skybox from the sky camera; the map over them with the depth cleared, and the
//!   shown throw's flight ([`crate::throw`]); the map's effects and the throw's fire; the composite, which adds the
//!   smoke ([`crate::smoke`]); then exposure, bloom and the present pass ([`crate::post`]).

use std::collections::HashMap;
use std::num::{NonZeroU32, NonZeroU64};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use nades_pack::Pack;
use nades_pack::scene::{AlphaMode, CubemapFog, Draw, GradientFog, IndexFormat, Lightmaps, Material, Scene, Semantic, Shading, Texture, TextureFormat};
use wgpu::util::DeviceExt as _;

use crate::post::{self, HDR_FORMAT};
use crate::smoke::{Composite, SmokeLight};
use crate::throw::{ThrowLayer, ThrowTargets, to_scene};
use crate::view::{INCH, SkyPlacement, View};

/// The depth buffer's format. Depth is reversed: 1 at the near plane, 0 at the far one.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// An absent stream or texture, in slots and materials.
const NONE: u32 = u32::MAX;

/// One drawn instance, as `map.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Slot {
    world: [f32; 16],
    /// The world matrix's inverse transpose: three columns, each padded to four floats.
    normal_matrix: [f32; 12],
    /// Offsets in 4-byte words into the Geometry section, or [`NONE`].
    positions: u32,
    uv0: u32,
    uv1: u32,
    lightmap: u32,
    frames: u32,
    colors: u32,
    paint: u32,
    material: u32,
}

/// A material, as `map.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuMaterial {
    tex0: [u32; 4],
    tex1: [u32; 4],
    tex2: [u32; 4],
    info: [u32; 4],
    factor: [f32; 4],
    tint: [f32; 4],
    tint2: [f32; 4],
    uv: [[f32; 4]; 6],
    adjust: [[f32; 4]; 9],
    tint_adjust: [[f32; 4]; 9],
    layer_a: [[f32; 4]; 3],
    layer_b: [[f32; 4]; 3],
    layer_c: [[f32; 4]; 3],
    fade: [f32; 4],
    fresnel: [f32; 4],
    effects: [f32; 4],
    masks: [[f32; 4]; 3],
    facing: [[f32; 4]; 2],
    facing_range: [f32; 4],
}

/// `map.wgsl`'s Frame: what one pass (the map's, or the 3D skybox's) draws with.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct FrameUniform {
    view_proj: [f32; 16],
    inverse_view_proj: [f32; 16],
    eye: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    lightmaps: [u32; 4],
    sky: [f32; 4],
    sky_info: [f32; 4],
    fog_gradient: [f32; 4],
    fog_gradient_exp: [f32; 4],
    fog_gradient_color: [f32; 4],
    fog_cube: [f32; 4],
    fog_cube_height: [f32; 4],
    fog_cube_color: [f32; 4],
    screen: [f32; 4],
    shadow_mask: [f32; 4],
    probe_info: [f32; 4],
    point: [f32; 4],
    point_color: [f32; 4],
    point_falloff: [f32; 4],
}

/// One light probe volume, as `map.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuProbeVolume {
    to_local: [f32; 16],
    atlas_scale: [f32; 4],
    atlas_offset: [f32; 4],
    border_min: [f32; 4],
    border_max: [f32; 4],
    inverse_edge: [f32; 4],
    bounds_extend: [f32; 4],
}

// info.z flags, as map.wgsl names them
const MODEL_TINT_1: u32 = 1;
const NEW_BLENDING: u32 = 8;
const UV2_1: u32 = 16;
const RGB_NORMAL_1: u32 = 128;
const ADDITIVE: u32 = 1024;
const MASK_1: u32 = 2048;
const DOUBLE_SIDED: u32 = 16384;

/// How a draw's fragments reach the target.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
enum Stage {
    Opaque,
    Mask,
    Blend,
    Additive,
    Effects,
    EffectsAdditive,
}

/// What a draw's pipeline depends on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Key {
    stage: Stage,
    double_sided: bool,
}

fn stage(m: &Material) -> Stage {
    match (&m.shading, m.alpha) {
        (Shading::Effects(e), _) => {
            if e.additive {
                Stage::EffectsAdditive
            } else {
                Stage::Effects
            }
        }
        (Shading::Unlit { additive: true }, _) => Stage::Additive,
        (_, AlphaMode::Blend) => Stage::Blend,
        (_, AlphaMode::Mask { .. }) => Stage::Mask,
        (_, AlphaMode::Opaque) => Stage::Opaque,
    }
}

fn key(scene: &Scene, material: u32) -> Key {
    let m = &scene.materials[material as usize];
    Key { stage: stage(m), double_sided: m.double_sided }
}

/// One effects draw, recorded each frame (it reads the depth drawn before it).
struct EffectDraw {
    key: Key,
    indices: std::ops::Range<u32>,
    format: wgpu::IndexFormat,
    slots: std::ops::Range<u32>,
}

/// One pass's camera-independent constants.
#[derive(Clone, Copy)]
struct PassLight {
    lightmaps: Option<Lightmaps>,
    shadow_width: f32,
    fog: FogValues,
    /// Light probe volumes it samples: the map's in the map's pass, none in the skybox's (its own space).
    probes: usize,
}

/// The fog of one pass, as map.wgsl's Frame holds it (render/fog.ts `fogValues`).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct FogValues {
    pub gradient: [f32; 4],
    pub gradient_exp: [f32; 4],
    pub gradient_color: [f32; 4],
    pub cube: [f32; 4],
    pub cube_height: [f32; 4],
    pub cube_color: [f32; 4],
}

/// Fog authored in the map's units, in a pass's space: distances × `distance`, heights h × `distance` + `height`.
#[derive(Clone, Copy, Debug)]
pub struct FogSpace {
    pub distance: f32,
    pub height: f32,
}

pub const WORLD_SPACE: FogSpace = FogSpace { distance: 1.0, height: 0.0 };

/// The fog values for one space: `gradient` is the env_gradient_fog this pass draws with, the cubemap fog always the
/// map's; `sky_face` is the sky cube's face width, which sets how blurred its haziest colour is.
pub fn fog_values(gradient: Option<&GradientFog>, cube: Option<&CubemapFog>, space: FogSpace, sky_face: u32) -> FogValues {
    let d = |x: f32| x * space.distance;
    let h = |x: f32| x * space.distance + space.height;
    let mut v = FogValues { gradient: [0.0, 1.0, 0.0, 0.0], gradient_exp: [1.0, 1.0, 0.0, 0.0], cube: [0.0, 0.0, 0.0, 1.0], cube_height: [1.0, 0.0, 0.0, 0.0], cube_color: [1.0, 0.0, 0.0, 0.0], ..Default::default() };
    if let Some(g) = gradient.filter(|g| g.end > g.start) {
        let start = d(g.start);
        let scale = 1.0 / (d(g.end) - start);
        let (mut height_scale, mut height_bias) = (0.0, 1.0);
        if g.height && g.height_start != g.height_end {
            height_scale = 1.0 / (h(g.height_start) - h(g.height_end));
            height_bias = -h(g.height_end) * height_scale;
        }
        v.gradient = [-start * scale, height_bias, scale, height_scale];
        v.gradient_exp = [g.falloff, if g.height { g.vertical_exponent } else { 1.0 }, 0.0, 0.0];
        v.gradient_color = [g.color[0], g.color[1], g.color[2], g.max_opacity];
    }
    if let Some(f) = cube.filter(|f| f.end > f.start && sky_face > 0) {
        let start = d(f.start);
        let scale = 1.0 / (d(f.end) - start);
        v.cube = [-start * scale, scale, f.lod_bias, f.falloff];
        // the blurriest sky: three mips above one texel of a face
        let max_lod = ((sky_face as f32).log2() - 3.0).max(0.0);
        v.cube_height = [1.0, 0.000_001, 0.0, max_lod];
        if f.height && f.height_end - f.height_start > 0.0 {
            let height_start = h(f.height_start);
            let height_scale = 1.0 / (height_start - h(f.height_end));
            v.cube_height = [1.0 - height_start * height_scale, height_scale, f.height_exponent, max_lod];
        }
        v.cube_color = [f.sky_scale, f.max_opacity, 0.0, 0.0];
    }
    v
}

/// The targets for one frame size: the scene's colour and depth, and the post-processing's.
pub struct Targets {
    pub post: post::Targets,
    /// The scene before the composite pass adds the smoke into `post.hdr`.
    scene: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// The depth as the effects passes read it.
    depth_group: wgpu::BindGroup,
    /// The scene's colour and depth, as the composite reads them.
    scene_group: wgpu::BindGroup,
    throw: ThrowTargets,
}

impl Targets {
    pub fn size(&self) -> (u32, u32) {
        self.post.size
    }
}

pub struct Map {
    /// This map's own number in the process: targets made for one map are not another's.
    pub id: u64,
    /// What it holds on the GPU: its textures as the GPU allocated them, and its geometry.
    pub gpu_bytes: u64,
    pub draws: usize,
    pub triangles: u64,
    frame_layout: wgpu::BindGroupLayout,
    depth_layout: wgpu::BindGroupLayout,
    scene_group: wgpu::BindGroup,
    map_frame: (wgpu::Buffer, wgpu::BindGroup),
    sky_frame: (wgpu::Buffer, wgpu::BindGroup),
    map_bundle: wgpu::RenderBundle,
    sky_bundle: Option<wgpu::RenderBundle>,
    map_effects: Vec<EffectDraw>,
    sky_effects: Vec<EffectDraw>,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
    sky_pipeline: wgpu::RenderPipeline,
    geometry: wgpu::Buffer,
    map_light: PassLight,
    sky_light: PassLight,
    sun: Option<nades_pack::scene::Sun>,
    sky: Option<(nades_pack::scene::Sky, f32, f32)>,
    placement: Option<SkyPlacement>,
    post: post::Effects,
    /// The composite pass and the shown smoke.
    pub composite: Composite,
    /// The shown throw's flight, bursts and fire.
    pub throw: ThrowLayer,
}

/// A point light an effect puts on the map, in Hammer units (three.js's PointLight in the web view).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    pub at: Vec3,
    pub color: [f32; 3],
    /// Candela.
    pub intensity: f32,
    /// How far it reaches (Hammer units), and how it falls off.
    pub distance: f32,
    pub decay: f32,
}

/// What a frame draws of the scene's throws beyond what [`Map::throw`] and [`Map::composite`] hold: the effects' light
/// on the map, and how many jittered frames the target has averaged.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameEffects {
    pub light: Option<PointLight>,
    pub accumulated: u32,
}

impl Map {
    /// Puts `scene` on the GPU from `pack` and records its draws for `output` targets. The texture copies are
    /// submitted before it returns; the GPU runs them ahead of any frame. `mark` records start-up stages.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, pack: &'static Pack, scene: &Scene, output: wgpu::TextureFormat, mark: &(dyn Fn(&'static str) + Sync)) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("map"), source: wgpu::ShaderSource::Wgsl(include_str!("map.wgsl").into()) });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(std::mem::size_of::<FrameUniform>() as u64) },
                count: None,
            }],
        });
        let count = scene.textures.len().max(1) as u32;
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene"),
            entries: &[
                storage(0, wgpu::ShaderStages::VERTEX),
                storage(1, wgpu::ShaderStages::VERTEX),
                storage(2, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: texture_type(wgpu::TextureViewDimension::D2), count: NonZeroU32::new(count) },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: texture_type(wgpu::TextureViewDimension::Cube), count: None },
                sampler(5),
                sampler(6),
                wgpu::BindGroupLayoutEntry { binding: 7, visibility: wgpu::ShaderStages::FRAGMENT, ty: texture_type(wgpu::TextureViewDimension::D2), count: None },
                wgpu::BindGroupLayoutEntry { binding: 8, visibility: wgpu::ShaderStages::FRAGMENT, ty: texture_type(wgpu::TextureViewDimension::D3), count: None },
                wgpu::BindGroupLayoutEntry { binding: 9, visibility: wgpu::ShaderStages::FRAGMENT, ty: texture_type(wgpu::TextureViewDimension::D3), count: None },
                storage(10, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("map"), bind_group_layouts: &[Some(&frame_layout), Some(&scene_layout)], immediate_size: 0 });
        let effects_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("effects"), bind_group_layouts: &[Some(&frame_layout), Some(&scene_layout), Some(&depth_layout)], immediate_size: 0 });
        let mut keys: Vec<Key> = scene.all_draws().map(|d| key(scene, d.material)).collect();
        keys.sort_by_key(|k| (k.stage, k.double_sided));
        keys.dedup();

        let (shader, layout, effects_layout) = (&shader, &layout, &effects_layout);
        let (pipelines, sky_pipeline, post, textures, geometry, composite, mut throw) = std::thread::scope(|s| {
            // each pipeline on its own thread, while this one sets up the buffers and textures
            let pipelines: Vec<_> = keys.iter().map(|&k| s.spawn(move || (k, pipeline(device, if matches!(k.stage, Stage::Effects | Stage::EffectsAdditive) { effects_layout } else { layout }, shader, k)))).collect();
            let sky = s.spawn(move || sky_pipeline(device, layout, shader));
            let post = s.spawn(move || post::Effects::new(device, queue, &scene.post, output));
            let composite = s.spawn(move || Composite::new(device, SmokeLight::of(scene.sun.as_ref())));
            let throw = s.spawn(move || ThrowLayer::new(device, queue));
            let geometry = buffer_over(device, "geometry", pack.geometry(), wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX | wgpu::BufferUsages::STORAGE);
            mark("map:buffers");
            let textures: Vec<wgpu::Texture> = scene.textures.iter().map(|t| create_texture(device, t)).collect();
            upload_textures(device, queue, pack, scene, &textures);
            mark("map:copies");
            let pipelines: HashMap<Key, wgpu::RenderPipeline> = pipelines.into_iter().map(|p| p.join().expect("a pipeline thread")).collect();
            let sky = sky.join().expect("the sky's pipeline thread");
            let post = post.join().expect("the post-processing's thread");
            let composite = composite.join().expect("the composite's thread");
            let throw = throw.join().expect("the throw's thread");
            mark("map:pipelines");
            (pipelines, sky, post, textures, geometry, composite, throw)
        });

        let gpu_bytes = geometry.size() + scene.textures.iter().zip(&textures).map(|(t, texture)| allocated_bytes(texture).unwrap_or_else(|| texture_bytes(t))).sum::<u64>();
        let white = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor { label: Some("white"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255; 4],
        );
        let white = white.create_view(&Default::default());
        let views: Vec<wgpu::TextureView> = scene.textures.iter().zip(&textures).map(|(t, texture)| if t.is_cube() || t.volume { white.clone() } else { texture.create_view(&Default::default()) }).collect();
        let volume_view = |t: Option<u32>| match t {
            Some(t) => textures[t as usize].create_view(&Default::default()),
            None => device
                .create_texture_with_data(
                    queue,
                    &wgpu::TextureDescriptor { label: Some("no probes"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D3, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &[0; 4],
                )
                .create_view(&Default::default()),
        };
        let probe_irradiance = volume_view(scene.probes.as_ref().map(|p| p.irradiance));
        let probe_shadows = volume_view(scene.probes.as_ref().and_then(|p| p.shadows));
        let mut probe_volumes: Vec<GpuProbeVolume> = scene
            .probes
            .iter()
            .flat_map(|p| &p.volumes)
            .map(|v| {
                let four = |a: [f32; 3]| [a[0], a[1], a[2], 0.0];
                GpuProbeVolume { to_local: v.to_local, atlas_scale: four(v.atlas_scale), atlas_offset: four(v.atlas_offset), border_min: four(v.border_min), border_max: four(v.border_max), inverse_edge: four(v.inverse_edge), bounds_extend: four(v.bounds_extend) }
            })
            .collect();
        let probe_count = probe_volumes.len();
        if probe_volumes.is_empty() {
            probe_volumes.push(GpuProbeVolume::zeroed());
        }
        let probe_volumes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("probe volumes"), contents: bytemuck::cast_slice(&probe_volumes), usage: wgpu::BufferUsages::STORAGE });
        let views: Vec<&wgpu::TextureView> = if views.is_empty() { vec![&white] } else { views.iter().collect() };
        let sky_view = match scene.sky {
            Some(sky) => textures[sky.texture as usize].create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() }),
            None => device
                .create_texture_with_data(
                    queue,
                    &wgpu::TextureDescriptor { label: Some("no sky"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 6 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &[0; 24],
                )
                .create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() }),
        };
        let noise = noise_texture(device, queue);
        let material_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("materials"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lightmaps and sky"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let all: Vec<&Draw> = scene.all_draws().collect();
        let slots = slots(&all, pack.geometry());
        let slots = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("slots"), contents: bytemuck::cast_slice(&slots), usage: wgpu::BufferUsages::STORAGE });
        let materials: Vec<GpuMaterial> = scene.materials.iter().map(|m| gpu_material(m, scene)).collect();
        let materials = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("materials"), contents: bytemuck::cast_slice(&materials), usage: wgpu::BufferUsages::STORAGE });
        let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene"),
            layout: &scene_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: slots.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: geometry.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: materials.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureViewArray(&views) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&sky_view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&material_sampler) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&clamp_sampler) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&noise) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&probe_irradiance) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&probe_shadows) },
                wgpu::BindGroupEntry { binding: 10, resource: probe_volumes.as_entire_binding() },
            ],
        });
        let frame = |label: &str| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: std::mem::size_of::<FrameUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(label), layout: &frame_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }] });
            (buffer, group)
        };
        let map_frame = frame("map frame");
        let sky_frame = frame("sky frame");

        // the draws: a bundle of the opaque and blended ones per pass, the effects listed for each frame
        let record = |draws: &[Draw], first_slot: u32, frame: &wgpu::BindGroup| -> (Option<wgpu::RenderBundle>, Vec<EffectDraw>) {
            let mut encoder = device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                label: Some("map"),
                color_formats: &[Some(HDR_FORMAT)],
                depth_stencil: Some(wgpu::RenderBundleDepthStencil { format: DEPTH_FORMAT, depth_read_only: false, stencil_read_only: true }),
                sample_count: 1,
                multiview: None,
            });
            encoder.set_bind_group(0, frame, &[]);
            encoder.set_bind_group(1, &scene_group, &[]);
            let (mut bound_key, mut bound_format) = (None, None);
            let mut slot = first_slot;
            let mut effects = Vec::new();
            for d in draws {
                let k = key(scene, d.material);
                let (first, format) = index_range(d);
                let instances = d.instance_count();
                if matches!(k.stage, Stage::Effects | Stage::EffectsAdditive) {
                    effects.push(EffectDraw { key: k, indices: first..first + d.index_count, format, slots: slot..slot + instances });
                    slot += instances;
                    continue;
                }
                if bound_key != Some(k) {
                    encoder.set_pipeline(&pipelines[&k]);
                    bound_key = Some(k);
                }
                if bound_format != Some(format) {
                    encoder.set_index_buffer(geometry.slice(..), format);
                    bound_format = Some(format);
                }
                encoder.draw_indexed(first..first + d.index_count, 0, slot..slot + instances);
                slot += instances;
            }
            (Some(encoder.finish(&wgpu::RenderBundleDescriptor { label: Some("map") })), effects)
        };
        let (map_bundle, map_effects) = record(&scene.draws, 0, &map_frame.1);
        let map_slots = scene.draws.iter().map(Draw::instance_count).sum();
        let (sky_bundle, sky_effects) = match &scene.skybox {
            Some(skybox) if !skybox.draws.is_empty() => record(&skybox.draws, map_slots, &sky_frame.1),
            _ => (None, Vec::new()),
        };
        mark("map:bundle");

        throw.toward_sun = composite.light.toward_sun;
        let sky_face = scene.sky.map_or(0, |s| scene.textures[s.texture as usize].width);
        let shadow_width = |l: Option<Lightmaps>| l.and_then(|l| l.shadows).map_or(0.0, |t| scene.textures[t as usize].width as f32);
        let map_light = PassLight { lightmaps: scene.lightmaps, shadow_width: shadow_width(scene.lightmaps), fog: fog_values(scene.post.gradient_fog.as_ref(), scene.post.fog.as_ref(), WORLD_SPACE, sky_face), probes: probe_count };
        let placement = scene.skybox.as_ref().map(|k| {
            let origin = Vec3::from_array(k.origin);
            let radius = k.draws.iter().flat_map(|d| [d.bounds.min, d.bounds.max]).map(|corner| Vec3::from_array(corner).distance(origin)).fold(0.0f32, f32::max);
            SkyPlacement { scale: k.scale, origin, reference: Vec3::from_array(k.reference), radius: radius * 1.8 }
        });
        let sky_light = match (&scene.skybox, &placement) {
            (Some(k), Some(p)) => {
                let space = crate::map::FogSpace { distance: 1.0 / p.scale, height: p.origin.y - p.reference.y / p.scale };
                let gradient = k.gradient_fog.as_ref().or(scene.post.gradient_fog.as_ref());
                PassLight { lightmaps: k.lightmaps, shadow_width: shadow_width(k.lightmaps), fog: fog_values(gradient, scene.post.fog.as_ref(), space, sky_face), probes: 0 }
            }
            _ => PassLight { lightmaps: None, shadow_width: 0.0, fog: map_light.fog, probes: 0 },
        };
        let sky = scene.sky.map(|s| {
            let t = &scene.textures[s.texture as usize];
            (s, ((t.width as f32).log2() - 3.0).max(0.0), (t.mips.len() as f32 - 3.0).max(0.0))
        });
        static MAPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: MAPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            gpu_bytes,
            draws: all.len(),
            triangles: all.iter().map(|d| d.triangles()).sum(),
            frame_layout,
            depth_layout,
            scene_group,
            map_frame,
            sky_frame,
            map_bundle: map_bundle.expect("recorded"),
            sky_bundle,
            map_effects,
            sky_effects,
            pipelines,
            sky_pipeline,
            geometry,
            map_light,
            sky_light,
            sun: scene.sun,
            sky,
            placement,
            post,
            composite,
            throw,
        }
    }

    /// The targets for frames of `width`×`height` pixels.
    pub fn targets(&self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) -> Targets {
        let post = self.post.targets(device, queue, (width.max(1), height.max(1)));
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let depth_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("scene depth"), layout: &self.depth_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth) }] });
        let scene = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("scene"),
                size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let scene_group = self.composite.scene_group(device, &scene, &depth);
        let throw = self.throw.targets(device, &depth);
        let _ = &self.frame_layout;
        Targets { post, scene, depth, depth_group, scene_group, throw }
    }

    /// The map's sun (towards it, in Hammer units' axes), for what the throw lights with it.
    pub fn toward_sun(&self) -> Vec3 {
        self.composite.light.toward_sun
    }

    fn frame_uniform(&self, view: &View, aspect: f32, light: &PassLight, size: (u32, u32), seconds: f32, point: Option<&PointLight>) -> FrameUniform {
        let view_proj = view.view_proj(aspect);
        let lightmaps = light.lightmaps.map_or([NONE; 4], |l| [l.irradiance, l.directional.unwrap_or(NONE), l.shadows.unwrap_or(NONE), u32::from(light.shadow_width > 4096.0)]);
        let (sky, sky_info) = match &self.sky {
            Some((s, max_lod, ambient)) => ([s.tint[0], s.tint[1], s.tint[2], s.yaw.to_radians()], [1.0, *max_lod, *ambient, light.shadow_width]),
            None => ([1.0, 1.0, 1.0, 0.0], [0.0, 0.0, 0.0, light.shadow_width]),
        };
        let f = &light.fog;
        FrameUniform {
            view_proj: view_proj.to_cols_array(),
            inverse_view_proj: view_proj.inverse().to_cols_array(),
            eye: [view.eye.x, view.eye.y, view.eye.z, seconds],
            sun_dir: self.sun.map_or([0.0; 4], |s| [s.direction[0], s.direction[1], s.direction[2], 1.0]),
            sun_color: self.sun.map_or([0.0; 4], |s| [s.color[0], s.color[1], s.color[2], 0.0]),
            lightmaps,
            sky,
            sky_info,
            fog_gradient: f.gradient,
            fog_gradient_exp: f.gradient_exp,
            fog_gradient_color: f.gradient_color,
            fog_cube: f.cube,
            fog_cube_height: f.cube_height,
            fog_cube_color: f.cube_color,
            screen: [size.0 as f32, size.1 as f32, view.near, view.far],
            shadow_mask: std::array::from_fn(|c| if light.lightmaps.and_then(|l| l.sun_channel) == Some(c as u8) { 1.0 } else { 0.0 }),
            probe_info: [light.probes as f32, 0.0, 0.0, 0.0],
            point: point.map_or([0.0; 4], |p| {
                let at = to_scene(glam::DVec3::new(p.at.x.into(), p.at.y.into(), p.at.z.into()));
                [at.x, at.y, at.z, p.intensity]
            }),
            point_color: point.map_or([0.0; 4], |p| [p.color[0], p.color[1], p.color[2], p.distance]),
            point_falloff: [point.map_or(1.0, |p| p.decay), INCH, 0.0, 0.0],
        }
    }

    /// The csgo_effects surfaces of one pass (and, in the map's, the throw's fire), over the depth drawn before them.
    fn effects_pass(&self, encoder: &mut wgpu::CommandEncoder, targets: &Targets, frame: &wgpu::BindGroup, draws: &[EffectDraw], throw: bool) {
        let throw = throw && self.throw.has_effects();
        if draws.is_empty() && !throw {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("effects"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &targets.scene, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &targets.depth, depth_ops: None, stencil_ops: None }),
            ..Default::default()
        });
        if !draws.is_empty() {
            pass.set_bind_group(0, frame, &[]);
            pass.set_bind_group(1, &self.scene_group, &[]);
            pass.set_bind_group(2, &targets.depth_group, &[]);
            for d in draws {
                pass.set_pipeline(&self.pipelines[&d.key]);
                pass.set_index_buffer(self.geometry.slice(..), d.format);
                pass.draw_indexed(d.indices.clone(), 0, d.slots.clone());
            }
        }
        if throw {
            self.throw.draw_effects(&mut pass, &targets.throw);
        }
    }

    /// Every pass of a frame from `view` up to the present pass: the sky and the 3D skybox, the map and the shown throw,
    /// their effects, the composite (with the smoke), the exposure and the bloom, into `targets`. `seconds` animates
    /// the map effects' masks; `effects` says what of the throw this frame draws (its layer holds the rest).
    pub fn render(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, targets: &Targets, view: &View, seconds: f32, effects: &FrameEffects) {
        let size = targets.size();
        let aspect = size.0 as f32 / size.1 as f32;
        let sky_view = self.placement.map_or(*view, |p| p.view(view));
        queue.write_buffer(&self.map_frame.0, 0, bytemuck::bytes_of(&self.frame_uniform(view, aspect, &self.map_light, size, seconds, effects.light.as_ref())));
        queue.write_buffer(&self.sky_frame.0, 0, bytemuck::bytes_of(&self.frame_uniform(&sky_view, aspect, &self.sky_light, size, seconds, None)));
        self.composite.prepare(queue, view, size);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sky"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &targets.scene, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &targets.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                ..Default::default()
            });
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &self.sky_frame.1, &[]);
            pass.set_bind_group(1, &self.scene_group, &[]);
            pass.draw(0..3, 0..1);
            if let Some(bundle) = &self.sky_bundle {
                pass.execute_bundles(std::iter::once(bundle));
            }
        }
        self.effects_pass(encoder, targets, &self.sky_frame.1, &self.sky_effects, false);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &targets.scene, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &targets.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                ..Default::default()
            });
            pass.execute_bundles(std::iter::once(&self.map_bundle));
            self.throw.draw_scene(&mut pass);
        }
        self.effects_pass(encoder, targets, &self.map_frame.1, &self.map_effects, true);
        self.composite.encode(encoder, &targets.post.hdr_view, &targets.scene_group, if self.composite.any() { effects.accumulated } else { 0 }, view);
        self.post.encode(encoder, &targets.post);
    }

    /// Readies the composite's targets for the clouds at `targets`' size.
    pub fn ready_clouds(&mut self, device: &wgpu::Device, targets: &Targets) {
        self.composite.ready_targets(device, targets.size(), &targets.depth);
    }

    /// The present pass's draw, into a pass on the output.
    pub fn present(&self, pass: &mut wgpu::RenderPass<'_>, targets: &Targets) {
        self.post.present(pass, &targets.post);
    }
}

/// A draw's first index and its format, in the Geometry section bound whole as the index buffer.
fn index_range(d: &Draw) -> (u32, wgpu::IndexFormat) {
    let format = match d.index_format {
        IndexFormat::U16 => wgpu::IndexFormat::Uint16,
        IndexFormat::U32 => wgpu::IndexFormat::Uint32,
    };
    ((d.indices.offset / d.index_format.bytes()) as u32, format)
}

/// One slot per drawn instance, in draw order: a draw's instances are a run of slots.
fn slots(draws: &[&Draw], geometry: &[u8]) -> Vec<Slot> {
    let word = |offset: u64| (offset / 4) as u32;
    let mut slots = Vec::new();
    for d in draws {
        for world in d.world_transforms(geometry) {
            let m = Mat4::from_cols_array(&world);
            let n = glam::Mat3::from_mat4(m).inverse().transpose();
            let normal_matrix = [n.x_axis.x, n.x_axis.y, n.x_axis.z, 0.0, n.y_axis.x, n.y_axis.y, n.y_axis.z, 0.0, n.z_axis.x, n.z_axis.y, n.z_axis.z, 0.0];
            slots.push(Slot {
                world,
                normal_matrix,
                positions: word(d.positions.offset),
                uv0: word(d.uv0.offset),
                uv1: d.uv1.map_or(NONE, |s| word(s.offset)),
                lightmap: d.lightmap.map_or(NONE, |l| word(l.uv.offset)),
                frames: word(d.frames.offset),
                colors: d.colors.map_or(NONE, |s| word(s.offset)),
                paint: d.paint.map_or(NONE, |s| word(s.offset)),
                material: d.material,
            });
        }
    }
    slots
}

fn color_rows(m: &nades_pack::scene::ColorMatrix) -> [[f32; 4]; 3] {
    *m
}

/// A material as `map.wgsl` reads it.
fn gpu_material(m: &Material, scene: &Scene) -> GpuMaterial {
    let tex = |t: Option<u32>| t.unwrap_or(NONE);
    let rgb_normal = |t: Option<u32>| t.is_some_and(|t| scene.textures[t as usize].semantic == Semantic::NormalRgb);
    let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
    let mut g = GpuMaterial {
        tex0: [tex(m.base_color), tex(m.normal), tex(m.occlusion), NONE],
        tex1: [NONE; 4],
        tex2: [NONE; 4],
        info: [0, 1, 0, 0],
        factor: m.base_color_factor,
        tint: [1.0, 1.0, 1.0, 0.0],
        tint2: [1.0, 1.0, 1.0, 0.0],
        uv: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
        adjust: [identity[0], identity[1], identity[2], identity[0], identity[1], identity[2], identity[0], identity[1], identity[2]],
        tint_adjust: [identity[0], identity[1], identity[2], identity[0], identity[1], identity[2], identity[0], identity[1], identity[2]],
        layer_a: [[1.0, 1.0, 1.0, 0.5]; 3],
        layer_b: [[0.01, 1.0, 0.0, 1.0]; 3],
        layer_c: [[1.0, 1.0, 0.0, 0.0]; 3],
        fade: [1.0, 1.0, 0.0, 1.0],
        fresnel: [0.001, 1.0, 0.0, 1.0],
        effects: [0.0, 1.0, 1.0, 1.0],
        masks: [[1.0, 1.0, 0.0, 0.0]; 3],
        facing: [[0.0; 4]; 2],
        facing_range: [0.0, 1.0, 0.0, 1.0],
    };
    let mut flags = 0;
    if rgb_normal(m.normal) {
        flags |= RGB_NORMAL_1;
    }
    if m.double_sided {
        flags |= DOUBLE_SIDED;
    }
    let cutoff = if let AlphaMode::Mask { cutoff } = m.alpha { cutoff } else { 0.0 };
    match &m.shading {
        Shading::Generic => g.info[0] = 0,
        Shading::Environment(e) => {
            g.info[0] = 1;
            g.info[1] = e.layers.len() as u32;
            g.tint = [e.tint[0], e.tint[1], e.tint[2], e.tint_amount];
            if e.new_blending {
                flags |= NEW_BLENDING;
            }
            for (k, l) in e.layers.iter().enumerate() {
                if l.model_tint {
                    flags |= MODEL_TINT_1 << k;
                }
                if l.uv_set == 2 {
                    flags |= UV2_1 << k;
                }
                g.uv[k * 2] = [l.uv[0], l.uv[1], l.uv[2], 0.0];
                g.uv[k * 2 + 1] = [l.uv[3], l.uv[4], l.uv[5], 0.0];
                let (a, t) = (color_rows(&l.adjust), color_rows(&l.tint_adjust));
                for r in 0..3 {
                    g.adjust[k * 3 + r] = a[r];
                    g.tint_adjust[k * 3 + r] = t[r];
                }
                g.layer_a[k] = [l.tint_mask[0], l.tint_mask[1], l.height_scale, l.height_zero];
                g.layer_b[k] = [l.softness, l.underlying, l.mask_with_height, l.normal_contrast];
                g.layer_c[k] = [l.roughness[0], l.roughness[1], if l.metalness { 1.0 } else { 0.0 }, f32::from(l.vertex_color)];
                if let Some(f) = l.facing.filter(|_| k > 0) {
                    g.facing[k - 1] = [f.direction[0], f.direction[1], f.direction[2], 1.0];
                    g.facing_range[(k - 1) * 2] = f.min;
                    g.facing_range[(k - 1) * 2 + 1] = f.max;
                }
            }
            g.tex0[3] = tex(e.layers[0].height);
            if let Some(l) = e.layers.get(1) {
                g.tex1[0] = tex(l.color);
                g.tex1[1] = tex(l.normal);
                g.tex1[2] = tex(l.height);
            }
            if let Some(l) = e.layers.get(2) {
                g.tex1[3] = tex(l.color);
                g.tex2[0] = tex(l.normal);
                g.tex2[1] = tex(l.height);
            }
        }
        Shading::Layered(l) => {
            g.info[0] = 2;
            g.tex2[2] = tex(l.color2);
            g.tex2[3] = tex(l.modulation);
            g.tint = [l.tint1[0], l.tint1[1], l.tint1[2], l.softness];
            g.tint2 = [l.tint2[0], l.tint2[1], l.tint2[2], f32::from(l.fancy)];
            g.uv[2] = [l.uv2_scale[0], l.uv2_scale[1], 0.0, 0.0];
        }
        Shading::Effects(e) => {
            g.info[0] = 3;
            if e.additive {
                flags |= ADDITIVE;
            }
            for (k, mask) in e.masks.iter().enumerate() {
                if let Some(mask) = mask {
                    flags |= MASK_1 << k;
                    g.masks[k] = *mask;
                }
            }
            g.fade = e.fade;
            g.fresnel = e.fresnel;
            g.effects = [e.feather[0], e.feather[1], e.color_boost, e.opacity];
        }
        Shading::Unlit { additive } => {
            g.info[0] = 4;
            if *additive {
                flags |= ADDITIVE;
            }
        }
        Shading::Black => g.info[0] = 5,
    }
    g.info[2] = flags;
    g.info[3] = cutoff.to_bits();
    g
}

/// map.wgsl's `debug_mode` from `NADES_DEBUG`: what a frame shows instead of the picture, for debugging.
fn debug_mode() -> f64 {
    match std::env::var("NADES_DEBUG").as_deref() {
        Ok("unlit") => 1.0,
        Ok("indirect") => 2.0,
        Ok("visibility") => 3.0,
        Ok("albedo") => 4.0,
        Ok("irradiance") => 5.0,
        Ok("direction") => 6.0,
        Ok("normal") => 7.0,
        _ => 0.0,
    }
}

fn pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule, k: Key) -> wgpu::RenderPipeline {
    let alpha = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
    let over = wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add }, alpha };
    let add = wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add }, alpha };
    let (fragment, blend, writes) = match k.stage {
        Stage::Opaque => ("opaque", None, true),
        Stage::Mask => ("mask", None, true),
        Stage::Blend => ("blend", Some(over), false),
        Stage::Additive => ("blend", Some(add), false),
        Stage::Effects => ("effects", Some(over), false),
        Stage::EffectsAdditive => ("effects", Some(add), false),
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment),
        layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[] },
        primitive: wgpu::PrimitiveState { cull_mode: if k.double_sided { None } else { Some(wgpu::Face::Back) }, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(writes),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: wgpu::PipelineCompilationOptions { constants: &[("debug_mode", debug_mode())], ..Default::default() },
            targets: &[Some(wgpu::ColorTargetState { format: HDR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn sky_pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky"),
        layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some("sky_vertex"), compilation_options: Default::default(), buffers: &[] },
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some("sky"), compilation_options: Default::default(), targets: &[Some(HDR_FORMAT.into())] }),
        multiview_mask: None,
        cache: None,
    })
}

/// A texture as the pack describes it, empty: its format, size, layers and mip chain.
pub fn create_texture(device: &wgpu::Device, t: &Texture) -> wgpu::Texture {
    use wgpu::TextureFormat as F;
    let format = match (t.format, t.srgb) {
        (TextureFormat::Bc1, false) => F::Bc1RgbaUnorm,
        (TextureFormat::Bc1, true) => F::Bc1RgbaUnormSrgb,
        (TextureFormat::Bc3, false) => F::Bc3RgbaUnorm,
        (TextureFormat::Bc3, true) => F::Bc3RgbaUnormSrgb,
        (TextureFormat::Bc4, _) => F::Bc4RUnorm,
        (TextureFormat::Bc5, _) => F::Bc5RgUnorm,
        (TextureFormat::Bc6h, _) => F::Bc6hRgbUfloat,
        (TextureFormat::Bc7, false) => F::Bc7RgbaUnorm,
        (TextureFormat::Bc7, true) => F::Bc7RgbaUnormSrgb,
        (TextureFormat::Rgba8, false) => F::Rgba8Unorm,
        (TextureFormat::Rgba8, true) => F::Rgba8UnormSrgb,
    };
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&t.name),
        size: wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: t.layers },
        mip_level_count: t.mips.len() as u32,
        sample_count: 1,
        dimension: if t.volume { wgpu::TextureDimension::D3 } else { wgpu::TextureDimension::D2 },
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// A soft, tiling value noise (mean about 0.5), the stand-in for the effects' masks the export leaves out
/// (show-me-the-nades' csgoEffects.ts `noise`).
fn noise_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    const SIZE: usize = 64;
    const CELLS: usize = 8;
    let mut seed: u64 = 7;
    let mut random = || {
        seed = (seed * 16807) % 2_147_483_647;
        (seed as f32 - 1.0) / 2_147_483_646.0
    };
    let lattice: Vec<f32> = (0..(CELLS + 1) * (CELLS + 1)).map(|_| random()).collect();
    let at = |x: usize, y: usize| lattice[(y % CELLS) * (CELLS + 1) + (x % CELLS)];
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let mut data = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (fx, fy) = (x as f32 / SIZE as f32 * CELLS as f32, y as f32 / SIZE as f32 * CELLS as f32);
            let (ix, iy) = (fx.floor() as usize, fy.floor() as usize);
            let (tx, ty) = (smooth(fx - ix as f32), smooth(fy - iy as f32));
            let top = at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx;
            let bottom = at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx;
            let v = ((top * (1.0 - ty) + bottom * ty) * 255.0).round() as u8;
            data[(y * SIZE + x) * 4..(y * SIZE + x) * 4 + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor { label: Some("mask noise"), size: wgpu::Extent3d { width: SIZE as u32, height: SIZE as u32, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        )
        .create_view(&Default::default())
}

/// Staging the Textures section may hold in flight where the GPU cannot read the pack's own pages.
const STAGING: u64 = 256 << 20;

/// Copies every texture's mips from the pack's Textures section into `textures`.
///
/// - On Metal the GPU reads the mapping itself: one buffer over the whole section, one submission. The pack's pages are
///   the staging, and macOS charges them to the file's cache, not to the process (S20).
/// - Elsewhere the section goes through staging buffers a chunk (≈32 MiB) at a time, at most [`STAGING`] bytes of them in
///   flight, so an upload never holds a copy of a whole map's textures.
fn upload_textures(device: &wgpu::Device, queue: &wgpu::Queue, pack: &'static Pack, scene: &Scene, textures: &[wgpu::Texture]) {
    let copy = |encoder: &mut wgpu::CommandEncoder, buffer: &wgpu::Buffer, base: u64, t: &Texture, texture: &wgpu::Texture| {
        for (level, mip) in t.mips.iter().enumerate() {
            let (width, height) = mip.copy_size(t.format);
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo { buffer, layout: wgpu::TexelCopyBufferLayout { offset: base + mip.offset, bytes_per_row: Some(mip.bytes_per_row), rows_per_image: Some(mip.rows) } },
                wgpu::TexelCopyTextureInfo { texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::Extent3d { width, height, depth_or_array_layers: mip.layers },
            );
        }
    };
    #[cfg(target_os = "macos")]
    if let Some(texels) = metal::no_copy(device, "textures", pack.textures(), wgpu::BufferUsages::COPY_SRC) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("textures") });
        for (t, texture) in scene.textures.iter().zip(textures) {
            copy(&mut encoder, &texels, scene.chunks[t.chunk as usize].offset, t, texture);
        }
        queue.submit([encoder.finish()]);
        return;
    }
    let section = pack.textures();
    let mut in_flight: std::collections::VecDeque<(wgpu::SubmissionIndex, u64)> = Default::default();
    let mut held = 0;
    for (k, chunk) in scene.chunks.iter().enumerate() {
        let staging = buffer_copy(device, "textures chunk", &section[chunk.range()], wgpu::BufferUsages::COPY_SRC);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("textures") });
        for (t, texture) in scene.textures.iter().zip(textures).filter(|(t, _)| t.chunk as usize == k) {
            copy(&mut encoder, &staging, 0, t, texture);
        }
        let size = staging.size();
        in_flight.push_back((queue.submit([encoder.finish()]), size));
        held += size;
        // the oldest chunks' staging is let go once the GPU has copied out of it
        while held > STAGING
            && let Some((index, size)) = in_flight.pop_front()
        {
            let _ = device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None });
            held -= size;
        }
    }
}

/// A texture's bytes as the GPU stores them: whole blocks, every mip and layer, no row padding.
fn texture_bytes(t: &Texture) -> u64 {
    let block = t.format.block_dim();
    t.mips.iter().map(|m| u64::from(m.width.div_ceil(block)) * u64::from(m.height.div_ceil(block)) * u64::from(t.format.block_bytes()) * u64::from(m.layers)).sum()
}

/// What the GPU allocated for `texture` (Metal says; 4–15% above its blocks), where the backend can say.
fn allocated_bytes(texture: &wgpu::Texture) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        use objc2_metal::MTLAllocation as _;
        // SAFETY: only the texture's allocated size is read
        unsafe { texture.as_hal::<wgpu::hal::api::Metal>().map(|raw| raw.raw_handle().allocatedSize() as u64) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = texture;
        None
    }
}

/// A GPU buffer holding a copy of `bytes`, written through a mapping at creation.
fn buffer_copy(device: &wgpu::Device, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    let size = (bytes.len() as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT).max(wgpu::COPY_BUFFER_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: true });
    buffer.get_mapped_range_mut(..bytes.len() as u64).expect("mapped at creation").copy_from_slice(bytes);
    buffer.unmap();
    buffer
}

/// A GPU buffer holding `bytes`: on Metal the bytes' own pages, elsewhere (or where Metal declines) a copy.
pub fn buffer_over(device: &wgpu::Device, label: &str, bytes: &'static [u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    #[cfg(target_os = "macos")]
    if let Some(buffer) = metal::no_copy(device, label, bytes, usage) {
        return buffer;
    }
    buffer_copy(device, label, bytes, usage)
}

#[cfg(target_os = "macos")]
mod metal {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2_metal::{MTLDevice as _, MTLResourceOptions};
    use wgpu::hal::api::Metal;

    /// Apple silicon's page size, which the pack aligns its sections to.
    const PAGE: usize = 16 * 1024;

    /// A Metal buffer whose storage is `bytes` themselves (no copy), when they start on a page and Metal accepts them.
    pub fn no_copy(device: &wgpu::Device, label: &str, bytes: &'static [u8], usage: wgpu::BufferUsages) -> Option<wgpu::Buffer> {
        let pointer = NonNull::new(bytes.as_ptr().cast_mut().cast::<c_void>())?;
        if !(pointer.as_ptr() as usize).is_multiple_of(PAGE) || bytes.is_empty() {
            return None;
        }
        // whole pages: the mapping covers the last page too
        let length = bytes.len().next_multiple_of(PAGE);
        // SAFETY: the pages belong to a read-only mapping that lives as long as the process (`bytes` is 'static) and
        // that nothing writes; the GPU only reads them (vertex, index, storage and copy-source use)
        unsafe {
            let raw = device.as_hal::<Metal>()?.raw_device().newBufferWithBytesNoCopy_length_options_deallocator(pointer, length, MTLResourceOptions::StorageModeShared, None)?;
            let buffer = wgpu::hal::metal::Device::buffer_from_raw(raw, length as u64);
            Some(device.create_buffer_from_hal::<Metal>(buffer, &wgpu::BufferDescriptor { label: Some(label), size: length as u64, usage, mapped_at_creation: false }))
        }
    }
}

fn storage(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None }
}

fn texture_type(view_dimension: wgpu::TextureViewDimension) -> wgpu::BindingType {
    wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension, multisampled: false }
}

fn sampler(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }
}

#[cfg(test)]
mod tests {
    use nades_pack::scene::{Bounds, Instances, LightmapUv, Span, UvSet};

    use super::*;

    fn translation(x: f32) -> [f32; 16] {
        glam::Mat4::from_translation(glam::Vec3::new(x, 0.0, 0.0)).to_cols_array()
    }

    /// A draw's instances become a run of slots, each with `model × instance`, its normal matrix, its streams as word
    /// offsets (absent ones marked) and its material.
    #[test]
    fn every_drawn_instance_gets_a_slot_in_draw_order() {
        let mut geometry = vec![0u8; 1024];
        let instances: Vec<u8> = [translation(10.0), translation(20.0)].iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        geometry[768..768 + 128].copy_from_slice(&instances);
        let span = |offset| Span { offset, len: 24 };
        let mut scaled = glam::Mat4::from_scale(glam::Vec3::new(2.0, 1.0, 1.0)).to_cols_array();
        scaled[12] = 1.0;
        let lit = Draw {
            name: "wall".into(),
            model: scaled,
            vertex_count: 3,
            positions: span(0),
            uv0: span(256),
            uv1: None,
            lightmap: Some(LightmapUv { uv: span(512), from: UvSet::Texcoord2 }),
            frames: span(0),
            colors: Some(span(256)),
            paint: None,
            indices: Span { offset: 512, len: 6 },
            index_format: IndexFormat::U16,
            index_count: 3,
            instances: None,
            material: 0,
            bounds: Bounds { min: [0.0; 3], max: [1.0; 3] },
        };
        let leaves = Draw { name: "leaves".into(), model: translation(1.0), lightmap: None, colors: None, instances: Some(Instances { count: 2, transforms: Span { offset: 768, len: 128 } }), material: 1, ..lit.clone() };
        let slots = slots(&[&lit, &leaves], &geometry);
        assert_eq!(slots.len(), 3);
        assert_eq!((slots[0].positions, slots[0].uv0, slots[0].lightmap, slots[0].colors, slots[0].uv1, slots[0].paint), (0, 64, 128, 64, NONE, NONE));
        // a normal scaled by the inverse transpose: x halves
        assert_eq!(slots[0].normal_matrix[0], 0.5);
        assert_eq!((slots[1].lightmap, slots[1].material), (NONE, 1));
        assert_eq!([slots[1].world, slots[2].world], [translation(11.0), translation(21.0)]);
        assert_eq!(std::mem::size_of::<Slot>(), 144, "map.wgsl's Slot is 144 bytes");
        assert_eq!(std::mem::size_of::<GpuMaterial>(), 784, "map.wgsl's Material is 784 bytes");
        assert_eq!(std::mem::size_of::<FrameUniform>(), 416, "map.wgsl's Frame is 416 bytes");
        assert_eq!(std::mem::size_of::<GpuProbeVolume>(), 160, "map.wgsl's ProbeVolume is 160 bytes");
    }

    #[test]
    fn fog_moves_into_the_skyboxs_space() {
        let gradient = GradientFog { start: 10.0, end: 110.0, falloff: 1.5, height: true, height_start: 0.0, height_end: 100.0, vertical_exponent: 1.0, color: [0.5, 0.5, 0.5], max_opacity: 1.0 };
        let world = fog_values(Some(&gradient), None, WORLD_SPACE, 1024);
        assert!(world.gradient.iter().zip([-0.1, 1.0, 0.01, -0.01]).all(|(a, b)| (a - b).abs() < 1e-6), "{:?}", world.gradient);
        assert_eq!(world.cube_color[1], 0.0, "no cubemap fog");
        // a skybox at 1/16 sees the same fog at a sixteenth of the distance
        let sky = fog_values(Some(&gradient), None, FogSpace { distance: 1.0 / 16.0, height: 0.0 }, 1024);
        assert!((sky.gradient[2] - 16.0 * 0.01).abs() < 1e-6);
    }
}
