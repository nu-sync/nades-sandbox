//! What a fresh device's first submissions cost in memory, with no map: an empty submission, a cleared target, a
//! fullscreen triangle. macOS only (the footprint is macOS's).

use nades_render::{Gpu, memory};

fn report(step: &str) {
    let f = memory::footprint().unwrap_or_default();
    println!("{step:44} footprint {:8.1} MB  graphics {:8.1} MB", f.now_mb(), memory::mb(f.graphics));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gpu = Gpu::new(nades_render::instance(), None)?;
    let (device, queue) = (&gpu.device, &gpu.queue);
    report("device made");
    queue.submit([device.create_command_encoder(&Default::default()).finish()]);
    gpu.wait_idle();
    report("an empty submission");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 1444, height: 1420, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let pass = |pipeline: Option<&wgpu::RenderPipeline>| {
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            if let Some(p) = pipeline {
                pass.set_pipeline(p);
                pass.draw(0..3, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
        gpu.wait_idle();
    };
    pass(None);
    report("a 1444×1420 half-float target cleared");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl("@vertex fn v(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> { return vec4<f32>(f32(i & 1u) * 4.0 - 1.0, f32(i >> 1u) * 4.0 - 1.0, 0.0, 1.0); } @fragment fn f() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }".into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("v"), compilation_options: Default::default(), buffers: &[] },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("f"), compilation_options: Default::default(), targets: &[Some(wgpu::TextureFormat::Rgba16Float.into())] }),
        multiview_mask: None,
        cache: None,
    });
    report("a trivial pipeline made");
    pass(Some(&pipeline));
    report("a fullscreen triangle drawn");
    pass(Some(&pipeline));
    report("and again");
    Ok(())
}
