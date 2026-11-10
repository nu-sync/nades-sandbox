//! The GPU's NV12 of a frame (`nades_render::yuv`) against FFmpeg's of the same RGBA, byte for byte, with the
//! reference in Rust between them: frames of noise and of flat blocks, with and without painted rectangles, at the
//! sizes a video may have (a row of 1920 is padded in the readback, one of 1280 is not), down to the 12 rows under
//! which FFmpeg's chroma filter no longer fits the frame and its edges differ. Needs a GPU and `ffmpeg`.
//!
//!     cargo run --release -p nades-render --example nv12_vs_ffmpeg
//!
//! Exits with an error when any byte differs (S51).

use std::io::Write as _;
use std::process::{Command, Stdio};

use nades_render::Gpu;
use nades_render::yuv::{Nv12, Paint, nv12_of_rgba};

/// A frame of `width` × `height` RGBA: noise when `blocks` is 0, else flat blocks of that size with a little noise.
fn frame(width: usize, height: usize, blocks: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 24) as u8
    };
    if blocks == 0 {
        return (0..width * height * 4).map(|k| if k % 4 == 3 { 255 } else { next() }).collect();
    }
    let across = width.div_ceil(blocks);
    let colours: Vec<[u8; 3]> = (0..across * height.div_ceil(blocks)).map(|_| [next(), next(), next()]).collect();
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            let c = colours[(y / blocks) * across + x / blocks];
            let wobble = next() % 5;
            [c[0].saturating_add(wobble), c[1].saturating_sub(wobble), c[2], 255]
        })
        .collect()
}

fn painted(rgba: &[u8], width: usize, height: usize, paints: &[Paint]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for ([x0, y0, x1, y1], colour) in paints {
        for y in (*y0).max(0)..(*y1).min(height as i32) {
            for x in (*x0).max(0)..(*x1).min(width as i32) {
                let at = (y as usize * width + x as usize) * 4;
                out[at..at + 3].copy_from_slice(colour);
            }
        }
    }
    out
}

fn ffmpeg(rgba: &[u8], width: usize, height: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{width}x{height}"), "-i", "pipe:0", "-f", "rawvideo", "-pix_fmt", "nv12", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let bytes = rgba.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&bytes));
    let output = child.wait_with_output()?;
    writer.join().map_err(|_| "the writer panicked")??;
    Ok(output.stdout)
}

fn gpu_nv12(gpu: &Gpu, rgba: &[u8], width: u32, height: u32, paints: &[Paint], overlay: bool) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let (device, queue) = (&gpu.device, &gpu.queue);
    let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let source = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::COPY_DST | Nv12::SOURCE_USAGE,
        view_formats: Nv12::SOURCE_VIEW_FORMATS,
    });
    queue.write_texture(source.as_image_copy(), rgba, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) }, size);
    let nv12 = Nv12::new(device, &source, paints);
    let (row, rows) = nv12.rows();
    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: u64::from(row) * u64::from(rows), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut encoder = device.create_command_encoder(&Default::default());
    nv12.encode(&mut encoder, &readback, overlay);
    let submitted = queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| { let _ = sender.send(result); });
    device.poll(wgpu::PollType::Wait { submission_index: Some(submitted), timeout: Some(std::time::Duration::from_secs(10)) })?;
    receiver.recv()??;
    let mapped = readback.slice(..).get_mapped_range()?;
    let bytes: Vec<u8> = mapped.chunks_exact(row as usize).flat_map(|line| line[..width as usize].iter().copied()).collect();
    assert_eq!(bytes.len(), nv12.bytes());
    Ok(bytes)
}

fn differing(a: &[u8], b: &[u8]) -> usize {
    if a.len() != b.len() { usize::MAX } else { a.iter().zip(b).filter(|(x, y)| x != y).count() }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gpu = Gpu::new(nades_render::instance(), None)?;
    let mut failed = false;
    for (width, height) in [(1920u32, 1080u32), (1280, 720), (854 & !1, 480), (640, 360), (64, 48), (16, 12)] {
        // rectangles as a crosshair's: over one another, one reaching past the frame
        let (cx, cy) = (width as i32 / 2, height as i32 / 2);
        let paints: Vec<Paint> = vec![([cx - 9, cy - 2, cx + 9, cy + 3], [0, 0, 0]), ([cx - 7, cy - 1, cx + 7, cy + 1], [50, 250, 50]), ([-4, -4, 3, 3], [255, 0, 255]), ([width as i32 - 3, height as i32 - 5, width as i32 + 8, height as i32 + 8], [10, 20, 240])];
        for (blocks, overlay) in [(0, false), (0, true), (7, false), (16, true)] {
            let rgba = frame(width as usize, height as usize, blocks, u64::from(width) * 31 + blocks as u64);
            let shown = if overlay { painted(&rgba, width as usize, height as usize, &paints) } else { rgba.clone() };
            let reference = nv12_of_rgba(&shown, width as usize, height as usize);
            let from_gpu = gpu_nv12(&gpu, &rgba, width, height, &paints, overlay)?;
            let from_ffmpeg = ffmpeg(&shown, width as usize, height as usize)?;
            let (gpu_off, ffmpeg_off) = (differing(&from_gpu, &reference), differing(&from_ffmpeg, &reference));
            println!("{width}x{height} {} {}: {} bytes; GPU differs from the reference in {gpu_off}, FFmpeg in {ffmpeg_off}", if blocks == 0 { "noise".to_string() } else { format!("blocks of {blocks}") }, if overlay { "painted" } else { "plain" }, reference.len());
            failed |= gpu_off != 0 || ffmpeg_off != 0;
        }
    }
    if failed {
        return Err("the conversions differ".into());
    }
    println!("the GPU's NV12 is FFmpeg's, byte for byte");
    Ok(())
}
