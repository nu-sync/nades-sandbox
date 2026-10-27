//! The demo adapter (docs/standalone-api.md §6): a CS2 `.dem` file, parsed with awpy, becomes one cs2-throws 2.x file in
//! a cache folder, named by the demo's SHA-256, which the file adapter then reads. A demo already in the cache is not
//! parsed again.
//!
//! The file holds facts only (§3): no tags. Ids are the format's recommendation, so another producer of the same demo
//! writes the same ones: the demo is its file's SHA-256, a throw `<sha256>:<tick>:<projectile entity index>`.
//! `played_at` is the file's modification time, which archive tools keep from the archive (HLTV's included).

pub mod extract;
pub mod tracks;

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context as _, Result, bail};
use rusqlite::{Connection, OpenFlags, Transaction, params};
use sha2::{Digest, Sha256};

pub use extract::{Facts, extract};

/// What wrote a file, as `meta.producer` says it. The reader's revision goes up whenever what it extracts changes, so a
/// cached file from an older one is parsed again.
pub const PRODUCER: &str = concat!("nades-sandbox ", env!("CARGO_PKG_VERSION"), " demo reader 1 (awpy 3.1)");

/// Whether `path` names a demo file.
pub fn is_demo(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dem"))
}

/// The demos `path` names: itself, or a folder's `.dem` files in name order.
pub fn demos_in(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.is_dir() {
        return Ok(if is_demo(path) { vec![path.to_path_buf()] } else { Vec::new() });
    }
    let mut found: Vec<PathBuf> = fs::read_dir(path)?.filter_map(|e| Some(e.ok()?.path())).filter(|p| p.is_file() && is_demo(p)).collect();
    found.sort();
    Ok(found)
}

/// The file's SHA-256, hex.
pub fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Where the cache keeps the demo with this SHA-256.
pub fn cached(cache: &Path, sha256: &str) -> PathBuf {
    cache.join(format!("{sha256}.sqlite"))
}

/// Whether `path` is this reader's file for the demo `sha256`.
fn is_current(path: &Path, sha256: &str) -> bool {
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else { return false };
    let meta = |key: &str| conn.query_row("select value from meta where key = ?1", [key], |r| r.get::<_, String>(0)).ok();
    let demo = conn.query_row("select demo_id from demos", [], |r| r.get::<_, String>(0)).ok();
    meta("format").as_deref() == Some(nades_source_file::FORMAT)
        && meta("version").as_deref() == Some(nades_source_file::VERSION)
        && meta("producer").as_deref() == Some(PRODUCER)
        && demo.as_deref() == Some(sha256)
}

/// How an import went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub demo: PathBuf,
    pub file: PathBuf,
    pub sha256: String,
    /// False when the cache already had it.
    pub parsed: bool,
    pub throws: usize,
}

/// Brings `demo` into `cache`: parses it unless the cache already has a current file for its SHA-256. `progress` hears
/// the share done, 0 to 1.
pub fn import(demo: &Path, cache: &Path, progress: &mut dyn FnMut(f32)) -> Result<Imported> {
    let sha256 = sha256(demo)?;
    let file = cached(cache, &sha256);
    if is_current(&file, &sha256) {
        progress(1.0);
        let throws = Connection::open_with_flags(&file, OpenFlags::SQLITE_OPEN_READ_ONLY)?.query_row("select count(*) from throws", [], |r| r.get::<_, i64>(0))? as usize;
        return Ok(Imported { demo: demo.to_path_buf(), file, sha256, parsed: false, throws });
    }
    let facts = extract(demo, &mut |f| progress(0.95 * f))?;
    fs::create_dir_all(cache).with_context(|| format!("creating {}", cache.display()))?;
    write(&facts, &sha256, demo, &file)?;
    progress(1.0);
    Ok(Imported { demo: demo.to_path_buf(), file, sha256, parsed: true, throws: facts.throws.len() })
}

/// Unix ms of the file's modification time.
fn modified_ms(path: &Path) -> Option<i64> {
    let t = fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(t.as_secs()).ok()?.checked_mul(1000)
}

/// Writes `facts` as a cs2-throws 2.x file at `path`: built beside it under a temporary name and renamed into place, so
/// a reader never sees part of one.
pub fn write(facts: &Facts, sha256: &str, demo: &Path, path: &Path) -> Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let build = path.with_file_name(format!(".{name}.{}.build.tmp", std::process::id()));
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    for left in [&build, &tmp] {
        let _ = fs::remove_file(left);
    }
    let written = (|| -> Result<()> {
        let mut out = Connection::open(&build).with_context(|| format!("creating {}", build.display()))?;
        out.execute_batch(&format!("pragma page_size = {}; pragma journal_mode = off; pragma synchronous = off;", nades_source_file::PAGE_SIZE))?;
        let tx = out.transaction()?;
        tx.execute_batch(nades_source_file::SCHEMA)?;
        rows(&tx, facts, sha256, demo)?;
        tx.commit()?;
        // a copy in key order fills the pages of the `without rowid` tables
        out.execute("vacuum into ?1", [tmp.to_str().context("a temporary path that is not UTF-8")?])?;
        Ok(())
    })();
    let _ = fs::remove_file(&build);
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path).with_context(|| format!("moving {} into place", path.display()))
}

/// A player as the format keys one: a Steam id and a name.
type Who = (Option<u64>, Option<String>);

fn rows(tx: &Transaction, facts: &Facts, sha256: &str, demo: &Path) -> Result<()> {
    tx.execute("insert into meta (key, value) values ('format', ?1), ('version', ?2), ('producer', ?3)", params![nades_source_file::FORMAT, nades_source_file::VERSION, PRODUCER])?;
    let file_name = demo.file_name().map(|n| n.to_string_lossy().into_owned());
    tx.execute("insert into demos (demo, demo_id, map, played_at, file_name) values (1, ?1, ?2, ?3, ?4)", params![sha256, facts.map, modified_ms(demo), file_name])?;

    // players in Steam id order, then name; places in name order
    let mut who: BTreeSet<(Option<u64>, Option<String>)> = BTreeSet::new();
    who.extend(facts.throws.iter().map(|t| (t.steamid, t.player.clone())));
    who.extend(facts.blinds.iter().map(|b| (b.steamid, b.player.clone())));
    who.extend(facts.samples.iter().map(|s| (Some(s.steamid), s.player.clone())));
    let mut ordered: Vec<Who> = who.into_iter().filter(|(s, n)| s.is_some() || n.is_some()).collect();
    ordered.sort_by(|a, b| (a.0.is_none(), a.0, a.1.is_none(), &a.1).cmp(&(b.0.is_none(), b.0, b.1.is_none(), &b.1)));
    let mut players: HashMap<Who, i64> = HashMap::new();
    let mut insert = tx.prepare("insert into players (demo, player, name, steamid) values (1, ?1, ?2, ?3)")?;
    for (key, w) in (1..).zip(ordered) {
        insert.execute(params![key, w.1, w.0.map(|s| s.to_string())])?;
        players.insert(w, key);
    }
    let player = |steamid: Option<u64>, name: &Option<String>| players.get(&(steamid, name.clone())).copied();
    let names: BTreeSet<&str> = facts.throws.iter().filter_map(|t| t.place.as_deref()).chain(facts.samples.iter().filter_map(|s| s.place.as_deref())).collect();
    let mut places: HashMap<&str, i64> = HashMap::new();
    let mut insert = tx.prepare("insert into places (place, name) values (?1, ?2)")?;
    for (key, name) in (1..).zip(names) {
        insert.execute(params![key, name])?;
        places.insert(name, key);
    }
    let place = |name: &Option<String>| name.as_deref().and_then(|n| places.get(n).copied());

    let mut insert = tx.prepare("insert into rounds (demo, number, freeze_end_tick, plant_tick, end_tick, winner) values (1, ?1, ?2, ?3, ?4, ?5)")?;
    let mut numbers = BTreeSet::new();
    for r in &facts.rounds {
        if !numbers.insert(r.number) {
            bail!("round {} is in the demo twice", r.number);
        }
        insert.execute(params![r.number, r.freeze_end, r.plant, r.end, r.winner])?;
    }

    // throws in (tick, id) order; a throw without a side breaks the format and is left out
    let mut order: Vec<(usize, String)> = facts.throws.iter().enumerate().filter(|(_, t)| t.side.is_some()).map(|(i, t)| (i, format!("{sha256}:{}:{}", t.tick, t.entity))).collect();
    order.sort_by(|a, b| (facts.throws[a.0].tick, &a.1).cmp(&(facts.throws[b.0].tick, &b.1)));
    let mut keys: HashMap<usize, i64> = HashMap::new();
    let mut insert = tx.prepare(
        "insert into throws (throw, throw_id, demo, round, tick, grenade, side, player, team, place, start_x, start_y, start_z, vel_x, vel_y, vel_z,
                             origin_x, origin_y, origin_z, takeoff_x, takeoff_y, takeoff_z, pitch, yaw, pvel_x, pvel_y, pvel_z, on_ground, duck, walking,
                             detonate_tick, det_x, det_y, det_z, end_tick, flight)
         values (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29,
                 ?30, ?31, ?32, ?33, ?34, ?35)",
    )?;
    let at = |p: Option<[f32; 3]>, i: usize| p.map(|p| p[i]);
    for (key, (i, id)) in (1..).zip(&order) {
        let t = &facts.throws[*i];
        let det = t.detonate_tick.map(|_| t.land);
        let mut flight = Vec::with_capacity(t.flight.len() * 16);
        for p in &t.flight {
            flight.extend(p.tick.to_le_bytes());
            for v in p.pos {
                flight.extend(v.to_le_bytes());
            }
        }
        insert.execute(params![
            key,
            id,
            t.round,
            t.tick,
            t.grenade(),
            t.side,
            player(t.steamid, &t.player),
            t.team,
            place(&t.place),
            t.start[0],
            t.start[1],
            t.start[2],
            t.velocity[0],
            t.velocity[1],
            t.velocity[2],
            at(t.origin, 0),
            at(t.origin, 1),
            at(t.origin, 2),
            at(t.takeoff, 0),
            at(t.takeoff, 1),
            at(t.takeoff, 2),
            at(t.eye, 0),
            at(t.eye, 1),
            at(t.pawn_velocity, 0),
            at(t.pawn_velocity, 1),
            at(t.pawn_velocity, 2),
            t.on_ground,
            t.duck,
            t.walking,
            t.detonate_tick,
            at(det, 0),
            at(det, 1),
            at(det, 2),
            t.end_tick(),
            flight,
        ])?;
        keys.insert(*i, key);
    }

    let mut voxels = tx.prepare("insert into smoke_voxels (throw, first_tick, x, y, z, bytes) values (?1, ?2, ?3, ?4, ?5, ?6)")?;
    let mut flames = tx.prepare("insert into flames (throw, slot, x, y, z, nx, ny, nz, first_tick, last_tick) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)")?;
    for (i, _) in &order {
        let (t, key) = (&facts.throws[*i], keys[i]);
        if let Some(v) = &t.voxels {
            let d = v.detonation;
            voxels.execute(params![key, v.first_tick, d[0], d[1], d[2], v.log])?;
        }
        for f in &t.flames {
            flames.execute(params![key, f.slot as i64, f.pos[0], f.pos[1], f.pos[2], at(f.normal, 0), at(f.normal, 1), at(f.normal, 2), f.first_tick, f.last_tick])?;
        }
    }
    let mut insert = tx.prepare("insert into blinds (throw, player, side, x, y, z, seconds) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
    for b in &facts.blinds {
        let Some(&key) = keys.get(&b.throw) else { continue };
        insert.execute(params![key, player(b.steamid, &b.player), b.side, at(b.pos, 0), at(b.pos, 1), at(b.pos, 2), b.seconds])?;
    }
    let mut insert = tx.prepare("insert into player_samples (demo, tick, player, side, x, y, z, pitch, yaw, duck, alive, place) values (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)")?;
    for s in &facts.samples {
        insert.execute(params![s.tick, player(Some(s.steamid), &s.player), s.side, s.pos[0], s.pos[1], s.pos[2], at(s.eye, 0), at(s.eye, 1), s.duck, s.alive, place(&s.place)])?;
    }
    Ok(())
}
