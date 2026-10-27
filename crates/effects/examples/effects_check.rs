//! How alike the simulated effects are to recorded ones, per map (S33).
//!
//! ```text
//! cargo run --release -p nades-effects --example effects_check -- <cs2-throws export> [--per-map N] [--twins]
//!     [--json <report.json>] [--fixture <fixture.json> [--shape <map>:<x>:<y>:<z> ...]]
//! ```
//!
//! The export is a local cs2-throws file (show-me-the-nades' `nades export --cs2-throws`), never committed; the
//! colliders are the packs' (`~/.cache/nades/packs`, or `NADES_PACKS`). For each map it reads a fixed spread of
//! recorded smokes and fires, simulates each from its recorded detonation point, and reports:
//! - smokes: voxel IoU between the cloud `fill_cloud` grows from the simulated grid and the one it grows from the
//!   recorded voxel log, over the whole smoke and at 0.5, 1, 2, 4 and 12 s; and the solid voxels found from the
//!   collider against the log's, where the log sent them;
//! - fires: of the floor the recorded flames cover, the share the simulated ones cover too, and the reverse, over the
//!   whole fire and at 0.5 and 1 s;
//! - with `--twins`, the same measures between recorded smokes and fires that went off within a unit of each other:
//!   how alike the game is to itself, the ceiling.
//!
//! `--fixture` writes the committed fixture (`fixtures/effects.json`) that `tests/fixture.rs` holds the scores with: a
//! smoke and two fires a map, as derived numbers only (the voxelized geometry around each, the recorded cloud and
//! flames). Each `--shape` adds a place a simulated smoke went wrong in the app, with the shape its cloud has now (its
//! voxel count, unrounded and rounded): the Mirage stairs smoke that drew as a cube.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use glam::DVec3;
use nades_effects::measure::{cloud_at, coverage, iou};
use nades_effects::{smoke, voxels};
use nades_scene::collider::Collider;
use nades_scene::fire::{FireKind, Flame};
use nades_scene::smoke::{CELLS, HALF_SPAN, build_grid, fill_cloud, smoke_cloud};
use rayon::prelude::*;
use rusqlite::Connection;
use serde_json::{Value, json};

const MAPS: [&str; 7] = ["de_ancient", "de_anubis", "de_cache", "de_dust2", "de_inferno", "de_mirage", "de_nuke"];
const SMOKE_TIMES: [Option<f64>; 6] = [Some(0.5), Some(1.0), Some(2.0), Some(4.0), Some(12.0), None];
const FIRE_TIMES: [Option<f64>; 3] = [Some(0.5), Some(1.0), None];

struct Smoke {
    throw_id: String,
    det: DVec3,
    log: Vec<u8>,
}

struct Fire {
    throw_id: String,
    kind: FireKind,
    det: DVec3,
    flames: Vec<Flame>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let export = args.first().context("usage: effects_check <cs2-throws export> [--per-map N] [--twins] [--json file] [--fixture file]")?;
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let per_map: usize = flag("--per-map").map_or(Ok(60), |v| v.parse())?;
    let twins = args.iter().any(|a| a == "--twins");
    let db = Connection::open(export)?;
    let mut report = Vec::new();
    let mut fixture = json!({"format": "nades-effects-fixture", "version": 1, "smokes": [], "fires": [], "shapes": []});
    let mut table = String::new();
    writeln!(table, "map         smokes  IoU full  0.5s  1s    2s    4s    12s   solid | molotov rec/sim (1 s)   incendiary rec/sim (1 s)")?;
    for map in MAPS {
        let collider = collider(map)?;
        let smokes = smokes(&db, map, per_map)?;
        let fires = fires(&db, map, per_map)?;
        let s = smoke_scores(&smokes, &collider);
        let f = fire_scores(&fires, &collider);
        let mean = |k: usize| s.iter().map(|x| x.iou[k]).sum::<f64>() / s.len().max(1) as f64;
        let solid = s.iter().map(|x| x.solid).sum::<f64>() / s.len().max(1) as f64;
        let fire_mean = |kind: FireKind, k: usize, rec: bool| {
            let v: Vec<f64> = f.iter().filter(|x| x.kind == kind).map(|x| if rec { x.rec_covered[k] } else { x.sim_covered[k] }).collect();
            v.iter().sum::<f64>() / v.len().max(1) as f64
        };
        let mut entry = json!({
            "map": map,
            "smokes": s.len(),
            "smoke_iou": SMOKE_TIMES.iter().enumerate().map(|(k, t)| (t.map_or("full".to_string(), |t| format!("{t}s")), json!(mean(k)))).collect::<serde_json::Map<_, _>>(),
            "solid_iou": solid,
        });
        for kind in [FireKind::Molotov, FireKind::Incendiary] {
            entry[kind_name(kind)] = json!({
                "fires": f.iter().filter(|x| x.kind == kind).count(),
                "recorded_covered": fire_mean(kind, 2, true), "simulated_covered": fire_mean(kind, 2, false),
                "recorded_covered_1s": fire_mean(kind, 1, true), "simulated_covered_1s": fire_mean(kind, 1, false),
            });
        }
        write!(table, "{map:<11} {:>6}  {:.3}    {:.3} {:.3} {:.3} {:.3} {:.3} {solid:.3} |", s.len(), mean(5), mean(0), mean(1), mean(2), mean(3), mean(4))?;
        for kind in [FireKind::Molotov, FireKind::Incendiary] {
            write!(table, " {:>3} {:.3}/{:.3} ({:.3}/{:.3})", f.iter().filter(|x| x.kind == kind).count(), fire_mean(kind, 2, true), fire_mean(kind, 2, false), fire_mean(kind, 1, true), fire_mean(kind, 1, false))?;
        }
        writeln!(table)?;
        if twins {
            let (smoke_twins, fire_twins) = ceiling(&db, map)?;
            entry["twins"] = json!({"smoke_iou": smoke_twins, "fire_covered": fire_twins});
            writeln!(table, "  twins: smoke IoU {:.3} ({} pairs); fire covered molotov {:.3} ({}), incendiary {:.3} ({})", smoke_twins.0, smoke_twins.1, fire_twins[0].0, fire_twins[0].1, fire_twins[1].0, fire_twins[1].1)?;
        }
        report.push(entry);
        if flag("--fixture").is_some() {
            add_to_fixture(&mut fixture, map, &smokes, &fires, &collider);
        }
        eprint!("{table}");
        table.clear();
    }
    if let Some(path) = flag("--json") {
        std::fs::write(&path, serde_json::to_string_pretty(&report)?)?;
    }
    for site in args.windows(2).filter(|w| w[0] == "--shape").map(|w| w[1].clone()) {
        let parts: Vec<&str> = site.split(':').collect();
        let [map, x, y, z] = parts[..] else { anyhow::bail!("--shape takes <map>:<x>:<y>:<z>") };
        let det = DVec3::new(x.parse()?, y.parse()?, z.parse()?);
        let solid = voxels::surface(det - DVec3::splat(HALF_SPAN), &collider(map)?, voxels::SAMPLES, 0.0);
        let count = |round: f64| fill_cloud(&smoke::grid(&solid, &smoke::Model { round, ..smoke::Model::default() }), det).map_or(0, |c| c.count);
        fixture["shapes"].as_array_mut().expect("an array").push(json!({
            "map": map, "det": det.to_array(), "solid": pack(solid.iter().map(|&v| v == 1)),
            "count": count(smoke::Model::default().round), "square_count": count(0.0),
        }));
    }
    if let Some(path) = flag("--fixture") {
        // one smoke or fire a line
        let lines = |key: &str| fixture[key].as_array().expect("an array").iter().map(|v| format!("    {v}")).collect::<Vec<_>>().join(",\n");
        let text = format!("{{\n  \"format\": \"nades-effects-fixture\",\n  \"version\": 1,\n  \"smokes\": [\n{}\n  ],\n  \"fires\": [\n{}\n  ],\n  \"shapes\": [\n{}\n  ]\n}}\n", lines("smokes"), lines("fires"), lines("shapes"));
        std::fs::write(&path, text)?;
        eprintln!("wrote {path}");
    }
    Ok(())
}

fn kind_name(kind: FireKind) -> &'static str {
    match kind {
        FireKind::Molotov => "molotov",
        FireKind::Incendiary => "incendiary",
    }
}

fn collider(map: &str) -> Result<Collider> {
    let packs = std::env::var_os("NADES_PACKS").map_or_else(|| Path::new(&std::env::var("HOME").unwrap_or_default()).join(".cache/nades/packs"), PathBuf::from);
    let path = packs.join(format!("{map}.pack"));
    let pack = nades_pack::Pack::open(&path).with_context(|| format!("opening {}", path.display()))?;
    Collider::from_awmh(pack.collider().context("the pack has no Collider section")?).context("unreadable collider")
}

/// `limit` smokes of `map` whose log runs its whole course (not removed early), a fixed spread over the export.
fn smokes(db: &Connection, map: &str, limit: usize) -> Result<Vec<Smoke>> {
    let mut q = db.prepare(
        "select t.throw_id, s.x, s.y, s.z, s.bytes from smoke_voxels s join throws t using (throw) join demos d using (demo)
         where d.map = ?1 order by (s.throw * 2654435761) % 1000003 limit ?2",
    )?;
    let rows = q.query_map(rusqlite::params![map, (limit * 2) as i64], |r| Ok(Smoke { throw_id: r.get(0)?, det: DVec3::new(r.get(1)?, r.get(2)?, r.get(3)?), log: r.get(4)? }))?;
    let all: Vec<Smoke> = rows.collect::<Result<_, _>>()?;
    Ok(all.into_iter().filter(|s| build_grid(&s.log).is_some_and(|g| g.frames >= 200 && !g.emitters.is_empty())).take(limit).collect())
}

/// `limit` fires of `map` with at least four flames that burned out on their own (not put out by a smoke).
fn fires(db: &Connection, map: &str, limit: usize) -> Result<Vec<Fire>> {
    let mut q = db.prepare(
        "select t.throw, t.throw_id, t.grenade, t.det_x, t.det_y, t.det_z, t.detonate_tick from throws t join demos d using (demo)
         where d.map = ?1 and t.grenade in ('molotov', 'incendiary') and t.det_x is not null
           and (select count(*) from flames f where f.throw = t.throw) >= 4
           and (select min(last_tick) from flames f where f.throw = t.throw) - t.detonate_tick > 4 * 64
         order by (t.throw * 2654435761) % 1000003 limit ?2",
    )?;
    type Head = (i64, String, String, DVec3, i64);
    let heads: Vec<Head> = q
        .query_map(rusqlite::params![map, limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, DVec3::new(r.get(3)?, r.get(4)?, r.get(5)?), r.get(6)?)))?
        .collect::<Result<_, _>>()?;
    let mut f = db.prepare("select x, y, z, nx, ny, nz, first_tick, last_tick from flames where throw = ?1 order by slot")?;
    let mut out = Vec::new();
    for (throw, throw_id, grenade, det, tick) in heads {
        let flames: Vec<Flame> = f
            .query_map([throw], |r| {
                let normal = [r.get::<_, Option<f64>>(3)?.unwrap_or(0.0), r.get::<_, Option<f64>>(4)?.unwrap_or(0.0), r.get::<_, Option<f64>>(5)?.unwrap_or(1.0)];
                Ok(Flame { pos: [r.get(0)?, r.get(1)?, r.get(2)?], normal, start: (r.get::<_, i64>(6)? - tick) as f64 / 64.0, end: (r.get::<_, i64>(7)? - tick) as f64 / 64.0 })
            })?
            .collect::<Result<_, _>>()?;
        let kind = if grenade == "incendiary" { FireKind::Incendiary } else { FireKind::Molotov };
        out.push(Fire { throw_id, kind, det, flames });
    }
    Ok(out)
}

struct SmokeScore {
    iou: Vec<f64>,
    solid: f64,
}

fn smoke_scores(smokes: &[Smoke], collider: &Collider) -> Vec<SmokeScore> {
    smokes
        .par_iter()
        .map(|s| {
            let recorded = smoke_cloud(&s.log, s.det);
            let solid = voxels::surface(s.det - DVec3::splat(HALF_SPAN), collider, voxels::SAMPLES, 0.0);
            let simulated = fill_cloud(&smoke::grid(&solid, &smoke::Model::default()), s.det);
            let iou_at = SMOKE_TIMES.iter().map(|&t| iou(&cloud_at(simulated.as_ref(), t), &cloud_at(recorded.as_ref(), t))).collect();
            // the collider's solid voxels against the log's, where the log sent its bricks
            let log = build_grid(&s.log).expect("checked when read");
            let mine: Vec<bool> = (0..CELLS).map(|c| log.sent[c] == 1 && solid[c] == 1).collect();
            let theirs: Vec<bool> = (0..CELLS).map(|c| log.solid[c] == 1).collect();
            SmokeScore { iou: iou_at, solid: iou(&mine, &theirs) }
        })
        .collect()
}

struct FireScore {
    kind: FireKind,
    rec_covered: Vec<f64>,
    sim_covered: Vec<f64>,
}

fn fire_scores(fires: &[Fire], collider: &Collider) -> Vec<FireScore> {
    fires
        .par_iter()
        .map(|f| {
            let simulated = nades_effects::fire_flames_of(f.kind, f.det, collider);
            let share = |a: &[Flame], b: &[Flame]| FIRE_TIMES.iter().map(|&t| coverage(a, b, t).unwrap_or(0.0)).collect();
            FireScore { kind: f.kind, rec_covered: share(&f.flames, &simulated), sim_covered: share(&simulated, &f.flames) }
        })
        .collect()
}

/// A mean and how many it is over.
type Mean = (f64, usize);

/// Recorded smokes and fires that went off within a unit of another of their kind on `map`, against each other:
/// (mean, pairs) for smokes, and for molotovs and incendiaries.
fn ceiling(db: &Connection, map: &str) -> Result<(Mean, [Mean; 2])> {
    let near = |a: DVec3, b: DVec3| a.distance(b) < 1.0;
    let smokes = smokes(db, map, 4000)?;
    let mut s = Vec::new();
    for (i, a) in smokes.iter().enumerate() {
        for b in smokes[i + 1..].iter().filter(|b| near(a.det, b.det)) {
            if s.len() < 300 {
                s.push(iou(&cloud_at(smoke_cloud(&a.log, a.det).as_ref(), None), &cloud_at(smoke_cloud(&b.log, b.det).as_ref(), None)));
            }
        }
    }
    let fires = fires(db, map, 4000)?;
    let mut f = [Vec::new(), Vec::new()];
    for (i, a) in fires.iter().enumerate() {
        for b in fires[i + 1..].iter().filter(|b| b.kind == a.kind && near(a.det, b.det)) {
            let k = usize::from(a.kind == FireKind::Incendiary);
            if f[k].len() < 300 {
                f[k].push(coverage(&a.flames, &b.flames, None).unwrap_or(0.0));
            }
        }
    }
    let mean = |v: &[f64]| (v.iter().sum::<f64>() / v.len().max(1) as f64, v.len());
    Ok((mean(&s), [mean(&f[0]), mean(&f[1])]))
}

/// A voxel set as a bitset (voxel `c` is bit `c % 8` of byte `c / 8`), LZ4-compressed with its length first, in hex.
fn pack(set: impl Iterator<Item = bool>) -> String {
    let mut bits = vec![0u8; CELLS / 8];
    for (c, on) in set.enumerate() {
        bits[c / 8] |= u8::from(on) << (c % 8);
    }
    lz4_flex::compress_prepend_size(&bits).iter().map(|b| format!("{b:02x}")).collect()
}

/// A smoke and two fires (a molotov and an incendiary) of `map` for the committed fixture, with the scores the
/// fixture's own geometry gives them (voxels at 20 units, coarser than the collider, so fires score a little lower).
fn add_to_fixture(fixture: &mut Value, map: &str, smokes: &[Smoke], fires: &[Fire], collider: &Collider) {
    for s in smokes.iter().take(1) {
        let solid = voxels::surface(s.det - DVec3::splat(HALF_SPAN), collider, voxels::SAMPLES, 0.0);
        let recorded = smoke_cloud(&s.log, s.det).expect("a recorded cloud");
        let simulated = fill_cloud(&smoke::grid(&solid, &smoke::Model::default()), s.det);
        let score = |t: Option<f64>| iou(&cloud_at(simulated.as_ref(), t), &cloud_at(Some(&recorded), t));
        fixture["smokes"].as_array_mut().expect("an array").push(json!({
            "map": map, "throw_id": s.throw_id, "det": s.det.to_array(),
            "solid": pack(solid.iter().map(|&v| v == 1)),
            "cloud": pack(cloud_at(Some(&recorded), None).into_iter()), "cloud_2s": pack(cloud_at(Some(&recorded), Some(2.0)).into_iter()),
            "iou": score(None), "iou_2s": score(Some(2.0)),
        }));
    }
    for kind in [FireKind::Molotov, FireKind::Incendiary] {
        let Some(f) = fires.iter().find(|f| f.kind == kind && f.flames.len() >= 12) else { continue };
        let origin = f.det - DVec3::splat(HALF_SPAN);
        let solid = voxels::surface(origin, collider, voxels::SAMPLES, 0.0);
        let simulated = nades_effects::fire_flames_of(kind, f.det, &voxels::collider_of(origin, &solid));
        fixture["fires"].as_array_mut().expect("an array").push(json!({
            "map": map, "throw_id": f.throw_id, "kind": kind_name(kind), "det": f.det.to_array(), "solid": pack(solid.iter().map(|&v| v == 1)),
            "flames": f.flames.iter().map(|x| [x.pos[0], x.pos[1], x.pos[2], x.start, x.end].map(|v| (v * 100.0).round() / 100.0)).collect::<Vec<_>>(),
            "recorded_covered": coverage(&f.flames, &simulated, None), "simulated_covered": coverage(&simulated, &f.flames, None),
        }));
    }
}
