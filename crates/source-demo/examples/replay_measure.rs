//! What a replay's player data costs at each rate, and how far its interpolation strays from every tick: the
//! measurement behind docs/decisions.md S30.
//!
//!   cargo run --release -p nades-source-demo --example replay_measure -- <demo>...
//!
//! For each demo: the gaps between a player's ticks; then for strides of 2–32 ticks, the position (units) and view
//! (degrees) errors of linear interpolation between kept points against every tick, and the bytes of a
//! `without rowid` table of rows and of one blob per player and round.

use std::collections::BTreeMap;
use std::path::PathBuf;

use nades_source_demo::extract::{Options, SampleFact, extract_with};

fn lerp_angle(a: f32, b: f32, f: f32) -> f32 {
    let d = ((b - a + 540.0) % 360.0) - 180.0;
    a + d * f
}

fn angle_error(a: f32, b: f32) -> f32 {
    (((a - b + 540.0) % 360.0) - 180.0).abs()
}

fn quantiles(mut v: Vec<f32>) -> (f32, f32, f32, f32) {
    if v.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    v.sort_by(f32::total_cmp);
    let q = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
    (q(0.5), q(0.99), q(0.999), *v.last().unwrap())
}

fn size(rows: impl Fn(&rusqlite::Connection)) -> u64 {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("pragma page_size = 16384").unwrap();
    rows(&db);
    db.execute_batch("vacuum").unwrap();
    db.query_row("select page_count * page_size from pragma_page_count(), pragma_page_size()", [], |r| r.get::<_, i64>(0)).unwrap() as u64
}

fn main() -> anyhow::Result<()> {
    for demo in std::env::args_os().skip(1).map(PathBuf::from) {
        let facts = extract_with(&demo, Options { track_every: 1 }, &mut |_| {})?;
        let mut by_player: BTreeMap<u64, Vec<&SampleFact>> = BTreeMap::new();
        for t in &facts.tracks {
            by_player.entry(t.steamid).or_default().push(t);
        }
        let index: BTreeMap<u64, i64> = by_player.keys().zip(1..).map(|(id, k)| (*id, k)).collect();
        let mut gaps: BTreeMap<i32, usize> = BTreeMap::new();
        for points in by_player.values() {
            for w in points.windows(2) {
                *gaps.entry((w[1].tick - w[0].tick).min(65)).or_default() += 1;
            }
        }
        println!("{}: {} players, {} track points over {} rounds", demo.display(), by_player.len(), facts.tracks.len(), facts.rounds.len());
        println!("  tick gaps (65 = more): {gaps:?}");
        println!("  {:>6} {:>10} {:>28} {:>28} {:>10} {:>10}", "every", "points", "position p50/p99/p99.9/max", "view p50/p99/p99.9/max", "rows MB", "blobs MB");
        for every in [1, 2, 4, 8, 16, 32] {
            let (mut pos_err, mut view_err) = (Vec::new(), Vec::new());
            let mut kept_all: Vec<(u64, &SampleFact)> = Vec::new();
            for (id, points) in &by_player {
                // keep a point `every` ticks after the last kept, as the reader does; a gap of more than two strides
                // (a round's end, a respawn) is never interpolated across
                let mut kept: Vec<&SampleFact> = Vec::new();
                for p in points {
                    if kept.last().is_none_or(|k| p.tick - k.tick >= every) {
                        kept.push(p);
                    }
                }
                kept_all.extend(kept.iter().map(|k| (*id, *k)));
                let mut k = 0;
                for p in points {
                    while k + 1 < kept.len() && kept[k + 1].tick <= p.tick {
                        k += 1;
                    }
                    let (a, Some(b)) = (kept[k], kept.get(k + 1)) else { continue };
                    if b.tick - a.tick > 2 * every.max(2) || !a.alive || !b.alive || p.tick < a.tick {
                        continue;
                    }
                    let f = (p.tick - a.tick) as f32 / (b.tick - a.tick) as f32;
                    let at: [f32; 3] = std::array::from_fn(|i| a.pos[i] + (b.pos[i] - a.pos[i]) * f);
                    pos_err.push(((at[0] - p.pos[0]).powi(2) + (at[1] - p.pos[1]).powi(2) + (at[2] - p.pos[2]).powi(2)).sqrt());
                    if let (Some(ea), Some(eb), Some(ep)) = (a.eye, b.eye, p.eye) {
                        let yaw = lerp_angle(ea[1], eb[1], f);
                        let pitch = ea[0] + (eb[0] - ea[0]) * f;
                        view_err.push(angle_error(yaw, ep[1]).max((pitch - ep[0]).abs()));
                    }
                }
            }
            let rows = size(|db| {
                db.execute_batch("create table t (demo integer not null, tick integer not null, player integer not null, x real not null, y real not null, z real not null, pitch real, yaw real, alive integer not null, primary key (demo, tick, player)) without rowid").unwrap();
                let mut sorted = kept_all.clone();
                sorted.sort_by_key(|(id, p)| (p.tick, *id));
                let tx = db.unchecked_transaction().unwrap();
                let mut q = tx.prepare("insert into t values (1, ?, ?, ?, ?, ?, ?, ?, ?)").unwrap();
                for (id, p) in sorted {
                    let e = p.eye.unwrap_or_default();
                    q.execute(rusqlite::params![p.tick, index[&id], p.pos[0], p.pos[1], p.pos[2], e[0], e[1], p.alive]).unwrap();
                }
                drop(q);
                tx.commit().unwrap();
            });
            let blobs = size(|db| {
                db.execute_batch("create table t (demo integer not null, round integer not null, player integer not null, first_tick integer not null, every integer not null, points blob not null, primary key (demo, round, player)) without rowid").unwrap();
                let mut per: BTreeMap<(i32, u64), Vec<u8>> = BTreeMap::new();
                for (id, p) in &kept_all {
                    let round = facts.rounds.iter().filter(|r| r.freeze_end.is_some_and(|f| f <= p.tick)).map(|r| r.number).max().unwrap_or(0);
                    let blob = per.entry((round, *id)).or_default();
                    let e = p.eye.unwrap_or_default();
                    for v in [p.pos[0], p.pos[1], p.pos[2], e[0], e[1]] {
                        blob.extend(v.to_le_bytes());
                    }
                }
                let mut q = db.prepare("insert into t values (1, ?, ?, 0, ?, ?)").unwrap();
                for ((round, id), blob) in per {
                    q.execute(rusqlite::params![round, index[&id], every, blob]).unwrap();
                }
            });
            let (p50, p99, p999, pmax) = quantiles(pos_err);
            let (v50, v99, v999, vmax) = quantiles(view_err);
            println!(
                "  {every:>6} {:>10} {:>28} {:>28} {:>10.1} {:>10.1}",
                kept_all.len(),
                format!("{p50:.1}/{p99:.1}/{p999:.1}/{pmax:.0}"),
                format!("{v50:.2}/{v99:.1}/{v999:.1}/{vmax:.0}"),
                rows as f64 / 1e6,
                blobs as f64 / 1e6
            );
        }
    }
    Ok(())
}
