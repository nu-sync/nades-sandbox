//! How much of each lightmapped draw's coordinates land on the lightmap's charts: per draw, the share of its vertices
//! whose texel in the game's chart map (`debug_chart_color.vtex_c` in the map's VPK) is black, by the attribute its
//! coordinates came from. `cargo run --release -p nades-packer --example lightmap_coverage -- de_ancient`

use anyhow::{Context as _, Result};
use nades_pack::Pack;
use nades_pack::scene::UvSet;

fn main() -> Result<()> {
    let map = std::env::args().nth(1).unwrap_or_else(|| "de_ancient".into());
    let inputs = nades_packer::Inputs::for_map(&map)?;
    let pack = Pack::open(&inputs.out)?;
    let scene = pack.scene()?;
    let vpk = nades_source2::vpk::Vpk::open(&inputs.vpk())?;
    let entry = format!("maps/{map}/lightmaps/debug_chart_color.vtex_c");
    let bytes = vpk.read(&entry).with_context(|| entry.clone())?;
    let vtex = nades_source2::parse(&bytes)?;
    let mut blocks = vec![0u8; vtex.mip_bytes(0)];
    vtex.read_mip(&bytes, 0, &mut blocks)?;
    let (w, h) = (vtex.width as usize, vtex.height as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for by in 0..h / 4 {
        for bx in 0..w / 4 {
            let block = &blocks[(by * (w / 4) + bx) * 8..][..8];
            let mut out = [0u8; 64];
            bcdec_rs::bc1(block, &mut out, 16);
            for y in 0..4 {
                let row = (by * 4 + y) * w + bx * 4;
                rgba[row * 4..row * 4 + 16].copy_from_slice(&out[y * 16..y * 16 + 16]);
            }
        }
    }
    let charted = |u: f32, v: f32| {
        let x = ((u.clamp(0.0, 1.0) * w as f32) as usize).min(w - 1);
        let y = ((v.clamp(0.0, 1.0) * h as f32) as usize).min(h - 1);
        let t = &rgba[(y * w + x) * 4..][..3];
        t.iter().map(|&c| u32::from(c)).sum::<u32>() > 12
    };
    // the irradiance at level 1 (half size), decoded
    let ientry = format!("maps/{map}/lightmaps/irradiance.vtex_c");
    let ibytes = vpk.read(&ientry)?;
    let ivtex = nades_source2::parse(&ibytes)?;
    let mut iblocks = vec![0u8; ivtex.mip_bytes(1)];
    ivtex.read_mip(&ibytes, 1, &mut iblocks)?;
    let (iw, ih) = ((ivtex.width >> 1) as usize, (ivtex.height >> 1) as usize);
    let mut irr = vec![0f32; iw * ih];
    for by in 0..ih / 4 {
        for bx in 0..iw / 4 {
            let block = &iblocks[(by * (iw / 4) + bx) * 16..][..16];
            let mut out = [0f32; 48];
            bcdec_rs::bc6h_float(block, &mut out, 12, false);
            for y in 0..4 {
                for x in 0..4 {
                    let t = &out[y * 12 + x * 3..][..3];
                    irr[(by * 4 + y) * iw + bx * 4 + x] = 0.2126 * t[0] + 0.7152 * t[1] + 0.0722 * t[2];
                }
            }
        }
    }
    let lit = |u: f32, v: f32| {
        let x = ((u.clamp(0.0, 1.0) * iw as f32) as usize).min(iw - 1);
        let y = ((v.clamp(0.0, 1.0) * ih as f32) as usize).min(ih - 1);
        irr[y * iw + x]
    };
    // chart coherence: the share of triangles whose three corners, pulled a little toward the centre, sample one chart
    let chart_of = |u: f32, v: f32| -> [u8; 3] {
        let x = ((u.clamp(0.0, 1.0) * w as f32) as usize).min(w - 1);
        let y = ((v.clamp(0.0, 1.0) * h as f32) as usize).min(h - 1);
        let t = &rgba[(y * w + x) * 4..][..3];
        [t[0], t[1], t[2]]
    };
    let mut coherences: Vec<(f32, String, UvSet)> = Vec::new();
    for d in &scene.draws {
        let Some(l) = d.lightmap else { continue };
        let uv = l.uv.of(pack.geometry());
        let at = |k: usize| (f32::from_le_bytes(uv[k * 8..k * 8 + 4].try_into().unwrap()), f32::from_le_bytes(uv[k * 8 + 4..k * 8 + 8].try_into().unwrap()));
        let idx = d.indices.of(pack.geometry());
        let index = |k: usize| match d.index_format {
            nades_pack::scene::IndexFormat::U16 => usize::from(u16::from_le_bytes([idx[k * 2], idx[k * 2 + 1]])),
            nades_pack::scene::IndexFormat::U32 => u32::from_le_bytes(idx[k * 4..k * 4 + 4].try_into().unwrap()) as usize,
        };
        let triangles = d.index_count as usize / 3;
        let step = (triangles / 2000).max(1);
        let (mut same, mut n) = (0, 0);
        for t in (0..triangles).step_by(step) {
            let p: Vec<(f32, f32)> = (0..3).map(|c| at(index(t * 3 + c))).collect();
            let centre = ((p[0].0 + p[1].0 + p[2].0) / 3.0, (p[0].1 + p[1].1 + p[2].1) / 3.0);
            let inset: Vec<[u8; 3]> = p.iter().map(|q| chart_of(q.0 * 0.7 + centre.0 * 0.3, q.1 * 0.7 + centre.1 * 0.3)).collect();
            n += 1;
            if inset[0] == inset[1] && inset[1] == inset[2] {
                same += 1;
            }
        }
        coherences.push((same as f32 / n.max(1) as f32, d.name.clone(), l.from));
    }
    for set in [UvSet::Texcoord1, UvSet::Texcoord2] {
        let mut c: Vec<&(f32, String, UvSet)> = coherences.iter().filter(|x| x.2 == set).collect();
        c.sort_by(|a, b| a.0.total_cmp(&b.0));
        let bins = [0.25, 0.5, 0.75, 0.9, 1.01].map(|hi| c.iter().filter(|x| x.0 < hi).count());
        println!("{set:?}: {} draws; coherence below 0.25 {}, 0.5 {}, 0.75 {}, 0.9 {}, all {}", c.len(), bins[0], bins[1], bins[2], bins[3], bins[4]);
        for x in c.iter().filter(|x| x.0 < 0.8) {
            println!("   {:.2} {}", x.0, x.1);
        }
        for x in c.iter().rev().take(3) {
            println!("   {:.2} {}", x.0, x.1);
        }
    }
    let mut totals = [(0u32, 0u32); 2];
    for (i, d) in scene.draws.iter().enumerate() {
        let Some(l) = d.lightmap else { continue };
        let uv = l.uv.of(pack.geometry());
        let n = d.vertex_count as usize;
        let at = |k: usize| (f32::from_le_bytes(uv[k * 8..k * 8 + 4].try_into().unwrap()), f32::from_le_bytes(uv[k * 8 + 4..k * 8 + 8].try_into().unwrap()));
        let off = (0..n).filter(|&k| { let (u, v) = at(k); !charted(u, v) }).count();
        let dark = (0..n).filter(|&k| { let (u, v) = at(k); lit(u, v) < 0.002 }).count();
        let set = if l.from == UvSet::Texcoord1 { 0 } else { 1 };
        totals[set].0 += 1;
        let share = off as f32 / n.max(1) as f32;
        let dark_share = dark as f32 / n.max(1) as f32;
        if share > 0.5 || dark_share > 0.3 {
            totals[set].1 += 1;
            let (u0, v0) = at(0);
            println!("draw {i} {} ({:?}, {} vertices, {} instances, material {}): {:.0}% off the charts, {:.0}% on black irradiance, first uv ({u0:.4}, {v0:.4})", d.name, l.from, n, d.instance_count(), scene.materials[d.material as usize].name, share * 100.0, dark_share * 100.0);
        }
    }
    println!("TEXCOORD_1: {} draws, {} mostly off the charts; TEXCOORD_2: {} draws, {} mostly off", totals[0].0, totals[0].1, totals[1].0, totals[1].1);
    let instanced: Vec<_> = scene.draws.iter().filter(|d| d.lightmap.is_some() && d.instances.is_some()).map(|d| (d.name.as_str(), d.instance_count())).collect();
    println!("lightmapped draws with instances: {} {:?}", instanced.len(), &instanced[..instanced.len().min(10)]);
    Ok(())
}
