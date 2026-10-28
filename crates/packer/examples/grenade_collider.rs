//! The map's world physics reduced several ways, for the physics bench (`flight_check --colliders <dir> --variant
//! <name>`, `--surfaces <dir>`): `cargo run --release -p nades-packer --example grenade_collider -- <map> <out dir>`
//! writes `<map>.<variant>.awmh` into `<out dir>` (local files built from the game's, never committed) and lists the
//! export's physics materials with their triangle counts.
//!
//! Variants: `los` is the Collider section (awpy's reduction: clips, sky, glass dropped, clustered on 8 units);
//! `welded` keeps the same primitives unclustered; `grenade` is the GrenadeCollider section (welded, grenade clips
//! kept, [`physics::grenade_from_glb`]). Also `<map>.surfaces/<material>.awmh`: each material a grenade meets, alone.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use nades_packer::{Inputs, gltf, physics};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let map = args.next().context("usage: grenade_collider <map> <out dir>")?;
    let out = PathBuf::from(args.next().context("usage: grenade_collider <map> <out dir>")?);
    std::fs::create_dir_all(&out)?;
    let inputs = Inputs::for_map(&map)?;
    let glb = physics::glb_of_map(inputs.cli().with_context(|| nades_packer::no_cli(&inputs))?, &inputs.vpk(), &map, &std::env::temp_dir())?;

    let parts = gltf::split(&glb)?;
    let doc = gltf::parse(parts.json)?;
    let buffers = gltf::buffers(&doc, parts.bin)?;
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    for mesh in &doc.meshes {
        for primitive in &mesh.primitives {
            let name = primitive.material.and_then(|i| doc.materials.get(i)).and_then(|m| m.name.as_deref()).or(mesh.name.as_deref()).unwrap_or("").to_string();
            let n = match primitive.indices {
                Some(i) => gltf::elements(&doc, &buffers, i)?.integers()?.len() / 3,
                None => 0,
            };
            *names.entry(name).or_default() += n;
        }
    }
    for (name, n) in &names {
        println!("{n:>8} {name}");
    }

    for (label, p) in [("los", physics::from_glb(&glb)?), ("welded", physics::reduce(&glb, &physics::is_occluder, None)?), ("grenade", physics::grenade_from_glb(&glb)?)] {
        let path = out.join(format!("{map}.{label}.awmh"));
        std::fs::write(&path, p.to_awmh())?;
        println!("{label}: {} triangles ({} exported, {} dropped) -> {}", p.triangles.len(), p.exported, p.dropped, path.display());
    }
    let dir = out.join(format!("{map}.surfaces"));
    std::fs::create_dir_all(&dir)?;
    for name in names.keys().filter(|n| physics::stops_grenades(Some(n))) {
        let p = physics::reduce(&glb, &|n| n == Some(name.as_str()), None)?;
        if !p.triangles.is_empty() {
            std::fs::write(dir.join(format!("{name}.awmh")), p.to_awmh())?;
        }
    }
    Ok(())
}
