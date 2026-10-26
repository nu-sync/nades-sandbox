//! `nades-pack <map>`: builds `~/.cache/nades/packs/<map>.pack` (crates/packer has the details) and prints what is in
//! it. `--export`, `--game`, `--depot`, `--decompiled` and `--out` move the inputs and the pack from their places under
//! `~/.cache/nades`.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use nades_pack::scene::{AlphaMode, Semantic, TextureFormat, TextureSource};
use nades_packer::{Inputs, Report, build};

const USAGE: &str = "usage: nades-pack <map> [--export <glb>] [--game <dir>] [--depot <dir>] [--decompiled <dir>] [--cli <Source2Viewer-CLI>] [--out <pack>]";

fn main() -> Result<()> {
    let started = Instant::now();
    let mut args = std::env::args().skip(1);
    let map = match args.next() {
        Some(map) if !map.starts_with('-') => map,
        _ => bail!("{USAGE}"),
    };
    let mut inputs = Inputs::for_map(&map)?;
    while let Some(flag) = args.next() {
        let value = PathBuf::from(args.next().with_context(|| format!("{flag} needs a value\n{USAGE}"))?);
        match flag.as_str() {
            "--export" => inputs.export = value,
            "--game" => inputs.game = value,
            "--depot" => inputs.depot = value,
            "--decompiled" => inputs.decompiled = value,
            "--cli" => inputs.cli = value,
            "--out" => inputs.out = value,
            _ => bail!("unknown flag {flag}\n{USAGE}"),
        }
    }
    let report = build(&inputs)?;
    print(&inputs, &report, started.elapsed().as_secs_f64());
    Ok(())
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

fn print(inputs: &Inputs, r: &Report, seconds: f64) {
    let s = &r.scene;
    let st = &s.stats;
    println!("{}: {} ({}) in {seconds:.2} s", inputs.map, inputs.out.display(), mb(r.file_size));
    println!("  source: game build {}, export {} (fnv1a {})", r.source.game_build, inputs.export.display(), r.source.export_hash);
    println!(
        "  draws {} and 3D skybox {} ({} instanced, {} instances), {} triangles drawn, {} vertices stored",
        st.draws, st.skybox_draws, st.instanced_draws, st.instances, st.triangles, st.vertices
    );
    println!(
        "  lightmap coordinates: TEXCOORD_2 {}, TEXCOORD_1 {}, none {}; the web view's sampled check differs on {}; mirrored draws rewound {}",
        st.lightmapped_texcoord2, st.lightmapped_texcoord1, st.unlit, st.lightmap_rule_differs_from_web, st.mirrored
    );
    if r.texcoord2_outside > 0 || r.missing_uv0 > 0 || r.missing_frames > 0 {
        println!(
            "  note: {} draws have TEXCOORD_2 outside [0, 1]; {} primitives have no TEXCOORD_0 (drawn with uv0 = 0); {} lack a normal or tangent",
            r.texcoord2_outside, r.missing_uv0, r.missing_frames
        );
    }
    let alpha = |f: fn(&AlphaMode) -> bool| s.materials.iter().filter(|m| f(&m.alpha)).count();
    println!(
        "  materials {} (opaque {}, alpha-test {}, blend {}; double-sided {}; without a base colour texture {})",
        s.materials.len(),
        alpha(|a| matches!(a, AlphaMode::Opaque)),
        alpha(|a| matches!(a, AlphaMode::Mask { .. })),
        alpha(|a| matches!(a, AlphaMode::Blend)),
        s.materials.iter().filter(|m| m.double_sided).count(),
        s.materials.iter().filter(|m| m.base_color.is_none()).count()
    );
    let [generic, environment, layered, effects, unlit, black] = st.shading;
    println!("  shading: generic {generic}, environment {environment}, two-layer {layered}, effects {effects}, unlit {unlit}, black {black}");
    println!(
        "  base colour images {} of the export's {}: by name {}, through TextureParams {}, from the export's WebP {}",
        st.base_color_images, st.images, st.by_name, st.by_texture_params, st.by_fallback
    );
    println!(
        "  material textures: from the game's files {}, from the export's images {}, missing {} (pak01: {} entries)",
        st.material_textures_game, st.material_textures_export, st.material_textures_missing, r.pak_entries
    );
    let mut formats: Vec<(TextureFormat, bool, usize, u64)> = Vec::new();
    for t in &s.textures {
        match formats.iter_mut().find(|f| f.0 == t.format && f.1 == t.srgb) {
            Some(f) => {
                f.2 += 1;
                f.3 += t.bytes();
            }
            None => formats.push((t.format, t.srgb, 1, t.bytes())),
        }
    }
    let formats: Vec<String> = formats.iter().map(|(f, srgb, n, bytes)| format!("{f:?}{} ×{n} {}", if *srgb { " sRGB" } else { "" }, mb(*bytes))).collect();
    println!("  textures {}: {}", s.textures.len(), formats.join(", "));
    let mut semantics: Vec<(Semantic, usize, u64)> = Vec::new();
    for t in &s.textures {
        match semantics.iter_mut().find(|f| f.0 == t.semantic) {
            Some(f) => {
                f.1 += 1;
                f.2 += t.bytes();
            }
            None => semantics.push((t.semantic, 1, t.bytes())),
        }
    }
    let semantics: Vec<String> = semantics.iter().map(|(k, n, bytes)| format!("{k:?} ×{n} {}", mb(*bytes))).collect();
    println!("  by role: {}", semantics.join(", "));
    println!("  chunks {}", s.chunks.len());
    let describe = |t: u32| {
        let t = &s.textures[t as usize];
        let from = match &t.source {
            TextureSource::MapVpk { entry, .. } => entry.clone(),
            TextureSource::ByTextureParams { file, .. } => file.clone(),
            _ => "?".into(),
        };
        let mips: Vec<String> = t.mips.iter().take(2).map(|m| format!("{}x{}", m.width, m.height)).collect();
        format!("{:?} {}{} ({}) from {from}", t.format, mips.join(" + "), if t.mips.len() > 2 { format!(" + {} more", t.mips.len() - 2) } else { String::new() }, mb(t.bytes()))
    };
    for (what, l) in [("lightmaps", s.lightmaps), ("3D skybox lightmaps", s.skybox.as_ref().and_then(|k| k.lightmaps))] {
        if let Some(l) = l {
            println!("  {what}: irradiance {}", describe(l.irradiance));
            for (name, t) in [("directional", l.directional), ("shadows", l.shadows)] {
                println!("    {name}: {}", t.map_or("none".to_string(), describe));
            }
            println!("    the sun's shadow: {}", l.sun_channel.map_or("none".to_string(), |c| format!("channel {c}")));
        }
    }
    match r.game_meshes {
        Some((lit, all)) => println!(
            "  the game's meshes: {lit} of {all} lightmapped; {} primitives lit by probes that the UV rule would have lightmapped; {} of meshes it does not name",
            st.probe_lit_by_game, st.lightmapped_unknown
        ),
        None => println!("  the game's meshes: not asked (no Source2Viewer-CLI at {}): lightmapping by the UV rule alone", inputs.cli.display()),
    }
    if let Some(p) = &s.probes {
        println!("  light probes: {} volumes; atlas {}, shadows {}", p.volumes.len(), describe(p.irradiance), p.shadows.map_or("none".into(), describe));
    }
    match &s.sky {
        Some(sky) => println!("  sky: {} cube, yaw {}°, tint {:?}", describe(sky.texture), sky.yaw, sky.tint),
        None => println!("  sky: none"),
    }
    match &s.sun {
        Some(sun) => println!("  sun: towards {:?}, colour × brightness {:?}", sun.direction, sun.color),
        None => println!("  sun: none"),
    }
    if let Some(k) = &s.skybox {
        println!("  3D skybox: 1:{} about {:?}, reference {:?}, own fog {}", k.scale, k.origin, k.reference, k.gradient_fog.is_some());
    }
    let p = &s.post;
    println!(
        "  post: tone curve {}, exposure {:?}, bloom {:?}, lookup table {}, cubemap fog {}, gradient fog {}",
        p.tonemap.is_some(),
        p.exposure.map(|e| (e.min, e.max)),
        p.bloom.map(|b| b.mode),
        p.lut.as_ref().map_or("none".into(), |l| format!("{}³", l.dim)),
        p.fog.is_some(),
        p.gradient_fog.is_some()
    );
    let sections: Vec<String> = r.sections.iter().map(|(k, n)| format!("{k:?} {}", if *n < 1_000_000 { format!("{:.1} KB", *n as f64 / 1e3) } else { mb(*n) })).collect();
    println!("  sections: {}", sections.join(", "));
    for fallback in &r.fallbacks {
        println!("  from the export's WebP: {fallback}");
    }
    for missing in r.missing.iter().take(20) {
        println!("  missing: {missing}");
    }
    if r.missing.len() > 20 {
        println!("  missing: … and {} more", r.missing.len() - 20);
    }
    for skipped in &st.skipped {
        println!("  left out: {} × {}", skipped.count, skipped.reason);
    }
    let stages: Vec<String> = r.stages.iter().map(|(name, ms)| format!("{name} {ms:.0} ms")).collect();
    println!("  stages: {} (game files indexed: {})", stages.join(", "), r.game_files);
}
