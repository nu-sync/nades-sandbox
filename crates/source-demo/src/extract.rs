//! One demo's §3 facts, read with awpy: every grenade throw with its thrower's pawn at release, what it did, rounds,
//! blinds and player samples. Ported from show-me-the-nades' `extract.rs`, whose readings are checked against real
//! demos (its nades-ingestion skill): the release from the projectile's own `m_vInitialPosition` and
//! `m_vInitialVelocity`, the detonate events instead of the projectile's last tick, and fires matched to their molotov
//! by place and time, since an inferno carries no usable thrower.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context as _, Result};
use awpy::{Entity, EventDatasetSelection, FieldValue, INVALID_ENTITY_HANDLE, Parser, ProjectileSelection, Round, Serializer, cell_to_world};

use crate::tracks::{self, Flame, InfernoKeys, PathPoint, SmokeKeys, SmokeVoxels};

/// Ticks between player samples: two a second at 64 ticks.
pub const SAMPLE_EVERY: i32 = 32;
const ORIGIN: &str = "CBodyComponent.m_skeletonInstance.m_vecOrigin";
const PAWN: &str = "CCSPlayerPawn";
const CONTROLLER: &str = "CCSPlayerController";
const TEAM: &str = "CCSTeam";
const INFERNO: &str = "CInferno";
const PROJECTILES: [(&str, &str); 5] = [
    ("CSmokeGrenadeProjectile", "smoke"),
    ("CFlashbangProjectile", "flash"),
    ("CHEGrenadeProjectile", "he"),
    ("CMolotovProjectile", "molotov"),
    ("CDecoyProjectile", "decoy"),
];
/// A fire matches the molotov whose last position is nearest its origin, within this many units.
const FIRE_REACH: f32 = 250.0;
/// A fire starts at most this many ticks after its molotov's projectile is gone.
const FIRE_LATE: i32 = 32;
/// A blind belongs to its thrower's flash that went off within this many ticks of it.
const BLIND_TICKS: i32 = 4;

/// One demo's facts, in the format's terms.
#[derive(Debug, Default)]
pub struct Facts {
    pub map: String,
    pub rounds: Vec<RoundFact>,
    pub throws: Vec<ThrowFact>,
    pub blinds: Vec<BlindFact>,
    pub samples: Vec<SampleFact>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoundFact {
    pub number: i32,
    pub freeze_end: Option<i32>,
    /// The round's first `bomb_planted`.
    pub plant: Option<i32>,
    pub end: i32,
    pub winner: Option<&'static str>,
}

#[derive(Debug, Clone, Default)]
pub struct ThrowFact {
    /// The projectile's entity index: with the first tick and the demo, the throw's id.
    pub entity: i32,
    /// The projectile's first tick.
    pub tick: i32,
    /// The projectile's last tick.
    pub last_tick: i32,
    pub round: Option<i32>,
    /// From the projectile's class: `smoke`, `flash`, `he`, `molotov` (both fires) or `decoy`.
    pub nade: &'static str,
    /// `m_bIsIncGrenade`.
    pub incendiary: bool,
    pub side: Option<&'static str>,
    pub player: Option<String>,
    pub steamid: Option<u64>,
    pub team: Option<String>,
    pub place: Option<String>,
    pub start: [f32; 3],
    pub velocity: [f32; 3],
    pub origin: Option<[f32; 3]>,
    /// An airborne thrower's origin at its last tick on the ground, when it was seen there.
    pub takeoff: Option<[f32; 3]>,
    pub eye: Option<[f32; 3]>,
    pub pawn_velocity: Option<[f32; 3]>,
    pub on_ground: Option<bool>,
    pub duck: Option<f32>,
    pub walking: Option<bool>,
    pub detonate_tick: Option<i32>,
    /// The detonate event's position, else the projectile's last.
    pub land: [f32; 3],
    /// Ticks from the detonation to the expiry event (smokes and fires).
    pub effect_ticks: Option<i32>,
    pub flight: Vec<PathPoint>,
    pub voxels: Option<SmokeVoxels>,
    pub flames: Vec<Flame>,
}

impl ThrowFact {
    /// The format's grenade name.
    pub fn grenade(&self) -> &'static str {
        if self.nade == "molotov" && self.incendiary { "incendiary" } else { self.nade }
    }

    /// The expiry event's tick.
    pub fn end_tick(&self) -> Option<i32> {
        Some(self.detonate_tick? + self.effect_ticks?)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlindFact {
    /// Index into [`Facts::throws`] of the flash.
    pub throw: usize,
    pub player: Option<String>,
    pub steamid: Option<u64>,
    pub side: Option<&'static str>,
    pub pos: Option<[f32; 3]>,
    pub seconds: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SampleFact {
    pub tick: i32,
    pub steamid: u64,
    pub player: Option<String>,
    pub side: &'static str,
    pub pos: [f32; 3],
    pub eye: Option<[f32; 3]>,
    pub duck: Option<f32>,
    pub alive: bool,
    pub place: Option<String>,
}

pub(crate) fn get(e: &Entity, key: Option<u64>) -> Option<&FieldValue> {
    key.and_then(|k| e.fields.get(&k))
}

pub(crate) fn vec3(v: Option<&FieldValue>) -> Option<[f32; 3]> {
    match v? {
        FieldValue::Vector3(a) | FieldValue::QAngle(a) => Some(*a),
        FieldValue::FloatVector(a) if a.len() >= 3 => Some([a[0], a[1], a[2]]),
        _ => None,
    }
}

pub(crate) fn num(v: Option<&FieldValue>) -> Option<f64> {
    match v? {
        FieldValue::F32(x) => Some(f64::from(*x)),
        FieldValue::I32(x) => Some(f64::from(*x)),
        FieldValue::U32(x) => Some(f64::from(*x)),
        FieldValue::I64(x) => Some(*x as f64),
        FieldValue::U64(x) => Some(*x as f64),
        FieldValue::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

pub(crate) fn uint(v: Option<&FieldValue>) -> Option<u64> {
    match v? {
        FieldValue::U64(x) => Some(*x),
        FieldValue::U32(x) => Some(u64::from(*x)),
        FieldValue::I32(x) => Some(u64::from(*x as u32)),
        FieldValue::I64(x) => Some(*x as u64),
        _ => None,
    }
}

pub(crate) fn boolean(v: Option<&FieldValue>) -> Option<bool> {
    match v? {
        FieldValue::Bool(b) => Some(*b),
        other => num(Some(other)).map(|x| x != 0.0),
    }
}

fn text(v: Option<&FieldValue>) -> Option<String> {
    match v? {
        FieldValue::String(s) => Some(String::from_utf8_lossy(s).into_owned()).filter(|s| !s.is_empty()),
        _ => None,
    }
}

/// `T` or `CT` from `m_iTeamNum`.
fn side_of_team(team: i64) -> Option<&'static str> {
    match team {
        2 => Some("T"),
        3 => Some("CT"),
        _ => None,
    }
}

/// `T` or `CT` from awpy's side names.
fn side_of_name(side: Option<&str>) -> Option<&'static str> {
    match side?.to_ascii_lowercase().as_str() {
        "terrorist" | "t" => Some("T"),
        "counter-terrorist" | "ct" => Some("CT"),
        _ => None,
    }
}

struct OriginKeys {
    cell: [Option<u64>; 3],
    offset: [Option<u64>; 3],
}

impl OriginKeys {
    fn new(ser: &Serializer) -> Self {
        let k = |part: &str| ser.resolve_field_key(&format!("{ORIGIN}.{part}"));
        OriginKeys { cell: [k("m_cellX"), k("m_cellY"), k("m_cellZ")], offset: [k("m_vecX"), k("m_vecY"), k("m_vecZ")] }
    }

    fn world(&self, e: &Entity) -> Option<[f32; 3]> {
        if !self.offset[0].is_some_and(|k| e.fields.contains_key(&k)) {
            return None;
        }
        let mut out = [0f32; 3];
        for (i, v) in out.iter_mut().enumerate() {
            *v = cell_to_world(i32::try_from(e.get_i64(self.cell[i])).ok()?, e.get_f32(self.offset[i]));
        }
        Some(out)
    }
}

struct PawnKeys {
    origin: OriginKeys,
    eye: Option<u64>,
    vel: [Option<u64>; 3],
    ground: Option<u64>,
    duck: Option<u64>,
    walking: Option<u64>,
    team: Option<u64>,
    controller: Option<u64>,
    place: Option<u64>,
    life: Option<u64>,
    health: Option<u64>,
}

impl PawnKeys {
    fn new(ser: &Serializer) -> Self {
        let k = |n: &str| ser.resolve_field_key(n);
        PawnKeys {
            origin: OriginKeys::new(ser),
            eye: k("m_angEyeAngles"),
            vel: [k("m_vecVelocity.m_vecX"), k("m_vecVelocity.m_vecY"), k("m_vecVelocity.m_vecZ")],
            ground: k("m_hGroundEntity"),
            duck: k("m_pMovementServices.m_flDuckAmount").or_else(|| k("m_flDuckAmount")),
            walking: k("m_bIsWalking"),
            team: k("m_iTeamNum"),
            controller: k("m_hController"),
            place: k("m_szLastPlaceName"),
            life: k("m_lifeState"),
            health: k("m_iHealth"),
        }
    }
}

struct ProjectileKeys {
    origin: OriginKeys,
    p0: Option<u64>,
    v0: Option<u64>,
    thrower: Option<u64>,
    incendiary: Option<u64>,
    /// Smokes only.
    smoke: Option<SmokeKeys>,
}

struct ControllerKeys {
    steamid: Option<u64>,
    name: Option<u64>,
}

impl ControllerKeys {
    fn new(s: &Serializer) -> Self {
        ControllerKeys { steamid: s.resolve_field_key("m_steamID"), name: s.resolve_field_key("m_iszPlayerName") }
    }
}

struct TeamKeys {
    number: Option<u64>,
    name: Option<u64>,
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn round_start(r: &Round) -> i32 {
    r.start_tick.or(r.freeze_end_tick).unwrap_or(r.end_tick)
}

/// The round a tick belongs to: the last non-knife round that started at or before it.
fn round_at(rounds: &[Round], tick: i32) -> Option<&Round> {
    rounds.iter().filter(|r| !r.is_knife_round && round_start(r) <= tick).max_by_key(|r| round_start(r))
}

/// Whether a sample is due at `tick`: `SAMPLE_EVERY` ticks after the last, which survives skipped and repeated ticks.
fn sample_due(last: &mut Option<i32>, tick: i32) -> bool {
    if tick < 0 || last.is_some_and(|last| tick.saturating_sub(last) < SAMPLE_EVERY) {
        return false;
    }
    *last = Some(tick);
    true
}

fn finite(value: [f32; 3]) -> Option<[f32; 3]> {
    value.iter().all(|v| v.is_finite()).then_some(value)
}

/// Samples inside live rounds (freeze end to round end, knife rounds left out), without a player seen twice at one tick
/// (two pawns for one Steam id would put them in an arbitrary place), in (tick, Steam id) order.
fn live_samples(mut samples: Vec<SampleFact>, rounds: &[Round]) -> Vec<SampleFact> {
    samples.retain(|p| rounds.iter().any(|r| !r.is_knife_round && r.freeze_end_tick.is_some_and(|start| start <= p.tick && p.tick <= r.end_tick)));
    let mut seen = HashSet::new();
    let mut twice = HashSet::new();
    for p in &samples {
        if !seen.insert((p.tick, p.steamid)) {
            twice.insert((p.tick, p.steamid));
        }
    }
    samples.retain(|p| !twice.contains(&(p.tick, p.steamid)));
    samples.sort_unstable_by_key(|p| (p.tick, p.steamid));
    samples
}

/// Reads `path`. `progress` hears the share done, 0 to 1.
pub fn extract(path: &Path, progress: &mut dyn FnMut(f32)) -> Result<Facts> {
    let parser = Parser::from_file(path).with_context(|| format!("opening {}", path.display()))?;
    let header = parser.file_header().with_context(|| format!("reading {}'s header", path.display()))?;
    let map = header.map_name.clone().filter(|m| !m.is_empty()).with_context(|| format!("{} names no map", path.display()))?;
    let total_ticks = parser.file_info().ok().and_then(|i| i.playback_ticks).filter(|t| *t > 0);
    let nade_of: HashMap<&str, &'static str> = PROJECTILES.iter().copied().collect();
    let mut filter: HashSet<&str> = nade_of.keys().copied().collect();
    filter.extend([PAWN, CONTROLLER, TEAM, INFERNO]);

    let mut pawn_keys: Option<PawnKeys> = None;
    let mut ctrl_keys: Option<ControllerKeys> = None;
    let mut team_keys: Option<TeamKeys> = None;
    let mut proj_keys: HashMap<i32, ProjectileKeys> = HashMap::new();
    let mut inferno_keys: Option<InfernoKeys> = None;
    // inferno entity -> its flame slots, given to the molotov the fire is matched to
    let mut infernos: HashMap<(i32, u32), Vec<Option<Flame>>> = HashMap::new();
    let mut team_names: [Option<String>; 4] = Default::default();
    let mut last_ground: HashMap<i32, [f32; 3]> = HashMap::new();
    let mut live: HashMap<(i32, u32), usize> = HashMap::new();
    let mut throws: Vec<ThrowFact> = vec![];
    let mut samples = vec![];
    let mut last_sample = None;
    let mut reported = 0;

    // The entity pass is most of the work: 0 to 0.7.
    parser.run_to_end_filtered(&filter, |ctx| {
        let tick = ctx.tick();
        if let Some(total) = total_ticks
            && tick - reported >= 640
        {
            reported = tick;
            progress(0.7 * (tick as f32 / total as f32).clamp(0.0, 1.0));
        }
        let sample = sample_due(&mut last_sample, tick);
        for (_, e) in ctx.entities().iter() {
            if !e.active {
                continue;
            }
            match e.class_name.as_ref() {
                TEAM => {
                    if team_keys.is_none()
                        && let Some(s) = ctx.serializers().get(&e.class_name)
                    {
                        team_keys = Some(TeamKeys { number: s.resolve_field_key("m_iTeamNum"), name: s.resolve_field_key("m_szClanTeamname") });
                    }
                    let Some(k) = team_keys.as_ref() else { continue };
                    if let Some(n) = num(get(e, k.number)).map(|n| n as usize).filter(|n| *n < 4) {
                        team_names[n] = text(get(e, k.name));
                    }
                }
                PAWN => {
                    if pawn_keys.is_none()
                        && let Some(s) = ctx.serializers().get(&e.class_name)
                    {
                        pawn_keys = Some(PawnKeys::new(s));
                    }
                    let Some(k) = pawn_keys.as_ref() else { continue };
                    let Some(pos) = k.origin.world(e) else { continue };
                    if uint(get(e, k.ground)).is_some_and(|g| g as u32 != INVALID_ENTITY_HANDLE) {
                        last_ground.insert(e.index, pos);
                    }
                    if !sample {
                        continue;
                    }
                    let Some(pos) = finite(pos) else { continue };
                    let Some(side) = num(get(e, k.team)).and_then(|n| side_of_team(n as i64)) else { continue };
                    let Some(health) = num(get(e, k.health)).filter(|n| n.is_finite() && *n >= f64::from(i32::MIN) && *n <= f64::from(i32::MAX)) else { continue };
                    let Some(life) = num(get(e, k.life)).filter(|n| n.is_finite()) else { continue };
                    let Some(c) = uint(get(e, k.controller)).and_then(|h| ctx.entities().get_by_handle(h as u32)).filter(|c| c.active && c.class_name.as_ref() == CONTROLLER) else {
                        continue;
                    };
                    if ctrl_keys.is_none()
                        && let Some(s) = ctx.serializers().get(&c.class_name)
                    {
                        ctrl_keys = Some(ControllerKeys::new(s));
                    }
                    let Some(ck) = ctrl_keys.as_ref() else { continue };
                    let Some(steamid) = uint(get(c, ck.steamid)).filter(|id| *id != 0) else { continue };
                    samples.push(SampleFact {
                        tick,
                        steamid,
                        player: text(get(c, ck.name)),
                        side,
                        pos,
                        eye: vec3(get(e, k.eye)).and_then(finite),
                        duck: num(get(e, k.duck)).map(|d| d as f32).filter(|d| d.is_finite()),
                        alive: life == 0.0 && health > 0.0,
                        place: text(get(e, k.place)),
                    });
                }
                _ => {}
            }
        }

        for (_, e) in ctx.entities().iter() {
            if !e.active {
                continue;
            }
            if e.class_name.as_ref() == INFERNO {
                if inferno_keys.is_none()
                    && let Some(s) = ctx.serializers().get(&e.class_name)
                {
                    inferno_keys = Some(InfernoKeys::new(s));
                }
                if let Some(k) = inferno_keys.as_ref() {
                    k.read(e, tick, infernos.entry((e.index, e.serial)).or_default());
                }
                continue;
            }
            let Some(&nade) = nade_of.get(e.class_name.as_ref()) else { continue };
            if !proj_keys.contains_key(&e.class_id)
                && let Some(s) = ctx.serializers().get(&e.class_name)
            {
                proj_keys.insert(
                    e.class_id,
                    ProjectileKeys {
                        origin: OriginKeys::new(s),
                        p0: s.resolve_field_key("m_vInitialPosition"),
                        v0: s.resolve_field_key("m_vInitialVelocity"),
                        thrower: s.resolve_field_key("m_hThrower"),
                        incendiary: s.resolve_field_key("m_bIsIncGrenade"),
                        smoke: (nade == "smoke").then(|| SmokeKeys::new(s)),
                    },
                );
            }
            let Some(pk) = proj_keys.get(&e.class_id) else { continue };
            let pos = pk.origin.world(e);
            if let Some(&i) = live.get(&(e.index, e.serial)) {
                let t = &mut throws[i];
                if let Some(p) = pos {
                    t.land = p;
                    tracks::sample_path(&mut t.flight, tick, p);
                }
                t.last_tick = tick;
                if let Some(k) = pk.smoke.as_ref() {
                    k.read(e, tick, &mut t.voxels);
                }
                continue;
            }
            let (Some(start), Some(velocity)) = (vec3(get(e, pk.p0)), vec3(get(e, pk.v0))) else { continue };
            let mut t = ThrowFact {
                entity: e.index,
                tick,
                last_tick: tick,
                nade,
                incendiary: nade == "molotov" && boolean(get(e, pk.incendiary)) == Some(true),
                start,
                velocity,
                land: pos.unwrap_or(start),
                flight: vec![PathPoint { tick, pos: pos.unwrap_or(start) }],
                ..Default::default()
            };
            let pawn = uint(get(e, pk.thrower)).and_then(|h| ctx.entities().get_by_handle(h as u32));
            if let (Some(pawn), Some(k)) = (pawn, pawn_keys.as_ref()) {
                t.origin = k.origin.world(pawn);
                t.eye = vec3(get(pawn, k.eye));
                if let [Some(x), Some(y), Some(z)] = k.vel.map(|key| num(get(pawn, key)).map(|v| v as f32)) {
                    t.pawn_velocity = Some([x, y, z]);
                }
                let airborne = uint(get(pawn, k.ground)).map(|g| g as u32 == INVALID_ENTITY_HANDLE);
                t.on_ground = airborne.map(|a| !a);
                t.duck = num(get(pawn, k.duck)).map(|d| d as f32);
                t.walking = boolean(get(pawn, k.walking));
                t.place = text(get(pawn, k.place));
                // its last origin on the ground; never seen there, it has no take-off point
                t.takeoff = if airborne == Some(true) { last_ground.get(&pawn.index).copied().filter(|g| Some(*g) != t.origin) } else { None };
                if let Some(team) = num(get(pawn, k.team)).map(|n| n as i64) {
                    t.side = side_of_team(team);
                    t.team = usize::try_from(team).ok().and_then(|n| team_names.get(n).cloned().flatten());
                }
                if let Some(c) = uint(get(pawn, k.controller)).and_then(|h| ctx.entities().get_by_handle(h as u32)) {
                    if ctrl_keys.is_none()
                        && let Some(s) = ctx.serializers().get(&c.class_name)
                    {
                        ctrl_keys = Some(ControllerKeys::new(s));
                    }
                    if let Some(ck) = ctrl_keys.as_ref() {
                        t.steamid = uint(get(c, ck.steamid)).filter(|s| *s != 0);
                        t.player = text(get(c, ck.name));
                    }
                }
            }
            live.insert((e.index, e.serial), throws.len());
            throws.push(t);
        }
    })?;
    // `land` is still the projectile's last position here; detonate events refine it below.
    for t in &mut throws {
        tracks::finish_path(&mut t.flight, t.last_tick, t.land);
    }

    progress(0.7);
    let rounds = parser.rounds()?;
    progress(0.8);
    let events = parser.event_datasets_selected(EventDatasetSelection { kills: false, damages: false, bomb: true, blinds: true, shots: false })?;
    let projectiles = parser.projectiles_selected(ProjectileSelection { grenades: false, fires: true, smokes: true })?;
    let detonations = parser.events(None)?;
    progress(0.95);

    let plants: Vec<i32> = events.bomb.iter().filter(|b| b.event == "finish_plant").map(|b| b.tick).collect();
    let round_facts = rounds
        .iter()
        .map(|r| RoundFact {
            number: r.round_num,
            freeze_end: r.freeze_end_tick,
            plant: plants.iter().copied().find(|tick| round_at(&rounds, *tick).is_some_and(|x| x.round_num == r.round_num)),
            end: r.end_tick,
            winner: side_of_name(Some(&r.winner_side)).or_else(|| side_of_team(i64::from(r.winner))),
        })
        .collect();
    for t in &mut throws {
        t.round = round_at(&rounds, t.tick).map(|r| r.round_num);
    }

    // Detonate events carry the projectile's entity index, the tick and the position.
    for ev in &detonations {
        let nade = match ev.name.as_str() {
            "hegrenade_detonate" => "he",
            "flashbang_detonate" => "flash",
            "smokegrenade_detonate" => "smoke",
            "decoy_started" => "decoy",
            _ => continue,
        };
        let key = |k: &str| ev.keys.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let Some(entity) = key("entityid").and_then(|v| v.parse::<i32>().ok()) else { continue };
        let found = throws.iter().enumerate().filter(|(_, t)| t.nade == nade && t.entity == entity && t.tick <= ev.tick && t.detonate_tick.is_none()).max_by_key(|(_, t)| t.tick).map(|(i, _)| i);
        if let Some(i) = found {
            throws[i].detonate_tick = Some(ev.tick);
            let coord = |k: &str| key(k).and_then(|v| v.parse::<f32>().ok());
            if let (Some(x), Some(y), Some(z)) = (coord("x"), coord("y"), coord("z")) {
                throws[i].land = [x, y, z];
            }
        }
    }

    // A smoke's cloud shares its projectile's entity; one still up when the round reset has no expiry event and
    // lasted until its projectile was removed.
    for s in &projectiles.smokes {
        if let Some(&i) = live.get(&(s.entity_id, s.entity_serial))
            && throws[i].nade == "smoke"
        {
            throws[i].detonate_tick.get_or_insert(s.start_tick);
            throws[i].effect_ticks = Some(s.end_tick - s.start_tick);
        }
    }
    for t in throws.iter_mut().filter(|t| t.nade == "smoke" && t.effect_ticks.is_none()) {
        if let Some(d) = t.detonate_tick {
            t.effect_ticks = Some(t.last_tick - d);
        }
    }

    // A fire starts where its molotov broke.
    for f in &projectiles.fires {
        let origin = [f.x, f.y, f.z];
        let found = throws
            .iter()
            .enumerate()
            .filter(|(_, t)| t.nade == "molotov" && t.tick <= f.start_tick && f.start_tick <= t.last_tick + FIRE_LATE)
            .map(|(i, t)| (i, distance(t.land, origin)))
            .filter(|(_, d)| *d < FIRE_REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        if let Some(i) = found {
            throws[i].detonate_tick = Some(f.start_tick);
            throws[i].effect_ticks = Some(f.end_tick - f.start_tick);
            if let Some(slots) = infernos.remove(&(f.entity_id, f.entity_serial)) {
                throws[i].flames = slots.into_iter().flatten().collect();
            }
        }
    }
    for t in throws.iter_mut().filter(|t| t.nade == "molotov" && t.detonate_tick.is_none()) {
        t.detonate_tick = Some(t.last_tick); // broke without burning
    }

    // A blind belongs to the flash its thrower set off within a few ticks of it.
    let mut blinds = vec![];
    for b in &events.blinds {
        let flash = throws
            .iter()
            .enumerate()
            .filter(|(_, t)| t.nade == "flash" && b.attacker_steamid.is_some() && t.steamid == b.attacker_steamid)
            .filter_map(|(i, t)| Some((i, (t.detonate_tick? - b.tick).abs())))
            .filter(|(_, d)| *d <= BLIND_TICKS)
            .min_by_key(|(_, d)| *d)
            .map(|(i, _)| i);
        if let Some(throw) = flash {
            let side = side_of_name(b.victim_side.as_deref());
            // a victim on no side (a spectator) has no pawn: awpy gives it the world's origin
            let pos = match (b.victim_x, b.victim_y, b.victim_z) {
                (Some(x), Some(y), Some(z)) if side.is_some() => finite([x, y, z]),
                _ => None,
            };
            blinds.push(BlindFact { throw, player: b.victim_name.clone(), steamid: b.victim_steamid.filter(|s| *s != 0), side, pos, seconds: b.duration });
        }
    }
    progress(1.0);
    Ok(Facts { map, rounds: round_facts, throws, blinds, samples: live_samples(samples, &rounds) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(number: i32, freeze: Option<i32>, end: i32, knife: bool) -> Round {
        Round {
            round_num: number,
            start_tick: freeze.map(|tick| tick - 20),
            freeze_end_tick: freeze,
            end_tick: end,
            official_end_tick: Some(end + 20),
            winner: 2,
            winner_side: "terrorist".into(),
            reason: 1,
            reason_name: "t_killed".into(),
            is_knife_round: knife,
        }
    }

    fn sample(tick: i32, steamid: u64) -> SampleFact {
        SampleFact { tick, steamid, player: None, side: "T", pos: [1.0, 2.0, 3.0], eye: None, duck: None, alive: true, place: None }
    }

    #[test]
    fn samples_are_due_every_32_ticks_through_skipped_and_repeated_ticks() {
        let mut last = None;
        let due: Vec<_> = [-32, 0, 0, 31, 33, 64, 65, 99, 100, 163].into_iter().filter(|&tick| sample_due(&mut last, tick)).collect();
        assert_eq!(due, [0, 33, 65, 99, 163]);
    }

    #[test]
    fn samples_are_kept_in_live_rounds_only_and_never_twice_for_one_player() {
        let rounds = [round(1, Some(100), 150, false), round(2, Some(200), 240, true), round(3, None, 350, false), round(4, Some(400), 500, false)];
        let rows = [99, 100, 150, 151, 200, 240, 300, 350, 400, 501].into_iter().map(|tick| sample(tick, 1)).chain([sample(132, 2), sample(132, 2), sample(132, 3)]).collect();
        let kept = live_samples(rows, &rounds);
        assert_eq!(kept.iter().map(|p| (p.tick, p.steamid)).collect::<Vec<_>>(), [(100, 1), (132, 3), (150, 1), (400, 1)]);
    }

    #[test]
    fn a_throw_belongs_to_the_last_round_started_before_it() {
        let rounds = [round(1, Some(100), 150, false), round(2, Some(200), 240, true), round(3, Some(300), 350, false)];
        assert_eq!(round_at(&rounds, 50).map(|r| r.round_num), None);
        assert_eq!(round_at(&rounds, 250).map(|r| r.round_num), Some(1), "a knife round is no throw's");
        assert_eq!(round_at(&rounds, 290).map(|r| r.round_num), Some(3), "its start is before its freeze end");
    }

    #[test]
    fn a_fire_kind_and_its_end() {
        let t = ThrowFact { nade: "molotov", incendiary: true, detonate_tick: Some(1000), effect_ticks: Some(448), ..Default::default() };
        assert_eq!((t.grenade(), t.end_tick()), ("incendiary", Some(1448)));
        let t = ThrowFact { nade: "flash", detonate_tick: Some(1000), ..Default::default() };
        assert_eq!((t.grenade(), t.end_tick()), ("flash", None));
    }
}
