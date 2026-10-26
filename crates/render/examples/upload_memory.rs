//! Where a map's memory goes on its way to the GPU, step by step: the process's footprint and its graphics share after
//! the pack is mapped, after its textures are made, after the GPU has read the Geometry section and the Textures section
//! through buffers over the mapping, and after each is let go. Then the map as the app makes it (`Map::new`), drawn five
//! times offscreen at the app's view size with each frame's GPU time: three frames from a de_cache camera, two turned
//! the other way (on another map the camera is wherever those coordinates fall). macOS only (the footprint is macOS's).
//!
//!     cargo run --release -p nades-render --example upload_memory -- ~/.cache/nades/packs/de_cache.pack [--vmmap]
//!
//! `--vmmap` adds macOS's region summary after the frames (`owned unmapped` is memory the driver holds for the process).

use std::time::Duration;

use nades_pack::Pack;
use nades_render::{Gpu, map, memory};

fn report(step: &str) {
    let f = memory::footprint().unwrap_or_default();
    println!("{step:44} footprint {:8.1} MB  graphics {:8.1} MB  peak {:8.1} MB", f.now_mb(), memory::mb(f.graphics), memory::mb(f.peak));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("a pack's path")?;
    report("start");
    let pack: &'static Pack = Box::leak(Box::new(Pack::open(std::path::Path::new(&path))?));
    let scene = pack.scene()?;
    let gpu = Gpu::new(nades_render::instance(), None)?;
    let (device, queue) = (&gpu.device, &gpu.queue);
    let blocks: u64 = scene
        .textures
        .iter()
        .flat_map(|t| t.mips.iter().map(move |m| u64::from(m.width.div_ceil(t.format.block_dim())) * u64::from(m.height.div_ceil(t.format.block_dim())) * u64::from(t.format.block_bytes()) * u64::from(m.layers)))
        .sum();
    println!(
        "{}: geometry {:.1} MB, textures section {:.1} MB, {} textures holding {:.1} MB of blocks",
        pack.contents().source.map,
        memory::mb(pack.geometry().len() as u64),
        memory::mb(pack.textures().len() as u64),
        scene.textures.len(),
        memory::mb(blocks)
    );
    let mut by_format: std::collections::BTreeMap<String, (usize, u64)> = Default::default();
    for t in &scene.textures {
        let bytes: u64 = t.mips.iter().map(|m| u64::from(m.width.div_ceil(t.format.block_dim())) * u64::from(m.height.div_ceil(t.format.block_dim())) * u64::from(t.format.block_bytes()) * u64::from(m.layers)).sum();
        let e = by_format.entry(format!("{:?}{}{}", t.format, if t.srgb { " srgb" } else { "" }, if t.volume { " volume" } else if t.is_cube() { " cube" } else { "" })).or_default();
        e.0 += 1;
        e.1 += bytes;
    }
    for (format, (count, bytes)) in &by_format {
        println!("    {format:20} {count:4} textures {:8.1} MB", memory::mb(*bytes));
    }
    report("pack mapped, device made");
    let wait = || {
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        std::thread::sleep(Duration::from_millis(200));
    };

    let textures: Vec<wgpu::Texture> = scene.textures.iter().map(|t| map::create_texture(device, t)).collect();
    report("textures made, empty");

    let geometry = map::buffer_over(device, "geometry", pack.geometry(), wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::STORAGE);
    report("geometry: buffer over the mapping");
    let copy = device.create_buffer(&wgpu::BufferDescriptor { label: Some("geometry read"), size: geometry.size(), usage: wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&geometry, 0, &copy, 0, geometry.size());
    queue.submit([encoder.finish()]);
    wait();
    report("geometry read by the GPU into a private copy");
    drop(copy);
    wait();
    report("private copy let go");

    let texels = map::buffer_over(device, "textures", pack.textures(), wgpu::BufferUsages::COPY_SRC);
    report("textures section: buffer over the mapping");
    let mut encoder = device.create_command_encoder(&Default::default());
    for (t, texture) in scene.textures.iter().zip(&textures) {
        let chunk = scene.chunks[t.chunk as usize].offset;
        for (level, mip) in t.mips.iter().enumerate() {
            let (width, height) = mip.copy_size(t.format);
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo { buffer: &texels, layout: wgpu::TexelCopyBufferLayout { offset: chunk + mip.offset, bytes_per_row: Some(mip.bytes_per_row), rows_per_image: Some(mip.rows) } },
                wgpu::TexelCopyTextureInfo { texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::Extent3d { width, height, depth_or_array_layers: mip.layers },
            );
        }
    }
    queue.submit([encoder.finish()]);
    wait();
    report("textures copied from it, finished on the GPU");
    drop(texels);
    wait();
    report("textures section's buffer let go");

    #[cfg(target_os = "macos")]
    {
        use objc2_metal::{MTLAllocation as _, MTLTexture as _};
        let mut allocated = 0u64;
        for t in &textures {
            // SAFETY: only the texture's allocated size is read
            unsafe {
                if let Some(raw) = t.as_hal::<wgpu::hal::api::Metal>() {
                    let raw = raw.raw_handle();
                    let _ = raw.width();
                    allocated += raw.allocatedSize() as u64;
                }
            }
        }
        println!("Metal allocated {:.1} MB for the {} textures ({:.1} MB of blocks)", memory::mb(allocated), textures.len(), memory::mb(blocks));
    }
    drop(textures);
    wait();
    report("textures let go");
    drop(geometry);
    wait();
    report("geometry's buffer let go");

    // the real path: the map as the app makes it, then frames drawn offscreen at the app's view size
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let map = map::Map::new(device, queue, pack, &scene, format, &|_| {});
    wait();
    report("Map::new, its copies finished");
    let (width, height) = (1444, 1420);
    let targets = map.targets(device, queue, width, height);
    wait();
    report("targets made");
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("output"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let output = output.create_view(&Default::default());
    let first = nades_render::View::from_cs([-516.7, 14.1, 1728.0], 0.5, 8.6, 73.74);
    let turned = nades_render::View::from_cs([-516.7, 14.1, 1728.0], 0.5, 188.6, 73.74);
    for k in 0..5 {
        // three frames from one camera, then two looking the other way
        let eye = if k < 3 { first } else { turned };
        let mut encoder = device.create_command_encoder(&Default::default());
        map.render(queue, &mut encoder, &targets, &eye, k as f32 / 60.0, &map::FrameEffects::default());
        map.present(
            &mut encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &output, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            }),
            &targets,
        );
        let started = std::time::Instant::now();
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let gpu_ms = started.elapsed().as_secs_f64() * 1e3;
        wait();
        report(&format!("frame {} drawn offscreen ({gpu_ms:.1} ms)", k + 1));
    }
    if std::env::args().any(|a| a == "--vmmap") {
        let out = std::process::Command::new("vmmap").args(["--summary", &std::process::id().to_string()]).output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines().filter(|l| l.contains("MALLOC") || l.contains("IOAccelerator") || l.contains("IOSurface") || l.contains("TOTAL") || l.contains("owned") || l.contains("Physical footprint")) {
            println!("    {line}");
        }
    }
    drop(targets);
    wait();
    report("targets let go");
    drop(map);
    wait();
    report("map let go");
    Ok(())
}
