//! Whether a map's irradiance mip 1 is mip 0 halved: decodes both and compares a few hundred 2×2 averages.
//! `cargo run --release -p nades-packer --example lightmap_mips -- de_ancient`

use anyhow::Result;

fn decode(blocks: &[u8], w: usize, h: usize) -> Vec<[f32; 3]> {
    let mut out = vec![[0f32; 3]; w * h];
    for by in 0..h / 4 {
        for bx in 0..w / 4 {
            let mut px = [0f32; 48];
            bcdec_rs::bc6h_float(&blocks[(by * (w / 4) + bx) * 16..][..16], &mut px, 12, false);
            for y in 0..4 {
                for x in 0..4 {
                    out[(by * 4 + y) * w + bx * 4 + x] = [px[y * 12 + x * 3], px[y * 12 + x * 3 + 1], px[y * 12 + x * 3 + 2]];
                }
            }
        }
    }
    out
}

fn main() -> Result<()> {
    let map = std::env::args().nth(1).unwrap_or_else(|| "de_ancient".into());
    let inputs = nades_packer::Inputs::for_map(&map)?;
    let vpk = nades_source2::vpk::Vpk::open(&inputs.vpk())?;
    let bytes = vpk.read(&format!("maps/{map}/lightmaps/irradiance.vtex_c"))?;
    let vtex = nades_source2::parse(&bytes)?;
    println!("{}x{} depth {} mips {} flags {:#x} stored {:?} lz4 {}", vtex.width, vtex.height, vtex.depth, vtex.mips, vtex.flags, vtex.stored, vtex.lz4);
    let (w, h) = (vtex.width as usize, vtex.height as usize);
    let mut b0 = vec![0u8; vtex.mip_bytes(0)];
    vtex.read_mip(&bytes, 0, &mut b0)?;
    let mut b1 = vec![0u8; vtex.mip_bytes(1)];
    vtex.read_mip(&bytes, 1, &mut b1)?;
    let m0 = decode(&b0, w, h);
    let m1 = decode(&b1, w / 2, h / 2);
    let (mut err, mut sum, mut n) = (0f64, 0f64, 0);
    for k in 0..400 {
        let (x, y) = ((k * 7919) % (w / 2), (k * 104_729) % (h / 2));
        let avg: f32 = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().map(|&(dx, dy)| m0[(2 * y + dy) * w + 2 * x + dx][1]).sum::<f32>() / 4.0;
        let one = m1[y * (w / 2) + x][1];
        err += f64::from((avg - one).abs());
        sum += f64::from(avg);
        n += 1;
    }
    println!("mean |mip1 - mip0 halved| {:.4} against a mean of {:.4} over {n} texels (green)", err / f64::from(n), sum / f64::from(n));
    Ok(())
}
