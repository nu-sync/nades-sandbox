//! The physics bench: recorded throws from a cs2-throws file against the simulation.
//!
//! For each map with a pack and each grenade it reports the landing error of the flight simulated from the recorded
//! release (median, p90, share within 32 units), the detonation time error, and why the worst flights part from the
//! game's (see `Why`); then the release error from the recorded setup per movement. `--fixture` writes the committed
//! test fixture (crates/physics/tests/fixture.json); `--set key=value` changes one rule of the model, for sweeps.
//!
//! cargo run --release -p nades-physics --example flight_check -- --throws <cs2-throws file> [--per 400] [--maps
//! de_cache,de_dust2] [--set elasticity=0.45] [--fixture crates/physics/tests/fixture.json]

use std::collections::BTreeMap;
use std::path::PathBuf;

use glam::DVec3;
use nades_physics::{Click, Grenade, Model, Movement, Release, TICK_RATE, Throw, fly_with};
use nades_scene::collider::{Collider, par_map};

/// One recorded throw, as the file has it.
#[derive(Debug, Clone)]
struct Recorded {
    key: i64,
    map: String,
    grenade: Grenade,
    start: DVec3,
    vel: DVec3,
    /// The step after the release the detonate event fires on, and the position it reports.
    det_step: i64,
    det: DVec3,
    /// The networked flight: steps after the release (the start is step 0) and positions.
    flight: Vec<(i64, DVec3)>,
    setup: Option<Throw>,
    /// The player's recorded velocity matches the movement's canonical one (for the release check).
    canonical: bool,
    demo: i64,
    tick: i64,
    /// Each living player's feet over the flight (steps after the release), from the file's player samples.
    players: Vec<Vec<(i64, DVec3)>>,
}

fn grenade(name: &str) -> Option<Grenade> {
    Some(match name {
        "smoke" => Grenade::Smoke,
        "flash" => Grenade::Flash,
        "he" => Grenade::He,
        "molotov" => Grenade::Molotov,
        "incendiary" => Grenade::Incendiary,
        "decoy" => Grenade::Decoy,
        _ => return None,
    })
}

/// The setup a recorded throw had, in the simulator's terms, when it is one of the five movements.
fn setup(r: &rusqlite::Row) -> rusqlite::Result<(Option<Throw>, bool)> {
    let get = |i: &str| r.get::<_, Option<f64>>(i);
    let (Some(pitch), Some(yaw), Some(duck), Some(pvx), Some(pvy)) = (get("pitch")?, get("yaw")?, get("duck")?, get("pvel_x")?, get("pvel_y")?)
    else {
        return Ok((None, false));
    };
    let on_ground = r.get::<_, Option<i64>>("on_ground")?.unwrap_or(0) == 1;
    let walking = r.get::<_, Option<i64>>("walking")?.unwrap_or(0) == 1;
    let g = grenade(&r.get::<_, String>("grenade")?).expect("a grenade");
    let vel = DVec3::new(r.get("vel_x")?, r.get("vel_y")?, r.get("vel_z")?);
    // the release model inverted (show-me-the-nades' physics.rs): strength and the player's vertical speed
    let f = nades_physics::throw_forward(pitch, yaw);
    let (rx, ry) = (vel.x - 1.25 * pvx, vel.y - 1.25 * pvy);
    let speed = (rx * f.x + ry * f.y) / (f.x * f.x + f.y * f.y);
    let strength = (speed / 675.0 - 0.3) / 0.7;
    let pvz = (vel.z - f.z * speed) / 1.25;
    let click = if strength > 0.75 {
        Click::Left
    } else if strength < 0.25 {
        Click::Right
    } else {
        Click::Middle
    };
    let hs = pvx.hypot(pvy);
    let along = if hs > 1.0 { (pvx * yaw.to_radians().cos() + pvy * yaw.to_radians().sin()) / hs } else { 1.0 };
    let crouch = duck > 0.5;
    let (movement, feet, canonical) = if on_ground {
        let feet = [get("origin_x")?, get("origin_y")?, get("origin_z")?];
        let m = if hs < 5.0 {
            Movement::Stand
        } else if walking {
            Movement::Walk
        } else {
            Movement::Run
        };
        let target = match m {
            Movement::Walk => nades_physics::WALK_SPEED,
            Movement::Run => nades_physics::RUN_SPEED,
            _ => 0.0,
        };
        (m, feet, (hs - target).abs() < 1.0 && along > 0.9998 && !(0.01..=0.99).contains(&duck))
    } else {
        let canonical_vz = (pvz - nades_physics::JUMPTHROW_VZ).abs() < 1.0 || (pvz - nades_physics::CROUCH_JUMPTHROW_VZ).abs() < 1.0;
        if !canonical_vz {
            return Ok((None, false));
        }
        let feet = [get("takeoff_x")?, get("takeoff_y")?, get("takeoff_z")?];
        let m = if hs < 100.0 { Movement::Jumpthrow } else { Movement::RunJumpthrow };
        let target = if m == Movement::Jumpthrow { 0.0 } else { nades_physics::RUN_SPEED };
        (m, feet, (hs - target).abs() < 1.0 && along > 0.9998 && !(0.01..=0.99).contains(&duck))
    };
    let [Some(x), Some(y), Some(z)] = feet else {
        return Ok((None, false));
    };
    Ok((Some(Throw { grenade: g, feet: [x, y, z], pitch, yaw, crouch, movement, click }), canonical))
}

fn read(file: &str, maps: &[String], per: usize) -> Vec<Recorded> {
    let db = rusqlite::Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("the cs2-throws file opens");
    let mut out = Vec::new();
    for map in maps {
        for name in ["smoke", "flash", "he", "molotov", "incendiary", "decoy"] {
            let count: i64 = db
                .query_row("select count(*) from throws t join demos d using (demo) where d.map = ?1 and t.grenade = ?2 and t.flight is not null and t.detonate_tick is not null", [map, name], |r| r.get(0))
                .expect("count");
            // an even spread over the file's order: every n-th throw
            let stride = (count as usize).div_ceil(per.max(1)).max(1) as i64;
            let mut q = db
                .prepare(
                    "select t.throw, t.demo, t.grenade, t.tick, t.start_x, t.start_y, t.start_z, t.vel_x, t.vel_y, t.vel_z, t.detonate_tick, t.det_x, t.det_y, t.det_z, t.flight,
                            t.origin_x, t.origin_y, t.origin_z, t.takeoff_x, t.takeoff_y, t.takeoff_z, t.pitch, t.yaw, t.pvel_x, t.pvel_y, t.on_ground, t.duck, t.walking
                     from throws t join demos d using (demo)
                     where d.map = ?1 and t.grenade = ?2 and t.flight is not null and t.detonate_tick is not null and t.vel_x is not null
                     order by t.throw",
                )
                .expect("query");
            let rows = q
                .query_map(rusqlite::params![map, name], |r| {
                    let g = grenade(&r.get::<_, String>("grenade")?).expect("a grenade");
                    let tick: i64 = r.get("tick")?;
                    let blob: Vec<u8> = r.get("flight")?;
                    // per point: i32 tick, f32 x, y, z; the start is the tick before the first point's
                    let flight = blob
                        .chunks_exact(16)
                        .map(|c| {
                            let f = |o: usize| f64::from(f32::from_le_bytes(c[o..o + 4].try_into().expect("four bytes")));
                            (i64::from(i32::from_le_bytes(c[0..4].try_into().expect("four bytes"))) - tick + 1, DVec3::new(f(4), f(8), f(12)))
                        })
                        .collect();
                    let (setup, canonical) = setup(r)?;
                    Ok(Recorded {
                        key: r.get("throw")?,
                        map: map.clone(),
                        grenade: g,
                        start: DVec3::new(r.get("start_x")?, r.get("start_y")?, r.get("start_z")?),
                        vel: DVec3::new(r.get("vel_x")?, r.get("vel_y")?, r.get("vel_z")?),
                        det_step: r.get::<_, i64>("detonate_tick")? - tick + 1,
                        det: DVec3::new(r.get("det_x")?, r.get("det_y")?, r.get("det_z")?),
                        flight,
                        setup,
                        canonical,
                        demo: r.get("demo")?,
                        tick,
                        players: Vec::new(),
                    })
                })
                .expect("rows");
            out.extend(rows.filter_map(Result::ok).enumerate().filter(|(k, _)| *k as i64 % stride == 0).map(|(_, r)| r));
        }
    }
    let mut samples = db
        .prepare("select player, tick, x, y, z from player_samples where demo = ?1 and tick between ?2 and ?3 and alive = 1 order by player, tick")
        .expect("query");
    for r in &mut out {
        let end = r.tick + r.det_step + 32;
        let rows = samples
            .query_map(rusqlite::params![r.demo, r.tick - 32, end], |s| {
                Ok((s.get::<_, i64>(0)?, s.get::<_, i64>(1)?, DVec3::new(s.get(2)?, s.get(3)?, s.get(4)?)))
            })
            .expect("samples");
        let mut last = None;
        for (player, tick, p) in rows.filter_map(Result::ok) {
            if last != Some(player) {
                r.players.push(Vec::new());
                last = Some(player);
            }
            r.players.last_mut().expect("a track").push((tick - r.tick + 1, p));
        }
    }
    out
}

/// Why a simulated flight lands more than 32 units off, judged where it first parts from the game's by more than 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Why {
    /// Lands within 32 units.
    Lands,
    /// The game's grenade turned beside a living player: players are not in the collider.
    Player,
    /// The game's grenade kept less than 0.36 of its speed through the bounce (0.45 is the rule): a surface material
    /// with its own elasticity, which the collider does not carry.
    SoftSurface,
    /// The game's grenade turned where the collider has nothing: a surface the collision mesh lacks, or the game's
    /// box catching an edge between the rays that sweep it.
    MissingSurface,
    /// The simulation bounced where the game's grenade flew on: a surface the game's physics does not have.
    ExtraSurface,
    /// Both bounced there but went different ways: the surface's normal differs (an edge, a slope, a displacement).
    Normal,
    /// The flights part without a fresh bounce: rolling and small hops that drift apart, or where it comes to rest.
    Rolling,
}

/// Steps where the recorded flight changes velocity other than by gravity (a bounce, or the grenade stopping).
fn recorded_bounces(flight: &[(i64, DVec3)], gravity: f64) -> Vec<i64> {
    let mut out = Vec::new();
    for w in flight.windows(3) {
        let (a, b, c) = (w[0], w[1], w[2]);
        let (d1, d2) = ((b.0 - a.0) as f64 / TICK_RATE, (c.0 - b.0) as f64 / TICK_RATE);
        if d1 <= 0.0 || d2 <= 0.0 {
            continue;
        }
        let (v1, v2) = ((b.1 - a.1) / d1, (c.1 - b.1) / d2);
        let expected = v1 - DVec3::Z * gravity * (d1 + d2) * 0.5;
        if (v2 - expected).length() > 8.0 {
            out.push(b.0);
        }
    }
    out
}

struct Outcome {
    error: f64,
    dt_ticks: i64,
    why: Why,
}

/// The recorded velocity over the flight's intervals: (the step it ends on, velocity).
fn intervals(flight: &[(i64, DVec3)]) -> Vec<(i64, DVec3)> {
    flight.windows(2).filter(|w| w[1].0 > w[0].0).map(|w| (w[1].0, (w[1].1 - w[0].1) * TICK_RATE / (w[1].0 - w[0].0) as f64)).collect()
}

fn judge(r: &Recorded, release: &Release, collider: &Collider, model: &Model) -> Outcome {
    let path = fly_with(release, r.grenade, Some(collider), model);
    let error = (path.detonation.pos - r.det).length();
    let sim_step = (path.detonation.t * TICK_RATE).round() as i64;
    let dt_ticks = sim_step - r.det_step;
    if error <= 32.0 {
        return Outcome { error, dt_ticks, why: Why::Lands };
    }
    let sim_at = |step: i64| path.points.get(step as usize).map(|p| p.1);
    let parted = r.flight.iter().find(|(s, p)| sim_at(*s).is_some_and(|q| (q - *p).length() > 8.0)).map(|(s, _)| *s);
    let Some(parted) = parted else {
        return Outcome { error, dt_ticks, why: Why::Rolling };
    };
    let game = recorded_bounces(&r.flight, model.gravity).into_iter().rfind(|s| *s <= parted && parted - s <= 6);
    let sim = path.bounces.iter().map(|b| (b.t * TICK_RATE).round() as i64).any(|s| s <= parted && parted - s <= 6);
    let why = match game {
        Some(b) => {
            let at = r.flight.iter().find(|(s, _)| *s == b).map_or(DVec3::ZERO, |p| p.1);
            let near_player = r.players.iter().any(|track| {
                // the player's feet at the bounce, between the samples either side of it
                let before = track.iter().rev().find(|(s, _)| *s <= b);
                let after = track.iter().find(|(s, _)| *s >= b);
                let feet = match (before, after) {
                    (Some(&(s0, p0)), Some(&(s1, p1))) if s1 > s0 => p0.lerp(p1, (b - s0) as f64 / (s1 - s0) as f64),
                    (Some(&(_, p)), _) | (_, Some(&(_, p))) => p,
                    _ => return false,
                };
                (at - feet).truncate().length() <= 32.0 && at.z >= feet.z - 4.0 && at.z <= feet.z + 76.0
            });
            let v = intervals(&r.flight);
            let k = v.iter().position(|(s, _)| *s == b);
            let kept = k.and_then(|k| Some(v.get(k + 2)?.1.length() / v.get(k)?.1.length().max(1.0)));
            if near_player && b > 4 {
                Why::Player
            } else if kept.is_some_and(|e| e < 0.36) {
                Why::SoftSurface
            } else if !sim {
                Why::MissingSurface
            } else {
                Why::Normal
            }
        }
        None if sim => Why::ExtraSurface,
        None => Why::Rolling,
    };
    Outcome { error, dt_ticks, why }
}

/// The throw went off in the air without touching anything: no turn in its flight up to two ticks past the detonation
/// (an HE that bounces on its last tick is reported off its networked position), the event where a flight point is,
/// and a fire that burst in the air.
fn free(r: &Recorded, model: &Model) -> bool {
    let untouched = recorded_bounces(&r.flight, model.gravity).first().is_none_or(|b| *b > r.det_step + 2);
    // an HE reports where its last move left it, the others where the tick began
    let at = if r.grenade == Grenade::He { r.det_step } else { r.det_step - 1 };
    let on_path = r.flight.iter().any(|(s, p)| *s == at && (*p - r.det).length() < 0.1);
    let fire = matches!(r.grenade, Grenade::Molotov | Grenade::Incendiary);
    untouched && on_path && (!fire || r.det_step >= i64::from(model.molotov_air_ticks))
}

/// A recorded throw as the committed fixture keeps it: derived numbers only (the release, the networked flight until
/// it first turns, every 8th tick, the detonation, the setup), and the errors this model has on it.
fn fixture_entry(r: &Recorded, o: &Outcome, model: &Model) -> serde_json::Value {
    let round = |v: f64| (v * 1000.0).round() / 1000.0;
    let bounces = recorded_bounces(&r.flight, model.gravity);
    let first = bounces.first().copied().unwrap_or(i64::MAX);
    let mut next = 0;
    let flight: Vec<_> = r
        .flight
        .iter()
        .filter(|(s, _)| *s < first)
        .filter(|(s, _)| {
            let keep = *s >= next;
            if keep {
                next = s + 8;
            }
            keep
        })
        .map(|(s, p)| serde_json::json!([s, round(p.x), round(p.y), round(p.z)]))
        .collect();
    let release = r.setup.filter(|_| r.canonical).map(|setup| {
        let rel = nades_physics::release(&setup);
        serde_json::json!({ "throw": setup, "p0_error": round((rel.p0 - r.start).length()) + 0.001, "v0_error": round((rel.v0 - r.vel).length()) + 0.001 })
    });
    serde_json::json!({
        "map": r.map, "grenade": r.grenade, "start": r.start.to_array(), "vel": r.vel.to_array(),
        "det_step": r.det_step, "det": r.det.to_array().map(round),
        // the flight reaches the detonation without turning: no collider needed to land it
        "free": free(r, model),
        "landing_error": (o.error * 100.0).ceil() / 100.0, "why": format!("{:?}", o.why),
        "flight": flight, "release": release,
    })
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let file = flag("--throws").expect("--throws <cs2-throws file>");
    let per: usize = flag("--per").map_or(300, |s| s.parse().expect("--per <count>"));
    let packs = nades_config::packs(flag("--packs").as_deref().map(std::path::Path::new)).expect("the packs folder").path;
    let maps: Vec<String> = match flag("--maps") {
        Some(m) => m.split(',').map(str::to_string).collect(),
        None => ["de_ancient", "de_anubis", "de_cache", "de_dust2", "de_inferno", "de_mirage", "de_nuke"]
            .iter()
            .filter(|m| packs.join(format!("{m}.pack")).exists())
            .map(|m| m.to_string())
            .collect(),
    };
    let tight: f64 = flag("--tight").map_or(32.0, |s| s.parse().expect("--tight <units>"));
    let mut model = Model::CS2;
    for (i, a) in args.iter().enumerate() {
        if a != "--set" {
            continue;
        }
        let (k, v) = args[i + 1].split_once('=').expect("--set key=value");
        let f: f64 = v.parse().expect("a number");
        match k {
            "gravity" => model.gravity = f,
            "elasticity" => model.elasticity = f,
            "stop_speed" => model.stop_speed = f,
            "floor_z" => model.floor_z = f,
            "radius" => model.radius = f,
            "molotov_floor_z" => model.molotov_floor_z = f,
            "flash_ticks" => model.flash_ticks = f as u32,
            "he_ticks" => model.he_ticks = f as u32,
            "molotov_air_ticks" => model.molotov_air_ticks = f as u32,
            "rest_ticks" => model.rest_ticks = f as u32,
            "bounces_per_tick" => model.bounces_per_tick = f as u32,
            _ => panic!("no rule {k}"),
        }
    }
    eprintln!("model {model:?}");
    let recorded = read(&file, &maps, per);
    eprintln!("{} recorded throws", recorded.len());

    if let Some(key) = flag("--show") {
        let key: i64 = key.parse().expect("--show <throw key>");
        let r = recorded.iter().find(|r| r.key == key).expect("that throw is in the sample");
        let pack = nades_pack::Pack::open(&packs.join(format!("{}.pack", r.map))).expect("the pack opens");
        let collider = Collider::from_awmh(pack.collider().expect("a Collider section")).expect("a triangle mesh");
        let path = fly_with(&Release { t: 0.0, p0: r.start, v0: r.vel }, r.grenade, Some(&collider), &model);
        for (s, p) in &r.flight {
            let q = path.points.get(*s as usize).map_or(DVec3::NAN, |q| q.1);
            let bounce = path
                .bounces
                .iter()
                .filter(|b| ((b.t * TICK_RATE).round() as i64 - s).abs() <= 1)
                .map(|b| format!(" bounce n {:.2} {:.2} {:.2}", b.normal.x, b.normal.y, b.normal.z))
                .collect::<String>();
            println!(
                "{s:>4} game {:>8.1} {:>8.1} {:>8.1}  sim {:>8.1} {:>8.1} {:>8.1}  off {:>6.1}{bounce}",
                p.x,
                p.y,
                p.z,
                q.x,
                q.y,
                q.z,
                (q - *p).length()
            );
        }
        println!("det game step {} {:?}  sim {:.0} {:?}", r.det_step, r.det, path.detonation.t * TICK_RATE, path.detonation.pos);
        return;
    }
    if args.iter().any(|a| a == "--rest-gap") {
        // how far above the collider's floor the game's smokes and decoys lie when they go off
        for map in &maps {
            let pack = nades_pack::Pack::open(&packs.join(format!("{map}.pack"))).expect("the pack opens");
            let collider = Collider::from_awmh(pack.collider().expect("a Collider section")).expect("a triangle mesh");
            let mut gaps: Vec<f64> = recorded
                .iter()
                .filter(|r| &r.map == map && matches!(r.grenade, Grenade::Smoke | Grenade::Decoy))
                .filter_map(|r| collider.floor_below(r.det + DVec3::Z * 8.0, 64.0).map(|f| r.det.z - f))
                .collect();
            gaps.sort_by(f64::total_cmp);
            println!(
                "{map:<11} n {:>5} gap p10 {:.2} p25 {:.2} median {:.2} p75 {:.2} p90 {:.2}",
                gaps.len(),
                quantile(&gaps, 0.1),
                quantile(&gaps, 0.25),
                quantile(&gaps, 0.5),
                quantile(&gaps, 0.75),
                quantile(&gaps, 0.9)
            );
        }
        return;
    }
    if args.iter().any(|a| a == "--first-bounce") {
        // the error 8 ticks after the game's first bounce, for flights that are still together just before it
        let mut errors: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for map in &maps {
            let pack = nades_pack::Pack::open(&packs.join(format!("{map}.pack"))).expect("the pack opens");
            let collider = Collider::from_awmh(pack.collider().expect("a Collider section")).expect("a triangle mesh");
            let throws: Vec<&Recorded> = recorded.iter().filter(|r| &r.map == map).collect();
            let found = par_map(&throws, |r| {
                let b = recorded_bounces(&r.flight, model.gravity).into_iter().find(|s| *s > 2)?;
                let path = fly_with(&Release { t: 0.0, p0: r.start, v0: r.vel }, r.grenade, Some(&collider), &model);
                let before = r.flight.iter().rev().find(|(s, _)| *s < b)?;
                if (path.points.get(before.0 as usize)?.1 - before.1).length() > 1.0 {
                    return None;
                }
                let after = r.flight.iter().find(|(s, _)| *s >= b + 8)?;
                Some((path.points.get(after.0 as usize)?.1 - after.1).length())
            });
            for (r, e) in throws.iter().zip(found) {
                if let Some(e) = e {
                    errors.entry(format!("{:?}", r.grenade)).or_default().push(e);
                }
            }
        }
        for (g, e) in &mut errors {
            e.sort_by(f64::total_cmp);
            let within = |d: f64| e.iter().filter(|x| **x <= d).count() as f64 / e.len() as f64 * 100.0;
            println!(
                "{g:<10} n {:>5} median {:>5.2} p75 {:>5.2} p90 {:>6.2}  ≤1 {:>3.0}%  ≤4 {:>3.0}%",
                e.len(),
                quantile(e, 0.5),
                quantile(e, 0.75),
                quantile(e, 0.9),
                within(1.0),
                within(4.0)
            );
        }
        return;
    }
    let mut fixture = Vec::new();
    let mut all: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut whys: BTreeMap<(String, Why), usize> = BTreeMap::new();
    let mut dt_all: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    println!("{:<11} {:<10} {:>5} {:>8} {:>8} {:>7} {:>9} {:>9}", "map", "grenade", "n", "median", "p90", "≤32", "dt=0", "|dt|≤2");
    for map in &maps {
        let path = packs.join(format!("{map}.pack"));
        let pack = nades_pack::Pack::open(&path).expect("the pack opens");
        let collider = Collider::from_awmh(pack.collider().expect("a Collider section")).expect("a triangle mesh");
        let throws: Vec<&Recorded> = recorded.iter().filter(|r| &r.map == map).collect();
        let outcomes = par_map(&throws, |r| judge(r, &Release { t: 0.0, p0: r.start, v0: r.vel }, &collider, &model));
        for name in ["smoke", "flash", "he", "molotov", "incendiary", "decoy"] {
            let g = grenade(name).expect("a grenade");
            let picked: Vec<(&&Recorded, &Outcome)> = throws.iter().zip(&outcomes).filter(|(r, _)| r.grenade == g).collect();
            if picked.is_empty() {
                continue;
            }
            let mut errors: Vec<f64> = picked.iter().map(|(_, o)| o.error).collect();
            errors.sort_by(f64::total_cmp);
            let within = errors.iter().filter(|e| **e <= 32.0).count() as f64 / errors.len() as f64;
            let exact = picked.iter().filter(|(_, o)| o.dt_ticks == 0).count() as f64 / picked.len() as f64;
            let near = picked.iter().filter(|(_, o)| o.dt_ticks.abs() <= 2).count() as f64 / picked.len() as f64;
            println!(
                "{map:<11} {name:<10} {:>5} {:>8.1} {:>8.1} {:>6.0}% {:>8.0}% {:>8.0}%",
                errors.len(),
                quantile(&errors, 0.5),
                quantile(&errors, 0.9),
                within * 100.0,
                exact * 100.0,
                near * 100.0
            );
            all.entry(name.to_string()).or_default().extend(errors);
            let mut dts: Vec<f64> = picked
                .iter()
                .filter(|(_, o)| o.why == Why::Lands && (o.error <= tight || matches!(g, Grenade::Flash | Grenade::He)))
                .map(|(_, o)| o.dt_ticks as f64)
                .collect();
            dts.sort_by(f64::total_cmp);
            dt_all.entry(name.to_string()).or_default().extend(dts);
            for (r, o) in &picked {
                *whys.entry((name.to_string(), o.why)).or_default() += 1;
                if args.iter().any(|a| a == "--list") && (o.why != Why::Lands || o.dt_ticks.abs() > 4 || o.error > 8.0) {
                    println!("  {} {name} {:?} error {:.0} dt {}", r.key, o.why, o.error, o.dt_ticks);
                }
            }
            // the fixture, per grenade and map: the first two, the first that lands far off, the first that goes off
            // before it touches anything, and the first thrown from a canonical setup
            let chosen = picked
                .iter()
                .take(2)
                .chain(picked.iter().filter(|(_, o)| o.why != Why::Lands).take(1))
                .chain(picked.iter().filter(|(r, _)| free(r, &model)).take(1))
                .chain(picked.iter().filter(|(r, _)| r.canonical).take(1));
            let mut keys = Vec::new();
            for (r, o) in chosen {
                if !keys.contains(&r.key) {
                    keys.push(r.key);
                    fixture.push(fixture_entry(r, o, &model));
                }
            }
        }
    }
    println!("\nall maps");
    for (name, errors) in &mut all {
        errors.sort_by(f64::total_cmp);
        let within = errors.iter().filter(|e| **e <= 32.0).count() as f64 / errors.len() as f64;
        println!(
            "{name:<10} {:>6} median {:>6.1} p90 {:>7.1} ≤32 {:>4.0}%",
            errors.len(),
            quantile(errors, 0.5),
            quantile(errors, 0.9),
            within * 100.0
        );
    }
    println!("\ndetonation time error of the flights that land within 32 units, ticks (simulated − game)");
    for (name, dts) in &mut dt_all {
        dts.sort_by(f64::total_cmp);
        let exact = dts.iter().filter(|d| **d == 0.0).count() as f64 / dts.len().max(1) as f64;
        println!(
            "{name:<10} p10 {:>4} median {:>4} p90 {:>4} exact {:>4.0}%",
            quantile(dts, 0.1),
            quantile(dts, 0.5),
            quantile(dts, 0.9),
            exact * 100.0
        );
    }
    println!("\nwhere the flights part (more than 32 units off)");
    for ((name, why), n) in &whys {
        println!("{name:<10} {why:?}: {n}");
    }

    println!("\nrelease from the recorded setup (p0 error, v0 error, units and u/s)");
    let mut by: BTreeMap<(String, bool), (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for r in &recorded {
        let Some(setup) = r.setup else { continue };
        let rel = nades_physics::release(&setup);
        // a jump-throw's start is where it is let go in the air; compare like with like
        let e = by.entry((format!("{:?}", setup.movement), r.canonical)).or_default();
        e.0.push((rel.p0 - r.start).length());
        e.1.push((rel.v0 - r.vel).length());
    }
    for ((m, canonical), (p, v)) in &mut by {
        p.sort_by(f64::total_cmp);
        v.sort_by(f64::total_cmp);
        println!(
            "{m:<13} {:<13} n {:>5}  p0 median {:>6.2} p90 {:>6.2}  v0 median {:>7.2} p90 {:>7.2}",
            if *canonical { "canonical" } else { "any speed" },
            p.len(),
            quantile(p, 0.5),
            quantile(p, 0.9),
            quantile(v, 0.5),
            quantile(v, 0.9)
        );
    }
    if let Some(out) = flag("--fixture") {
        // one throw a line, so a change to the fixture reads as a diff of throws
        let head = serde_json::json!({ "source": "show-me-the-nades' nades.duckdb (247 demos) through nades export --cs2-throws, 2026-10-27", "model": format!("{model:?}") });
        let throws: Vec<String> = fixture.iter().map(|t| serde_json::to_string(t).expect("json")).collect();
        let head = serde_json::to_string(&head).expect("json");
        std::fs::write(PathBuf::from(&out), format!("{},\n\"throws\": [\n{}\n]}}\n", &head[..head.len() - 1], throws.join(",\n")))
            .expect("the fixture is written");
        eprintln!("wrote {} throws to {out}", fixture.len());
    }
}
