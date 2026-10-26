//! After the scene: the exposure the frame adapts to (a GPU reduction, never read back), the map's bloom (four
//! levels) and the present pass with its tone curve and colour lookup table (`post.wgsl`). The parameters come from the
//! pack's [`Post`] as show-me-the-nades' web view sets them (render/composite.ts `setLook`, `setExposure`, `bloomOf`).

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use nades_pack::scene::{BloomMode, Post, Tonemap};

/// The scene's colour: half floats, the map's light before exposure.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const LEVELS: usize = 4;
/// The post-processing's blur weights (Source 2 Viewer's BloomRenderer); the first combine takes the last two.
const BLUR_WEIGHTS: [f32; 5] = [0.151_096, 0.151_096, 0.151_096, 0.151_096, 0.395_616];
/// Each pass's parameters sit at a multiple of this in one buffer.
const STRIDE: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct Params {
    texel: [f32; 4],
    a: [f32; 4],
    b: [f32; 4],
    bloom: [f32; 4],
    curve: [f32; 4],
    curve2: [f32; 4],
    lut: [f32; 4],
    pad: [f32; 4],
}

/// The map's post-processing, resolved into the present and exposure passes' numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// Exposure: the scene value the curve shows as middle grey (0 when the map does not adapt), the bounds.
    pub adapt: [f32; 3],
    /// The map's exposure scale: 2^(bias + compensation), times the fixed exposure of a map that does not adapt.
    pub exposure: f32,
    pub tonemap: Tonemap,
    /// Bloom: strength, threshold, threshold width, when the map has any.
    pub bloom: Option<[f32; 3]>,
    /// Add, screen and blur strengths normalized together, and blur's own.
    pub bloom_mix: [f32; 4],
    pub lut_weight: f32,
}

fn curve_at(t: &Tonemap, x: f32) -> f32 {
    (x * (t.shoulder_strength * x + t.linear_strength * t.linear_angle) + t.toe_num * t.toe_strength) / (x * (t.shoulder_strength * x + t.linear_strength) + t.toe_denom * t.toe_strength) - t.toe_num / t.toe_denom
}

/// The scene value (before the × 2.8) that a tone curve displays as `display` (Source 2 Viewer's InvertTonemapping).
pub fn scene_value_for(t: &Tonemap, display: f32) -> Option<f32> {
    let white = 2f32.powf(t.white_point);
    let g = display * curve_at(t, white) + t.toe_num / t.toe_denom;
    let a = t.shoulder_strength * (1.0 - g);
    let b = t.linear_strength * (t.linear_angle - g);
    let c = t.toe_strength * (t.toe_num - g * t.toe_denom);
    let x = if a == 0.0 {
        -c / b
    } else {
        let d = b * b - 4.0 * a * c;
        if d < 0.0 {
            return None;
        }
        let x = (-b + d.sqrt()) / (2.0 * a);
        if x <= 0.0 { (-b - d.sqrt()) / (2.0 * a) } else { x }
    };
    (x.is_finite() && x > 0.0).then_some(x / 2.8)
}

impl Look {
    pub fn of(post: &Post) -> Self {
        let tonemap = post.tonemap.unwrap_or(Tonemap::DEFAULT);
        let exposure = post.exposure;
        let adapts = exposure.is_some_and(|e| e.enabled && e.min < e.max);
        // bounds that leave nothing to choose from (min at or above max) give the minimum, whatever the frame
        let fixed = match exposure {
            Some(e) if e.enabled && !adapts => e.min,
            _ => 1.0,
        };
        let adapt = match exposure {
            Some(e) if adapts => [scene_value_for(&tonemap, 0.18).unwrap_or(0.0), e.min, e.max],
            _ => [0.0, 1.0, 1.0],
        };
        let bloom = post.bloom.and_then(|b| {
            let strength = match b.mode {
                BloomMode::Screen => b.screen,
                BloomMode::Blur => b.blur,
                BloomMode::Add => b.strength,
            } * b.opacity;
            (strength > 0.0 && b.threshold_width > 0.0).then_some([strength, b.threshold, b.threshold_width])
        });
        let bloom_mix = match post.bloom {
            Some(b) if bloom.is_some() => {
                let s = [b.strength * b.opacity, b.screen * b.opacity, b.blur * b.opacity];
                let length = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
                let n = if length > 0.0 { s.map(|x| x / length) } else { s };
                [n[0], n[1], n[2], s[2]]
            }
            _ => [0.0; 4],
        };
        Self {
            adapt,
            exposure: fixed * 2f32.powf(tonemap.exposure_bias + exposure.map_or(0.0, |e| e.compensation)),
            tonemap,
            bloom,
            bloom_mix,
            lut_weight: post.lut.as_ref().map_or(0.0, |l| l.weight),
        }
    }
}

fn texture(device: &wgpu::Device, label: &str, (width, height): (u32, u32), format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// The post-processing pipelines and the map's settings; [`Targets`] holds what depends on the frame's size.
pub struct Effects {
    layout: wgpu::BindGroupLayout,
    luminance: wgpu::RenderPipeline,
    reduce: wgpu::RenderPipeline,
    threshold: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    copy: wgpu::RenderPipeline,
    combine: wgpu::RenderPipeline,
    present: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    lut: wgpu::TextureView,
    black: wgpu::TextureView,
    pub look: Look,
    lut_dim: u32,
}

/// A pass's inputs and where its parameters are.
struct Step {
    group: wgpu::BindGroup,
    offset: u32,
}

/// The targets and bind groups for one frame size.
pub struct Targets {
    pub size: (u32, u32),
    pub hdr: wgpu::Texture,
    pub hdr_view: wgpu::TextureView,
    luminance: wgpu::TextureView,
    exposure: wgpu::TextureView,
    levels: Vec<(wgpu::TextureView, (u32, u32))>,
    scratch: Vec<wgpu::TextureView>,
    combined: Vec<wgpu::TextureView>,
    luminance_step: Step,
    reduce_step: Step,
    threshold_step: Option<Step>,
    /// per level: copy from the one above (none for level 0), blur across, blur down
    level_steps: Vec<(Option<Step>, Step, Step)>,
    combine_steps: Vec<Step>,
    present_step: Step,
}

impl Effects {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, post: &Post, output: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("post"), source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()) });
        let filterable = wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false };
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: NonZeroU64::new(std::mem::size_of::<Params>() as u64) }),
                entry(1, filterable),
                entry(2, filterable),
                entry(3, filterable),
                entry(4, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(5, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D3, multisampled: false }),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("post"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = |entry: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("fullscreen"), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(format.into())] }),
                multiview_mask: None,
                cache: None,
            })
        };
        let r16 = wgpu::TextureFormat::R16Float;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("post"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let (lut, lut_dim) = match &post.lut {
            Some(l) => (lut_texture(device, queue, l.dim, &l.rgba), l.dim),
            None => (lut_texture(device, queue, 2, &[255; 32]), 2),
        };
        let black = texture(device, "no bloom", (1, 1), HDR_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING).create_view(&Default::default());
        Self {
            luminance: pipeline("luminance", r16),
            reduce: pipeline("reduce", r16),
            threshold: pipeline("threshold", HDR_FORMAT),
            blur: pipeline("blur", HDR_FORMAT),
            copy: pipeline("copy", HDR_FORMAT),
            combine: pipeline("combine", HDR_FORMAT),
            present: pipeline("present", output),
            layout,
            sampler,
            lut,
            black,
            look: Look::of(post),
            lut_dim,
        }
    }

    fn step(&self, device: &wgpu::Device, params: &wgpu::Buffer, offset: u32, input: &wgpu::TextureView, lower: &wgpu::TextureView, exposure: &wgpu::TextureView) -> Step {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: params, offset: 0, size: NonZeroU64::new(std::mem::size_of::<Params>() as u64) }) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(input) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(lower) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(exposure) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&self.lut) },
            ],
        });
        Step { group, offset }
    }

    /// The targets for frames of `size` pixels, with every pass's parameters written.
    pub fn targets(&self, device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32)) -> Targets {
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let hdr = texture(device, "hdr", size, HDR_FORMAT, attachment);
        let hdr_view = hdr.create_view(&Default::default());
        let luminance = texture(device, "luminance", (64, 36), wgpu::TextureFormat::R16Float, attachment).create_view(&Default::default());
        let exposure = texture(device, "exposure", (1, 1), wgpu::TextureFormat::R16Float, attachment).create_view(&Default::default());
        let mut levels = Vec::new();
        let (mut w, mut h) = ((size.0 / 4).max(1), (size.1 / 4).max(1));
        for _ in 0..LEVELS {
            levels.push((texture(device, "bloom", (w, h), HDR_FORMAT, attachment).create_view(&Default::default()), (w, h)));
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
        let scratch: Vec<_> = levels.iter().map(|(_, s)| texture(device, "bloom scratch", *s, HDR_FORMAT, attachment).create_view(&Default::default())).collect();
        let combined: Vec<_> = levels.iter().map(|(_, s)| texture(device, "bloom combined", *s, HDR_FORMAT, attachment).create_view(&Default::default())).collect();

        // every pass's parameters, in one buffer
        let look = &self.look;
        let t = &look.tonemap;
        let white = 2f32.powf(t.white_point);
        let mut params: Vec<Params> = Vec::new();
        let mut push = |p: Params| {
            params.push(p);
            ((params.len() - 1) as u64 * STRIDE) as u32
        };
        let present = Params {
            b: [look.adapt[0], look.adapt[1], look.adapt[2], look.exposure],
            bloom: look.bloom_mix,
            curve: [t.shoulder_strength, t.linear_strength, t.linear_angle, t.toe_strength],
            curve2: [t.toe_num, t.toe_denom, white, 1.0 / curve_at(t, white)],
            lut: [look.lut_weight, (self.lut_dim - 1) as f32 / self.lut_dim as f32, 0.5 / self.lut_dim as f32, if look.bloom.is_some() { 1.0 } else { 0.0 }],
            ..Default::default()
        };
        let present_offset = push(present);
        let exposure_offset = push(present);
        let bloom_steps = look.bloom.map(|[strength, threshold, width]| {
            let texel = |s: (u32, u32)| [1.0 / s.0 as f32, 1.0 / s.1 as f32];
            let threshold_offset = push(Params { texel: [1.0 / size.0 as f32, 1.0 / size.1 as f32, 0.0, 0.0], a: [-threshold / width, 1.0 / width, strength, look.exposure], ..Default::default() });
            let copy_offset = push(Params::default());
            let blurs: Vec<(u32, u32)> = levels
                .iter()
                .map(|(_, s)| {
                    let [x, y] = texel(*s);
                    (push(Params { texel: [x, y, x, 0.0], ..Default::default() }), push(Params { texel: [x, y, 0.0, y], ..Default::default() }))
                })
                .collect();
            let combines: Vec<u32> = (1..LEVELS)
                .rev()
                .map(|i| {
                    let [x, y] = texel(levels[i].1);
                    let lower_weight = if i == LEVELS - 1 { BLUR_WEIGHTS[i + 1] } else { 1.0 };
                    push(Params { texel: [x, y, 0.0, 0.0], a: [BLUR_WEIGHTS[i], lower_weight, 0.0, 0.0], ..Default::default() })
                })
                .collect();
            (threshold_offset, copy_offset, blurs, combines)
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post params"), size: params.len() as u64 * STRIDE, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut bytes = vec![0u8; (params.len() as u64 * STRIDE) as usize];
        for (k, p) in params.iter().enumerate() {
            let at = k * STRIDE as usize;
            bytes[at..at + std::mem::size_of::<Params>()].copy_from_slice(bytemuck::bytes_of(p));
        }
        queue.write_buffer(&buffer, 0, &bytes);

        let black = &self.black;
        let luminance_step = self.step(device, &buffer, exposure_offset, &hdr_view, black, black);
        let reduce_step = self.step(device, &buffer, exposure_offset, &luminance, black, black);
        let (threshold_step, level_steps, combine_steps) = match bloom_steps {
            Some((threshold_offset, copy_offset, blurs, combines)) => {
                let threshold = self.step(device, &buffer, threshold_offset, &hdr_view, black, &exposure);
                let level_steps = (0..LEVELS)
                    .map(|i| {
                        let copy = (i > 0).then(|| self.step(device, &buffer, copy_offset, &levels[i - 1].0, black, black));
                        (copy, self.step(device, &buffer, blurs[i].0, &levels[i].0, black, black), self.step(device, &buffer, blurs[i].1, &scratch[i], black, black))
                    })
                    .collect();
                let mut combine_steps = Vec::new();
                for (k, i) in (1..LEVELS).rev().enumerate() {
                    let lower = if i == LEVELS - 1 { &levels[i].0 } else { &combined[i] };
                    combine_steps.push(self.step(device, &buffer, combines[k], &levels[i - 1].0, lower, black));
                }
                (Some(threshold), level_steps, combine_steps)
            }
            None => (None, Vec::new(), Vec::new()),
        };
        let bloom = if threshold_step.is_some() { &combined[0] } else { black };
        let present_step = self.step(device, &buffer, present_offset, &hdr_view, bloom, &exposure);
        Targets { size, hdr, hdr_view, luminance, exposure, levels, scratch, combined, luminance_step, reduce_step, threshold_step, level_steps, combine_steps, present_step }
    }

    fn pass(&self, encoder: &mut wgpu::CommandEncoder, label: &str, target: &wgpu::TextureView, pipeline: &wgpu::RenderPipeline, step: &Step) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &step.group, &[step.offset]);
        pass.draw(0..3, 0..1);
    }

    /// The exposure and bloom passes, after the scene is in `targets.hdr`.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, targets: &Targets) {
        if self.look.adapt[0] > 0.0 {
            self.pass(encoder, "luminance", &targets.luminance, &self.luminance, &targets.luminance_step);
        }
        self.pass(encoder, "exposure", &targets.exposure, &self.reduce, &targets.reduce_step);
        let Some(threshold) = &targets.threshold_step else { return };
        self.pass(encoder, "bloom threshold", &targets.levels[0].0, &self.threshold, threshold);
        for (i, (copy, across, down)) in targets.level_steps.iter().enumerate() {
            if let Some(copy) = copy {
                self.pass(encoder, "bloom copy", &targets.levels[i].0, &self.copy, copy);
            }
            self.pass(encoder, "bloom blur", &targets.scratch[i], &self.blur, across);
            self.pass(encoder, "bloom blur", &targets.levels[i].0, &self.blur, down);
        }
        for (k, i) in (1..LEVELS).rev().enumerate() {
            self.pass(encoder, "bloom combine", &targets.combined[i - 1], &self.combine, &targets.combine_steps[k]);
        }
    }

    /// The present pass's one draw, into a pass on the output (which egui may draw into after it).
    pub fn present(&self, pass: &mut wgpu::RenderPass<'_>, targets: &Targets) {
        pass.set_pipeline(&self.present);
        pass.set_bind_group(0, &targets.present_step.group, &[targets.present_step.offset]);
        pass.draw(0..3, 0..1);
    }
}

fn lut_texture(device: &wgpu::Device, queue: &wgpu::Queue, dim: u32, rgba: &[u8]) -> wgpu::TextureView {
    use wgpu::util::DeviceExt as _;
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("lookup table"),
            size: wgpu::Extent3d { width: dim, height: dim, depth_or_array_layers: dim },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        rgba,
    );
    texture.create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    use nades_pack::scene::{Bloom, Exposure, Lut};

    use super::*;

    #[test]
    fn middle_grey_inverts_the_curve() {
        let t = Tonemap::DEFAULT;
        let x = scene_value_for(&t, 0.18).unwrap();
        let white = 2f32.powf(t.white_point);
        let shown = curve_at(&t, (x * 2.8).min(white)) / curve_at(&t, white);
        assert!((shown - 0.18).abs() < 1e-4, "{shown}");
    }

    #[test]
    fn a_maps_look_resolves_as_the_web_view_sets_it() {
        // de_cache: adapts between 1 and 1.2, screen bloom, a 32³ table
        let post = Post {
            tonemap: Some(Tonemap { exposure_bias: -0.1, ..Tonemap::DEFAULT }),
            exposure: Some(Exposure { enabled: true, min: 1.0, max: 1.2, compensation: 0.0 }),
            bloom: Some(Bloom { mode: BloomMode::Screen, strength: 0.0, screen: 0.0301, blur: 0.0, threshold: 4.03, threshold_width: 9.999, opacity: 1.0 }),
            lut: Some(Lut { dim: 32, weight: 1.0, rgba: vec![0; 32 * 32 * 32 * 4] }),
            fog: None,
            gradient_fog: None,
        };
        let look = Look::of(&post);
        assert!(look.adapt[0] > 0.0 && look.adapt[1..] == [1.0, 1.2]);
        assert!((look.exposure - 2f32.powf(-0.1)).abs() < 1e-6);
        assert_eq!(look.bloom, Some([0.0301, 4.03, 9.999]));
        assert_eq!(look.bloom_mix, [0.0, 1.0, 0.0, 0.0]);
        // bounds with nothing to choose from fix the exposure at their minimum
        let locked = Post { exposure: Some(Exposure { enabled: true, min: 1.5, max: 1.5, compensation: 1.0 }), ..post.clone() };
        let look = Look::of(&locked);
        assert_eq!(look.adapt, [0.0, 1.0, 1.0]);
        assert!((look.exposure - 1.5 * 2f32.powf(0.9)).abs() < 1e-5);
        // no bloom without a strength
        let dull = Post { bloom: Some(Bloom { screen: 0.0, ..post.bloom.unwrap() }), ..post };
        assert_eq!((Look::of(&dull).bloom, Look::of(&dull).bloom_mix), (None, [0.0; 4]));
    }
}
