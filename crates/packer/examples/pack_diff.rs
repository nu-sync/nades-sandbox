//! What differs between two packs of one map: `cargo run --release -p nades-packer --example pack_diff -- <a.pack>
//! <b.pack>`. Compares the scene record by record (draws, materials, textures and their sources, the light, the sky,
//! the post-processing), the Geometry and Textures bytes, and the Collider sections.

use anyhow::Result;
use nades_pack::{Pack, SectionKind};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (a, b) = (Pack::open(args[0].as_ref())?, Pack::open(args[1].as_ref())?);
    let (sa, sb) = (a.scene()?, b.scene()?);
    println!("sources: {:?} | {:?}", a.contents().source, b.contents().source);
    println!("scene equal: {}", sa == sb);
    println!("draws equal: {} ({} | {})", sa.draws == sb.draws, sa.draws.len(), sb.draws.len());
    println!("materials equal: {} ({} | {})", sa.materials == sb.materials, sa.materials.len(), sb.materials.len());
    println!("textures: {} | {}", sa.textures.len(), sb.textures.len());
    let mut shown = 0;
    for (k, (x, y)) in sa.textures.iter().zip(&sb.textures).enumerate() {
        if x != y && shown < 15 {
            shown += 1;
            println!("  texture {k}: {} {:?} | {} {:?}", x.name, x.source, y.name, y.source);
        }
    }
    let differing = sa.textures.iter().zip(&sb.textures).filter(|(x, y)| x != y).count();
    println!("  {differing} texture records differ");
    for (what, x, y) in [("lightmaps", format!("{:?}", sa.lightmaps), format!("{:?}", sb.lightmaps)), ("probes", format!("{:?}", sa.probes), format!("{:?}", sb.probes)), ("sun", format!("{:?}", sa.sun), format!("{:?}", sb.sun)), ("sky", format!("{:?}", sa.sky), format!("{:?}", sb.sky)), ("skybox", format!("{:?}", sa.skybox.as_ref().map(|s| (s.scale, s.origin, s.reference, &s.gradient_fog, s.draws.len()))), format!("{:?}", sb.skybox.as_ref().map(|s| (s.scale, s.origin, s.reference, &s.gradient_fog, s.draws.len())))), ("post", format!("{:?}", sa.post), format!("{:?}", sb.post)), ("stats", format!("{:?}", sa.stats), format!("{:?}", sb.stats))] {
        if x == y {
            println!("{what}: equal");
        } else {
            println!("{what}: DIFFER\n  {x}\n  {y}");
        }
    }
    for kind in [SectionKind::Geometry, SectionKind::Textures, SectionKind::Collider] {
        let (x, y) = (a.section(kind), b.section(kind));
        println!("{kind:?}: {}", match (x, y) {
            (Some(x), Some(y)) if x == y => format!("equal ({} bytes)", x.len()),
            (Some(x), Some(y)) => format!("DIFFER ({} | {} bytes, {} bytes differ)", x.len(), y.len(), x.iter().zip(y).filter(|(p, q)| p != q).count()),
            (x, y) => format!("{} | {}", x.map_or("none".into(), |x| format!("{} bytes", x.len())), y.map_or("none".into(), |y| format!("{} bytes", y.len()))),
        });
    }
    Ok(())
}
