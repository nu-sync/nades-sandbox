//! Search `smoke::Model`'s numbers against recorded clouds (S39): every combination of the values given, scored by
//! the whole-smoke IoU of the cloud `fill_cloud` grows from the simulated grid against the one it grows from the
//! recorded log, per map, with the clouds' sizes.
//!
//! ```text
//! cargo run --release -p nades-effects --example smoke_tune -- <cs2-throws export> [--per-map N] [--skip K]
//!     [--set field=v1,v2,...]...
//! ```
//!
//! The smokes are `effects_check`'s spread over the export, after its first `--skip` (so a search can run on smokes
//! the check does not score). Fields: volume, first_volume, spread_frames, bricks, up, down, emitters.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use glam::DVec3;
use nades_effects::measure::{cloud_at, iou};
use nades_effects::{smoke, voxels};
use nades_scene::collider::Collider;
use nades_scene::smoke::{Cloud, GRID, HALF_SPAN, NEVER, VOXEL, build_grid, cell, fill_cloud, smoke_cloud};
use rayon::prelude::*;
use rusqlite::Connection;

const MAPS: [&str; 7] = ["de_ancient", "de_anubis", "de_cache", "de_dust2", "de_inferno", "de_mirage", "de_nuke"];

struct Case {
    map: usize,
    det: DVec3,
    solid: Vec<u8>,
    recorded: Vec<bool>,
    recorded_size: [f64; 3],
}

/// Voxels, the horizontal radius holding 90% of them, and the height from the lowest to the 95th percentile
/// (`smoke_size`'s measure).
fn size(c: &Cloud, det: DVec3) -> [f64; 3] {
    let (mut r, mut z) = (Vec::new(), Vec::new());
    for zz in 0..GRID {
        for y in 0..GRID {
            for x in 0..GRID {
                if c.arrival[cell(x, y, zz)] != NEVER {
                    let p = c.voxel_center(x, y, zz);
                    r.push((p - det).truncate().length());
                    z.push(p.z);
                }
            }
        }
    }
    r.sort_by(f64::total_cmp);
    z.sort_by(f64::total_cmp);
    let q = |v: &[f64], f: f64| v[((v.len() - 1) as f64 * f) as usize];
    [c.count as f64, q(&r, 0.9), q(&z, 0.95) - z[0] + VOXEL]
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn set(model: &mut smoke::Model, field: &str, v: f64) -> Result<()> {
    match field {
        "volume" => model.volume = v,
        "first_volume" => model.first_volume = v,
        "spread_frames" => model.spread_frames = v,
        "bricks" => model.bricks = v as usize,
        "up" => model.up = v,
        "down" => model.down = v,
        "emitters" => model.emitters = v as usize,
        _ => bail!("unknown field {field}"),
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let db = Connection::open(args.first().context("usage: smoke_tune <export> [--per-map N] [--skip K] [--set field=v1,v2]")?)?;
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let per: usize = flag("--per-map").map_or(Ok(60), |v| v.parse())?;
    let skip: usize = flag("--skip").map_or(Ok(0), |v| v.parse())?;
    let mut axes: Vec<(String, Vec<f64>)> = Vec::new();
    for spec in args.windows(2).filter(|w| w[0] == "--set").map(|w| w[1].clone()) {
        let (field, values) = spec.split_once('=').context("--set takes field=v1,v2,...")?;
        axes.push((field.to_string(), values.split(',').map(str::parse).collect::<Result<_, _>>()?));
    }

    let mut cases = Vec::new();
    for (m, map) in MAPS.iter().enumerate() {
        let packs = std::env::var_os("NADES_PACKS").map_or_else(|| Path::new(&std::env::var("HOME").unwrap_or_default()).join(".cache/nades/packs"), PathBuf::from);
        let pack = nades_pack::Pack::open(&packs.join(format!("{map}.pack")))?;
        let collider = Collider::from_awmh(pack.collider().context("no collider")?).context("collider")?;
        let mut q = db.prepare("select s.x, s.y, s.z, s.bytes from smoke_voxels s join throws t using (throw) join demos d using (demo) where d.map = ?1 order by (s.throw * 2654435761) % 1000003 limit ?2")?;
        let rows: Vec<(DVec3, Vec<u8>)> = q.query_map(rusqlite::params![map, ((per + skip) * 2) as i64], |r| Ok((DVec3::new(r.get(0)?, r.get(1)?, r.get(2)?), r.get(3)?)))?.collect::<Result<_, _>>()?;
        let rows: Vec<_> = rows.into_iter().filter(|(_, l)| build_grid(l).is_some_and(|g| g.frames >= 200 && !g.emitters.is_empty())).skip(skip).take(per).collect();
        let mut more: Vec<Case> = rows
            .par_iter()
            .filter_map(|(det, log)| {
                let cloud = smoke_cloud(log, *det)?;
                let (recorded, recorded_size) = (cloud_at(Some(&cloud), None), size(&cloud, *det));
                let solid = voxels::surface(*det - DVec3::splat(HALF_SPAN), &collider, voxels::SAMPLES, 0.0);
                Some(Case { map: m, det: *det, solid, recorded, recorded_size })
            })
            .collect();
        eprintln!("{map}: {} smokes", more.len());
        cases.append(&mut more);
    }

    let rec = [0, 1, 2].map(|k| median(cases.iter().map(|c| c.recorded_size[k]).collect()));
    println!("recorded: {:.0} voxels, 90% within {:.0}, height {:.0} (medians over all maps)", rec[0], rec[1], rec[2]);
    let mut combos = vec![smoke::Model::default()];
    for (field, values) in &axes {
        combos = combos
            .iter()
            .flat_map(|m| values.iter().map(move |&v| (*m, v)))
            .map(|(mut m, v)| set(&mut m, field, v).map(|()| m))
            .collect::<Result<_>>()?;
    }
    println!("{:<62} {:>6} {}", "model", "mean", MAPS.map(|m| format!("{:>6}", &m[3..])).join(" "));
    for model in combos {
        let scored: Vec<(usize, f64, [f64; 3])> = cases
            .par_iter()
            .map(|c| {
                let cloud = fill_cloud(&smoke::grid(&c.solid, &model), c.det);
                let s = cloud.as_ref().map_or([0.0; 3], |cl| size(cl, c.det));
                (c.map, iou(&cloud_at(cloud.as_ref(), None), &c.recorded), s)
            })
            .collect();
        let per_map: Vec<f64> = (0..MAPS.len())
            .map(|m| {
                let v: Vec<f64> = scored.iter().filter(|s| s.0 == m).map(|s| s.1).collect();
                v.iter().sum::<f64>() / v.len().max(1) as f64
            })
            .collect();
        let mean = per_map.iter().sum::<f64>() / per_map.len() as f64;
        let sizes = [0, 1, 2].map(|k| median(scored.iter().map(|s| s.2[k]).collect()));
        let name = format!(
            "vol {:.0}/{:.0} sf {:.0} br {} up {:.2} dn {:.2} em {} | {:.0} {:.0} {:.0}",
            model.volume, model.first_volume, model.spread_frames, model.bricks, model.up, model.down, model.emitters, sizes[0], sizes[1], sizes[2]
        );
        println!("{name:<62} {mean:.4} {}", per_map.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>().join(" "));
    }
    Ok(())
}
