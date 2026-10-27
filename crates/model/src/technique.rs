//! How a grenade was thrown, from CS2's release model: a port of show-me-the-nades' `physics.rs`, verified on real
//! demos there (median residual 0.004 u/s), with its tests.
//!
//! CS2 keeps the CS:GO release model:
//! `v0 = forward(throw_pitch(eye_pitch), yaw) · 675 · lerp(0.3, 1, strength) + 1.25 · v_player`.
//! Solving it with the pawn's horizontal velocity recovers the throw strength (left, middle or right click) and the
//! player's vertical speed at release, which marks a jump-throw.

use std::fmt;

use crate::facts::Throw;

/// Full-strength release speed (`750 · 0.9`).
pub const THROW_SPEED: f64 = 675.0;
/// The share of the player's velocity added to the grenade.
pub const PLAYER_VELOCITY_SCALE: f64 = 1.25;
/// The player's vertical speed CS2 uses for a jump-throw released early in the jump.
pub const JUMPTHROW_VZ: f64 = 218.8;
/// The same, for a crouch-jump.
pub const CROUCH_JUMPTHROW_VZ: f64 = 221.9;
/// A duck amount above this is a crouched throw.
pub const CROUCHED: f32 = 0.5;

/// Eye pitch (positive looks down) to the pitch the grenade leaves at.
pub fn throw_pitch(eye_pitch: f64) -> f64 {
    if eye_pitch < 90.0 { -10.0 + eye_pitch * 80.0 / 90.0 } else { -10.0 + (360.0 - eye_pitch) * -80.0 / 90.0 }
}

fn forward(eye_pitch: f64, yaw: f64) -> [f64; 3] {
    let elevation = (-throw_pitch(eye_pitch)).to_radians();
    let yaw = yaw.to_radians();
    [elevation.cos() * yaw.cos(), elevation.cos() * yaw.sin(), elevation.sin()]
}

/// The release velocity the model predicts.
pub fn release_velocity(eye_pitch: f64, yaw: f64, strength: f64, player_velocity: [f64; 3]) -> [f64; 3] {
    let speed = THROW_SPEED * (0.3 + 0.7 * strength.clamp(0.0, 1.0));
    let f = forward(eye_pitch, yaw);
    [f[0] * speed + PLAYER_VELOCITY_SCALE * player_velocity[0], f[1] * speed + PLAYER_VELOCITY_SCALE * player_velocity[1], f[2] * speed + PLAYER_VELOCITY_SCALE * player_velocity[2]]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Release {
    /// 1 left click, 0.5 both buttons, 0 right click.
    pub strength: f64,
    /// The player's vertical speed at release, solved from the release velocity.
    pub player_vz: f64,
    /// The horizontal mismatch between the model and the observed release velocity (u/s).
    pub residual: f64,
}

/// Solves the release model for the strength and the player's vertical speed.
pub fn invert(eye_pitch: f64, yaw: f64, player_vxy: [f64; 2], v0: [f64; 3]) -> Release {
    let f = forward(eye_pitch, yaw);
    let rx = v0[0] - PLAYER_VELOCITY_SCALE * player_vxy[0];
    let ry = v0[1] - PLAYER_VELOCITY_SCALE * player_vxy[1];
    let speed = (rx * f[0] + ry * f[1]) / (f[0] * f[0] + f[1] * f[1]);
    let residual = (rx - f[0] * speed).hypot(ry - f[1] * speed);
    Release { strength: (speed / THROW_SPEED - 0.3) / 0.7, player_vz: (v0[2] - f[2] * speed) / PLAYER_VELOCITY_SCALE, residual }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Click {
    Left,
    Middle,
    Right,
}

impl Click {
    pub fn of(strength: f64) -> Click {
        if strength > 0.75 {
            Click::Left
        } else if strength < 0.25 {
            Click::Right
        } else {
            Click::Middle
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Click::Left => "left",
            Click::Middle => "middle",
            Click::Right => "right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Movement {
    Stand,
    Walk,
    Run,
    Jumpthrow,
    RunJumpthrow,
    Jump,
    RunJump,
}

impl Movement {
    /// Movement at release, from the ground state, the solved vertical speed and the horizontal speed.
    pub fn of(airborne: bool, player_vz: f64, horizontal_speed: f64) -> Movement {
        if airborne {
            let canonical = (player_vz - JUMPTHROW_VZ).abs() < 1.0 || (player_vz - CROUCH_JUMPTHROW_VZ).abs() < 1.0;
            match (canonical, horizontal_speed >= 100.0) {
                (true, false) => Movement::Jumpthrow,
                (true, true) => Movement::RunJumpthrow,
                (false, false) => Movement::Jump,
                (false, true) => Movement::RunJump,
            }
        } else if horizontal_speed < 5.0 {
            Movement::Stand
        } else if horizontal_speed < 140.0 {
            Movement::Walk
        } else {
            Movement::Run
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Movement::Stand => "stand",
            Movement::Walk => "walk",
            Movement::Run => "run",
            Movement::Jumpthrow => "jumpthrow",
            Movement::RunJumpthrow => "run-jumpthrow",
            Movement::Jump => "jump",
            Movement::RunJump => "run-jump",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Technique {
    pub movement: Movement,
    pub click: Option<Click>,
    pub crouched: bool,
}

impl Technique {
    /// `movement[+crouch]/click`, e.g. `jumpthrow/left` or `stand+crouch/right`.
    pub fn name(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Technique {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.movement.name())?;
        if self.crouched {
            f.write_str("+crouch")?;
        }
        if let Some(click) = self.click {
            write!(f, "/{}", click.name())?;
        }
        Ok(())
    }
}

/// The technique and the solved release, from the initial velocity and the thrower's angles, velocity and ground
/// state; None while any of them is unknown.
pub fn of(throw: &Throw) -> Option<(Technique, Release)> {
    let v0 = throw.velocity?;
    let pawn = throw.thrower.as_ref()?;
    let (pitch, yaw, velocity, on_ground) = (pawn.pitch?, pawn.yaw?, pawn.velocity?, pawn.on_ground?);
    let release = invert(f64::from(pitch), f64::from(yaw), [f64::from(velocity[0]), f64::from(velocity[1])], v0.map(f64::from));
    let horizontal = f64::from(velocity[0]).hypot(f64::from(velocity[1]));
    let technique = Technique {
        movement: Movement::of(!on_ground, release.player_vz, horizontal),
        click: Some(Click::of(release.strength)),
        crouched: pawn.duck.is_some_and(|d| d > CROUCHED),
    };
    Some((technique, release))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverting_the_model_recovers_strength_and_vertical_velocity() {
        for (pitch, yaw, strength, vel) in [
            (-47.9, -165.5, 1.0, [0.0, 0.0, 0.0]),
            (-4.36, 144.58, 1.0, [-183.98, 131.64, 0.0]),
            (-0.79, -33.2, 0.0, [7.08, -4.61, 0.0]),
            (-16.34, -135.28, 0.5, [-20.0, 15.0, 218.8]),
            (12.0, 90.0, 1.0, [0.0, 240.0, -150.0]),
        ] {
            let v0 = release_velocity(pitch, yaw, strength, vel);
            let r = invert(pitch, yaw, [vel[0], vel[1]], v0);
            assert!((r.strength - strength).abs() < 1e-9, "strength {r:?}");
            assert!((r.player_vz - vel[2]).abs() < 1e-6, "vz {r:?}");
            assert!(r.residual < 1e-6, "residual {r:?}");
        }
    }

    #[test]
    fn matches_a_recorded_throw() {
        // xertioN's right-click flash on de_cache, tick 2786: eye angles, networked velocity, m_vInitialVelocity
        let r = invert(-0.792053, -33.195526, [7.078125, -4.609375], [175.346497, -114.711655, 37.611374]);
        assert_eq!(Click::of(r.strength), Click::Right);
        assert!(r.residual < 0.05);
        assert!(r.player_vz.abs() < 1.0);
    }

    #[test]
    fn classifies_movement() {
        assert_eq!(Movement::of(true, 218.8, 0.0), Movement::Jumpthrow);
        assert_eq!(Movement::of(true, 219.2, 180.0), Movement::RunJumpthrow);
        assert_eq!(Movement::of(true, 221.9, 0.0), Movement::Jumpthrow);
        assert_eq!(Movement::of(true, 150.0, 0.0), Movement::Jump);
        assert_eq!(Movement::of(true, -40.0, 230.0), Movement::RunJump);
        assert_eq!(Movement::of(false, 0.0, 0.0), Movement::Stand);
        assert_eq!(Movement::of(false, 0.0, 120.0), Movement::Walk);
        assert_eq!(Movement::of(false, 0.0, 240.0), Movement::Run);
    }

    #[test]
    fn classifies_click() {
        assert_eq!(Click::of(1.0), Click::Left);
        assert_eq!(Click::of(0.48), Click::Middle);
        assert_eq!(Click::of(0.0), Click::Right);
    }

    #[test]
    fn a_throw_needs_its_release_facts_and_names_its_technique_as_the_format_does() {
        let mut t = crate::fixture::throw("t", "d", crate::Grenade::Smoke, crate::Side::T, [0.0, 0.0, 64.0]);
        assert_eq!(of(&t), None, "no initial velocity, no pawn");
        // a crouched jump-throw, left click: the player's vertical speed 221.9 at release
        let (pitch, yaw) = (-20.0_f32, 45.0_f32);
        let v0 = release_velocity(f64::from(pitch), f64::from(yaw), 1.0, [0.0, 0.0, CROUCH_JUMPTHROW_VZ]);
        t.velocity = Some(v0.map(|v| v as f32));
        t.thrower = Some(crate::Pawn { origin: [0.0, 0.0, 0.0], pitch: Some(pitch), yaw: Some(yaw), velocity: Some([0.0, 0.0, 0.0]), on_ground: Some(false), duck: Some(1.0), ..Default::default() });
        let (technique, release) = of(&t).unwrap();
        assert_eq!(technique, Technique { movement: Movement::Jumpthrow, click: Some(Click::Left), crouched: true });
        assert!((release.player_vz - CROUCH_JUMPTHROW_VZ).abs() < 1e-3);
        assert_eq!(technique.name(), "jumpthrow+crouch/left");
        // the ground state is needed
        t.thrower.as_mut().unwrap().on_ground = None;
        assert_eq!(of(&t), None);
    }
}
