//! Draws one draw's lightmap triangles over the decoded irradiance (half size, tone-mapped) as a PPM, to see whether
//! its coordinates land on charts of its own shape. `cargo run --release -p nades-packer --example lightmap_plot --
//! de_ancient <draw index> <out.ppm>`

use anyhow::Result;
use nades_pack::Pack;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let map = args.next().unwrap_or_else(|| "de_ancient".into());
    let draw: usize = args.next().unwrap_or_else(|| "174".into()).parse()?;
    let out = args.next().unwrap_or_else(|| "plot.ppm".into());
    let inputs = nades_packer::Inputs::for_map(&map)?;
    let pack = Pack::open(&inputs.out)?;
    let scene = pack.scene()?;
    let vpk = nades_source2::vpk::Vpk::open(&inputs.vpk())?;
    let bytes = vpk.read(&format!("maps/{map}/lightmaps/irradiance.vtex_c"))?;
    let vtex = nades_source2::parse(&bytes)?;
    let mut blocks = vec![0u8; vtex.mip_bytes(1)];
    vtex.read_mip(&bytes, 1, &mut blocks)?;
    let (w, h) = ((vtex.width >> 1) as usize, (vtex.height >> 1) as usize);
    let mut rgb = vec![0u8; w * h * 3];
    for by in 0..h / 4 {
        for bx in 0..w / 4 {
            let mut px = [0f32; 48];
            bcdec_rs::bc6h_float(&blocks[(by * (w / 4) + bx) * 16..][..16], &mut px, 12, false);
            for y in 0..4 {
                for x in 0..4 {
                    for c in 0..3 {
                        let v = px[y * 12 + x * 3 + c];
                        rgb[((by * 4 + y) * w + bx * 4 + x) * 3 + c] = ((v / (1.0 + v)).sqrt() * 255.0) as u8;
                    }
                }
            }
        }
    }
    let d = &scene.draws[draw];
    let l = d.lightmap.expect("a lightmapped draw");
    let uv = l.uv.of(pack.geometry());
    let at = |k: usize| (f32::from_le_bytes(uv[k * 8..k * 8 + 4].try_into().unwrap()), f32::from_le_bytes(uv[k * 8 + 4..k * 8 + 8].try_into().unwrap()));
    let idx = d.indices.of(pack.geometry());
    let index = |k: usize| match d.index_format {
        nades_pack::scene::IndexFormat::U16 => usize::from(u16::from_le_bytes([idx[k * 2], idx[k * 2 + 1]])),
        nades_pack::scene::IndexFormat::U32 => u32::from_le_bytes(idx[k * 4..k * 4 + 4].try_into().unwrap()) as usize,
    };
    let (mut lo, mut hi) = ((1.0f32, 1.0f32), (0.0f32, 0.0f32));
    for t in 0..d.index_count as usize / 3 {
        let p: Vec<(f32, f32)> = (0..3).map(|c| at(index(t * 3 + c))).collect();
        for i in 0..3 {
            let (a, b) = (p[i], p[(i + 1) % 3]);
            lo = (lo.0.min(a.0), lo.1.min(a.1));
            hi = (hi.0.max(a.0), hi.1.max(a.1));
            for s in 0..=32 {
                let f = s as f32 / 32.0;
                let (u, v) = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
                let (x, y) = (((u * w as f32) as usize).min(w - 1), ((v * h as f32) as usize).min(h - 1));
                rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[255, 0, 255]);
            }
        }
    }
    // crop around the draw's charts
    let margin = 0.01;
    let (x0, y0) = ((((lo.0 - margin).max(0.0)) * w as f32) as usize, (((lo.1 - margin).max(0.0)) * h as f32) as usize);
    let (x1, y1) = ((((hi.0 + margin).min(1.0)) * w as f32) as usize, (((hi.1 + margin).min(1.0)) * h as f32) as usize);
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut ppm = format!("P6 {cw} {ch} 255\n").into_bytes();
    for y in y0..y1 {
        ppm.extend_from_slice(&rgb[(y * w + x0) * 3..(y * w + x1) * 3]);
    }
    std::fs::write(&out, ppm)?;
    println!("draw {draw} {}: uv box ({:.4}, {:.4})–({:.4}, {:.4}), {cw}x{ch} texels", d.name, lo.0, lo.1, hi.0, hi.1);
    Ok(())
}
