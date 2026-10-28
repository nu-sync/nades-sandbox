//! How big the clouds are: recorded logs rebuilt by `fill_cloud`, and simulated ones, per map.
//!
//! ```text
//! cargo run --release -p nades-effects --example smoke_size -- <cs2-throws export> [--per-map N]
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use glam::DVec3;
use nades_effects::{smoke, voxels};
use nades_scene::collider::Collider;
use nades_scene::smoke::{Cloud, GRID, HALF_SPAN, NEVER, VOXEL, build_grid, cell, fill_cloud, smoke_cloud};
use rayon::prelude::*;
use rusqlite::Connection;

const MAPS: [&str; 7] = ["de_ancient", "de_anubis", "de_cache", "de_dust2", "de_inferno", "de_mirage", "de_nuke"];

/// Voxels, the horizontal radius holding 90% of them, and the height from the lowest to the 95th percentile.
fn size(c: &Cloud, det: DVec3) -> (usize, f64, f64) {
    let mut r = Vec::new();
    let mut z = Vec::new();
    for zz in 0..GRID {
        for y in 0..GRID {
            for x in 0..GRID {
                if c.arrival[cell(x, y, zz)] == NEVER {
                    continue;
                }
                let p = c.origin + DVec3::new((x as f64 + 0.5) * VOXEL, (y as f64 + 0.5) * VOXEL, (zz as f64 + 0.5) * VOXEL);
                r.push((p - det).truncate().length());
                z.push(p.z);
            }
        }
    }
    r.sort_by(f64::total_cmp);
    z.sort_by(f64::total_cmp);
    let q = |v: &[f64], f: f64| v[((v.len() - 1) as f64 * f) as usize];
    (c.count, q(&r, 0.9), q(&z, 0.95) - z[0] + VOXEL)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let db = Connection::open(args.first().context("usage: smoke_size <export>")?)?;
    let per: usize = args.iter().position(|a| a == "--per-map").and_then(|i| args.get(i + 1)).map_or(40, |v| v.parse().unwrap());
    println!("map         n   recorded: voxels r90 height   simulated: voxels r90 height");
    for map in MAPS {
        let packs = std::env::var_os("NADES_PACKS").map_or_else(|| Path::new(&std::env::var("HOME").unwrap_or_default()).join(".cache/nades/packs"), PathBuf::from);
        let pack = nades_pack::Pack::open(&packs.join(format!("{map}.pack")))?;
        let collider = Collider::from_awmh(pack.collider().context("no collider")?).context("collider")?;
        let mut q = db.prepare("select s.x, s.y, s.z, s.bytes from smoke_voxels s join throws t using (throw) join demos d using (demo) where d.map = ?1 order by (s.throw * 2654435761) % 1000003 limit ?2")?;
        let rows: Vec<(DVec3, Vec<u8>)> = q.query_map(rusqlite::params![map, (per * 2) as i64], |r| Ok((DVec3::new(r.get(0)?, r.get(1)?, r.get(2)?), r.get(3)?)))?.collect::<Result<_, _>>()?;
        let rows: Vec<_> = rows.into_iter().filter(|(_, l)| build_grid(l).is_some_and(|g| g.frames >= 200 && !g.emitters.is_empty())).take(per).collect();
        let sizes: Vec<_> = rows
            .par_iter()
            .filter_map(|(det, log)| {
                let rec = size(&smoke_cloud(log, *det)?, *det);
                let solid = voxels::surface(*det - DVec3::splat(HALF_SPAN), &collider, voxels::SAMPLES, 0.0);
                let sim = size(&fill_cloud(&smoke::grid(&solid, &smoke::Model::default()), *det)?, *det);
                Some((rec, sim))
            })
            .collect();
        type Sizes = ((usize, f64, f64), (usize, f64, f64));
        let m = |f: &dyn Fn(&Sizes) -> f64| median(sizes.iter().map(f).collect());
        println!(
            "{map:<10} {:>3}   {:>15.0} {:>4.0} {:>6.0}   {:>16.0} {:>4.0} {:>6.0}",
            sizes.len(),
            m(&|s| s.0.0 as f64), m(&|s| s.0.1), m(&|s| s.0.2),
            m(&|s| s.1.0 as f64), m(&|s| s.1.1), m(&|s| s.1.2)
        );
    }
    Ok(())
}
