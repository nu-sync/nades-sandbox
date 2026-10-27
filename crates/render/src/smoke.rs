//! The composite pass (`smoke.wgsl`): the scene's colour into the HDR target the post-processing reads, with the
//! scene's smokes raymarched over it against the scene's depth, as show-me-the-nades' web view draws it
//! (`render/composite.ts`). It always runs; without a smoke it is a copy.
//!
//! Each cloud is a 32³ RGBA volume ([`nades_scene::smoke::smoke_volume`]) with its own values ([`Composite::set_clouds`]);
//! the pass runs once per cloud, the farthest first, through two targets of the frame's size, the last into the frame's
//! own. The noise is the game's 128³ texture, or the stand-in, put on the GPU once ([`noise_texture`]) and shared by
//! every map. A still view averages jittered frames ([`SmokeFrame::accumulated`]), so it converges to a clean image as
//! the web view's does.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use nades_scene::smoke::{CELLS, Cloud, GRID};

use crate::post::HDR_FORMAT;
use crate::throw::{source_to_scene, to_source_dir};
use crate::view::{INCH, View};

/// Jittered frames a still view averages before its image is final (the web view's ACCUMULATE).
pub const ACCUMULATE: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct SmokeUniform {
    inverse_view_proj: [f32; 16],
    eye: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    back: [f32; 4],
    origin: [f32; 4],
    box_min: [f32; 4],
    box_max: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    sky_color: [f32; 4],
    screen: [f32; 4],
}

/// What one frame's composite draws of the smoke.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmokeFrame {
    /// Seconds after detonation.
    pub seconds: f32,
    /// 1 while the cloud holds, falling to 0 as it fades.
    pub fade: f32,
    /// Jittered frames averaged into the target so far: 0 starts the picture over.
    pub accumulated: u32,
}

/// The light the smoke is drawn in: towards the sun (Hammer units' axes), the sun's colour and the sky's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmokeLight {
    pub toward_sun: Vec3,
    pub sun_color: Vec3,
    pub sky_color: Vec3,
}

impl SmokeLight {
    /// The web view's light for a map: its sun (direction raised to at least 0.25 up, colour × brightness × 1.6), or
    /// its stand-in sun; and its sky's fill.
    pub fn of(sun: Option<&nades_pack::scene::Sun>) -> Self {
        let sky_color = Vec3::new(0.62, 0.68, 0.76) * 0.9;
        match sun {
            Some(s) => {
                let mut d = to_source_dir(Vec3::from_array(s.direction)).normalize();
                if d.z < 0.25 {
                    d.z = 0.25;
                    d = d.normalize();
                }
                // the pack's colour is already colour × brightness
                Self { toward_sun: d, sun_color: Vec3::from_array(s.color) * 1.6, sky_color }
            }
            None => Self { toward_sun: Vec3::new(0.45, 0.3, 0.84).normalize(), sun_color: Vec3::new(1.0, 0.95, 0.86) * 3.2, sky_color },
        }
    }
}

pub struct Composite {
    pipeline: wgpu::RenderPipeline,
    smoke_layout: wgpu::BindGroupLayout,
    scene_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    volume: wgpu::Texture,
    noise: Option<wgpu::Texture>,
    clamped: wgpu::Sampler,
    repeated: wgpu::Sampler,
    group: wgpu::BindGroup,
    pub light: SmokeLight,
    /// The clouds shown.
    slots: Vec<Slot>,
    /// The two targets the clouds' passes go through, for one frame size.
    pong: Option<Pong>,
}

/// One cloud on the GPU.
struct Slot {
    key: u64,
    uniform: wgpu::Buffer,
    volume: wgpu::Texture,
    group: wgpu::BindGroup,
    cloud: (Vec3, Vec3, Vec3),
    frame: Option<SmokeFrame>,
}

struct Pong {
    size: (u32, u32),
    views: [wgpu::TextureView; 2],
    groups: [wgpu::BindGroup; 2],
}

/// A cloud's grid corner and box, in Hammer units.
fn placement(cloud: &Cloud) -> (Vec3, Vec3, Vec3) {
    let (lo, hi) = cloud.bounding_box();
    let f = |v: glam::DVec3| Vec3::new(v.x as f32, v.y as f32, v.z as f32);
    (f(cloud.origin), f(lo), f(hi))
}

fn volume_texture(device: &wgpu::Device, label: &str, size: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: size },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// The smoke's noise (a cube of `texels`, one channel each: 128³ for the game's) on the GPU.
pub fn noise_texture(device: &wgpu::Device, queue: &wgpu::Queue, texels: &[u8]) -> wgpu::Texture {
    let size = (texels.len() as f64).cbrt().round() as u32;
    let noise = volume_texture(device, "smoke noise", size, wgpu::TextureFormat::R8Unorm);
    queue.write_texture(
        noise.as_image_copy(),
        texels,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size), rows_per_image: Some(size) },
        wgpu::Extent3d { width: size, height: size, depth_or_array_layers: size },
    );
    noise
}

impl Composite {
    pub fn new(device: &wgpu::Device, light: SmokeLight) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("smoke"), source: wgpu::ShaderSource::Wgsl(include_str!("smoke.wgsl").into()) });
        let fragment = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None };
        let volume_type = wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D3, multisampled: false };
        let smoke_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("smoke"),
            entries: &[
                fragment(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(std::mem::size_of::<SmokeUniform>() as u64) }),
                fragment(1, volume_type),
                fragment(2, volume_type),
                fragment(3, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                fragment(4, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("smoke scene"),
            entries: &[
                fragment(0, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
                fragment(1, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("smoke"), bind_group_layouts: &[Some(&smoke_layout), Some(&scene_layout)], immediate_size: 0 });
        // the running average: the new frame by the blend constant, what is there by one less it
        let average = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Constant, dst_factor: wgpu::BlendFactor::OneMinusConstant, operation: wgpu::BlendOperation::Add };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("fullscreen"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: HDR_FORMAT, blend: Some(wgpu::BlendState { color: average, alpha: average }), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("smoke"), size: std::mem::size_of::<SmokeUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let volume = volume_texture(device, "smoke volume", GRID as u32, wgpu::TextureFormat::Rgba8Unorm);
        let placeholder_noise = volume_texture(device, "no smoke noise", 1, wgpu::TextureFormat::R8Unorm);
        let clamped = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("smoke volume"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let repeated = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("smoke noise"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let group = Self::bind(device, &smoke_layout, &uniform, &volume, &placeholder_noise, &clamped, &repeated);
        Self { pipeline, smoke_layout, scene_layout, uniform, volume, noise: None, clamped, repeated, group, light, slots: Vec::new(), pong: None }
    }

    /// Shows these clouds, each `(key, cloud, volume, frame)`: a key seen before keeps its volume on the GPU.
    pub fn set_clouds(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, clouds: &[(u64, &Cloud, &[u8], Option<SmokeFrame>)]) {
        let mut old: Vec<Slot> = std::mem::take(&mut self.slots);
        for &(key, cloud, volume, frame) in clouds.iter().filter(|(_, _, v, _)| v.len() == CELLS * 4) {
            let slot = match old.iter().position(|s| s.key == key) {
                Some(i) => old.swap_remove(i),
                None => {
                    // a slot of a cloud gone is filled again, else a new one is made
                    let mut slot = old.pop().unwrap_or_else(|| {
                        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("smoke"), size: std::mem::size_of::<SmokeUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                        let volume = volume_texture(device, "smoke volume", GRID as u32, wgpu::TextureFormat::Rgba8Unorm);
                        let noise = self.noise.clone().unwrap_or_else(|| volume_texture(device, "no smoke noise", 1, wgpu::TextureFormat::R8Unorm));
                        let group = Self::bind(device, &self.smoke_layout, &uniform, &volume, &noise, &self.clamped, &self.repeated);
                        Slot { key, uniform, volume, group, cloud: placement(cloud), frame }
                    });
                    queue.write_texture(
                        slot.volume.as_image_copy(),
                        volume,
                        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(GRID as u32 * 4), rows_per_image: Some(GRID as u32) },
                        wgpu::Extent3d { width: GRID as u32, height: GRID as u32, depth_or_array_layers: GRID as u32 },
                    );
                    slot.key = key;
                    slot
                }
            };
            self.slots.push(Slot { cloud: placement(cloud), frame, ..slot });
        }
    }

    /// Whether a cloud shows this frame.
    pub fn any(&self) -> bool {
        self.slots.iter().any(|s| s.frame.is_some())
    }

    /// The two targets the clouds' passes go through, for frames of `size` over `depth`.
    pub fn ready_targets(&mut self, device: &wgpu::Device, size: (u32, u32), depth: &wgpu::TextureView) {
        if self.pong.as_ref().is_some_and(|p| p.size == size) {
            return;
        }
        let view = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: HDR_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let views = [view("smoke pass a"), view("smoke pass b")];
        let groups = [self.scene_group(device, &views[0], depth), self.scene_group(device, &views[1], depth)];
        self.pong = Some(Pong { size, views, groups });
    }

    fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniform: &wgpu::Buffer, volume: &wgpu::Texture, noise: &wgpu::Texture, clamped: &wgpu::Sampler, repeated: &wgpu::Sampler) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smoke"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&volume.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&noise.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(clamped) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(repeated) },
            ],
        })
    }

    /// The per-size bind group: the scene's colour and depth.
    pub fn scene_group(&self, device: &wgpu::Device, color: &wgpu::TextureView, depth: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smoke scene"),
            layout: &self.scene_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(color) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) }],
        })
    }

    /// Whether the smoke's noise is on the GPU.
    pub fn has_noise(&self) -> bool {
        self.noise.is_some()
    }

    /// Raymarches the smoke through `noise` ([`noise_texture`], made once and shared by every map's composite).
    pub fn set_noise(&mut self, device: &wgpu::Device, noise: &wgpu::Texture) {
        self.group = Self::bind(device, &self.smoke_layout, &self.uniform, &self.volume, noise, &self.clamped, &self.repeated);
        for slot in &mut self.slots {
            slot.group = Self::bind(device, &self.smoke_layout, &slot.uniform, &slot.volume, noise, &self.clamped, &self.repeated);
        }
        self.noise = Some(noise.clone());
    }

    /// Writes this frame's values: each cloud's own, from `view` into frames of `size`.
    pub fn prepare(&self, queue: &wgpu::Queue, view: &View, size: (u32, u32)) {
        self.write(queue, &self.uniform, view, size, None, None);
        for slot in &self.slots {
            self.write(queue, &slot.uniform, view, size, Some(slot.cloud), slot.frame);
        }
    }

    fn write(&self, queue: &wgpu::Queue, buffer: &wgpu::Buffer, view: &View, size: (u32, u32), cloud: Option<(Vec3, Vec3, Vec3)>, frame: Option<SmokeFrame>) {
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let view_proj = view.view_proj(aspect) * source_to_scene();
        let axis = |v: Vec3| {
            let d = to_source_dir(view.rotation * v);
            [d.x, d.y, d.z, 0.0]
        };
        let eye = to_source_dir(view.eye) / INCH;
        let (origin, lo, hi) = cloud.unwrap_or((Vec3::ZERO, Vec3::ZERO, Vec3::ZERO));
        let on = frame.is_some() && cloud.is_some() && self.noise.is_some();
        let f = frame.unwrap_or(SmokeFrame { seconds: 0.0, fade: 0.0, accumulated: 0 });
        let l = &self.light;
        let uniform = SmokeUniform {
            inverse_view_proj: view_proj.inverse().to_cols_array(),
            eye: [eye.x, eye.y, eye.z, if on { 1.0 } else { 0.0 }],
            right: axis(Vec3::X),
            up: axis(Vec3::Y),
            back: axis(Vec3::Z),
            origin: [origin.x, origin.y, origin.z, GRID as f32 * 20.0],
            box_min: [lo.x, lo.y, lo.z, f.seconds],
            box_max: [hi.x, hi.y, hi.z, f.fade],
            sun_dir: [l.toward_sun.x, l.toward_sun.y, l.toward_sun.z, f.accumulated as f32],
            sun_color: [l.sun_color.x, l.sun_color.y, l.sun_color.z, 0.0],
            sky_color: [l.sky_color.x, l.sky_color.y, l.sky_color.z, 0.0],
            screen: [size.0 as f32, size.1 as f32, 0.0, 0.0],
        };
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
    }

    /// The pass: `scene` (colour and depth) into `target`, averaged in by `1 / (accumulated + 1)`. The clouds take one
    /// pass each, the farthest first, through the two targets of [`Composite::ready_targets`].
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, scene: &wgpu::BindGroup, accumulated: u32, view: &View) {
        let mut shown: Vec<&Slot> = self.slots.iter().filter(|s| s.frame.is_some()).collect();
        let Some(pong) = self.pong.as_ref().filter(|_| !shown.is_empty()) else {
            return self.pass(encoder, target, &self.group, scene, accumulated, true);
        };
        let eye = to_source_dir(view.eye) / INCH;
        let distance = |s: &Slot| ((s.cloud.1 + s.cloud.2) * 0.5).distance(eye);
        shown.sort_by(|a, b| distance(b).total_cmp(&distance(a)));
        let mut from = scene;
        for (i, slot) in shown.iter().enumerate() {
            let last = i + 1 == shown.len();
            let to = if last { target } else { &pong.views[i % 2] };
            self.pass(encoder, to, &slot.group, from, accumulated, last);
            from = &pong.groups[i % 2];
        }
    }

    /// One composite pass: `scene` through `smoke`'s cloud into `target`, averaged in when `average`, else written.
    fn pass(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, smoke: &wgpu::BindGroup, scene: &wgpu::BindGroup, accumulated: u32, average: bool) {
        let accumulated = if average { accumulated } else { 0 };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: if accumulated == 0 { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load }, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        let weight = 1.0 / (f64::from(accumulated) + 1.0);
        pass.set_blend_constant(wgpu::Color { r: weight, g: weight, b: weight, a: weight });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, smoke, &[]);
        pass.set_bind_group(1, scene, &[]);
        pass.draw(0..3, 0..1);
    }
}
