//! CS2's grenade throw, simulated: where it leaves the hand and how fast (`release`), its flight through the map with
//! its bounces (`fly`), and where and when it goes off. docs/simulator.md is the contract.
//!
//! SKELETON: the types and signatures are the contract; the bodies are placeholders (a release without movement, a
//! flight that stops at its first hit) for the physics unit to replace and verify against recorded throws.

use glam::DVec3;
use nades_scene::collider::Collider;
use serde::{Deserialize, Serialize};

/// Server ticks a second.
pub const TICK_RATE: f64 = 64.0;

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

/// CS2's release model (`v0 = forward(throw_pitch(pitch), yaw) · 675 · lerp(0.3, 1, strength) + 1.25 · v_player`).
/// PLACEHOLDER: no player velocity; the release point is the eye.
pub fn release(throw: &Throw) -> Release {
    let strength = match throw.click {
        Click::Left => 1.0,
        Click::Middle => 0.5,
        Click::Right => 0.0,
    };
    let pitch = if throw.pitch < 90.0 { -10.0 + throw.pitch * 80.0 / 90.0 } else { -10.0 + (360.0 - throw.pitch) * -80.0 / 90.0 };
    let (elevation, yaw) = ((-pitch).to_radians(), throw.yaw.to_radians());
    let forward = DVec3::new(elevation.cos() * yaw.cos(), elevation.cos() * yaw.sin(), elevation.sin());
    Release { t: 0.0, p0: eye(throw), v0: forward * 675.0 * (0.3 + 0.7 * strength) }
}

/// The flight through the map. PLACEHOLDER: gravity only, stopping where it first meets the collider (or after 3 s).
pub fn fly(release: &Release, _grenade: Grenade, collider: Option<&Collider>) -> Path {
    let dt = 1.0 / TICK_RATE;
    let (mut p, mut v, mut t) = (release.p0, release.v0, release.t);
    let mut points = vec![(t, p)];
    let mut normal = None;
    while t - release.t < 3.0 {
        let next = p + v * dt + DVec3::new(0.0, 0.0, -0.5 * 320.0 * dt * dt);
        if let Some(c) = collider {
            let step = next - p;
            if let Some(d) = c.hit_distance(p, step.normalize_or_zero(), step.length()) {
                p += step.normalize_or_zero() * d;
                t += dt;
                points.push((t, p));
                normal = Some(DVec3::Z);
                break;
            }
        }
        v.z -= 320.0 * dt;
        p = next;
        t += dt;
        points.push((t, p));
    }
    Path { release: *release, points, bounces: Vec::new(), detonation: Detonation { t, pos: p, normal } }
}

/// The console commands that put a player in this throw's spot and view.
pub fn practice_command(throw: &Throw) -> String {
    let [x, y, z] = throw.feet;
    format!("setpos {x:.2} {y:.2} {z:.2}; setang {:.2} {:.2} 0", throw.pitch, throw.yaw)
}
