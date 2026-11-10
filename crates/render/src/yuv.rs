//! A presented frame as NV12 for a video encoder, converted on the GPU (`yuv.wgsl`): the luma plane, then the chroma
//! rows with U and V interleaved, in one R8 target of `width` × `height * 3 / 2` bytes. The arithmetic is swscale's
//! own for packed RGBA to yuv420p (BT.601, limited range, its integer coefficients and its vertical chroma filter),
//! so the bytes are the ones FFmpeg made of the same frame (S51): 3 bytes for every 8 the encoder was sent before,
//! and no conversion left for it to do.

use bytemuck::{Pod, Zeroable};

/// A rectangle painted over the frame before it is converted: `x0, y0, x1, y1` (the ends exclusive) and its colour.
pub type Paint = ([i32; 4], [u8; 3]);

/// How many rectangles an overlay holds (a crosshair is eight: four arms and their outlines).
pub const PAINTS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    size: [u32; 2],
    paints: u32,
    pad: u32,
    rects: [[i32; 4]; PAINTS],
    colors: [[i32; 4]; PAINTS],
}

/// The converter for one frame size and one source texture.
pub struct Nv12 {
    pub size: (u32, u32),
    target: wgpu::Texture,
    view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    /// The frame as it is, and with the overlay painted over it.
    groups: [wgpu::BindGroup; 2],
}

impl Nv12 {
    /// A converter of `source`'s frames: an `Rgba8UnormSrgb` texture of even `size` that can be bound and viewed as
    /// `Rgba8Unorm` (its stored bytes are what is converted). `overlay` is what [`Nv12::encode`] paints when asked.
    pub fn new(device: &wgpu::Device, source: &wgpu::Texture, overlay: &[Paint]) -> Self {
        let size = (source.width(), source.height());
        assert!(size.0.is_multiple_of(2) && size.1.is_multiple_of(2), "NV12 needs even dimensions");
        assert!(overlay.len() <= PAINTS, "an overlay holds at most {PAINTS} rectangles");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("yuv"), source: wgpu::ShaderSource::Wgsl(include_str!("yuv.wgsl").into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("yuv"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let format = wgpu::TextureFormat::R8Uint;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yuv"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("fullscreen"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("nv12"), compilation_options: Default::default(), targets: &[Some(format.into())] }),
            multiview_mask: None,
            cache: None,
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nv12"),
            size: wgpu::Extent3d { width: size.0, height: size.1 * 3 / 2, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        // the bytes as stored: the sRGB encoding is the frame's, not undone on the way in
        let stored = source.create_view(&wgpu::TextureViewDescriptor { format: Some(wgpu::TextureFormat::Rgba8Unorm), ..Default::default() });
        let group = |paints: &[Paint]| {
            use wgpu::util::DeviceExt as _;
            let mut params = Params { size: [size.0, size.1], paints: paints.len() as u32, pad: 0, rects: [[0; 4]; PAINTS], colors: [[0; 4]; PAINTS] };
            for (k, (rect, color)) in paints.iter().enumerate() {
                params.rects[k] = *rect;
                params.colors[k] = [i32::from(color[0]), i32::from(color[1]), i32::from(color[2]), 0];
            }
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("yuv"), contents: bytemuck::bytes_of(&params), usage: wgpu::BufferUsages::UNIFORM });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("yuv"),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&stored) }],
            })
        };
        Self { size, target, view, pipeline, groups: [group(&[]), group(overlay)] }
    }

    /// What a source texture needs besides being drawn into: to be bound, and viewed as its stored bytes.
    pub const SOURCE_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING;
    pub const SOURCE_VIEW_FORMATS: &'static [wgpu::TextureFormat] = &[wgpu::TextureFormat::Rgba8Unorm];

    /// The frame's bytes: `width * height * 3 / 2`.
    pub fn bytes(&self) -> usize {
        self.size.0 as usize * self.size.1 as usize * 3 / 2
    }

    /// A readback row's bytes (rows are padded to the copy's alignment) and how many rows there are.
    pub fn rows(&self) -> (u32, u32) {
        (self.size.0.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT), self.size.1 * 3 / 2)
    }

    /// The conversion's pass, after the source's own, and the copy of its result into `readback`
    /// (of [`Nv12::rows`]' size); `overlay` paints the rectangles given at creation over the frame first.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, readback: &wgpu::Buffer, overlay: bool) {
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nv12"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &self.view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.groups[usize::from(overlay)], &[]);
            pass.draw(0..3, 0..1);
        }
        let (row, rows) = self.rows();
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(rows) } },
            wgpu::Extent3d { width: self.size.0, height: rows, depth_or_array_layers: 1 },
        );
    }
}

/// swscale's packed RGBA to yuv420p, as the shader does it: the reference the shader is checked against (and the
/// record of where its numbers come from). `rgba` is `width` × `height` pixels; the result is NV12.
///
/// Luma is `((8414 r + 16519 g + 3208 b + 0x80100) >> 9) + 32) >> 6`. Chroma is taken from each pair of pixels across
/// (their sum), at 15 bits, and filtered down the rows by swscale's bicubic at half size: eight taps summing to 4096
/// over rows `2j - 3 ..= 2j + 4`, the rows clamped at the frame's edges.
pub fn nv12_of_rgba(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    const TAPS: [i32; 8] = [-58, -172, 492, 1786, 1786, 492, -172, -58];
    let mut out = Vec::with_capacity(width * height * 3 / 2);
    for p in rgba.chunks_exact(4) {
        let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
        out.push(((((8414 * r + 16519 * g + 3208 * b + 0x80100) >> 9) + 32) >> 6).min(255) as u8);
    }
    let half = width / 2;
    // each row's chroma at 15 bits, the pixels summed in pairs
    let mut chroma = vec![[0i32; 2]; half * height];
    for y in 0..height {
        for x in 0..half {
            let at = (y * width + 2 * x) * 4;
            let sum = |c: usize| i32::from(rgba[at + c]) + i32::from(rgba[at + 4 + c]);
            let (r, g, b) = (sum(0), sum(1), sum(2));
            chroma[y * half + x] = [((-4865 * r - 9528 * g + 14392 * b + (0x4001 << 9)) >> 10) << 1, ((14392 * r - 12061 * g - 2332 * b + (0x4001 << 9)) >> 10) << 1];
        }
    }
    for j in 0..height / 2 {
        for x in 0..half {
            let mut sum = [0i32; 2];
            for (k, tap) in TAPS.iter().enumerate() {
                let [u, v] = chroma[(2 * j + k).saturating_sub(3).min(height - 1) * half + x];
                sum = [sum[0] + tap * u, sum[1] + tap * v];
            }
            out.extend(sum.map(|s| ((s + (64 << 12)) >> 19).clamp(0, 255) as u8));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_colours_convert_as_swscale_does() {
        // FFmpeg 9's values for frames of one colour (rawvideo rgba to yuv420p)
        for (rgb, yuv) in [([255, 0, 0], [81, 90, 240]), ([0, 255, 0], [145, 54, 34]), ([0, 0, 255], [41, 240, 110]), ([255, 255, 255], [235, 128, 128]), ([0, 0, 0], [16, 128, 128]), ([50, 250, 50], [160, 70, 54]), ([128, 128, 128], [126, 128, 128])] {
            let rgba: Vec<u8> = (0..8 * 8).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect();
            let nv12 = nv12_of_rgba(&rgba, 8, 8);
            assert_eq!(nv12.len(), 96);
            assert!(nv12[..64].iter().all(|y| *y == yuv[0]), "{rgb:?}: luma {}", nv12[0]);
            assert!(nv12[64..].chunks_exact(2).all(|uv| uv == [yuv[1], yuv[2]]), "{rgb:?}: chroma {:?}", &nv12[64..66]);
        }
    }

    #[test]
    fn an_edge_between_rows_rings_as_the_bicubic_does() {
        // yellow above row 20, blue from it: FFmpeg's U down the rows is 16, 16, 13, 30, 226, 243, 240
        let (width, height) = (8, 48);
        let rgba: Vec<u8> = (0..height).flat_map(|y| (0..width).flat_map(move |_| if y < 20 { [255, 255, 0, 255] } else { [0, 0, 255, 255] })).collect();
        let nv12 = nv12_of_rgba(&rgba, width, height);
        let u: Vec<u8> = (6..13).map(|row| nv12[width * height + row * width]).collect();
        assert_eq!(u, [16, 16, 13, 30, 226, 243, 240]);
    }
}
