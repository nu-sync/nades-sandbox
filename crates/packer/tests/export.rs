//! Checks of a built pack against the export's own decoded files, on this machine only: they read
//! `~/.cache/nades/packs/de_cache.pack` (run `nades-pack de_cache` first), the export and its decoded lightmap,
//! all derived from Valve's assets and never in the repository. Run them with
//!
//! ```text
//! cargo test --release -p nades-packer --test export -- --ignored --nocapture --test-threads 1
//! ```

use std::path::PathBuf;

use nades_pack::Pack;
use nades_pack::scene::{AlphaMode, Scene, Semantic, TextureFormat, TextureSource};
use nades_packer::{Inputs, gltf, textures};
use rayon::prelude::*;

fn inputs() -> Inputs {
    Inputs::for_map("de_cache").unwrap()
}

fn open() -> (Pack, Scene) {
    let path = inputs().out;
    let pack = Pack::open(&path).unwrap_or_else(|e| panic!("{}: {e} (run nades-pack de_cache first)", path.display()));
    let scene = pack.scene().unwrap();
    (pack, scene)
}

fn exr_path() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap();
    PathBuf::from(home).join(".cache/nades/cs2/export/de_cache/lightmaps/maps/de_cache/lightmaps/irradiance.exr")
}

/// The lightmap's mip 0 decoded block by block: texel (x, y) of a `width`-wide RGB f32 image.
fn decode_bc6h(blocks: &[u8], width: usize, height: usize, bytes_per_row: usize, signed: bool) -> Vec<f32> {
    let mut out = vec![0f32; width * height * 3];
    out.par_chunks_mut(width * 3 * 4).enumerate().for_each(|(by, rows)| {
        for bx in 0..width / 4 {
            let at = by * bytes_per_row + bx * 16;
            let mut texels = [0f32; 48];
            bcdec_rs::bc6h_float(&blocks[at..at + 16], &mut texels, 12, signed);
            for r in 0..4 {
                let row = r * width * 3 + bx * 12;
                rows[row..row + 12].copy_from_slice(&texels[r * 12..r * 12 + 12]);
            }
        }
    });
    out
}

#[test]
#[ignore = "reads the de_cache pack in ~/.cache/nades/packs"]
fn the_pack_opens_validates_and_holds_the_whole_map() {
    // what a renderer pays before its first upload: map the file, read the table, decode and check the scene
    let mut times = Vec::new();
    for _ in 0..7 {
        let started = std::time::Instant::now();
        let (pack, scene) = open();
        times.push(started.elapsed().as_secs_f64() * 1e3);
        std::hint::black_box((pack.contents().sections.len(), scene.draws.len()));
    }
    times.sort_by(f64::total_cmp);
    println!("Pack::open + scene(), warm, 7 runs: median {:.2} ms, best {:.2} ms", times[3], times[0]);
    let (pack, scene) = open();
    let s = &scene.stats;
    println!("source {:?}", pack.contents().source);
    println!("draws {}, materials {}, textures {}, chunks {}", scene.draws.len(), scene.materials.len(), scene.textures.len(), scene.chunks.len());
    assert_eq!(s.draws as usize, scene.draws.len());
    assert_eq!(s.lightmapped_texcoord2 + s.lightmapped_texcoord1 + s.unlit, s.draws + s.skybox_draws);
    // every index of every draw names one of its vertices (the packer checked; this reads them back from the file)
    let geometry = pack.geometry();
    for d in &scene.draws {
        let bytes = d.indices.of(geometry);
        let max = match d.index_format {
            nades_pack::scene::IndexFormat::U16 => bytes.chunks_exact(2).map(|c| u32::from(u16::from_le_bytes([c[0], c[1]]))).max(),
            nades_pack::scene::IndexFormat::U32 => bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).max(),
        };
        assert!(max.unwrap() < d.vertex_count, "{}", d.name);
        // w is 1.0 in every position
        assert!(d.positions.of(geometry).chunks_exact(8).all(|p| i16::from_le_bytes([p[6], p[7]]) == i16::MAX), "{}", d.name);
    }
    // the map's extent, in glTF metres: the bench camera (-4.0, 42.6, 29.6) looks down on it
    let lo = scene.draws.iter().fold([f32::INFINITY; 3], |a, d| std::array::from_fn(|k| a[k].min(d.bounds.min[k])));
    let hi = scene.draws.iter().fold([f32::NEG_INFINITY; 3], |a, d| std::array::from_fn(|k| a[k].max(d.bounds.max[k])));
    println!("world bounds {lo:?} .. {hi:?}");
    // instanced draws share one set of lightmap coordinates across their instances, as the web view draws them
    for d in scene.draws.iter().filter(|d| d.instances.is_some() && d.lightmap.is_some()) {
        println!("  instanced and lightmapped: {} ({} instances, {:?}, {})", d.name, d.instance_count(), d.lightmap.unwrap().from, scene.materials[d.material as usize].shader);
    }
    // opaque and alpha-tested draws, then blended ones (additive unlit surfaces among them), then csgo_effects
    let late = |d: &nades_pack::scene::Draw| {
        let m = &scene.materials[d.material as usize];
        m.alpha == nades_pack::scene::AlphaMode::Blend || matches!(m.shading, nades_pack::scene::Shading::Unlit { additive: true } | nades_pack::scene::Shading::Effects(_))
    };
    let blended = scene.draws.iter().position(late).unwrap();
    assert!(scene.draws[blended..].iter().all(late), "blended draws come last");
    let effects = scene.draws.iter().position(|d| scene.materials[d.material as usize].is_effect()).unwrap_or(scene.draws.len());
    assert!(scene.draws[effects..].iter().all(|d| scene.materials[d.material as usize].is_effect()), "effects come after everything else");
}

#[test]
#[ignore = "reads the de_cache pack and the export's irradiance.exr (1 GB of floats)"]
fn the_lightmap_blocks_decode_to_the_exports_exr() {
    let (pack, scene) = open();
    let lightmap = &scene.textures[scene.lightmaps.unwrap().irradiance as usize];
    assert_eq!((lightmap.format, lightmap.srgb), (TextureFormat::Bc6h, false));
    let (w, h) = (lightmap.width as usize, lightmap.height as usize);
    let mip0 = scene.mip_bytes(pack.textures(), scene.lightmaps.unwrap().irradiance, 0);
    let started = std::time::Instant::now();
    let image = exr::prelude::read_first_rgba_layer_from_file(
        exr_path(),
        |size, _| (size.width(), vec![0f32; size.width() * size.height() * 3]),
        |(width, pixels): &mut (usize, Vec<f32>), at, (r, g, b, _a): (f32, f32, f32, f32)| {
            let i = (at.y() * *width + at.x()) * 3;
            pixels[i..i + 3].copy_from_slice(&[r, g, b]);
        },
    )
    .unwrap();
    let (exr_width, exr) = image.layer_data.channel_data.pixels;
    assert_eq!((exr_width, exr.len()), (w, w * h * 3));
    println!("EXR read in {:.1} s", started.elapsed().as_secs_f64());

    let unsigned = decode_bc6h(mip0, w, h, lightmap.mips[0].bytes_per_row as usize, false);
    // every texel, as stored (row 0 = the EXR's first scanline): the difference is the EXR's own rounding at most
    let compare = |decoded: &[f32], flip: bool| -> (f64, f32, usize) {
        let (sum, max, exact) = (0..h)
            .into_par_iter()
            .map(|y| {
                let ey = if flip { h - 1 - y } else { y };
                let (mut sum, mut max, mut exact) = (0f64, 0f32, 0usize);
                for k in 0..w * 3 {
                    let d = (decoded[y * w * 3 + k] - exr[ey * w * 3 + k]).abs();
                    sum += f64::from(d);
                    max = max.max(d);
                    exact += usize::from(d == 0.0);
                }
                (sum, max, exact)
            })
            .reduce(|| (0.0, 0.0, 0), |a, b| (a.0 + b.0, a.1.max(b.1), a.2 + b.2));
        (sum / (w * h * 3) as f64, max, exact)
    };
    let (mean, max, exact) = compare(&unsigned, false);
    let mean_value = exr.par_iter().map(|&v| f64::from(v)).sum::<f64>() / exr.len() as f64;
    println!("unsigned BC6H vs EXR, every texel: mean |diff| {mean:.3e}, max {max:.3e}, {:.4}% exact; the EXR's mean value {mean_value:.4}", 100.0 * exact as f64 / exr.len() as f64);
    let (flip_mean, _, flip_exact) = compare(&unsigned, true);
    println!("the same blocks against the EXR flipped vertically: mean |diff| {flip_mean:.3e}, {:.2}% exact", 100.0 * flip_exact as f64 / exr.len() as f64);
    // a sample decoded as signed BC6H: wrong, which the EXR shows
    let rows = 64.min(h / 4);
    let signed = decode_bc6h(&mip0[..rows * lightmap.mips[0].bytes_per_row as usize], w, rows * 4, lightmap.mips[0].bytes_per_row as usize, true);
    let signed_diff = signed.iter().zip(&exr[..signed.len()]).map(|(a, b)| f64::from((a - b).abs())).sum::<f64>() / signed.len() as f64;
    let unsigned_diff = unsigned[..signed.len()].iter().zip(&exr[..signed.len()]).map(|(a, b)| f64::from((a - b).abs())).sum::<f64>() / signed.len() as f64;
    println!("first {} rows as signed BC6H: mean |diff| {signed_diff:.3e} (unsigned {unsigned_diff:.3e})", rows * 4);
    assert!(max <= 1e-3 * mean_value as f32 + 1e-6, "the blocks decode to the EXR's texels");
    assert!(flip_mean > 10.0 * mean.max(1e-9), "the comparison tells a flipped lightmap apart");
}

/// The lightmap coordinates as stored (v = 0 the texture's first row), against the same coordinates flipped or
/// swapped. Where two coplanar triangles share an edge but lie in different charts (a UV seam), both sides of the edge
/// are the same place on the same surface, so the irradiance read on either side must agree; a flip or a swap keeps
/// each chart whole but moves it, so the two sides read unrelated texels. (Lit-texel counts cannot tell them apart on
/// de_cache: its atlas is dense, and its 1% of black texels lie inside charts as well as between them.)
#[test]
#[ignore = "reads the de_cache pack"]
fn lightmap_coordinates_agree_across_uv_seams() {
    let (pack, scene) = open();
    let l = scene.lightmaps.unwrap().irradiance;
    let t = &scene.textures[l as usize];
    let (w, h) = (t.width as usize, t.height as usize);
    let image = decode_bc6h(scene.mip_bytes(pack.textures(), l, 0), w, h, t.mips[0].bytes_per_row as usize, false);
    let irradiance = |uv: glam::Vec2| -> f32 {
        let (x, y) = (((uv.x.clamp(0.0, 1.0) * w as f32) as usize).min(w - 1), ((uv.y.clamp(0.0, 1.0) * h as f32) as usize).min(h - 1));
        image[(y * w + x) * 3..][..3].iter().sum()
    };
    let geometry = pack.geometry();
    let (mut kept, mut flipped, mut swapped) = (Vec::new(), Vec::new(), Vec::new());
    for d in scene.draws.iter().filter(|d| d.instances.is_none()) {
        let Some(lm) = d.lightmap else { continue };
        let (uv, positions, indices) = (lm.uv.of(geometry), d.positions.of(geometry), d.indices.of(geometry));
        let model = glam::Mat4::from_cols_array(&d.model);
        let quantized = |i: usize| -> [i16; 3] { std::array::from_fn(|k| i16::from_le_bytes([positions[i * 8 + k * 2], positions[i * 8 + k * 2 + 1]])) };
        let world = |i: usize| model.transform_point3(glam::Vec3::from_array(quantized(i).map(|c| f32::from(c) / 32767.0)));
        let coordinates = |i: usize| glam::Vec2::new(f32::from_le_bytes(uv[i * 8..i * 8 + 4].try_into().unwrap()), f32::from_le_bytes(uv[i * 8 + 4..i * 8 + 8].try_into().unwrap()));
        let index = |k: usize| -> usize {
            match d.index_format {
                nades_pack::scene::IndexFormat::U16 => usize::from(u16::from_le_bytes([indices[k * 2], indices[k * 2 + 1]])),
                nades_pack::scene::IndexFormat::U32 => u32::from_le_bytes(indices[k * 4..k * 4 + 4].try_into().unwrap()) as usize,
            }
        };
        // each edge, by its two corners' positions, with the coordinates of those corners in each triangle on it
        type Sides = Vec<(glam::Vec2, glam::Vec2, glam::Vec3)>;
        let mut edges: std::collections::HashMap<([i16; 3], [i16; 3]), Sides> = std::collections::HashMap::new();
        for tri in 0..d.index_count as usize / 3 {
            let v = [index(tri * 3), index(tri * 3 + 1), index(tri * 3 + 2)];
            let normal = (world(v[1]) - world(v[0])).cross(world(v[2]) - world(v[0]));
            if normal.length() < 1e-9 {
                continue;
            }
            for e in 0..3 {
                let (a, b) = if quantized(v[e]) <= quantized(v[(e + 1) % 3]) { (v[e], v[(e + 1) % 3]) } else { (v[(e + 1) % 3], v[e]) };
                edges.entry((quantized(a), quantized(b))).or_default().push((coordinates(a), coordinates(b), normal.normalize()));
            }
        }
        let differ = |a: f32, b: f32| if a + b > 1e-6 { f64::from((a - b).abs() / (a + b)) } else { 0.0 };
        for sides in edges.values().filter(|s| s.len() == 2) {
            let ((a1, b1, n1), (a2, b2, n2)) = (sides[0], sides[1]);
            let texels = w as f32;
            if n1.dot(n2) < 0.99 || ((a1 - a2).length() * texels < 3.0 && (b1 - b2).length() * texels < 3.0) {
                continue; // a crease, or the same chart on both sides
            }
            let (m1, m2) = ((a1 + b1) / 2.0, (a2 + b2) / 2.0);
            let flip = |m: glam::Vec2| glam::Vec2::new(m.x, 1.0 - m.y);
            kept.push(differ(irradiance(m1), irradiance(m2)));
            flipped.push(differ(irradiance(flip(m1)), irradiance(flip(m2))));
            swapped.push(differ(irradiance(glam::Vec2::new(m1.y, m1.x)), irradiance(glam::Vec2::new(m2.y, m2.x))));
        }
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let count = kept.len();
    let (kept, flipped, swapped) = (median(&mut kept), median(&mut flipped), median(&mut swapped));
    println!("{count} UV seams between coplanar triangles; median |a - b| / (a + b) of the irradiance either side: {kept:.3} as stored, {flipped:.3} with v flipped, {swapped:.3} with u and v swapped");
    assert!(count > 10_000);
    assert!(kept < 0.1 && kept * 5.0 < flipped && kept * 5.0 < swapped, "the coordinates as stored read the right texels");
}

/// The game's blocks against the export's own images, which Source 2 Viewer decoded from the same files and resized:
/// the colour of every texel the export shows (alpha ≥ 128) should differ only by WebP's loss and the resampling, and
/// alpha-tested and blended materials should keep and drop the same texels. (Opaque materials' textures are left out
/// of the alpha check: the export dropped their alpha, which in the game's files holds other data.) The textures
/// checked are the colours planned from the map export's images, which carry their image's name; the other colours
/// (a second layer's, the 3D skybox's) were planned from material parameters or another export.
#[test]
#[ignore = "reads the de_cache pack and its export"]
fn base_colour_blocks_match_the_exports_images() {
    let (pack, scene) = open();
    let i = inputs();
    let glb = std::fs::read(&i.export).unwrap();
    let parts = gltf::split(&glb).unwrap();
    let doc = gltf::parse(parts.json).unwrap();
    let buffers = gltf::buffers(&doc, parts.bin).unwrap();
    let by_name: std::collections::HashMap<&str, usize> = doc.images.iter().enumerate().filter_map(|(k, im)| Some((im.name.as_deref()?, k))).collect();
    // the alpha cutoff each texture is tested at: its alpha-tested materials' cutoff, 0.5 for blended ones
    let mut cutoff: std::collections::HashMap<u32, f32> = std::collections::HashMap::new();
    for m in &scene.materials {
        match (m.base_color, m.alpha) {
            (Some(t), AlphaMode::Mask { cutoff: c }) => _ = cutoff.insert(t, c),
            (Some(t), AlphaMode::Blend) => _ = cutoff.entry(t).or_insert(0.5),
            _ => {}
        }
    }
    struct Compared {
        name: String,
        colour: f64,
        colour_flipped: f64,
        alpha_agrees: Option<f64>,
    }
    let results: Vec<Compared> = scene
        .textures
        .par_iter()
        .enumerate()
        .filter(|(_, t)| t.semantic == Semantic::Color && matches!(t.source, TextureSource::ByName { .. } | TextureSource::ByTextureParams { .. }))
        .filter_map(|(k, t)| Some((k, t, *by_name.get(t.name.as_str())?)))
        .map(|(k, t, image)| {
            let view = &doc.buffer_views[doc.images[image].buffer_view.unwrap()];
            let webp = &buffers[view.buffer][view.byte_offset..view.byte_offset + view.byte_length];
            let (ww, wh, export) = textures::decode_webp(webp).unwrap();
            assert_eq!((ww, wh), (t.width, t.height), "{}", t.name);
            let (w, h, bpr) = (t.width as usize, t.height as usize, t.mips[0].bytes_per_row as usize);
            let bytes = scene.mip_bytes(pack.textures(), k as u32, 0);
            let mut game = vec![0u8; w * h * 4];
            let size = t.format.block_bytes() as usize;
            for by in 0..h / 4 {
                for bx in 0..w / 4 {
                    let block = &bytes[by * bpr + bx * size..][..size];
                    let out = &mut game[(by * 4 * w + bx * 4) * 4..];
                    match t.format {
                        TextureFormat::Bc7 => bcdec_rs::bc7(block, out, w * 4),
                        TextureFormat::Bc1 => bcdec_rs::bc1(block, out, w * 4),
                        TextureFormat::Bc3 => bcdec_rs::bc3(block, out, w * 4),
                        other => panic!("{other:?}"),
                    }
                }
            }
            let (mut colour, mut colour_flipped, mut shown, mut agree) = (0f64, 0f64, 0usize, 0usize);
            let test = cutoff.get(&(k as u32)).map(|c| (c * 255.0).round() as u8);
            for y in 0..h {
                for x in 0..w {
                    let (p, q) = ((y * w + x) * 4, ((h - 1 - y) * w + x) * 4);
                    if export[p + 3] >= 128 {
                        shown += 1;
                        colour += (0..3).map(|c| f64::from(game[p + c].abs_diff(export[p + c]))).sum::<f64>() / 3.0;
                        colour_flipped += (0..3).map(|c| f64::from(game[p + c].abs_diff(export[q + c]))).sum::<f64>() / 3.0;
                    }
                    if let Some(cut) = test {
                        agree += usize::from((game[p + 3] >= cut) == (export[p + 3] >= cut));
                    }
                }
            }
            let shown = shown.max(1) as f64;
            Compared { name: t.name.clone(), colour: colour / shown, colour_flipped: colour_flipped / shown, alpha_agrees: test.map(|_| agree as f64 / (w * h) as f64) }
        })
        .collect();
    assert!(results.len() as u32 >= scene.stats.by_name.min(1) * 100, "{} textures checked", results.len());
    let mean = results.iter().map(|r| r.colour).sum::<f64>() / results.len() as f64;
    let worst = results.iter().max_by(|a, b| a.colour.total_cmp(&b.colour)).unwrap();
    let tested: Vec<&Compared> = results.iter().filter(|r| r.alpha_agrees.is_some()).collect();
    let least = tested.iter().min_by(|a, b| a.alpha_agrees.unwrap().total_cmp(&b.alpha_agrees.unwrap())).unwrap();
    println!(
        "{} textures against the export's WebP: colour of shown texels differs by {mean:.2}/255 on average (worst {} {:.2}, which flipped would be {:.2})",
        results.len(),
        worst.name,
        worst.colour,
        worst.colour_flipped
    );
    println!(
        "{} alpha-tested or blended textures keep the export's alpha decisions on {:.2}% of texels on average, {:.2}% at least ({})",
        tested.len(),
        100.0 * tested.iter().map(|r| r.alpha_agrees.unwrap()).sum::<f64>() / tested.len() as f64,
        100.0 * least.alpha_agrees.unwrap(),
        least.name
    );
    // a tiling texture can look much the same upside down, and a decal of one colour exactly the same (its shape is
    // its alpha, checked below), so each is only asked not to be closer upside down
    for r in results.iter().filter(|r| r.colour * 2.0 >= r.colour_flipped) {
        println!("  close either way up: {} {:.2} as stored, {:.2} flipped", r.name, r.colour, r.colour_flipped);
    }
    let flipped = results.iter().map(|r| r.colour_flipped).sum::<f64>() / results.len() as f64;
    println!("  upside down the average would be {flipped:.2}/255");
    assert!(mean < 6.0 && mean * 3.0 < flipped);
    assert!(results.iter().all(|r| r.colour < 20.0 && r.colour <= r.colour_flipped + 0.05), "each texture is its image, the right way up");
    assert!(tested.iter().all(|r| r.alpha_agrees.unwrap() > 0.97));
}
