//! `nades-pack <map>`: builds `<packs>/<map>.pack` (crates/packer has the details) and prints what is in it.
//! `nades-pack --effects` builds the shared effects pack, `<packs>/shared/effects.pack`. The settings
//! (docs/standalone-api.md §8): `--packs <dir>`, `--game <dir>` (the folder holding `game/csgo`) and `--s2v <CLI>`, else
//! `NADES_SANDBOX_PACKS`, `NADES_SANDBOX_GAME`, `NADES_SANDBOX_S2V`, else the config file's `[assets] packs`,
//! `[assets] game`, `[tools] s2v`. `--export`, `--unpacked` and `--out` move the other inputs and the pack.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use nades_pack::scene::{AlphaMode, Semantic, TextureFormat, TextureSource};
use nades_packer::{Flags, Inputs, Report, build};

const USAGE: &str = "usage: nades-pack <map> | --effects | setup <map>... [--depotdownloader <path>]   [--packs <dir>] [--game <dir>] [--s2v <Source2Viewer-CLI>] [--bunx <bunx>] [--export <glb>] [--unpacked <game/csgo>] [--work <dir>] [--out <pack>]";

fn main() -> Result<()> {
    let started = Instant::now();
    let mut args = std::env::args().skip(1);
    let target = match args.next() {
        Some(t) if t == "--effects" || !t.starts_with('-') => t,
        _ => bail!("{USAGE}"),
    };
    let mut flags = Flags::default();
    let mut moved: Vec<(String, PathBuf)> = Vec::new();
    while let Some(flag) = args.next() {
        if target == "setup" && !flag.starts_with('-') {
            moved.push(("map".into(), PathBuf::from(flag)));
            continue;
        }
        let value = PathBuf::from(args.next().with_context(|| format!("{flag} needs a value\n{USAGE}"))?);
        match flag.as_str() {
            "--packs" => flags.packs = Some(value),
            "--game" => flags.game = Some(value),
            "--s2v" | "--cli" => flags.s2v = Some(value),
            "--bunx" => flags.bunx = Some(value),
            "--depotdownloader" => flags.depotdownloader = Some(value),
            "--export" | "--unpacked" | "--work" | "--out" => moved.push((flag, value)),
            _ => bail!("unknown flag {flag}\n{USAGE}"),
        }
    }
    if target == "--effects" {
        return effects(&flags, moved, started);
    }
    if target == "setup" {
        return setup(&flags, moved, started);
    }
    let mut inputs = Inputs::with(&target, &flags)?;
    for (flag, value) in moved {
        match flag.as_str() {
            "--export" => inputs.export = Some(value),
            "--unpacked" => inputs.unpacked = Some(value),
            "--work" => inputs.work = value,
            _ => inputs.out = value,
        }
    }
    let report = build(&inputs)?;
    print(&inputs, &report, started.elapsed().as_secs_f64());
    Ok(())
}

/// `setup <map>...`: what each map needs, fetched from Steam's CS2 depot into the game folder.
fn setup(flags: &Flags, moved: Vec<(String, PathBuf)>, started: Instant) -> Result<()> {
    let maps: Vec<String> = moved.iter().filter(|(f, _)| f == "map").map(|(_, m)| m.to_string_lossy().into_owned()).collect();
    if maps.is_empty() {
        bail!("usage: nades-pack setup <map>... [--depotdownloader <DepotDownloader>]\n{USAGE}");
    }
    let tool = nades_config::asset(nades_config::Asset::DepotDownloader, flags.depotdownloader.as_deref())?
        .context("DepotDownloader is not set: pass --depotdownloader <path>, or set NADES_SANDBOX_DEPOTDOWNLOADER or [tools] depotdownloader")?
        .path;
    for map in maps {
        let mut inputs = Inputs::with(&map, flags)?;
        if let Some((_, work)) = moved.iter().find(|(f, _)| f == "--work") {
            inputs.work = work.clone();
        }
        let done = nades_packer::setup::run(&inputs, &tool)?;
        println!("setup {map}: {} file(s) fetched in {:.1} s; export rounds (label, could not load, in pak01): {:?}", done.fetched.len(), started.elapsed().as_secs_f64(), done.rounds);
        for line in &done.fetched {
            println!("  fetched {line}");
        }
    }
    Ok(())
}

/// `--effects`: the game's smoke noise, fire flipbooks and grenade models, from `pak01`, into the shared effects pack.
fn effects(flags: &Flags, moved: Vec<(String, PathBuf)>, started: Instant) -> Result<()> {
    let inputs = Inputs::with("effects", flags)?;
    let cli = inputs.cli().with_context(|| nades_packer::no_cli(&inputs))?;
    let out = match moved.into_iter().find(|(f, _)| f == "--out") {
        Some((_, out)) => out,
        None => nades_config::effects_pack(&nades_config::packs(flags.packs.as_deref())?.path),
    };
    let effects = nades_packer::effects::build(cli, &inputs.pak(), &std::env::temp_dir())?;
    let bytes = effects.to_bytes();
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let source = nades_pack::Source { map: "effects".into(), game_build: nades_packer::game_build(&inputs.game), export_hash: format!("{:016x}", nades_packer::fnv1a(&bytes)) };
    nades_pack::write(&out, source.clone(), &[(nades_pack::SectionKind::Effects, &bytes)]).with_context(|| format!("writing {}", out.display()))?;
    println!("effects: {} ({}) in {:.2} s, game build {}", out.display(), mb(std::fs::metadata(&out)?.len()), started.elapsed().as_secs_f64(), source.game_build);
    println!("  smoke noise: 128³ R + 0.95 G from {}", nades_packer::effects::NOISE);
    for f in &effects.flipbooks {
        println!("  {}: {} frames in {} sequences, {}×{} cells of {} texels", f.name, f.sequences.iter().map(|s| s.1).sum::<u32>(), f.sequences.len(), f.columns, f.rows, f.cell);
    }
    for g in &effects.grenades {
        let triangles: usize = g.parts.iter().map(|p| p.indices.len() / 3).sum();
        println!("  {}: {} parts, {triangles} triangles", g.name, g.parts.len());
    }
    Ok(())
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

fn print(inputs: &Inputs, r: &Report, seconds: f64) {
    let s = &r.scene;
    let st = &s.stats;
    println!("{}: {} ({}) in {seconds:.2} s", inputs.map, inputs.out.display(), mb(r.file_size));
    println!("  source: game build {}, export {} (fnv1a {})", r.source.game_build, inputs.glb().display(), r.source.export_hash);
    for (label, e) in &r.exported {
        println!(
            "  exported {label}: {} ({:.1} s export, {:.1} s layers, {:.1} s glTF-Transform); layered: {} environment, {} lightmapped ({} with a second layer, {} a third); {} files the CLI could not load",
            e.glb.display(),
            e.seconds[0],
            e.seconds[1],
            e.seconds[2],
            e.layered[0],
            e.layered[1],
            e.layered[2],
            e.layered[3],
            e.failed_to_load.len()
        );
    }
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
        None => println!("  the game's meshes: not asked ({}): lightmapping by the UV rule alone", nades_packer::no_cli(inputs)),
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
    match &r.collider {
        Ok(c) => println!(
            "  collider: {} triangles, {} vertices, from the map's physics ({} exported, {} non-occluders dropped, {} welded)",
            c.triangles.len(),
            c.vertices.len(),
            c.exported,
            c.dropped,
            c.welded
        ),
        Err(why) => println!("  collider: none ({why}): the app has no line of sight on this map"),
    }
    if let Ok(c) = &r.grenade_collider {
        println!("  grenade collider: {} triangles, {} vertices ({} exported, {} that grenades pass dropped)", c.triangles.len(), c.vertices.len(), c.exported, c.dropped);
    }
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
