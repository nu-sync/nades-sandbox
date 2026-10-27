//! `nades_packer::build` end to end on a map made up here, so it runs anywhere: a glb with a node hierarchy, an
//! instanced node with one mirrored instance, the three lightmap-coordinate cases and three alpha modes; game files
//! with one texture found by name, one whose name is ambiguous (found through TextureParams), and an image with no
//! game file (decoded from the export); a map VPK holding the three lightmaps (BC6H irradiance, BC1 direction, BC4
//! shadows); the entity lump naming the sun's shadow channel; and a stand-in for Source 2 Viewer's CLI that says which
//! meshes the game lightmaps. The stand-in is a shell script, so the test runs where one does (not on Windows).
#![cfg(unix)]

use std::path::Path;

use nades_pack::Pack;
use nades_pack::scene::{AlphaMode, IndexFormat, Shading, TextureFormat, TextureSource, UvSet};
use nades_packer::{Inputs, build};

/// A `.vtex_c`: resource header, block table with DATA, VTEX header, then the mips smallest first. With `lz4_sizes`,
/// a COMPRESSED_MIP_SIZE entry (method 1) lists the stored sizes; without, the mips are raw.
fn vtex(format: u8, (width, height): (u16, u16), flags: u16, mips: &[Vec<u8>], lz4_sizes: Option<&[u32]>) -> Vec<u8> {
    let mut f = vec![0u8; 16];
    f[8..12].copy_from_slice(&8u32.to_le_bytes());
    f[12..16].copy_from_slice(&1u32.to_le_bytes());
    f.extend_from_slice(b"DATA");
    f.extend_from_slice(&8u32.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    let mut header = vec![0u8; 40];
    header[0..2].copy_from_slice(&1u16.to_le_bytes());
    header[2..4].copy_from_slice(&flags.to_le_bytes());
    header[20..22].copy_from_slice(&width.to_le_bytes());
    header[22..24].copy_from_slice(&height.to_le_bytes());
    header[24..26].copy_from_slice(&1u16.to_le_bytes());
    header[26] = format;
    header[27] = mips.len() as u8;
    header[36..40].copy_from_slice(&u32::from(lz4_sizes.is_some()).to_le_bytes());
    f.extend_from_slice(&header);
    if let Some(sizes) = lz4_sizes {
        for word in [4u32, 8, 0, 1, 8, sizes.len() as u32] {
            f.extend_from_slice(&word.to_le_bytes());
        }
        for s in sizes {
            f.extend_from_slice(&s.to_le_bytes());
        }
    }
    let start = f.len() as u32;
    f[0..4].copy_from_slice(&start.to_le_bytes());
    for mip in mips.iter().rev() {
        f.extend_from_slice(mip);
    }
    f
}

/// A one-file VPK version 2 with `files` stored in the directory file.
fn vpk(files: &[(&str, &str, &str, &[u8])]) -> Vec<u8> {
    let (mut tree, mut data) = (Vec::new(), Vec::new());
    for (ext, folder, name, bytes) in files {
        for s in [ext, folder, name] {
            tree.extend_from_slice(s.as_bytes());
            tree.push(0);
        }
        tree.extend_from_slice(&nades_source2::vpk::crc32(bytes).to_le_bytes());
        tree.extend_from_slice(&0u16.to_le_bytes());
        tree.extend_from_slice(&nades_source2::vpk::IN_DIRECTORY.to_le_bytes());
        tree.extend_from_slice(&(data.len() as u32).to_le_bytes());
        tree.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        tree.extend_from_slice(&0xFFFFu16.to_le_bytes());
        data.extend_from_slice(bytes);
        tree.extend_from_slice(&[0, 0]);
    }
    tree.push(0);
    let mut out = Vec::new();
    for field in [nades_source2::vpk::SIGNATURE, 2, tree.len() as u32, data.len() as u32, 0, 0, 0] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    out.extend_from_slice(&tree);
    out.extend_from_slice(&data);
    out
}

fn webp(width: u32, height: u32, seed: u8) -> Vec<u8> {
    let pixels: Vec<u8> = (0..width * height).flat_map(|i| [seed.wrapping_add((i as u8).wrapping_mul(9)), 255 - seed, (i as u8).wrapping_mul(3), if i % 2 == 0 { 255 } else { 40 }]).collect();
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(&mut out).encode(&pixels, width, height, image_webp::ColorType::Rgba8).unwrap();
    out
}

/// Appends `bytes` 4-aligned and returns a buffer view for them.
fn view(bin: &mut Vec<u8>, bytes: &[u8], stride: Option<usize>) -> String {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let at = bin.len();
    bin.extend_from_slice(bytes);
    match stride {
        Some(s) => format!(r#"{{"buffer":0,"byteOffset":{at},"byteLength":{},"byteStride":{s}}}"#, bytes.len()),
        None => format!(r#"{{"buffer":0,"byteOffset":{at},"byteLength":{}}}"#, bytes.len()),
    }
}

fn le<T: Copy, const N: usize>(values: &[T], to: fn(T) -> [u8; N]) -> Vec<u8> {
    values.iter().flat_map(|&v| to(v)).collect()
}

/// The glb: three quads (meshes 0–2), four nodes, three materials over three images.
fn glb(images: [&[u8]; 3]) -> Vec<u8> {
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut add = |bytes: Vec<u8>, stride: Option<usize>, accessor: String| {
        views.push(view(&mut bin, &bytes, stride));
        accessors.push(accessor.replace("VIEW", &(views.len() - 1).to_string()));
        accessors.len() - 1
    };
    // a unit quad in snorm16, stride 8 as glTF-Transform writes VEC3 shorts
    let quad: Vec<i16> = vec![0, 0, 0, 0, 32767, 0, 0, 0, 32767, 0, 32767, 0, 0, 0, 32767, 0];
    let position = add(le(&quad, i16::to_le_bytes), Some(8), r#"{"bufferView":VIEW,"componentType":5122,"normalized":true,"count":4,"type":"VEC3"}"#.into());
    let uv0 = add(le(&[0.0f32, 0.0, 4.0, 0.0, 4.0, 4.0, 0.0, 4.0], f32::to_le_bytes), None, r#"{"bufferView":VIEW,"componentType":5126,"count":4,"type":"VEC2"}"#.into());
    let atlas = add(le(&[0u16, 0, 32768, 0, 32768, 65535, 0, 65535], u16::to_le_bytes), None, r#"{"bufferView":VIEW,"componentType":5123,"normalized":true,"count":4,"type":"VEC2"}"#.into());
    let tiling = add(le(&[0.0f32, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0, 2.0], f32::to_le_bytes), None, r#"{"bufferView":VIEW,"componentType":5126,"count":4,"type":"VEC2"}"#.into());
    let indices = add(le(&[0u16, 1, 2, 0, 2, 3], u16::to_le_bytes), None, r#"{"bufferView":VIEW,"componentType":5123,"count":6,"type":"SCALAR"}"#.into());
    let translation = add(le(&[0.0f32, 0.0, 0.0, 5.0, 0.0, 0.0, 0.0, 0.0, 5.0], f32::to_le_bytes), None, r#"{"bufferView":VIEW,"componentType":5126,"count":3,"type":"VEC3"}"#.into());
    let rotation = add(le(&[0.0f32, 0.0, 0.0, 1.0].repeat(3), f32::to_le_bytes), None, r#"{"bufferView":VIEW,"componentType":5126,"count":3,"type":"VEC4"}"#.into());
    let scale = add(le(&[1.0f32, 1.0, 1.0, -1.0, 1.0, 1.0, 2.0, 2.0, 2.0], f32::to_le_bytes), None, r#"{"bufferView":VIEW,"componentType":5126,"count":3,"type":"VEC3"}"#.into());
    let image_views: Vec<usize> = images
        .iter()
        .map(|bytes| {
            views.push(view(&mut bin, bytes, None));
            views.len() - 1
        })
        .collect();
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},
            "buffers":[{{"byteLength":{bin_len}}}],
            "bufferViews":[{views}],
            "accessors":[{accessors}],
            "images":[{{"bufferView":{i0},"mimeType":"image/webp","name":"wall_color_psd_1.vtex"}},
                      {{"bufferView":{i1},"mimeType":"image/webp","name":"poster_color_psd_2.vtex"}},
                      {{"bufferView":{i2},"mimeType":"image/webp","name":"dup_color_psd_3.vtex"}}],
            "textures":[{{"sampler":0,"extensions":{{"EXT_texture_webp":{{"source":0}}}}}},
                        {{"sampler":0,"extensions":{{"EXT_texture_webp":{{"source":1}}}}}},
                        {{"sampler":0,"extensions":{{"EXT_texture_webp":{{"source":2}}}}}}],
            "materials":[
              {{"name":"wall","pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}}}},
                "extras":{{"vmat":{{"ShaderName":"csgo_environment.vfx","TextureParams":{{"g_tColor1":"materials/a/wall_color_psd_1.vtex"}}}}}}}},
              {{"name":"poster","pbrMetallicRoughness":{{"baseColorTexture":{{"index":1}},"baseColorFactor":[1,0.5,0.5,1]}},"alphaMode":"MASK","alphaCutoff":0.3}},
              {{"name":"glass","pbrMetallicRoughness":{{"baseColorTexture":{{"index":2}},"baseColorFactor":[0.5,0.5,1,0.25]}},"alphaMode":"BLEND","doubleSided":true,
                "extras":{{"vmat":{{"ShaderName":"csgo_glass.vfx","TextureParams":{{"g_tColor":"materials/y/dup_color_psd_3.vtex"}}}}}}}}],
            "meshes":[
              {{"name":"n0_lr0_wall_0_fragment1","primitives":[{{"attributes":{{"POSITION":{position},"TEXCOORD_0":{uv0},"TEXCOORD_1":{tiling},"TEXCOORD_2":{atlas}}},"indices":{indices},"material":0}}]}},
              {{"name":"n0_lr0_posters_0_fragment1","primitives":[{{"attributes":{{"POSITION":{position},"TEXCOORD_0":{uv0},"TEXCOORD_1":{atlas}}},"indices":{indices},"material":1}}]}},
              {{"name":"n0_lr0_glass_0_fragment1","primitives":[{{"attributes":{{"POSITION":{position},"TEXCOORD_0":{uv0},"TEXCOORD_1":{tiling}}},"indices":{indices},"material":2}}]}},
              {{"name":"n0_lr0_awning_1_fragment1","primitives":[{{"attributes":{{"POSITION":{position},"TEXCOORD_0":{uv0},"TEXCOORD_1":{atlas}}},"indices":{indices},"material":1}}]}}],
            "nodes":[
              {{"name":"parent","translation":[10,0,0],"children":[1]}},
              {{"name":"wall","mesh":0,"translation":[0,1,0],"scale":[2,2,2]}},
              {{"name":"posters","mesh":1,"extensions":{{"EXT_mesh_gpu_instancing":{{"attributes":{{"TRANSLATION":{translation},"ROTATION":{rotation},"SCALE":{scale}}}}}}}}},
              {{"name":"glass","mesh":2}},
              {{"name":"awning","mesh":3,"translation":[0,3,0]}}],
            "scenes":[{{"nodes":[0,2,3,4]}}],"scene":0}}"#,
        bin_len = bin.len(),
        views = views.join(","),
        accessors = accessors.join(","),
        i0 = image_views[0],
        i1 = image_views[1],
        i2 = image_views[2],
    );
    let mut json = json.into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let mut out = b"glTF".to_vec();
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn builds_a_pack_from_an_export_the_game_files_and_the_maps_vpk() {
    let root = std::env::temp_dir().join(format!("nades-packer-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let game = root.join("game/csgo");
    // 8x8 BC7 game textures with two mips (64 + 16 bytes); the two "dup" files differ, so the right one shows
    let blocks = |seed: u8| [(0..64).map(|i| seed.wrapping_add(i)).collect::<Vec<u8>>(), vec![seed; 16]];
    let wall = blocks(10);
    let wall_lz4 = lz4_flex::block::compress(&wall[0]);
    write(&game.join("materials/a/wall_color_psd_1.vtex_c"), &vtex(20, (8, 8), 0, &[wall_lz4.clone(), wall[1].clone()], Some(&[wall_lz4.len() as u32, 16])));
    write(&game.join("materials/x/dup_color_psd_3.vtex_c"), &vtex(20, (8, 8), 0, &blocks(100), Some(&[64, 16])));
    let dup = blocks(200);
    write(&game.join("materials/y/dup_color_psd_3.vtex_c"), &vtex(20, (8, 8), 0, &dup, Some(&[64, 16])));
    // the lightmaps: an 8x8 BC6H array of one layer with raw mips, a BC1 direction and a BC4 shadow mask, in the
    // map's VPK
    let irradiance = [(0..64u8).collect::<Vec<u8>>(), (64..80u8).collect()];
    let lightmap = vtex(19, (8, 8), nades_source2::flags::ARRAY, &irradiance, None);
    let direction = vtex(1, (8, 8), nades_source2::flags::ARRAY, &[vec![7; 32]], None);
    let shadows = vtex(27, (8, 8), nades_source2::flags::ARRAY, &[vec![9; 32]], None);
    write(
        &root.join("depot/game/csgo/maps/de_test.vpk"),
        &vpk(&[
            ("vtex_c", "maps/de_test/lightmaps", "irradiance", &lightmap),
            ("vtex_c", "maps/de_test/lightmaps", "directional_irradiance", &direction),
            ("vtex_c", "maps/de_test/lightmaps", "direct_light_shadows", &shadows),
        ]),
    );
    write(&root.join("depot/.DepotDownloader/2347770_42.manifest"), b"");
    write(&root.join("maps/de_test.glb"), &glb([&webp(8, 8, 1), &webp(4, 4, 2), &webp(8, 8, 3)]));
    // Source 2 Viewer's CLI, as far as the packer asks it: the entity lump decompiled (the sun's baked shadow is the
    // first channel), and the meshes' draw calls (the posters' mesh is lightmapped, the awning's is not)
    let cli = root.join("tools/Source2Viewer-CLI");
    write(
        &cli,
        br#"#!/bin/sh
entry=""; out=""
while [ $# -gt 0 ]; do case "$1" in -f) entry="$2"; shift;; -o) out="$2"; shift;; esac; shift; done
case "$entry" in
  *default_ents.vents_c) mkdir -p "$out/maps/de_test/entities"; printf '====1====\nbakedshadowindex "0"\nclassname "light_environment"\n' > "$out/maps/de_test/entities/default_ents.vents";;
  *worldnodes/) echo '[1/2] maps/de_test/worldnodes/n0_lr0_posters_0.vmdl_c'; echo '      m_bHasBakedLightingFromLightMap = true'; echo '[2/2] maps/de_test/worldnodes/n0_lr0_awning_1.vmdl_c';;
esac
"#,
    );
    std::fs::set_permissions(&cli, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let inputs = Inputs {
        map: "de_test".into(),
        export: Some(root.join("maps/de_test.glb")),
        work: root.join("exports"),
        bunx: None,
        game: root.join("depot"),
        unpacked: Some(game.clone()),
        cli: Some(cli),
        out: root.join("packs/de_test.pack"),
    };

    let report = build(&inputs).unwrap();
    assert_eq!(report.source.game_build, "2347770_42");
    // the stand-in CLI exports no physics: a pack without a collider, and why
    assert!(report.collider.as_ref().is_err_and(|why| why.contains("no glb")), "{:?}", report.collider.as_ref().err());
    assert!(!report.sections.iter().any(|(k, _)| *k == nades_pack::SectionKind::Collider));
    assert_eq!(report.fallbacks, ["poster_color_psd_2.vtex: no game file is named poster_color_psd_2.vtex"]);
    let pack = Pack::open(&inputs.out).unwrap();
    let scene = pack.scene().unwrap();
    assert_eq!(scene, report.scene);
    let (geometry, textures) = (pack.geometry(), pack.textures());

    // draws: the wall, the posters split into kept and mirrored instances, the awning, then the blended glass
    let names: Vec<&str> = scene.draws.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["wall", "posters", "posters", "awning", "glass"]);
    let [wall_draw, kept, mirrored, awning, glass] = &scene.draws[..] else { unreachable!() };
    // the parent's translation and the wall's own: world = parent × local
    assert_eq!(wall_draw.model, glam::Mat4::from_scale_rotation_translation(glam::Vec3::splat(2.0), glam::Quat::IDENTITY, glam::Vec3::new(10.0, 1.0, 0.0)).to_cols_array());
    assert_eq!((wall_draw.bounds.min, wall_draw.bounds.max), ([10.0, 1.0, 0.0], [12.0, 1.0, 2.0]), "a unit quad in xz, scaled by 2 and moved");
    // positions keep their quantized values, with w = 1
    assert_eq!(&wall_draw.positions.of(geometry)[8..16], &[255, 127, 0, 0, 0, 0, 255, 127]);
    // lightmap coordinates: TEXCOORD_2 wins over a tiling TEXCOORD_1; TEXCOORD_1 inside the atlas; none for tiling
    assert_eq!(wall_draw.lightmap.map(|l| l.from), Some(UvSet::Texcoord2));
    assert_eq!(kept.lightmap.map(|l| l.from), Some(UvSet::Texcoord1));
    assert_eq!(glass.lightmap, None);
    // ...unless the game says the mesh is lit by probes: the awning's TEXCOORD_1 is the same atlas-like set
    assert_eq!((awning.lightmap, awning.uv1.is_some()), (None, true));
    assert_eq!((report.game_meshes, scene.stats.probe_lit_by_game, scene.stats.lightmapped_unknown), (Some((1, 2)), 1, 2));
    // no normals or tangents in this export: up and across, 8 bytes a vertex
    assert_eq!(&wall_draw.frames.of(geometry)[..8], &[0, 127, 0, 0, 127, 0, 0, 127]);
    let lm = wall_draw.lightmap.unwrap().uv.of(geometry);
    assert_eq!(f32::from_le_bytes(lm[8..12].try_into().unwrap()), 32768.0 / 65535.0);
    assert_eq!(&wall_draw.uv0.of(geometry)[8..12], &4.0f32.to_le_bytes());
    // instances: two keep the winding, the mirrored one gets the triangles reversed
    assert_eq!((kept.instances.unwrap().count, mirrored.instances.unwrap().count), (2, 1));
    assert_eq!(kept.world_transforms(geometry)[1], glam::Mat4::from_scale_rotation_translation(glam::Vec3::splat(2.0), glam::Quat::IDENTITY, glam::Vec3::new(0.0, 0.0, 5.0)).to_cols_array());
    let triangles = |d: &nades_pack::scene::Draw| -> Vec<u16> { d.indices.of(geometry).chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect() };
    assert_eq!((triangles(kept), triangles(mirrored)), (vec![0, 1, 2, 0, 2, 3], vec![0, 2, 1, 0, 3, 2]));
    assert_eq!((kept.index_format, kept.index_count), (IndexFormat::U16, 6));
    assert_eq!(kept.positions, mirrored.positions, "a primitive's streams are stored once");

    // materials and their textures
    let alpha: Vec<AlphaMode> = scene.materials.iter().map(|m| m.alpha).collect();
    assert_eq!(alpha, [AlphaMode::Opaque, AlphaMode::Mask { cutoff: 0.3 }, AlphaMode::Blend]);
    assert!(matches!(&scene.materials[0].shading, Shading::Environment(e) if e.layers.len() == 1 && e.layers[0].color == scene.materials[0].base_color));
    assert!(matches!(scene.materials[2].shading, Shading::Generic));
    assert!(scene.materials[2].double_sided && !scene.materials[0].double_sided);
    assert_eq!(scene.materials[1].base_color_factor, [1.0, 0.5, 0.5, 1.0]);
    let texture = |m: usize| &scene.textures[scene.materials[m].base_color.unwrap() as usize];
    assert_eq!(texture(0).source, TextureSource::ByName { file: "materials/a/wall_color_psd_1.vtex_c".into(), first_mip: 0 });
    assert_eq!(texture(2).source, TextureSource::ByTextureParams { param: "g_tColor".into(), file: "materials/y/dup_color_psd_3.vtex_c".into(), first_mip: 0 });
    assert_eq!(texture(1).source, TextureSource::ExportImage { image: 1 });
    assert_eq!((texture(0).format, texture(0).srgb, texture(1).format, texture(1).mips.len()), (TextureFormat::Bc7, true, TextureFormat::Rgba8, 3));
    let bytes_of = |m: usize, mip: usize| scene.mip_bytes(textures, scene.materials[m].base_color.unwrap(), mip);
    // the game's blocks as stored (mip 0 was LZ4 in the file), each row padded to 256 bytes
    assert_eq!(&bytes_of(0, 0)[..32], &wall[0][..32]);
    assert_eq!(&bytes_of(0, 0)[256..288], &wall[0][32..]);
    assert_eq!(&bytes_of(0, 1)[..16], &wall[1][..]);
    assert_eq!(&bytes_of(2, 0)[..32], &dup[0][..32], "the TextureParams path picks materials/y");
    // the poster's WebP, decoded: its first row is the image's
    let (_, _, rgba) = nades_packer::textures::decode_webp(&webp(4, 4, 2)).unwrap();
    assert_eq!(&bytes_of(1, 0)[..16], &rgba[..16]);

    // the lightmaps: the VPK's blocks as stored (raw in the files), and the sun's shadow in the mask's first channel
    let l = scene.lightmaps.unwrap();
    assert_eq!((scene.textures[l.irradiance as usize].format, scene.textures[l.irradiance as usize].srgb), (TextureFormat::Bc6h, false));
    assert_eq!(&scene.mip_bytes(textures, l.irradiance, 0)[256..288], &irradiance[0][32..]);
    assert_eq!(&scene.mip_bytes(textures, l.irradiance, 1)[..16], &irradiance[1][..]);
    assert_eq!(&scene.mip_bytes(textures, l.directional.unwrap(), 0)[..16], &[7; 16]);
    assert_eq!(&scene.mip_bytes(textures, l.shadows.unwrap(), 0)[..16], &[9; 16]);
    assert_eq!((scene.textures[l.shadows.unwrap() as usize].format, l.sun_channel), (TextureFormat::Bc4, Some(0)));

    let s = &scene.stats;
    assert_eq!((s.images, s.base_color_images, s.by_name, s.by_texture_params, s.by_fallback), (3, 3, 1, 1, 1));
    assert_eq!((s.draws, s.instanced_draws, s.instances, s.mirrored), (5, 2, 3, 1));
    assert_eq!((s.lightmapped_texcoord2, s.lightmapped_texcoord1, s.unlit, s.lightmap_rule_differs_from_web), (1, 2, 2, 0));
    assert_eq!((s.triangles, s.vertices), (2 + 4 + 2 + 2 + 2, 16));
    assert!(s.skipped.is_empty(), "{:?}", s.skipped);
    std::fs::remove_dir_all(&root).unwrap();
}
