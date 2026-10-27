//! The `cs2-throws` file adapter (docs/standalone-api.md §3, §6): reads version 1 files with SQLite, read-only, and
//! merges several into one [`ThrowSource`]. A demo in two files (the same `demo_id`) is taken once, from the first file
//! that has it; another major version is refused, naming both. Tables and columns this reader does not know are
//! ignored, and optional ones a file leaves out read as empty.
//!
//! Each file stays open between reads and is opened again once it changes on disk (its size or modification time),
//! which also changes the source's [`Revision`]. A file a producer is writing answers [`SourceError::Busy`].
//!
//! [`check`] reads one file the way `nades-sandbox check` reports it: its problems, and a census of the facts it holds
//! for the feature table (nades_model::features).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use nades_model::features::Census;
use nades_model::{
    Blind, Demo, DemoId, Effect, Flame, Grenade, MapFacts, MapName, Pawn, Player, Revision, Round, Sample, Side, SmokeVoxels, SourceError, Tags, Throw, ThrowId, ThrowSource, Tick, Tracks, Vec3,
};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Row, params};

pub const FORMAT: &str = "cs2-throws";
/// The major version this reader reads; any 1.x.
pub const MAJOR: u32 = 1;
const SUPPORTED: &str = "cs2-throws 1.x";

/// The tables and columns of version 1 this reader reads: each table's first `n` columns are required when the
/// table is there. `meta`, `demos` and `throws` must be there.
const TABLES: &[(&str, usize, &[&str])] = &[
    ("meta", 2, &["key", "value"]),
    ("demos", 2, &["demo_id", "map", "played_at", "file_name"]),
    ("rounds", 2, &["demo_id", "number", "freeze_end_tick", "plant_tick", "end_tick", "winner"]),
    (
        "throws",
        8,
        &[
            "throw_id", "demo_id", "tick", "grenade", "side", "start_x", "start_y", "start_z", "round", "player", "steamid", "team", "place", "vel_x", "vel_y", "vel_z", "origin_x", "origin_y", "origin_z", "takeoff_x", "takeoff_y", "takeoff_z", "pitch", "yaw", "pvel_x", "pvel_y", "pvel_z",
            "on_ground", "duck", "walking", "detonate_tick", "det_x", "det_y", "det_z", "end_tick", "flight",
        ],
    ),
    ("smoke_voxels", 3, &["throw_id", "first_tick", "bytes", "x", "y", "z"]),
    ("flames", 7, &["throw_id", "slot", "x", "y", "z", "first_tick", "last_tick", "nx", "ny", "nz"]),
    ("blinds", 2, &["throw_id", "seconds", "player", "steamid", "side", "x", "y", "z"]),
    ("player_samples", 7, &["demo_id", "tick", "side", "x", "y", "z", "alive", "player", "steamid", "pitch", "yaw", "duck", "place"]),
    ("tags", 3, &["subject", "key", "value"]),
];

/// A flight point's bytes: i32 tick, f32 x, y, z, little-endian.
const POINT: usize = 16;

/// The columns of the known tables a file has.
#[derive(Debug, Default)]
struct Schema(HashMap<&'static str, HashSet<String>>);

impl Schema {
    fn read(conn: &Connection, path: &Path) -> Result<Self, SourceError> {
        let mut schema = Schema::default();
        for &(table, required, columns) in TABLES {
            let mut q = conn.prepare("select name from pragma_table_info(?1)").map_err(sql(path))?;
            let present: HashSet<String> = q.query_map([table], |r| r.get(0)).map_err(sql(path))?.collect::<Result<_, _>>().map_err(sql(path))?;
            if present.is_empty() {
                if matches!(table, "meta" | "demos" | "throws") {
                    return Err(SourceError::Corrupt(format!("{} has no {table} table: not a {FORMAT} file", path.display())));
                }
                continue;
            }
            if let Some(missing) = columns[..required].iter().find(|c| !present.contains(**c)) {
                return Err(SourceError::Corrupt(format!("{}: the {table} table has no {missing} column", path.display())));
            }
            schema.0.insert(table, present);
        }
        Ok(schema)
    }

    fn has_table(&self, table: &str) -> bool {
        self.0.contains_key(table)
    }

    fn has(&self, table: &str, column: &str) -> bool {
        self.0.get(table).is_some_and(|c| c.contains(column))
    }

    /// `columns` as a select list, a column the file lacks as null.
    fn cols(&self, table: &str, columns: &[&str]) -> String {
        columns.iter().map(|c| if self.has(table, c) { format!("\"{c}\"") } else { "null".into() }).collect::<Vec<_>>().join(", ")
    }
}

/// SQLite's errors as the port's: a file being written is busy, anything else names the file.
fn sql(path: &Path) -> impl Fn(rusqlite::Error) -> SourceError + '_ {
    move |e| match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => SourceError::Busy,
        Some(ErrorCode::NotADatabase) => SourceError::Corrupt(format!("{} is not a SQLite file", path.display())),
        _ => SourceError::Corrupt(format!("{}: {e}", path.display())),
    }
}

/// What a file looked like on disk when it was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn stamp(path: &Path) -> Result<Stamp, SourceError> {
    match std::fs::metadata(path) {
        Ok(m) => Ok(Stamp { len: m.len(), modified: m.modified().ok() }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(SourceError::Missing(path.to_path_buf())),
        Err(e) => Err(SourceError::Io(e)),
    }
}

/// One open file.
struct Open {
    path: PathBuf,
    conn: Connection,
    stamp: Stamp,
    schema: Schema,
    /// `meta`, as written.
    meta: Vec<(String, String)>,
}

/// The format's version as `meta` gives it, refused unless its major version is [`MAJOR`].
fn version(meta: &[(String, String)], path: &Path) -> Result<String, SourceError> {
    let value = |key: &str| meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let format = value("format").ok_or_else(|| SourceError::Corrupt(format!("{}: meta has no format", path.display())))?;
    if format != FORMAT {
        return Err(SourceError::Incompatible { found: format!("{} is a {format} file", path.display()), supported: SUPPORTED });
    }
    let version = value("version").ok_or_else(|| SourceError::Corrupt(format!("{}: meta has no version", path.display())))?;
    match version.split('.').next().and_then(|m| m.trim().parse::<u32>().ok()) {
        Some(MAJOR) => Ok(version.to_string()),
        _ => Err(SourceError::Incompatible { found: format!("{} is {FORMAT} {version}", path.display()), supported: SUPPORTED }),
    }
}

fn open_file(path: &Path) -> Result<Open, SourceError> {
    let stamp = stamp(path)?;
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dem")) {
        return Err(SourceError::Incompatible { found: format!("{} is a demo", path.display()), supported: "cs2-throws 1.x files (reading demos comes with the demo adapter)" });
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(sql(path))?;
    conn.busy_timeout(Duration::from_secs(2)).map_err(sql(path))?;
    let schema = Schema::read(&conn, path)?;
    let meta: Vec<(String, String)> = {
        let mut q = conn.prepare("select key, value from meta order by rowid").map_err(sql(path))?;
        q.query_map([], |r| Ok((text(r, 0)?.unwrap_or_default(), text(r, 1)?.unwrap_or_default()))).map_err(sql(path))?.collect::<Result<_, _>>().map_err(sql(path))?
    };
    version(&meta, path)?;
    Ok(Open { path: path.to_path_buf(), conn, stamp, schema, meta })
}

/// Whether a file found in a folder is a cs2-throws file: SQLite's header, then its meta.
fn is_throws_file(path: &Path) -> bool {
    let mut header = [0u8; 16];
    let read = std::fs::File::open(path).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut header));
    read.is_ok() && &header == b"SQLite format 3\0" && open_file(path).is_ok()
}

/// The files `paths` name: a file as it is, a folder's cs2-throws files in name order.
pub fn expand(paths: &[PathBuf]) -> Result<Vec<PathBuf>, SourceError> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(path)?.filter_map(|e| Some(e.ok()?.path())).filter(|p| p.is_file() && is_throws_file(p)).collect();
            found.sort();
            files.extend(found);
        } else if path.exists() {
            files.push(path.clone());
        } else {
            return Err(SourceError::Missing(path.clone()));
        }
    }
    Ok(files)
}

struct Inner {
    files: Vec<Open>,
    /// Each demo's file and map: the first file that has it.
    owners: HashMap<String, (usize, String)>,
}

impl Inner {
    fn owns(&self, file: usize, demo: &str) -> bool {
        self.owners.get(demo).is_some_and(|(at, _)| *at == file)
    }

    fn rebuild_owners(&mut self) -> Result<(), SourceError> {
        self.owners.clear();
        for (i, f) in self.files.iter().enumerate() {
            let mut q = f.conn.prepare("select demo_id, map from demos").map_err(sql(&f.path))?;
            let rows = q.query_map([], |r| Ok((text(r, 0)?.unwrap_or_default(), text(r, 1)?.unwrap_or_default()))).map_err(sql(&f.path))?;
            for row in rows {
                let (demo, map) = row.map_err(sql(&f.path))?;
                self.owners.entry(demo).or_insert((i, map));
            }
        }
        Ok(())
    }
}

/// Several cs2-throws files as one source.
pub struct FileSource {
    paths: Vec<PathBuf>,
    inner: Mutex<Inner>,
}

impl FileSource {
    /// The files `paths` name (folders give their cs2-throws files), each opened and checked.
    pub fn open(paths: &[PathBuf]) -> Result<Self, SourceError> {
        let paths = expand(paths)?;
        let files = paths.iter().map(|p| open_file(p)).collect::<Result<Vec<_>, _>>()?;
        let mut inner = Inner { files, owners: HashMap::new() };
        inner.rebuild_owners()?;
        Ok(Self { paths, inner: Mutex::new(inner) })
    }

    /// The files read, in the order demos are taken from them.
    pub fn files(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Each file's `meta`.
    pub fn meta(&self) -> Vec<(PathBuf, Vec<(String, String)>)> {
        let inner = self.inner.lock().expect("file source");
        inner.files.iter().map(|f| (f.path.clone(), f.meta.clone())).collect()
    }

    /// Runs `read` over the files, any changed on disk opened again first.
    fn with<T>(&self, read: impl FnOnce(&Inner) -> Result<T, SourceError>) -> Result<T, SourceError> {
        let mut inner = self.inner.lock().expect("file source");
        let mut changed = false;
        for f in &mut inner.files {
            if stamp(&f.path)? != f.stamp {
                let path = f.path.clone();
                *f = open_file(&path)?;
                changed = true;
            }
        }
        if changed {
            inner.rebuild_owners()?;
        }
        read(&inner)
    }
}

impl ThrowSource for FileSource {
    fn revision(&self) -> Result<Revision, SourceError> {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for path in &self.paths {
            (path, stamp(path)?).hash(&mut hash);
        }
        Ok(Revision(hash.finish()))
    }

    fn maps(&self) -> Result<Vec<MapName>, SourceError> {
        self.with(|inner| {
            let mut maps: Vec<String> = inner.owners.values().map(|(_, m)| m.clone()).collect::<HashSet<_>>().into_iter().collect();
            maps.sort();
            Ok(maps.into_iter().map(MapName).collect())
        })
    }

    fn map(&self, map: &MapName) -> Result<MapFacts, SourceError> {
        self.with(|inner| {
            let mut facts = MapFacts::default();
            for (i, f) in inner.files.iter().enumerate() {
                read_map(f, map.as_str(), &|demo| inner.owns(i, demo), &mut facts).map_err(sql(&f.path))?;
            }
            Ok(facts)
        })
    }

    fn map_of(&self, throw: &ThrowId) -> Result<Option<MapName>, SourceError> {
        self.with(|inner| {
            for (i, f) in inner.files.iter().enumerate() {
                let demo: Option<String> = f.conn.query_row("select demo_id from throws where throw_id = ?1", [throw.as_str()], |r| text(r, 0)).optional().map_err(sql(&f.path))?.flatten();
                if let Some(demo) = demo.filter(|d| inner.owns(i, d)) {
                    return Ok(inner.owners.get(&demo).map(|(_, m)| MapName(m.clone())));
                }
            }
            Ok(None)
        })
    }

    fn tracks(&self, throw: &ThrowId) -> Result<Option<Tracks>, SourceError> {
        self.with(|inner| {
            for (i, f) in inner.files.iter().enumerate() {
                if let Some(tracks) = read_tracks(f, throw.as_str(), &|demo| inner.owns(i, demo))? {
                    return Ok(Some(tracks));
                }
            }
            Ok(None)
        })
    }

    fn samples(&self, demo: &DemoId, ticks: RangeInclusive<Tick>) -> Result<Vec<Sample>, SourceError> {
        self.with(|inner| {
            let Some(&(i, _)) = inner.owners.get(demo.as_str()) else { return Ok(Vec::new()) };
            let f = &inner.files[i];
            if !f.schema.has_table("player_samples") {
                return Ok(Vec::new());
            }
            let columns = f.schema.cols("player_samples", &["tick", "player", "steamid", "side", "x", "y", "z", "pitch", "yaw", "duck", "alive", "place"]);
            let mut q = f.conn.prepare(&format!("select {columns} from player_samples where demo_id = ?1 and tick between ?2 and ?3 order by tick")).map_err(sql(&f.path))?;
            let rows = q.query_map(params![demo.as_str(), ticks.start().0, ticks.end().0], |r| {
                Ok((|| {
                    Some(Sample {
                        tick: Tick(int(r, 0)? as i32),
                        player: Player { name: text_at(r, 1), steamid: steamid(r, 2) },
                        side: Side::parse(text_at(r, 3)?.as_str())?,
                        origin: [real_at(r, 4)?, real_at(r, 5)?, real_at(r, 6)?],
                        pitch: real_at(r, 7),
                        yaw: real_at(r, 8),
                        duck: real_at(r, 9),
                        alive: boolean(r, 10)?,
                        place: text_at(r, 11).filter(|p| !p.is_empty()),
                    })
                })())
            });
            let rows = rows.map_err(sql(&f.path))?;
            let mut out = Vec::new();
            for row in rows {
                out.extend(row.map_err(sql(&f.path))?);
            }
            Ok(out)
        })
    }

    fn places(&self, map: &MapName) -> Result<Vec<(Vec3, String)>, SourceError> {
        self.with(|inner| {
            let mut out = Vec::new();
            for (i, f) in inner.files.iter().enumerate() {
                if !f.schema.has_table("player_samples") || !f.schema.has("player_samples", "place") {
                    continue;
                }
                let demos: Vec<String> = inner.owners.iter().filter(|(_, (at, m))| *at == i && m == map.as_str()).map(|(d, _)| d.clone()).collect();
                let mut q = f.conn.prepare("select x, y, z, place from player_samples where demo_id = ?1 and alive <> 0 and place is not null and place <> ''").map_err(sql(&f.path))?;
                for demo in demos {
                    let rows = q.query_map([demo], |r| Ok((|| Some(([real_at(r, 0)?, real_at(r, 1)?, real_at(r, 2)?], text_at(r, 3)?)))())).map_err(sql(&f.path))?;
                    for row in rows {
                        out.extend(row.map_err(sql(&f.path))?);
                    }
                }
            }
            Ok(out)
        })
    }
}

/// An integer column, also when a producer stored it as a real or as text.
fn int(r: &Row, i: usize) -> Option<i64> {
    match r.get_ref(i).ok()? {
        ValueRef::Integer(v) => Some(v),
        ValueRef::Real(v) if v.fract() == 0.0 && v.is_finite() => Some(v as i64),
        ValueRef::Text(t) => std::str::from_utf8(t).ok()?.trim().parse().ok(),
        _ => None,
    }
}

fn real_at(r: &Row, i: usize) -> Option<f32> {
    match r.get_ref(i).ok()? {
        ValueRef::Real(v) => Some(v as f32),
        ValueRef::Integer(v) => Some(v as f32),
        ValueRef::Text(t) => std::str::from_utf8(t).ok()?.trim().parse().ok(),
        _ => None,
    }
    .filter(|v: &f32| v.is_finite())
}

fn text(r: &Row, i: usize) -> rusqlite::Result<Option<String>> {
    Ok(match r.get_ref(i)? {
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Integer(v) => Some(v.to_string()),
        ValueRef::Real(v) => Some(v.to_string()),
        _ => None,
    })
}

fn text_at(r: &Row, i: usize) -> Option<String> {
    text(r, i).ok().flatten()
}

fn boolean(r: &Row, i: usize) -> Option<bool> {
    int(r, i).map(|v| v != 0)
}

fn steamid(r: &Row, i: usize) -> Option<u64> {
    int(r, i).and_then(|v| u64::try_from(v).ok()).filter(|v| *v != 0)
}

fn three(r: &Row, i: usize) -> Option<Vec3> {
    Some([real_at(r, i)?, real_at(r, i + 1)?, real_at(r, i + 2)?])
}

fn point(bytes: &[u8]) -> (Tick, Vec3) {
    let word = |k: usize| <[u8; 4]>::try_from(&bytes[k..k + 4]).expect("four bytes");
    (Tick(i32::from_le_bytes(word(0))), [f32::from_le_bytes(word(4)), f32::from_le_bytes(word(8)), f32::from_le_bytes(word(12))])
}

/// A flight blob's points; a length that is not a whole number of points is corrupt.
fn flight(bytes: &[u8]) -> Option<Vec<(Tick, Vec3)>> {
    bytes.len().is_multiple_of(POINT).then(|| bytes.chunks_exact(POINT).map(point).collect())
}

const THROW_COLUMNS: &[&str] = &[
    "throw_id", "demo_id", "round", "tick", "grenade", "side", "player", "steamid", "team", "place", "start_x", "start_y", "start_z", "vel_x", "vel_y", "vel_z", "origin_x", "origin_y", "origin_z", "takeoff_x", "takeoff_y", "takeoff_z", "pitch", "yaw", "pvel_x", "pvel_y", "pvel_z", "on_ground",
    "duck", "walking", "detonate_tick", "det_x", "det_y", "det_z", "end_tick",
];

/// A `throws` row (THROW_COLUMNS, then the flight's last point); None when its required facts do not read.
fn throw_of(r: &Row) -> Option<Throw> {
    let pawn = three(r, 16).map(|origin| Pawn {
        origin,
        takeoff: three(r, 19),
        pitch: real_at(r, 22),
        yaw: real_at(r, 23),
        velocity: three(r, 24),
        on_ground: boolean(r, 27),
        duck: real_at(r, 28),
        walking: boolean(r, 29),
    });
    let flight_end = match r.get_ref(35).ok()? {
        ValueRef::Blob(b) if b.len() == POINT => Some(point(b)),
        _ => None,
    };
    Some(Throw {
        id: ThrowId(text_at(r, 0)?),
        demo: DemoId(text_at(r, 1)?),
        round: int(r, 2).and_then(|v| u32::try_from(v).ok()),
        tick: Tick(int(r, 3)? as i32),
        grenade: Grenade::parse(text_at(r, 4)?.as_str())?,
        side: Side::parse(text_at(r, 5)?.as_str())?,
        player: Player { name: text_at(r, 6).filter(|s| !s.is_empty()), steamid: steamid(r, 7) },
        team: text_at(r, 8).filter(|s| !s.is_empty()),
        place: text_at(r, 9).filter(|s| !s.is_empty()),
        start: three(r, 10)?,
        velocity: three(r, 13),
        thrower: pawn,
        detonation: int(r, 30).and_then(|t| Some((Tick(t as i32), three(r, 31)?))),
        end: int(r, 34).map(|t| Tick(t as i32)),
        flight_end,
        tags: Vec::new(),
    })
}

fn read_map(f: &Open, map: &str, owns: &dyn Fn(&str) -> bool, facts: &mut MapFacts) -> rusqlite::Result<()> {
    let s = &f.schema;
    let mut demo_tags: HashMap<String, Tags> = HashMap::new();
    let mut throw_tags: HashMap<String, Tags> = HashMap::new();
    if s.has_table("tags") {
        let mut q = f.conn.prepare("select subject, key, value from tags where subject in (select demo_id from demos where map = ?1) order by rowid")?;
        for row in q.query_map([map], |r| Ok((text(r, 0)?, text(r, 1)?, text(r, 2)?)))? {
            if let (Some(subject), Some(key), Some(value)) = row? {
                demo_tags.entry(subject).or_default().push((key, value));
            }
        }
        let mut q = f.conn.prepare("select g.subject, g.key, g.value from tags g join throws t on t.throw_id = g.subject where t.demo_id in (select demo_id from demos where map = ?1) order by g.rowid")?;
        for row in q.query_map([map], |r| Ok((text(r, 0)?, text(r, 1)?, text(r, 2)?)))? {
            if let (Some(subject), Some(key), Some(value)) = row? {
                throw_tags.entry(subject).or_default().push((key, value));
            }
        }
    }
    let mut q = f.conn.prepare(&format!("select demo_id, map, {} from demos where map = ?1", s.cols("demos", &["played_at", "file_name"])))?;
    for row in q.query_map([map], |r| Ok((text(r, 0)?, text(r, 1)?, int(r, 2), text(r, 3)?)))? {
        let (Some(id), Some(map), played_at, file_name) = row? else { continue };
        if owns(&id) {
            let tags = demo_tags.remove(&id).unwrap_or_default();
            facts.demos.push(Demo { id: DemoId(id), map: MapName(map), played_at, file_name, tags });
        }
    }
    if s.has_table("rounds") {
        let mut q = f.conn.prepare(&format!("select demo_id, number, {} from rounds where demo_id in (select demo_id from demos where map = ?1)", s.cols("rounds", &["freeze_end_tick", "plant_tick", "end_tick", "winner"])))?;
        for row in q.query_map([map], |r| {
            Ok((|| {
                Some(Round {
                    demo: DemoId(text_at(r, 0)?),
                    number: u32::try_from(int(r, 1)?).ok()?,
                    freeze_end: int(r, 2).map(|t| Tick(t as i32)),
                    plant: int(r, 3).map(|t| Tick(t as i32)),
                    end: int(r, 4).map(|t| Tick(t as i32)),
                    winner: text_at(r, 5).and_then(|w| Side::parse(&w)),
                })
            })())
        })? {
            if let Some(round) = row?.filter(|r| owns(r.demo.as_str())) {
                facts.rounds.push(round);
            }
        }
    }
    let flight_end = if s.has("throws", "flight") { format!("case when length(flight) >= {POINT} and length(flight) % {POINT} = 0 then substr(flight, -{POINT}) end") } else { "null".into() };
    let mut q = f.conn.prepare(&format!("select {}, {flight_end} from throws where demo_id in (select demo_id from demos where map = ?1)", s.cols("throws", THROW_COLUMNS)))?;
    for row in q.query_map([map], |r| Ok(throw_of(r)))? {
        if let Some(mut throw) = row?.filter(|t| owns(t.demo.as_str())) {
            throw.tags = throw_tags.remove(throw.id.as_str()).unwrap_or_default();
            facts.throws.push(throw);
        }
    }
    Ok(())
}

fn read_tracks(f: &Open, throw: &str, owns: &dyn Fn(&str) -> bool) -> Result<Option<Tracks>, SourceError> {
    let s = &f.schema;
    let e = sql(&f.path);
    let flight_column = if s.has("throws", "flight") { "flight" } else { "null" };
    // its demo, its grenade and its flight's bytes
    type Row = (Option<String>, Option<String>, Option<Vec<u8>>);
    let row: Option<Row> = f
        .conn
        .query_row(&format!("select demo_id, grenade, {flight_column} from throws where throw_id = ?1"), [throw], |r| {
            let blob = match r.get_ref(2)? {
                ValueRef::Blob(b) => Some(b.to_vec()),
                _ => None,
            };
            Ok((text(r, 0)?, text(r, 1)?, blob))
        })
        .optional()
        .map_err(&e)?;
    let Some((Some(demo), Some(grenade), blob)) = row else { return Ok(None) };
    if !owns(&demo) {
        return Ok(None);
    }
    let grenade = Grenade::parse(&grenade).ok_or_else(|| SourceError::Corrupt(format!("{}: throw {throw} has an unknown grenade {grenade:?}", f.path.display())))?;
    let flight = match blob {
        Some(bytes) => flight(&bytes).ok_or_else(|| SourceError::Corrupt(format!("{}: throw {throw}'s flight is {} bytes, not a whole number of {POINT}-byte points", f.path.display(), bytes.len())))?,
        None => Vec::new(),
    };
    let effect = match grenade {
        Grenade::Smoke if s.has_table("smoke_voxels") => {
            let voxels = f
                .conn
                .query_row(&format!("select first_tick, bytes, {} from smoke_voxels where throw_id = ?1", s.cols("smoke_voxels", &["x", "y", "z"])), [throw], |r| {
                    let bytes = match r.get_ref(1)? {
                        ValueRef::Blob(b) => b.to_vec(),
                        _ => Vec::new(),
                    };
                    Ok(int(r, 0).map(|first| SmokeVoxels { first: Tick(first as i32), origin: three(r, 2), bytes }))
                })
                .optional()
                .map_err(&e)?
                .flatten();
            Effect::Smoke(voxels)
        }
        Grenade::Smoke => Effect::Smoke(None),
        Grenade::Molotov | Grenade::Incendiary if s.has_table("flames") => {
            let mut q = f.conn.prepare(&format!("select slot, x, y, z, first_tick, last_tick, {} from flames where throw_id = ?1 order by slot", s.cols("flames", &["nx", "ny", "nz"]))).map_err(&e)?;
            let rows = q
                .query_map([throw], |r| Ok((|| Some(Flame { slot: u32::try_from(int(r, 0)?).ok()?, pos: three(r, 1)?, first: Tick(int(r, 4)? as i32), last: Tick(int(r, 5)? as i32), normal: three(r, 6) }))()))
                .map_err(&e)?;
            Effect::Fire(rows.collect::<Result<Vec<_>, _>>().map_err(&e)?.into_iter().flatten().collect())
        }
        Grenade::Molotov | Grenade::Incendiary => Effect::Fire(Vec::new()),
        Grenade::Flash if s.has_table("blinds") => {
            let mut q = f.conn.prepare(&format!("select seconds, {} from blinds where throw_id = ?1 order by rowid", s.cols("blinds", &["player", "steamid", "side", "x", "y", "z"]))).map_err(&e)?;
            let rows = q
                .query_map([throw], |r| Ok((|| Some(Blind { seconds: real_at(r, 0)?, player: Player { name: text_at(r, 1), steamid: steamid(r, 2) }, side: text_at(r, 3).and_then(|s| Side::parse(&s)), pos: three(r, 4) }))()))
                .map_err(&e)?;
            Effect::Flash(rows.collect::<Result<Vec<_>, _>>().map_err(&e)?.into_iter().flatten().collect())
        }
        Grenade::Flash => Effect::Flash(Vec::new()),
        Grenade::He => Effect::Explosion,
        Grenade::Decoy => Effect::Nothing,
    };
    Ok(Some(Tracks { flight, effect }))
}

/// One file's problems and facts, as `nades-sandbox check` reports them.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub meta: Vec<(String, String)>,
    /// What breaks the format: a reader leaves these rows out.
    pub errors: Vec<String>,
    /// What a reader can read but is likely a producer's mistake.
    pub warnings: Vec<String>,
    pub census: Census,
}

/// Reads one file as a whole: an error when it is not a cs2-throws 1.x file at all, else its problems and census.
pub fn check(path: &Path) -> Result<Report, SourceError> {
    let f = open_file(path)?;
    let s = &f.schema;
    let e = sql(path);
    let count = |query: &str| -> Result<u64, SourceError> { f.conn.query_row(query, [], |r| r.get::<_, i64>(0)).map(|n| n as u64).map_err(&e) };
    let mut report = Report { meta: f.meta.clone(), ..Default::default() };
    let some = |cols: &[&str]| cols.iter().map(|c| if s.has("throws", c) { format!("{c} is not null") } else { "0".into() }).collect::<Vec<_>>().join(" and ");
    let c = &mut report.census;
    c.demos = count("select count(*) from demos")?;
    c.throws = count("select count(*) from throws")?;
    let has_flight = if s.has("throws", "flight") { format!("(flight is not null and length(flight) >= {POINT})") } else { "0".into() };
    c.landed = count(&format!("select count(*) from throws where ({}) or {has_flight}", some(&["detonate_tick", "det_x", "det_y", "det_z"])))?;
    c.with_player = count(&format!("select count(*) from throws where {} and player <> ''", some(&["player"])))?;
    c.with_team = count(&format!("select count(*) from throws where {} and team <> ''", some(&["team"])))?;
    c.with_steamid = count(&format!("select count(*) from throws where {}", some(&["steamid"])))?;
    if s.has("throws", "flight") {
        c.with_flight = count(&format!("select count(*) from throws where flight is not null and length(flight) >= {}", 2 * POINT))?;
    }
    c.with_view = count(&format!("select count(*) from throws where {}", some(&["origin_x", "origin_y", "origin_z", "pitch", "yaw"])))?;
    if s.has("throws", "on_ground") {
        c.airborne = count("select count(*) from throws where on_ground = 0")?;
        c.airborne_with_takeoff = count(&format!("select count(*) from throws where on_ground = 0 and {}", some(&["takeoff_x", "takeoff_y", "takeoff_z"])))?;
    }
    c.with_duck = count(&format!("select count(*) from throws where {}", some(&["duck"])))?;
    c.with_release = count(&format!("select count(*) from throws where {}", some(&["vel_x", "vel_y", "vel_z", "pitch", "yaw", "pvel_x", "pvel_y", "pvel_z", "on_ground"])))?;
    c.smokes = count("select count(*) from throws where grenade = 'smoke'")?;
    c.fires = count("select count(*) from throws where grenade in ('molotov', 'incendiary')")?;
    c.flashes = count("select count(*) from throws where grenade = 'flash'")?;
    if s.has_table("smoke_voxels") {
        c.smokes_with_voxels = count("select count(*) from throws where grenade = 'smoke' and throw_id in (select throw_id from smoke_voxels)")?;
    }
    if s.has_table("flames") {
        c.fires_with_flames = count("select count(*) from throws where grenade in ('molotov', 'incendiary') and throw_id in (select throw_id from flames)")?;
    }
    if s.has_table("blinds") {
        c.blinds = count("select count(*) from blinds")?;
    }
    if s.has_table("rounds") {
        c.demos_with_rounds = count("select count(distinct demo_id) from rounds where demo_id in (select demo_id from demos)")?;
    }
    if s.has_table("player_samples") {
        c.samples = count("select count(*) from player_samples")?;
        c.demos_with_samples = count("select count(distinct demo_id) from player_samples where demo_id in (select demo_id from demos)")?;
        if s.has("player_samples", "place") {
            c.samples_with_place = count("select count(*) from player_samples where place is not null and place <> ''")?;
        }
    }

    let unlanded = report.census.throws - report.census.landed;
    let mut problem = |error: bool, n: u64, what: String| {
        if n > 0 {
            (if error { &mut report.errors } else { &mut report.warnings }).push(format!("{n} {what}"));
        }
    };
    let grenades = Grenade::ALL.map(|g| format!("'{}'", g.name())).join(", ");
    problem(true, count(&format!("select count(*) from throws where grenade not in ({grenades}) or grenade is null"))?, format!("throws with a grenade other than {grenades}"));
    problem(true, count("select count(*) from throws where side not in ('T', 'CT') or side is null")?, "throws with a side other than 'T' or 'CT'".into());
    problem(true, count("select count(*) from throws where start_x is null or start_y is null or start_z is null or tick is null")?, "throws without a tick or a start".into());
    problem(true, count("select count(*) from throws where demo_id not in (select demo_id from demos)")?, "throws whose demo is not in demos".into());
    problem(true, unlanded, "throws with neither a detonation nor a flight to land them".into());
    problem(true, count("select count(*) from demos where map is null or map = ''")?, "demos without a map".into());
    if s.has("throws", "flight") {
        problem(true, count(&format!("select count(*) from throws where flight is not null and length(flight) % {POINT} <> 0"))?, format!("flights that are not a whole number of {POINT}-byte points"));
    }
    if s.has_table("smoke_voxels") {
        let mut q = f.conn.prepare("select bytes from smoke_voxels").map_err(&e)?;
        let logs = q.query_map([], |r| Ok(matches!(r.get_ref(0)?, ValueRef::Blob(b) if nades_model::smoke::decode(b).is_err()))).map_err(&e)?;
        let bad = logs.collect::<Result<Vec<bool>, _>>().map_err(&e)?.into_iter().filter(|b| *b).count();
        problem(true, bad as u64, "smoke voxel logs that do not decode".into());
        problem(false, count("select count(*) from smoke_voxels where throw_id not in (select throw_id from throws where grenade = 'smoke')")?, "smoke voxel logs of no smoke in throws".into());
    }
    if s.has_table("flames") {
        problem(false, count("select count(*) from flames where throw_id not in (select throw_id from throws where grenade in ('molotov', 'incendiary'))")?, "flames of no fire in throws".into());
    }
    if s.has_table("blinds") {
        problem(false, count("select count(*) from blinds where throw_id not in (select throw_id from throws where grenade = 'flash')")?, "blinds of no flash in throws".into());
    }
    if s.has_table("player_samples") {
        problem(true, count("select count(*) from player_samples where side not in ('T', 'CT') or side is null")?, "player samples with a side other than 'T' or 'CT'".into());
    }
    if s.has_table("rounds") {
        problem(false, count("select count(*) from rounds where demo_id not in (select demo_id from demos)")?, "rounds of no demo in demos".into());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flight_is_whole_points() {
        let mut bytes = Vec::new();
        for (tick, x) in [(10i32, 1.0f32), (12, 2.5)] {
            bytes.extend(tick.to_le_bytes());
            for v in [x, 0.0, -3.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        assert_eq!(flight(&bytes), Some(vec![(Tick(10), [1.0, 0.0, -3.0]), (Tick(12), [2.5, 0.0, -3.0])]));
        assert_eq!(flight(&bytes[..20]), None);
    }

    #[test]
    fn versions_are_read_by_their_major_number() {
        let meta = |version: &str| vec![("format".to_string(), FORMAT.to_string()), ("version".to_string(), version.to_string())];
        let path = Path::new("x.sqlite");
        assert_eq!(version(&meta("1.0"), path).unwrap(), "1.0");
        assert_eq!(version(&meta("1.7"), path).unwrap(), "1.7");
        match version(&meta("2.0"), path) {
            Err(SourceError::Incompatible { found, supported }) => assert_eq!((found.as_str(), supported), ("x.sqlite is cs2-throws 2.0", "cs2-throws 1.x")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(version(&[("format".into(), "nades-lineups".into())], path), Err(SourceError::Incompatible { .. })));
    }
}
