//! CS2's grenade throw, simulated: where it leaves the hand and how fast (`release`), its flight through the map with
//! its bounces (`fly`), and where and when it goes off. docs/simulator.md is the contract; decision S32 records how
//! every constant here was measured from recorded throws (`examples/flight_check.rs` is the bench).

use glam::DVec3;
use nades_scene::collider::Collider;
use serde::{Deserialize, Serialize};

/// Server ticks a second.
pub const TICK_RATE: f64 = 64.0;
/// A grenade's gravity: sv_gravity 800 times the projectile's 0.4.
pub const GRAVITY: f64 = 320.0;
/// The player's gravity (sv_gravity), for the jump before a jump-throw.
pub const PLAYER_GRAVITY: f64 = 800.0;
/// A full-strength throw's speed (`750 · 0.9`).
pub const THROW_SPEED: f64 = 675.0;
/// The share of the player's velocity the grenade takes.
pub const PLAYER_VELOCITY_SCALE: f64 = 1.25;
/// The player's vertical speed at a jump-throw's release, standing and crouched.
pub const JUMPTHROW_VZ: f64 = 218.8;
pub const CROUCH_JUMPTHROW_VZ: f64 = 221.9;
/// Running and walking speed with a grenade out (walking is 0.52 of running).
pub const RUN_SPEED: f64 = 245.0;
/// The speed holding a direction in the air gives a player who jumped from a standstill: CS2's
/// `sv_air_max_wishspeed`. Pros' jump-throws are either still (0) or drift forward at exactly this (S35).
pub const AIR_MAX_WISHSPEED: f64 = 30.0;
pub const WALK_SPEED: f64 = 127.4;
/// How far in front of the eye the grenade leaves the hand, and how much lower a weaker throw lets go.
pub const RELEASE_FORWARD: f64 = 16.0;
pub const RELEASE_DROP: f64 = 12.0;
/// A jump-throw lets go this long after take-off, this high above it (standing, crouched).
pub const JUMPTHROW_RELEASE_T: f64 = 0.107;
pub const JUMPTHROW_RISE: f64 = 25.90;
pub const CROUCH_JUMPTHROW_RISE: f64 = 26.21;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Grenade {
    Smoke,
    Flash,
    He,
    Molotov,
    Incendiary,
    Decoy,
}

/// How the player moves as they let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Movement {
    Stand,
    Walk,
    Run,
    Jumpthrow,
    RunJumpthrow,
}

/// Left is a full throw, Middle (both buttons) a medium one, Right an underhand lob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Click {
    Left,
    Middle,
    Right,
}

impl Click {
    /// The throw strength: 1 left, 0.5 both buttons, 0 right.
    pub fn strength(self) -> f64 {
        match self {
            Click::Left => 1.0,
            Click::Middle => 0.5,
            Click::Right => 0.0,
        }
    }
}

/// A throw as the player sets it up: where they stand, where they look, how they throw.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Throw {
    pub grenade: Grenade,
    /// The player's origin (feet), Hammer units; for a jump-throw, where they take off.
    pub feet: [f64; 3],
    /// Degrees, CS2's convention: pitch positive looking down.
    pub pitch: f64,
    pub yaw: f64,
    pub crouch: bool,
    pub movement: Movement,
    pub click: Click,
    /// The player's horizontal speed as they let go, along the view (units a second; negative backwards). None is the
    /// movement's own: 0 standing and for a jump-throw, 127.4 walking, 245 running. A jump-throw with forward held is
    /// [`AIR_MAX_WISHSPEED`] (30).
    #[serde(default)]
    pub speed: Option<f64>,
}

/// Where the grenade leaves the hand and its velocity then.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Release {
    /// Seconds after the throw starts (a jump-throw lets go in the air).
    pub t: f64,
    pub p0: DVec3,
    pub v0: DVec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounce {
    pub t: f64,
    pub pos: DVec3,
    pub normal: DVec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detonation {
    pub t: f64,
    pub pos: DVec3,
    /// The surface it rests on or hit last, when it touches one (a fire spreads over it).
    pub normal: Option<DVec3>,
}

/// The whole flight, sampled every tick, from the release to the detonation.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub release: Release,
    pub points: Vec<(f64, DVec3)>,
    pub bounces: Vec<Bounce>,
    pub detonation: Detonation,
}

/// The eye height over the feet: 64 standing, 46 crouched.
pub fn eye(throw: &Throw) -> DVec3 {
    DVec3::from(throw.feet) + DVec3::Z * if throw.crouch { 46.0 } else { 64.0 }
}

/// The pitch the grenade leaves at for an eye pitch (positive down): -10° looking level, the eye's range squeezed to
/// 80° either way.
pub fn throw_pitch(eye_pitch: f64) -> f64 {
    let p = if eye_pitch > 180.0 { eye_pitch - 360.0 } else { eye_pitch };
    -10.0 + p * 80.0 / 90.0
}

/// The unit vector the grenade leaves along.
pub fn throw_forward(eye_pitch: f64, yaw: f64) -> DVec3 {
    let (elevation, yaw) = ((-throw_pitch(eye_pitch)).to_radians(), yaw.to_radians());
    DVec3::new(elevation.cos() * yaw.cos(), elevation.cos() * yaw.sin(), elevation.sin())
}

/// The movement's own horizontal speed: 0 standing and for a jump-throw, walking 127.4, running 245.
pub fn movement_speed(movement: Movement) -> f64 {
    match movement {
        Movement::Stand | Movement::Jumpthrow => 0.0,
        Movement::Walk => WALK_SPEED,
        Movement::Run | Movement::RunJumpthrow => RUN_SPEED,
    }
}

/// The player's velocity as they let go: its speed (the throw's, else the movement's) forward along the view's yaw,
/// and a jump-throw rising.
pub fn player_velocity(throw: &Throw) -> DVec3 {
    let yaw = throw.yaw.to_radians();
    let ahead = DVec3::new(yaw.cos(), yaw.sin(), 0.0) * throw.speed.unwrap_or_else(|| movement_speed(throw.movement));
    let jump = if throw.crouch { CROUCH_JUMPTHROW_VZ } else { JUMPTHROW_VZ };
    match throw.movement {
        Movement::Stand | Movement::Walk | Movement::Run => ahead,
        Movement::Jumpthrow | Movement::RunJumpthrow => ahead + DVec3::Z * jump,
    }
}

/// CS2's release model: `v0 = forward(throw_pitch(pitch), yaw) · 675 · lerp(0.3, 1, strength) + 1.25 · v_player`,
/// let go 16 units ahead of the eye along that forward, 12 · (1 − strength) lower. A jump-throw lets go 0.107 s after
/// take-off, 25.9 units higher (26.2 crouched), carried forward by a run.
pub fn release(throw: &Throw) -> Release {
    let strength = throw.click.strength();
    let forward = throw_forward(throw.pitch, throw.yaw);
    let player = player_velocity(throw);
    let hand = eye(throw) + forward * RELEASE_FORWARD - DVec3::Z * RELEASE_DROP * (1.0 - strength);
    let (t, p0) = match throw.movement {
        Movement::Jumpthrow | Movement::RunJumpthrow => {
            let rise = if throw.crouch { CROUCH_JUMPTHROW_RISE } else { JUMPTHROW_RISE };
            let carried = DVec3::new(player.x, player.y, 0.0) * JUMPTHROW_RELEASE_T;
            (JUMPTHROW_RELEASE_T, hand + carried + DVec3::Z * rise)
        }
        _ => (0.0, hand),
    };
    let v0 = forward * THROW_SPEED * (0.3 + 0.7 * strength) + player * PLAYER_VELOCITY_SCALE;
    Release { t, p0, v0 }
}

/// The flight's rules, CS2's by default ([`Model::CS2`]); the bench varies them to measure each one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    pub gravity: f64,
    /// The share of its speed a grenade keeps through a bounce (`PhysicsClipVelocity` with overbounce 2, then this).
    pub elasticity: f64,
    /// A bounce off a floor (normal z above `floor_z`) slower than this stops the grenade dead.
    pub stop_speed: f64,
    pub floor_z: f64,
    /// Off a floor (normal z above `floor_z`), a grenade leaves along the normal at most this fast: a faster bounce is
    /// scaled down whole, its slide with it (S40: the game's floor bounces from above 500 u/s straight down).
    pub floor_bounce_max: f64,
    /// The grenade's box reaches this far each way from its origin.
    pub radius: f64,
    /// A molotov or incendiary bursts on a surface whose normal z is at least this (`weapon_molotov_maxdetonateslope`).
    pub molotov_floor_z: f64,
    /// The tick after the release a flash goes off (before that tick's move), an HE (after it), and a molotov bursts
    /// in the air (before it).
    pub flash_ticks: u32,
    pub he_ticks: u32,
    pub molotov_air_ticks: u32,
    /// Ticks a smoke or decoy lies still before it goes off.
    pub rest_ticks: u32,
    /// Bounces resolved in one tick; the motion left after the last one is dropped.
    pub bounces_per_tick: u32,
    /// A smoke or decoy that never comes to rest stops being simulated after this many ticks.
    pub max_ticks: u32,
}

impl Model {
    pub const CS2: Model = Model {
        gravity: GRAVITY,
        elasticity: 0.45,
        stop_speed: 20.0,
        floor_z: 0.7,
        floor_bounce_max: 225.0,
        radius: 2.0,
        molotov_floor_z: 0.866_025_403_784_438_6,
        flash_ticks: 105,
        he_ticks: 105,
        molotov_air_ticks: 130,
        rest_ticks: 3,
        bounces_per_tick: 4,
        max_ticks: 20 * 64,
    };
}

impl Default for Model {
    fn default() -> Self {
        Model::CS2
    }
}

/// The flight through the map, tick by tick, until it goes off. Without a collider it flies through everything.
pub fn fly(release: &Release, grenade: Grenade, collider: Option<&Collider>) -> Path {
    fly_with(release, grenade, collider, &Model::CS2)
}

/// [`fly`] under other rules.
pub fn fly_with(release: &Release, grenade: Grenade, collider: Option<&Collider>, model: &Model) -> Path {
    let dt = 1.0 / TICK_RATE;
    let (mut p, mut v) = (release.p0, release.v0);
    let at = |tick: u32| release.t + f64::from(tick) * dt;
    let mut points = vec![(release.t, p)];
    let mut bounces = Vec::new();
    let mut last_normal: Option<DVec3> = None;
    let (mut still, mut resting) = (0, false);
    let fire = matches!(grenade, Grenade::Molotov | Grenade::Incendiary);
    let path =
        |points: Vec<(f64, DVec3)>, bounces, t, pos, normal| Path { release: *release, points, bounces, detonation: Detonation { t, pos, normal } };
    for tick in 1..=model.max_ticks {
        // a flash, and a fire still in the air, go off before the tick's move: where the last tick left them
        if (grenade == Grenade::Flash && tick >= model.flash_ticks) || (fire && tick >= model.molotov_air_ticks) {
            points.push((at(tick), p));
            return path(points, bounces, at(tick), p, None);
        }
        if !resting {
            let before = v;
            v.z -= model.gravity * dt;
            let start = p;
            let mut motion = (before + v) * 0.5 * dt;
            let mut left = dt;
            // each bounce spends part of the tick; the grenade goes on with the rest at its new velocity
            for _ in 0..model.bounces_per_tick {
                let Some(hit) = collider.and_then(|c| trace(c, p, motion, model.radius)) else {
                    p += motion;
                    break;
                };
                bounces.push(Bounce { t: at(tick), pos: hit.pos, normal: hit.normal });
                last_normal = Some(hit.normal);
                // a fire bursts on a floor-like surface, reported where the tick began
                if fire && hit.normal.z >= model.molotov_floor_z {
                    points.push((at(tick), start));
                    return path(points, bounces, at(tick), start, Some(hit.normal));
                }
                p = hit.pos;
                // PhysicsClipVelocity with overbounce 2 (a reflection), then the elasticity
                v = (v - 2.0 * v.dot(hit.normal) * hit.normal) * model.elasticity;
                let away = v.dot(hit.normal);
                if hit.normal.z > model.floor_z && away > model.floor_bounce_max {
                    v *= model.floor_bounce_max / away;
                }
                if hit.normal.z > model.floor_z && v.length() < model.stop_speed {
                    v = DVec3::ZERO;
                    resting = true;
                    break;
                }
                left *= 1.0 - hit.fraction;
                motion = v * left;
            }
        }
        points.push((at(tick), p));
        if grenade == Grenade::He && tick >= model.he_ticks {
            return path(points, bounces, at(tick), p, None);
        }
        if matches!(grenade, Grenade::Smoke | Grenade::Decoy) {
            still = if resting { still + 1 } else { 0 };
            if still >= model.rest_ticks {
                return path(points, bounces, at(tick), p, last_normal);
            }
        }
    }
    let normal = if resting { last_normal } else { None };
    path(points, bounces, at(model.max_ticks), p, normal)
}

struct Hit {
    /// Where the grenade stops, its box a skin's width off the surface.
    pos: DVec3,
    normal: DVec3,
    /// The share of the motion it got through.
    fraction: f64,
}

/// Source keeps a moving box this far off what it hits (`DIST_EPSILON`).
const SKIN: f64 = 1.0 / 32.0;

/// The grenade's box (`half` each way) moving by `motion` from `from`: where it first meets the map, if it does. The
/// collider answers rays, so the box is swept as nine of them: its centre, stopped where the box's nearest corner
/// would touch the plane it hits, and its eight corners, which catch the ledges and edges the centre passes.
fn trace(collider: &Collider, from: DVec3, motion: DVec3, half: f64) -> Option<Hit> {
    let len = motion.length();
    if len < 1e-9 {
        return None;
    }
    let dir = motion / len;
    // the centre: the plane's support distance to the box is half · (|nx| + |ny| + |nz|)
    let mut best: Option<(f64, DVec3, f64, DVec3)> = None;
    if let Some(d) = collider.hit_distance(from, dir, len + 3.0 * half + SKIN) {
        let normal = surface_normal(collider, from, dir, d);
        let facing = (-dir.dot(normal)).max(1e-3);
        let travel = d - (half * normal.abs().element_sum() + SKIN) / facing;
        if travel <= len {
            best = Some((travel, normal, d, from));
        }
    }
    for k in 0..8 {
        let corner = DVec3::new(if k & 1 == 0 { -half } else { half }, if k & 2 == 0 { -half } else { half }, if k & 4 == 0 { -half } else { half });
        // only the corners leading the motion can touch first
        if corner.dot(dir) <= 0.0 {
            continue;
        }
        let origin = from + corner;
        let Some(d) = collider.hit_distance(origin, dir, len + SKIN) else {
            continue;
        };
        if best.is_some_and(|(t, ..)| d - SKIN >= t) {
            continue;
        }
        let normal = surface_normal(collider, origin, dir, d);
        let facing = (-dir.dot(normal)).max(1e-3);
        let travel = d - SKIN / facing;
        if travel <= len && best.is_none_or(|(t, ..)| travel < t) {
            best = Some((travel, normal, d, origin));
        }
    }
    let (travel, normal, ..) = best?;
    let travel = travel.clamp(0.0, len);
    Some(Hit { pos: from + dir * travel, normal, fraction: travel / len })
}

/// The collider answers distances only, so the surface's normal comes from rays beside the first, a quarter turn
/// apart: the plane through the hit and two side hits, kept when a third side hit lies on it too (near an edge a side
/// ray finds another face). Smaller offsets are tried next, and the ray's own reverse is the last resort.
fn surface_normal(collider: &Collider, from: DVec3, dir: DVec3, d: f64) -> DVec3 {
    let hit = from + dir * d;
    let (u, w) = dir.any_orthonormal_pair();
    for offset in [0.5, 0.05] {
        let side = |o: DVec3| collider.hit_distance(from + o * offset, dir, d + 64.0).map(|s| from + o * offset + dir * s);
        let around = [side(u), side(w), side(-u), side(-w)];
        for k in 0..4 {
            let (Some(a), Some(b), check) = (around[k], around[(k + 1) % 4], around[(k + 2) % 4]) else {
                continue;
            };
            let n = (a - hit).cross(b - hit);
            if n.length_squared() < 1e-12 {
                continue;
            }
            let n = n.normalize();
            let n = if n.dot(dir) > 0.0 { -n } else { n };
            if check.is_none_or(|c| (c - hit).dot(n).abs() < 0.2 * offset) {
                return n;
            }
        }
    }
    -dir
}

/// The console commands that put a player in this throw's spot and view.
pub fn practice_command(throw: &Throw) -> String {
    let [x, y, z] = throw.feet;
    format!("setpos {x:.2} {y:.2} {z:.2}; setang {:.2} {:.2} 0", throw.pitch, throw.yaw)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / TICK_RATE;

    fn throw(movement: Movement, click: Click, pitch: f64, crouch: bool) -> Throw {
        Throw { grenade: Grenade::Smoke, feet: [100.0, 200.0, 0.0], pitch, yaw: 90.0, crouch, movement, click, speed: None }
    }

    #[test]
    fn a_jump_throw_with_forward_held_drifts_thirty_units_a_second_along_the_view() {
        let still = throw(Movement::Jumpthrow, Click::Left, -28.89, false);
        let drifting = Throw { speed: Some(AIR_MAX_WISHSPEED), ..still };
        assert_eq!(player_velocity(&still).truncate(), glam::DVec2::ZERO);
        let v = player_velocity(&drifting);
        assert!((v.truncate() - glam::DVec2::new(0.0, 30.0)).length() < 1e-9 && v.z == JUMPTHROW_VZ, "{v}");
        // the grenade carries 1.25 of it, and the hand 0.107 s of it
        let (a, b) = (release(&still), release(&drifting));
        assert!((b.v0 - a.v0 - DVec3::new(0.0, 37.5, 0.0)).length() < 1e-9);
        assert!((b.p0 - a.p0 - DVec3::new(0.0, 30.0 * JUMPTHROW_RELEASE_T, 0.0)).length() < 1e-9);
        // a walk at another speed, and the movements' own by default
        assert!((player_velocity(&Throw { speed: Some(60.0), ..throw(Movement::Walk, Click::Left, 0.0, false) }).y - 60.0).abs() < 1e-9);
        assert!((player_velocity(&throw(Movement::Run, Click::Left, 0.0, false)).y - RUN_SPEED).abs() < 1e-9);
        // a throw written before speed reads with none
        let old: Throw = serde_json::from_str(r#"{"grenade":"Smoke","feet":[0,0,0],"pitch":0,"yaw":0,"crouch":false,"movement":"Jumpthrow","click":"Left"}"#).unwrap();
        assert_eq!(old.speed, None);
    }

    /// A square of two triangles around `centre`, spanned by `u` and `v` (half-sizes).
    fn quad(centre: DVec3, u: DVec3, v: DVec3) -> [[[f32; 3]; 3]; 2] {
        let c = |p: DVec3| [p.x as f32, p.y as f32, p.z as f32];
        let (a, b, cc, d) = (centre - u - v, centre + u - v, centre + u + v, centre - u + v);
        [[c(a), c(b), c(cc)], [c(a), c(cc), c(d)]]
    }

    fn floor_at(z: f64) -> Collider {
        Collider::new(quad(DVec3::new(0.0, 0.0, z), DVec3::X * 5000.0, DVec3::Y * 5000.0).to_vec())
    }

    #[test]
    fn the_throw_pitch_squeezes_the_view_and_lifts_it_ten_degrees() {
        assert_eq!(throw_pitch(0.0), -10.0);
        assert_eq!(throw_pitch(90.0), 70.0);
        assert_eq!(throw_pitch(-90.0), -90.0);
        assert_eq!(throw_pitch(315.0), throw_pitch(-45.0));
        let f = throw_forward(0.0, 90.0);
        assert!((f - DVec3::new(0.0, 10f64.to_radians().cos(), 10f64.to_radians().sin())).length() < 1e-12);
    }

    #[test]
    fn a_standing_throw_leaves_16_ahead_of_the_eye_at_the_click_s_speed() {
        for (click, speed, drop) in [(Click::Left, 675.0, 0.0), (Click::Middle, 438.75, 6.0), (Click::Right, 202.5, 12.0)] {
            for crouch in [false, true] {
                let t = throw(Movement::Stand, click, 20.0, crouch);
                let r = release(&t);
                let f = throw_forward(20.0, 90.0);
                assert_eq!(r.t, 0.0);
                assert!((r.p0 - (eye(&t) + f * 16.0 - DVec3::Z * drop)).length() < 1e-9);
                assert!((r.v0 - f * speed).length() < 1e-9, "{click:?}");
            }
        }
        assert_eq!(eye(&throw(Movement::Stand, Click::Left, 0.0, true)).z, 46.0);
    }

    #[test]
    fn moving_adds_a_quarter_more_than_the_player_s_velocity() {
        let f = throw_forward(0.0, 90.0) * 675.0;
        let run = release(&throw(Movement::Run, Click::Left, 0.0, false));
        assert!((run.v0 - (f + DVec3::Y * 245.0 * 1.25)).length() < 1e-9);
        let walk = release(&throw(Movement::Walk, Click::Left, 0.0, false));
        assert!((walk.v0 - (f + DVec3::Y * 127.4 * 1.25)).length() < 1e-9);
        assert_eq!(run.p0, walk.p0);
    }

    #[test]
    fn a_jump_throw_lets_go_rising_above_its_take_off() {
        let stand = release(&throw(Movement::Stand, Click::Left, 0.0, false));
        let jump = release(&throw(Movement::Jumpthrow, Click::Left, 0.0, false));
        assert_eq!(jump.t, JUMPTHROW_RELEASE_T);
        assert!((jump.p0 - (stand.p0 + DVec3::Z * 25.90)).length() < 1e-9);
        assert!((jump.v0 - (stand.v0 + DVec3::Z * 218.8 * 1.25)).length() < 1e-9);
        let crouched = release(&throw(Movement::Jumpthrow, Click::Left, 0.0, true));
        assert!((crouched.v0.z - (throw_forward(0.0, 90.0).z * 675.0 + 221.9 * 1.25)).abs() < 1e-9);
        // a running jump-throw is carried forward by the run until it lets go
        let run = release(&throw(Movement::RunJumpthrow, Click::Left, 0.0, false));
        assert!((run.p0 - (jump.p0 + DVec3::Y * 245.0 * JUMPTHROW_RELEASE_T)).length() < 1e-9);
    }

    #[test]
    fn in_the_air_it_falls_at_320_and_keeps_its_horizontal_speed() {
        let r = Release { t: 0.5, p0: DVec3::new(0.0, 0.0, 100.0), v0: DVec3::new(300.0, -200.0, 400.0) };
        let path = fly(&r, Grenade::Smoke, None);
        for (k, (t, p)) in path.points.iter().enumerate().take(200) {
            let s = k as f64 * DT;
            assert!((t - (0.5 + s)).abs() < 1e-9);
            let expected = r.p0 + r.v0 * s - DVec3::Z * 0.5 * 320.0 * s * s;
            assert!((*p - expected).length() < 1e-6, "tick {k}");
        }
    }

    #[test]
    fn a_floor_bounce_reflects_at_045_and_goes_on_for_the_rest_of_the_tick() {
        let floor = floor_at(0.0);
        let r = Release { t: 0.0, p0: DVec3::new(0.0, 0.0, 10.0), v0: DVec3::new(400.0, 0.0, -400.0) };
        let path = fly(&r, Grenade::Flash, Some(&floor));
        let b = path.bounces[0];
        assert!((b.normal - DVec3::Z).length() < 1e-6);
        // the box (2 each way) stops a skin's width off the floor
        assert!((b.pos.z - (2.0 + SKIN)).abs() < 1e-6, "{}", b.pos.z);
        let k = (b.t * TICK_RATE).round() as usize;
        // after the bounce: 0.45 of the reflected velocity, gravity again from the next tick
        let (p1, p2) = (path.points[k + 1].1, path.points[k + 2].1);
        let v = (p2 - p1) * TICK_RATE;
        assert!((v.x - 180.0).abs() < 1e-6, "{v}");
        // the bounce tick ends past the bounce point: it moved on with what was left of the tick
        assert!(path.points[k].1.z > b.pos.z && path.points[k].1.x > b.pos.x);
    }

    #[test]
    fn a_fast_floor_bounce_leaves_at_most_225_up_scaled_whole() {
        let floor = floor_at(0.0);
        let weightless = Model { gravity: 0.0, ..Model::CS2 };
        let after = |v0: DVec3| {
            let r = Release { t: 0.0, p0: DVec3::new(0.0, 0.0, 20.0), v0 };
            let path = fly_with(&r, Grenade::Flash, Some(&floor), &weightless);
            let k = (path.bounces[0].t * TICK_RATE).round() as usize;
            (path.points[k + 2].1 - path.points[k + 1].1) * TICK_RATE
        };
        // 0.45 of 400 straight down is 180, under the cap: the plain reflection
        let slow = after(DVec3::new(300.0, 0.0, -400.0));
        assert!((slow - DVec3::new(135.0, 0.0, 180.0)).length() < 1e-6, "{slow}");
        // 0.45 of 800 would be 360 up: scaled to 225, the slide with it
        let fast = after(DVec3::new(600.0, 0.0, -800.0));
        assert!((fast - DVec3::new(270.0, 0.0, 360.0) * (225.0 / 360.0)).length() < 1e-6, "{fast}");
        // a wall keeps 0.45 however fast
        let wall = Collider::new(quad(DVec3::new(50.0, 0.0, 0.0), DVec3::Y * 1000.0, DVec3::Z * 1000.0).to_vec());
        let r = Release { t: 0.0, p0: DVec3::ZERO, v0: DVec3::new(1000.0, 0.0, 0.0) };
        let path = fly_with(&r, Grenade::Flash, Some(&wall), &weightless);
        let k = (path.bounces[0].t * TICK_RATE).round() as usize;
        assert!(((path.points[k + 2].1 - path.points[k + 1].1).x * TICK_RATE + 450.0).abs() < 1e-6);
    }

    #[test]
    fn a_wall_bounce_also_goes_on_for_the_rest_of_the_tick() {
        let wall = Collider::new(quad(DVec3::new(50.0, 0.0, 0.0), DVec3::Y * 1000.0, DVec3::Z * 1000.0).to_vec());
        let r = Release { t: 0.0, p0: DVec3::new(0.0, 0.0, 0.0), v0: DVec3::new(1000.0, 0.0, 0.0) };
        let path = fly(&r, Grenade::Flash, Some(&wall));
        let b = path.bounces[0];
        assert!((b.normal + DVec3::X).length() < 1e-6);
        assert!((b.pos.x - (48.0 - SKIN)).abs() < 1e-6);
        let k = (b.t * TICK_RATE).round() as usize;
        let after = path.points[k].1;
        // the tick's motion left after the hit, at 0.45 of the reflected speed
        let left = DT - (48.0 - SKIN) / 1000.0 + (k as f64 - 1.0) * DT;
        assert!((after.x - (b.pos.x - 450.0 * left)).abs() < 1e-6, "{after}");
    }

    #[test]
    fn the_box_catches_a_ledge_its_centre_passes() {
        // a step whose top edge lies 1.5 units below the grenade's centre line
        let shelf = Collider::new(quad(DVec3::new(100.0, 0.0, -6.0), DVec3::Y * 50.0, DVec3::Z * 4.5).to_vec());
        let r = Release { t: 0.0, p0: DVec3::new(0.0, 0.0, 0.0), v0: DVec3::new(1000.0, 0.0, 0.0) };
        let path = fly_with(&r, Grenade::Flash, Some(&shelf), &Model { gravity: 0.0, ..Model::CS2 });
        let b = path.bounces.first().expect("the box's lower corners meet the step");
        assert!((b.pos.x - (98.0 - SKIN)).abs() < 1e-6);
    }

    #[test]
    fn a_slow_bounce_off_a_floor_stops_it_and_a_smoke_goes_off_three_ticks_later() {
        let floor = floor_at(0.0);
        let r = Release { t: 0.0, p0: DVec3::new(0.0, 0.0, 2.5), v0: DVec3::new(20.0, 0.0, -10.0) };
        let path = fly(&r, Grenade::Smoke, Some(&floor));
        let stop = path.bounces.last().expect("it lands");
        assert_eq!(((path.detonation.t - stop.t) * TICK_RATE).round() as u32, Model::CS2.rest_ticks - 1);
        assert!((path.detonation.pos.z - (2.0 + SKIN)).abs() < 1e-6);
        assert_eq!(path.detonation.normal, Some(stop.normal));
        // a faster one keeps bouncing first
        let fast = fly(&Release { v0: DVec3::new(300.0, 0.0, -300.0), ..r }, Grenade::Smoke, Some(&floor));
        assert!(fast.bounces.len() > 2 && fast.detonation.t > path.detonation.t);
    }

    #[test]
    fn a_flash_goes_off_before_tick_105_s_move_and_an_he_after_it() {
        let r = Release { t: 0.0, p0: DVec3::ZERO, v0: DVec3::new(100.0, 0.0, 1000.0) };
        let flash = fly(&r, Grenade::Flash, None);
        assert_eq!((flash.detonation.t * TICK_RATE).round(), 105.0);
        assert_eq!(flash.detonation.pos, flash.points[104].1);
        assert_eq!(flash.detonation.normal, None);
        let he = fly(&r, Grenade::He, None);
        assert_eq!((he.detonation.t * TICK_RATE).round(), 105.0);
        assert!((he.detonation.pos - (r.p0 + r.v0 * 105.0 * DT - DVec3::Z * 160.0 * (105.0 * DT).powi(2))).length() < 1e-6);
    }

    #[test]
    fn a_fire_bursts_on_a_floor_and_bounces_off_a_steep_slope() {
        let floor = floor_at(0.0);
        let r = Release { t: 0.0, p0: DVec3::new(0.0, 0.0, 50.0), v0: DVec3::new(100.0, 0.0, 0.0) };
        let path = fly(&r, Grenade::Molotov, Some(&floor));
        assert!(path.bounces.len() == 1 && path.detonation.normal.is_some());
        // reported where its last tick began, on the tick it touched
        let k = (path.detonation.t * TICK_RATE).round() as usize;
        assert_eq!(path.detonation.pos, path.points[k - 1].1);
        // a 45° ramp is too steep: it bounces off
        let ramp = Collider::new(quad(DVec3::new(100.0, 0.0, 0.0), DVec3::new(500.0, 0.0, 500.0), DVec3::Y * 500.0).to_vec());
        let fire = fly(&Release { p0: DVec3::new(100.0, 0.0, 60.0), v0: DVec3::new(0.0, 0.0, -10.0), ..r }, Grenade::Incendiary, Some(&ramp));
        assert!(fire.bounces.len() > 1 || fire.detonation.normal.is_none());
        assert!(fire.bounces[0].normal.z < 0.8);
        // and with nothing below it bursts in the air at tick 130
        let air = fly(&r, Grenade::Molotov, None);
        assert_eq!((air.detonation.t * TICK_RATE).round(), 130.0);
        assert_eq!(air.detonation.normal, None);
    }

    #[test]
    fn the_surface_normal_is_the_hit_plane_s() {
        let n = DVec3::new(0.3, -0.2, 1.0).normalize();
        let (u, v) = n.any_orthonormal_pair();
        let slope = Collider::new(quad(DVec3::new(10.0, 20.0, 0.0), u * 300.0, v * 300.0).to_vec());
        let from = DVec3::new(10.0, 20.0, 100.0);
        let d = slope.hit_distance(from, DVec3::NEG_Z, 500.0).expect("it hits");
        assert!((surface_normal(&slope, from, DVec3::NEG_Z, d) - n).length() < 1e-4);
    }

    #[test]
    fn a_path_runs_from_the_release_to_the_detonation_every_tick() {
        let path = fly(&release(&throw(Movement::Jumpthrow, Click::Left, -30.0, false)), Grenade::He, None);
        assert_eq!(path.points.first().map(|p| p.0), Some(JUMPTHROW_RELEASE_T));
        assert_eq!(path.points.last().map(|p| (p.0, p.1)), Some((path.detonation.t, path.detonation.pos)));
        assert!(path.points.windows(2).all(|w| ((w[1].0 - w[0].0) - DT).abs() < 1e-9));
    }
}
