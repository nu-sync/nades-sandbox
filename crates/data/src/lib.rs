//! The nades database (show-me-the-nades' `nades.duckdb`), read-only and briefly.
//!
//! `nades` locks its database file for as long as a command runs (an ingest holds it for minutes), and an open
//! connection, read-only included, makes `nades` fail; the reverse holds too. So the sandbox never keeps a connection:
//! every read opens the file read-only, runs its queries and closes it, and a file held by `nades` is `Busy`, which
//! [`Database::read_when_free`] waits out. The DuckDB here must be the one `nades` writes with (`duckdb ~1.20000.0`),
//! or the file does not open.
//!
//! The queries use show-me-the-nades' views and table macros (`lineup_report`, `throw_scene`, `scene_viewers`,
//! `team_demos`, `slice_players`) as its working tree defines them, as its web server does: `nades` writes the macros
//! into the file only when it opens it, so the file can hold older ones. Each open applies the statements of
//! `sql/views.sql` these macros need as temp objects (the temp catalog is writable read-only, and names resolve to it
//! first), skipping any this database cannot bind yet; without the file, the database's own definitions serve.
//!
//! - [`lineups`]: one map's lineups as `nades lineups` orders `lineup_report`'s rows (most thrown first), cut by side,
//!   grenade, team, player and round phase.
//! - [`throw_scene`]: everything the 3D view draws of one throw, its flight as [`PathPoint`]s and its fire as
//!   [`Flame`]s (show-me-the-nades' own types), its smoke's voxel log as networked.
//! - [`scene_viewers`]: where the other side stood and looked while the effect was up.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use duckdb::types::Value;
pub use show_me_the_nades::tracks::{Flame, PathPoint};
/// The DuckDB the reads run on, for callers that pass their own read closures.
pub use duckdb;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DataError {
    #[error("the database is in use by nades (an ingest?): try again when it finishes")]
    Busy,
    #[error("no database at {0}")]
    Missing(PathBuf),
    #[error("database: {0}")]
    Duck(#[from] duckdb::Error),
}

/// Where the database is; nothing is open between reads.
#[derive(Debug, Clone)]
pub struct Database {
    path: PathBuf,
    /// The statements of views.sql the sandbox's queries need, as temp objects; empty without the file.
    views: std::sync::Arc<Vec<String>>,
}

/// The macros this crate's queries call.
const USED: [&str; 5] = ["lineup_report", "throw_scene", "scene_viewers", "team_demos", "slice_players"];

/// Where show-me-the-nades' views are: `NADES_VIEWS_SQL` (as for its web server), else its working tree's.
pub fn views_path() -> PathBuf {
    match std::env::var_os("NADES_VIEWS_SQL") {
        Some(path) => PathBuf::from(path),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../show-me-the-nades/sql/views.sql"),
    }
}

/// The statements of `sql` (views.sql) that `names` need, directly or through each other, in the file's order, each
/// made a temp object.
pub fn needed_views(sql: &str, names: &[&str]) -> Vec<String> {
    let statements = split_statements(sql);
    let declared: Vec<Option<String>> = statements.iter().map(|s| declared_name(s)).collect();
    let refers = |body: &str, name: &str| {
        let masked = mask(body);
        let bytes = masked.as_bytes();
        masked.match_indices(name).any(|(at, _)| {
            let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
            (at == 0 || !word(bytes[at - 1])) && bytes.get(at + name.len()).is_none_or(|b| !word(*b))
        })
    };
    let mut needed = vec![false; statements.len()];
    let mut queue: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
    while let Some(body) = queue.pop() {
        for (i, name) in declared.iter().enumerate() {
            if let Some(name) = name
                && !needed[i]
                && refers(&body, name)
            {
                needed[i] = true;
                queue.push(statements[i].clone());
            }
        }
    }
    statements.iter().zip(needed).filter(|(_, n)| *n).map(|(s, _)| to_temp(s)).collect()
}

/// `create or replace view|macro` as `create or replace temp view|macro`.
fn to_temp(statement: &str) -> String {
    let lower = statement.to_ascii_lowercase();
    let mut out = statement.to_string();
    for kind in ["view", "macro"] {
        let plain = format!("create or replace {kind}");
        if let Some(at) = lower.find(&plain) {
            out.replace_range(at..at + plain.len(), &format!("create or replace temp {kind}"));
            break;
        }
    }
    out
}

/// The name a statement declares (`create or replace [temp] view|macro <name>`).
fn declared_name(statement: &str) -> Option<String> {
    let masked = mask(statement).to_ascii_lowercase();
    let words: Vec<&str> = masked.split_whitespace().collect();
    let at = words.windows(3).position(|w| w == ["create", "or", "replace"])?;
    let mut rest = words[at + 3..].iter();
    let mut kind = rest.next()?;
    if *kind == "temp" {
        kind = rest.next()?;
    }
    (*kind == "view" || *kind == "macro").then(|| rest.next().map(|n| n.split('(').next().unwrap_or(n).to_string()))?
}

/// The statement with its comments and string literals blanked, so names inside them do not count.
fn mask(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '-' if chars.peek() == Some(&'-') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '\'' => {
                out.push(' ');
                while let Some(c) = chars.next() {
                    if c == '\'' {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                            continue;
                        }
                        break;
                    }
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// The file's statements, split at semicolons outside comments and string literals.
fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        match c {
            '-' if chars.peek() == Some(&'-') => {
                for c in chars.by_ref() {
                    current.push(c);
                    if c == '\n' {
                        break;
                    }
                }
            }
            '\'' => {
                while let Some(c) = chars.next() {
                    current.push(c);
                    if c == '\'' {
                        if chars.peek() == Some(&'\'') {
                            current.push(chars.next().expect("peeked"));
                            continue;
                        }
                        break;
                    }
                }
            }
            ';' => {
                if !mask(&current).trim().trim_end_matches(';').trim().is_empty() {
                    statements.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
            _ => {}
        }
    }
    statements
}

/// How long [`Database::read_when_free`] waits between tries while `nades` holds the file.
const RETRY: Duration = Duration::from_millis(500);

impl Database {
    /// The database at `path`, read with the views and macros of show-me-the-nades' working tree ([`views_path`]).
    pub fn at(path: impl Into<PathBuf>) -> Self {
        let views = std::fs::read_to_string(views_path()).map(|sql| needed_views(&sql, &USED)).unwrap_or_default();
        Self { path: path.into(), views: std::sync::Arc::new(views) }
    }

    /// The database at `path`, read with the definitions stored in it.
    pub fn stored(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), views: std::sync::Arc::new(Vec::new()) }
    }

    /// `NADES_DB`, else show-me-the-nades' own `nades.duckdb` beside this repository.
    pub fn default_path() -> PathBuf {
        match std::env::var_os("NADES_DB") {
            Some(path) => PathBuf::from(path),
            None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../show-me-the-nades/nades.duckdb"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// When the file last changed: an ingest writes it, and the rows read before are then stale.
    pub fn modified(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path).and_then(|m| m.modified()).ok()
    }

    /// Opens the database read-only, runs `read` and closes it.
    pub fn read<T>(&self, read: impl FnOnce(&duckdb::Connection) -> duckdb::Result<T>) -> Result<T, DataError> {
        if !self.path.exists() {
            return Err(DataError::Missing(self.path.clone()));
        }
        // a few of the cores: the reads run beside the frames being drawn
        let config = duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?.threads(4)?;
        let connection = duckdb::Connection::open_with_flags(&self.path, config).map_err(busy_or)?;
        // the working tree's definitions, over the file's (one this database cannot bind yet is left to the file's)
        for statement in self.views.iter() {
            let _ = connection.execute_batch(statement);
        }
        Ok(read(&connection)?)
    }

    /// [`Database::read`], tried again every half second while `nades` holds the file, for up to `patience`.
    pub fn read_when_free<T>(&self, mut read: impl FnMut(&duckdb::Connection) -> duckdb::Result<T>, patience: Duration) -> Result<T, DataError> {
        when_free(patience, || self.read(&mut read))
    }
}

/// `attempt` until it is not `Busy`, waiting [`RETRY`] between tries, for up to `patience`.
fn when_free<T>(patience: Duration, mut attempt: impl FnMut() -> Result<T, DataError>) -> Result<T, DataError> {
    let started = Instant::now();
    loop {
        match attempt() {
            Err(DataError::Busy) if started.elapsed() + RETRY <= patience => std::thread::sleep(RETRY),
            other => return other,
        }
    }
}

/// DuckDB reports another process's lock on the file as an I/O error that names the lock.
fn busy_or(error: duckdb::Error) -> DataError {
    if error.to_string().contains("lock") { DataError::Busy } else { DataError::Duck(error) }
}

/// The grenades, as the database names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nade {
    Smoke,
    Flash,
    He,
    Molotov,
    Decoy,
}

impl Nade {
    pub const ALL: [Nade; 5] = [Nade::Smoke, Nade::Flash, Nade::He, Nade::Molotov, Nade::Decoy];

    pub fn name(self) -> &'static str {
        match self {
            Nade::Smoke => "smoke",
            Nade::Flash => "flash",
            Nade::He => "he",
            Nade::Molotov => "molotov",
            Nade::Decoy => "decoy",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|n| n.name() == name)
    }
}

/// A side, as the database names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    T,
    Ct,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::T => "T",
            Side::Ct => "CT",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "T" => Some(Side::T),
            "CT" => Some(Side::Ct),
            _ => None,
        }
    }
}

/// When in the round a throw left the hand (`v_throws.phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Early,
    Mid,
    Late,
    PostPlant,
}

impl Phase {
    pub const ALL: [Phase; 4] = [Phase::Early, Phase::Mid, Phase::Late, Phase::PostPlant];

    pub fn name(self) -> &'static str {
        match self {
            Phase::Early => "early",
            Phase::Mid => "mid",
            Phase::Late => "late",
            Phase::PostPlant => "post-plant",
        }
    }
}

/// A cut of one map's throws, as the web app's Lineups page takes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    /// `de_cache`, or `cache`: the macros take either.
    pub map: String,
    pub side: Option<Side>,
    pub nade: Option<Nade>,
    /// A substring of the team's name, as the demo names it.
    pub team: Option<String>,
    /// A player's name, any case.
    pub player: Option<String>,
    pub phase: Option<Phase>,
    /// Lineups thrown fewer times are left out.
    pub min_throws: i64,
    pub limit: i64,
}

impl Filter {
    pub fn map(map: &str) -> Self {
        Self { map: map.to_string(), side: None, nade: None, team: None, player: None, phase: None, min_throws: 1, limit: 300 }
    }
}

/// One lineup of a slice: `lineup_report`'s row, as far as the sandbox shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Lineup {
    pub lineup_id: String,
    pub map: String,
    pub nade: String,
    pub side: Option<String>,
    pub throws: i64,
    pub matches: i64,
    pub from_place: Option<String>,
    pub to_place: Option<String>,
    /// The commonest technique, e.g. `jumpthrow/left`.
    pub technique: Option<String>,
    pub team_list: Option<String>,
    pub player_list: Option<String>,
    /// The throw the lineup is shown by: the earliest of its commonest technique.
    pub example_throw_id: String,
    pub example_player: Option<String>,
    pub example_team: Option<String>,
    pub example_technique: Option<String>,
    /// `setpos …; setang …` for the example throw, when it has a stance and angles.
    pub practice_cmd: Option<String>,
    /// Whether that command reproduces the throw: thrown standing still or as a plain jump-throw, with a known click.
    pub practice_ready: bool,
}

/// One map's lineups in the slice, most thrown first, ties by lineup id (`nades lineups`' order).
pub fn lineups(c: &duckdb::Connection, filter: &Filter) -> duckdb::Result<Vec<Lineup>> {
    let mut query = c.prepare(
        "select lineup_id, map, nade, side, throws, matches, from_place, to_place, technique, team_list, player_list,
                example_throw_id, example_player, example_team, example_technique, practice_cmd, practice_ready
         from lineup_report(p_map := ?, p_side := ?, p_team := ?, p_player := ?, p_nade := ?, p_phase := ?)
         where throws >= ? and lineup_id is not null
         order by throws desc, lineup_id
         limit ?",
    )?;
    let rows = query.query_map(
        duckdb::params![
            filter.map,
            filter.side.map(Side::name),
            filter.team,
            filter.player,
            filter.nade.map(Nade::name),
            filter.phase.map(Phase::name),
            filter.min_throws,
            filter.limit
        ],
        |r| {
            Ok(Lineup {
                lineup_id: r.get(0)?,
                map: r.get(1)?,
                nade: r.get(2)?,
                side: r.get(3)?,
                throws: r.get(4)?,
                matches: r.get(5)?,
                from_place: r.get(6)?,
                to_place: r.get(7)?,
                technique: r.get(8)?,
                team_list: r.get(9)?,
                player_list: r.get(10)?,
                example_throw_id: r.get(11)?,
                example_player: r.get(12)?,
                example_team: r.get(13)?,
                example_technique: r.get(14)?,
                practice_cmd: r.get(15)?,
                practice_ready: r.get::<_, Option<bool>>(16)?.unwrap_or(false),
            })
        },
    )?;
    rows.collect()
}

/// The teams with demos on a map, most demos first (`team_demos`).
pub fn teams(c: &duckdb::Connection, map: &str) -> duckdb::Result<Vec<String>> {
    let mut query = c.prepare("select team from team_demos(p_map := ?) where team is not null")?;
    query.query_map([map], |r| r.get(0))?.collect()
}

/// The players who threw on a map (for a team, when given), busiest first (`slice_players`).
pub fn players(c: &duckdb::Connection, map: &str, team: Option<&str>) -> duckdb::Result<Vec<String>> {
    let mut query = c.prepare("select player from slice_players(p_map := ?, p_team := ?)")?;
    query.query_map(duckdb::params![map, team], |r| r.get(0))?.collect()
}

/// A smoke's voxel log as networked, and the point its grid is centred on.
#[derive(Debug, Clone, PartialEq)]
pub struct SmokeLog {
    pub start_tick: i32,
    pub detonation: [f32; 3],
    pub log: Vec<u8>,
}

/// A player a flash blinded: where they stood (the nearest sampled position), for how long, and whether an enemy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blinded {
    pub pos: [f32; 3],
    pub seconds: f32,
    pub enemy: bool,
}

/// One throw in world units (Hammer units, z up), as `throw_scene` gives it to the web view's 3D page.
#[derive(Debug, Clone, PartialEq)]
pub struct Throw {
    pub throw_id: String,
    pub map: String,
    pub round: Option<i32>,
    /// The release tick.
    pub tick: i32,
    pub side: Option<String>,
    pub team: Option<String>,
    pub opponent: Option<String>,
    pub player: Option<String>,
    pub nade: String,
    /// `incendiary` or `molotov` for a fire.
    pub weapon: Option<String>,
    pub technique: Option<String>,
    pub movement: Option<String>,
    pub click: Option<String>,
    pub crouch: Option<bool>,
    pub duck: Option<f32>,
    /// The player's origin at release, and where they stood (a jump-throw's take-off), per axis as stored.
    pub pos: [Option<f32>; 3],
    pub stand: [Option<f32>; 3],
    pub pitch: Option<f32>,
    pub yaw: Option<f32>,
    /// The grenade's release point.
    pub p0: [f32; 3],
    pub land: [f32; 3],
    pub detonate_tick: Option<i32>,
    pub airtime: Option<f32>,
    pub effect_seconds: Option<f32>,
    pub origin_place: Option<String>,
    pub land_place: Option<String>,
    pub lineup_id: Option<String>,
    pub event: Option<String>,
    /// The recorded flight, release first; empty for a throw ingested before tracks existed.
    pub path: Vec<PathPoint>,
    pub smoke: Option<SmokeLog>,
    /// The flames its fire spread to, in the inferno's order.
    pub flames: Vec<Flame>,
    /// Who it blinded, longest first.
    pub blinded: Vec<Blinded>,
}

/// An opponent's sample while the effect was up (`scene_viewers`), nearest the landing first.
#[derive(Debug, Clone, PartialEq)]
pub struct Viewer {
    pub tick: i32,
    pub player: Option<String>,
    pub side: String,
    /// Their feet.
    pub pos: [f32; 3],
    pub pitch: Option<f32>,
    pub yaw: Option<f32>,
    pub distance: f64,
}

/// One throw's scene, or None when no throw has that id.
pub fn throw_scene(c: &duckdb::Connection, throw_id: &str) -> duckdb::Result<Option<Throw>> {
    let mut query = c.prepare(
        "select throw_id, map, round, tick, side, team, opponent, player, nade, weapon, technique, movement, click,
                crouch, duck, pos_x, pos_y, pos_z, stand_x, stand_y, stand_z, pitch, yaw, p0_x, p0_y, p0_z,
                land_x, land_y, land_z, detonate_tick, airtime, effect_seconds, origin_place, land_place, lineup_id, event,
                path, path_ticks, smoke_start_tick, smoke_x, smoke_y, smoke_z, smoke_log, flames, blinded
         from throw_scene(p_throw_id := ?)",
    )?;
    let mut rows = query.query([throw_id])?;
    let Some(r) = rows.next()? else { return Ok(None) };
    let path: Vec<[f32; 3]> = list(r.get(36)?).iter().filter_map(point).collect();
    let ticks: Vec<i32> = list(r.get(37)?).iter().filter_map(integer).collect();
    let path = if path.len() == ticks.len() { ticks.iter().zip(path).map(|(&tick, pos)| PathPoint { tick, pos }).collect() } else { Vec::new() };
    let smoke = match (r.get::<_, Option<i32>>(38)?, r.get::<_, Option<f32>>(39)?, r.get::<_, Option<f32>>(40)?, r.get::<_, Option<f32>>(41)?, r.get::<_, Option<Vec<u8>>>(42)?) {
        (Some(start_tick), Some(x), Some(y), Some(z), Some(log)) => Some(SmokeLog { start_tick, detonation: [x, y, z], log }),
        _ => None,
    };
    let flames = list(r.get(43)?)
        .iter()
        .enumerate()
        .filter_map(|(index, f)| {
            let pos = [field(f, "x")?, field(f, "y")?, field(f, "z")?];
            let normal = match (field(f, "nx"), field(f, "ny"), field(f, "nz")) {
                (Some(x), Some(y), Some(z)) => Some([x, y, z]),
                _ => None,
            };
            Some(Flame { index, pos, normal, first_tick: integer(member(f, "first_tick")?)?, last_tick: integer(member(f, "last_tick")?)? })
        })
        .collect();
    let blinded = list(r.get(44)?)
        .iter()
        .filter_map(|b| {
            let enemy = matches!(member(b, "enemy"), Some(Value::Boolean(true)));
            Some(Blinded { pos: [field(b, "x")?, field(b, "y")?, field(b, "z")?], seconds: field(b, "seconds")?, enemy })
        })
        .collect();
    let three = |at: usize| -> duckdb::Result<[Option<f32>; 3]> { Ok([r.get(at)?, r.get(at + 1)?, r.get(at + 2)?]) };
    Ok(Some(Throw {
        throw_id: r.get(0)?,
        map: r.get(1)?,
        round: r.get(2)?,
        tick: r.get(3)?,
        side: r.get(4)?,
        team: r.get(5)?,
        opponent: r.get(6)?,
        player: r.get(7)?,
        nade: r.get(8)?,
        weapon: r.get(9)?,
        technique: r.get(10)?,
        movement: r.get(11)?,
        click: r.get(12)?,
        crouch: r.get(13)?,
        duck: r.get(14)?,
        pos: three(15)?,
        stand: three(18)?,
        pitch: r.get(21)?,
        yaw: r.get(22)?,
        p0: three(23)?.map(Option::unwrap_or_default),
        land: three(26)?.map(Option::unwrap_or_default),
        detonate_tick: r.get(29)?,
        airtime: r.get(30)?,
        effect_seconds: r.get(31)?,
        origin_place: r.get(32)?,
        land_place: r.get(33)?,
        lineup_id: r.get(34)?,
        event: r.get(35)?,
        path,
        smoke,
        flames,
        blinded,
    }))
}

/// The other side's samples while a throw's effect was up, nearest first, as the web API reads them (at most 2,000).
pub fn scene_viewers(c: &duckdb::Connection, throw_id: &str) -> duckdb::Result<Vec<Viewer>> {
    let mut query = c.prepare("select tick, player, side, x, y, z, pitch, yaw, distance from scene_viewers(p_throw_id := ?) limit 2000")?;
    query
        .query_map([throw_id], |r| Ok(Viewer { tick: r.get(0)?, player: r.get(1)?, side: r.get(2)?, pos: [r.get(3)?, r.get(4)?, r.get(5)?], pitch: r.get(6)?, yaw: r.get(7)?, distance: r.get(8)? }))?
        .collect()
}

fn list(value: Value) -> Vec<Value> {
    match value {
        Value::List(items) | Value::Array(items) => items,
        _ => Vec::new(),
    }
}

fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Struct(fields) => fields.get(&name.to_string()),
        _ => None,
    }
}

fn number(value: &Value) -> Option<f32> {
    match *value {
        Value::Float(x) => Some(x),
        Value::Double(x) => Some(x as f32),
        _ => None,
    }
}

fn field(value: &Value, name: &str) -> Option<f32> {
    member(value, name).and_then(number)
}

fn integer(value: &Value) -> Option<i32> {
    match *value {
        Value::Int(x) => Some(x),
        Value::BigInt(x) => i32::try_from(x).ok(),
        Value::SmallInt(x) => Some(i32::from(x)),
        _ => None,
    }
}

fn point(value: &Value) -> Option<[f32; 3]> {
    match value {
        Value::List(xyz) | Value::Array(xyz) if xyz.len() == 3 => Some([number(&xyz[0])?, number(&xyz[1])?, number(&xyz[2])?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nades-data-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nades.duckdb");
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn the_working_trees_views_are_found_with_what_they_need_and_made_temp() {
        let sql = "-- a view\ncreate or replace view v_a as select 1 as x; -- the a's ';'\n\
                   create or replace macro m_b(p := null) as table select * from v_a where 'm_c' <> '';\n\
                   create or replace macro m_c() as table select 2;\n\
                   create or replace macro m_d() as table select * from m_b();";
        let needed = needed_views(sql, &["m_d"]);
        assert_eq!(needed.len(), 3);
        assert!(needed[0].contains("create or replace temp view v_a"));
        assert!(needed[2].contains("create or replace temp macro m_d"));
        // a name inside a string literal or a comment is not a reference
        assert!(!needed.iter().any(|s| s.contains("m_c()")));
        // they apply to a read-only file and override what it stores
        let path = scratch("views");
        duckdb::Connection::open(&path).unwrap().execute_batch("create view v_a as select 0 as x; create macro m_d() as table select 0 as x").unwrap();
        let db = Database { path: path.clone(), views: std::sync::Arc::new(needed) };
        assert_eq!(db.read(|c| c.query_row("select x from m_d()", [], |r| r.get::<_, i64>(0))).unwrap(), 1);
        assert_eq!(Database::stored(&path).read(|c| c.query_row("select x from m_d()", [], |r| r.get::<_, i64>(0))).unwrap(), 0);
    }

    #[test]
    fn reads_rows_and_leaves_nothing_open() {
        let path = scratch("read");
        {
            let writer = duckdb::Connection::open(&path).unwrap();
            writer.execute_batch("create table throws (throw_id varchar, nade varchar); insert into throws values ('a', 'smoke'), ('b', 'flash');").unwrap();
        }
        let db = Database::at(&path);
        let smokes: i64 = db.read(|c| c.query_row("select count(*) from throws where nade = 'smoke'", [], |r| r.get(0))).unwrap();
        assert_eq!(smokes, 1);
        // nothing stays open: a writer gets the file straight away
        let writer = duckdb::Connection::open(&path).unwrap();
        writer.execute_batch("insert into throws values ('c', 'smoke')").unwrap();
    }

    #[test]
    fn a_missing_file_is_named_and_writes_are_refused() {
        assert!(matches!(Database::at("/nonexistent/nades.duckdb").read(|_| Ok(())), Err(DataError::Missing(_))));
        let path = scratch("read-only");
        duckdb::Connection::open(&path).unwrap().execute_batch("create table t (x int)").unwrap();
        assert!(matches!(Database::at(&path).read(|c| c.execute_batch("insert into t values (1)")), Err(DataError::Duck(_))));
    }

    /// A database with nades' own schema and macros (show-me-the-nades' sql/, read in place) and one small match: two
    /// smokes of one lineup and a molotov on de_cache, with the T side's flight, voxel log and flames, and a CT
    /// player who stood watching.
    fn match_database(name: &str) -> PathBuf {
        let sql = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../show-me-the-nades/sql");
        let path = scratch(name);
        let c = duckdb::Connection::open(&path).unwrap();
        c.execute_batch(&std::fs::read_to_string(sql.join("schema.sql")).unwrap()).unwrap();
        c.execute_batch(&std::fs::read_to_string(sql.join("views.sql")).unwrap()).unwrap();
        c.execute_batch(
            "insert into demos (demo_id, sha256, file_name, map, event, match_epoch, team_a, team_b, rounds, parser)
                 values ('d1', 'sha', 'm.dem', 'de_cache', 'Test Cup', 1760000000, 'Vitality', 'MOUZ', 1, 'awpy');
             insert into rounds (demo_id, round, start_tick, freeze_end_tick, end_tick, winner_side, t_team, ct_team, t_buy, ct_buy, is_knife)
                 values ('d1', 1, 0, 1000, 9000, 'T', 'Vitality', 'MOUZ', 'full', 'full', false);
             insert into throws (throw_id, demo_id, map, round, tick, t_round, post_round, post_plant, player, team, opponent, side, nade, weapon,
                                 pos_x, pos_y, pos_z, stand_x, stand_y, stand_z, pitch, yaw, p0_x, p0_y, p0_z, click, movement, crouch, duck,
                                 detonate_tick, airtime, land_x, land_y, land_z, effect_seconds, lineup_id)
                 values ('t1', 'd1', 'de_cache', 1, 1500, 7.8, false, false, 'ropz', 'MOUZ', 'Vitality', 'T', 'smoke', 'smokegrenade',
                         10, 20, 30, 10, 20, 30, -12.5, 45, 12, 22, 94, 'left', 'stand', false, 0,
                         1600, 1.56, 800, 900, 40, 18, 'cache-smoke-0001'),
                        ('t2', 'd1', 'de_cache', 1, 2500, 23.4, false, false, 'ropz', 'MOUZ', 'Vitality', 'T', 'smoke', 'smokegrenade',
                         11, 21, 30, 11, 21, 30, -12.4, 45.1, 13, 23, 94, 'left', 'stand', false, 0,
                         2600, 1.5, 802, 901, 40, 18, 'cache-smoke-0001'),
                        ('t3', 'd1', 'de_cache', 1, 3000, 31.2, false, false, 'ZywOo', 'Vitality', 'MOUZ', 'CT', 'molotov', 'incendiary',
                         500, 600, 0, null, null, null, 5, 180, 502, 600, 60, 'left', 'run', true, 1,
                         3100, 1.6, 300, 600, 0, 7, 'cache-molotov-0001');
             insert into throw_paths values ('t1', 'd1', 0, 1500, 12, 22, 94), ('t1', 'd1', 1, 1502, 20, 30, 100), ('t1', 'd1', 2, 1600, 800, 900, 40);
             insert into smoke_voxels values ('t1', 'd1', 1600, 800, 900, 40, 1, '\\x00\\x00\\x03\\x00\\x00\\x00\\x00'::blob);
             insert into fire_flames values ('t3', 'd1', 0, 300, 600, 0, 0, 0, 1, 3100, 3400), ('t3', 'd1', 1, 340, 600, 0, null, null, null, 3120, 3500);
             insert into player_positions (demo_id, round, tick, steamid, pawn_entity, pawn_serial, player, side, x, y, z, pitch, yaw, health, alive)
                 values ('d1', 1, 1760, 7, 1, 1, 'Jimpphat', 'CT', 1200, 900, 0, 2, 180, 100, true),
                        ('d1', 1, 1792, 7, 1, 1, 'Jimpphat', 'CT', 1300, 900, 0, 2, 180, 100, true),
                        ('d1', 1, 1792, 8, 2, 1, 'ropz', 'T', 0, 0, 0, 0, 0, 100, true);",
        )
        .unwrap();
        path
    }

    #[test]
    fn lineups_come_most_thrown_first_and_cut_by_the_filter() {
        let db = Database::at(match_database("lineups"));
        let all = db.read(|c| lineups(c, &Filter::map("de_cache"))).unwrap();
        assert_eq!(all.iter().map(|l| (l.lineup_id.as_str(), l.throws)).collect::<Vec<_>>(), [("cache-smoke-0001", 2), ("cache-molotov-0001", 1)]);
        let smoke = &all[0];
        assert_eq!((smoke.side.as_deref(), smoke.example_throw_id.as_str(), smoke.example_player.as_deref()), (Some("T"), "t1", Some("ropz")));
        assert_eq!(smoke.practice_cmd.as_deref(), Some("setpos 10.00 20.00 30.00; setang -12.50 45.00 0"));
        assert!(smoke.practice_ready);
        // by side, grenade, team, player and phase, and the short map name
        let cut = |f: Filter| db.read(|c| lineups(c, &f)).unwrap().into_iter().map(|l| l.lineup_id).collect::<Vec<_>>();
        assert_eq!(cut(Filter { side: Some(Side::Ct), ..Filter::map("cache") }), ["cache-molotov-0001"]);
        assert_eq!(cut(Filter { nade: Some(Nade::Smoke), ..Filter::map("de_cache") }), ["cache-smoke-0001"]);
        assert_eq!(cut(Filter { team: Some("vital".into()), ..Filter::map("de_cache") }), ["cache-molotov-0001"]);
        assert_eq!(cut(Filter { player: Some("ROPZ".into()), ..Filter::map("de_cache") }), ["cache-smoke-0001"]);
        assert_eq!(cut(Filter { phase: Some(Phase::Early), ..Filter::map("de_cache") }), ["cache-smoke-0001"]);
        assert_eq!(cut(Filter { min_throws: 2, ..Filter::map("de_cache") }), ["cache-smoke-0001"]);
        let teams = db.read(|c| teams(c, "de_cache")).unwrap();
        assert_eq!(teams.len(), 2);
        assert_eq!(db.read(|c| players(c, "de_cache", Some("MOUZ"))).unwrap(), ["ropz"]);
    }

    #[test]
    fn a_throws_scene_has_its_flight_its_smoke_its_fire_and_its_viewers() {
        let db = Database::at(match_database("scene"));
        let smoke = db.read(|c| throw_scene(c, "t1")).unwrap().unwrap();
        assert_eq!((smoke.nade.as_str(), smoke.tick, smoke.detonate_tick, smoke.stand, smoke.pitch), ("smoke", 1500, Some(1600), [Some(10.0), Some(20.0), Some(30.0)], Some(-12.5)));
        assert_eq!(smoke.path, vec![PathPoint { tick: 1500, pos: [12.0, 22.0, 94.0] }, PathPoint { tick: 1502, pos: [20.0, 30.0, 100.0] }, PathPoint { tick: 1600, pos: [800.0, 900.0, 40.0] }]);
        let log = smoke.smoke.as_ref().unwrap();
        assert_eq!((log.start_tick, log.detonation, log.log.len()), (1600, [800.0, 900.0, 40.0], 7));
        assert!(smoke.flames.is_empty() && smoke.blinded.is_empty());

        let fire = db.read(|c| throw_scene(c, "t3")).unwrap().unwrap();
        assert_eq!((fire.weapon.as_deref(), fire.stand, fire.crouch, fire.duck), (Some("incendiary"), [None; 3], Some(true), Some(1.0)));
        assert_eq!(fire.flames.len(), 2);
        assert_eq!((fire.flames[0].pos, fire.flames[0].normal, fire.flames[0].first_tick, fire.flames[0].last_tick), ([300.0, 600.0, 0.0], Some([0.0, 0.0, 1.0]), 3100, 3400));
        assert_eq!(fire.flames[1].normal, None);
        assert!(fire.path.is_empty() && fire.smoke.is_none());
        assert!(db.read(|c| throw_scene(c, "nothing")).unwrap().is_none());

        // the CT player watched from 1.5 s after it went off, nearest first
        let viewers = db.read(|c| scene_viewers(c, "t1")).unwrap();
        assert_eq!(viewers.iter().map(|v| (v.tick, v.pos[0])).collect::<Vec<_>>(), [(1760, 1200.0), (1792, 1300.0)]);
        assert_eq!((viewers[0].player.as_deref(), viewers[0].side.as_str(), viewers[0].yaw), (Some("Jimpphat"), "CT", Some(180.0)));
    }

    #[test]
    fn a_busy_file_is_tried_again_until_it_is_free_or_patience_runs_out() {
        let mut tries = 0;
        let started = Instant::now();
        let read = when_free(Duration::from_secs(5), || {
            tries += 1;
            if tries < 3 { Err(DataError::Busy) } else { Ok(tries) }
        });
        assert_eq!(read.unwrap(), 3);
        assert!(started.elapsed() >= RETRY * 2);
        let mut tries = 0;
        let gave_up = when_free(Duration::from_millis(700), || -> Result<(), DataError> {
            tries += 1;
            Err(DataError::Busy)
        });
        assert!(matches!(gave_up, Err(DataError::Busy)));
        assert_eq!(tries, 2, "one try, one wait, one more try: a second wait would outlast the patience");
    }
}
