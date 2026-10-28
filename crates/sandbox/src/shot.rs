//! A scripted camera for a video (S34): moves played one after another from the clip's start while the scene's clock
//! runs with them. A follow trails a grenade until it goes off; an orbit circles what a throw left, horizontally, at a
//! radius and height, starting on the bearing the camera already has and easing in from it; the thrower's eye and a
//! fixed camera hold for their seconds.

use glam::DVec3;
use nades_control::video::ShotMove;
use nades_scene::camera::{View as CsView, angles_to, forward, vertical_fov};
use nades_scene::collider::Collider;

use crate::sim::{self, Placed};

/// How long an orbit takes to ease in from the camera before it.
const EASE: f64 = 1.0;

/// The moves laid out on the clip's time: each with when it starts and ends (seconds from the clip's start).
pub struct Shot {
    moves: Vec<(f64, f64, ShotMove)>,
    /// The camera each move began from, once it has.
    began: Vec<Option<CsView>>,
    last: Option<CsView>,
}

impl Shot {
    /// Lays `moves` out for a clip starting at `clip_start` on the scene's clock: a follow lasts until its throw goes off
    /// (and its `after_seconds`), at least a tenth of a second.
    pub fn new(moves: &[ShotMove], placed: &[Placed], clip_start: f64) -> Self {
        let mut at = 0.0;
        let mut laid = Vec::new();
        for m in moves {
            let seconds = match m {
                ShotMove::Follow { throw, after_seconds } => {
                    let (b, start) = &placed[*throw];
                    (b.detonates_at(*start) + after_seconds - (clip_start + at)).max(0.1)
                }
                other => other.seconds().unwrap_or(1.0),
            };
            laid.push((at, at + seconds, m.clone()));
            at += seconds;
        }
        Self { began: vec![None; laid.len()], moves: laid, last: None }
    }

    /// Seconds from the clip's start to the shot's end.
    pub fn seconds(&self) -> f64 {
        self.moves.last().map_or(0.0, |(_, end, _)| *end)
    }

    /// The camera at `elapsed` seconds into the clip; frames are asked for in order.
    pub fn camera(&mut self, elapsed: f64, placed: &[Placed], clip_start: f64, collider: Option<&Collider>) -> CsView {
        let k = self.moves.iter().position(|(_, end, _)| elapsed < *end).unwrap_or(self.moves.len() - 1);
        let (from, _, m) = self.moves[k].clone();
        let began = *self.began[k].get_or_insert_with(|| self.last.unwrap_or_else(|| first_camera(&m, placed)));
        let u = elapsed - from;
        let t = clip_start + elapsed;
        let camera = match m {
            ShotMove::Follow { throw, .. } => {
                let (b, start) = &placed[throw];
                sim::views::follow(b, b.local(*start, t), collider)
            }
            ShotMove::Orbit { throw, radius, height, seconds, degrees, start_degrees } => {
                let b = &placed[throw].0;
                let ring = Ring { focus: b.stage.focus, radius, height, nearest: b.reach() + 60.0, start_degrees };
                let target = ring.at(began, degrees * (u / seconds).clamp(0.0, 1.0), u, collider);
                // a camera that had to rise over a wall settles there over a few frames rather than jumping
                // (unless the way there passes a wall: then it goes straight to the clear spot)
                match self.last.filter(|_| u > EASE) {
                    Some(last) => {
                        let eye = last.eye.lerp(target.eye, 0.25);
                        if collider.is_none_or(|c| seen_from(c, eye, ring.focus)) {
                            let (pitch, yaw) = angles_to(eye, ring.focus);
                            CsView { eye, pitch, yaw, fov: vertical_fov() }
                        } else {
                            target
                        }
                    }
                    None => target,
                }
            }
            ShotMove::Thrower { throw, .. } => placed[throw].0.stage.thrower,
            ShotMove::Hold { camera, .. } => {
                let eye = DVec3::new(camera.position[0].into(), camera.position[1].into(), f64::from(camera.position[2]) + f64::from(camera.eye_height_units));
                CsView { eye, pitch: camera.pitch_degrees.into(), yaw: camera.yaw_degrees.into(), fov: camera.vertical_fov_degrees.into() }
            }
        };
        self.last = Some(camera);
        camera
    }
}

/// Where a shot that opens on `m` starts from: an orbit's own start, else its throw's thrower.
fn first_camera(m: &ShotMove, placed: &[Placed]) -> CsView {
    match m {
        ShotMove::Hold { camera, .. } => CsView { eye: DVec3::new(camera.position[0].into(), camera.position[1].into(), f64::from(camera.position[2]) + f64::from(camera.eye_height_units)), pitch: camera.pitch_degrees.into(), yaw: camera.yaw_degrees.into(), fov: camera.vertical_fov_degrees.into() },
        other => placed[other.throw().unwrap_or(0)].0.stage.thrower,
    }
}

/// An orbit's circle: around `focus`, `radius` out and `height` up, never nearer than `nearest` (what the throw left
/// reaches that far).
struct Ring {
    focus: DVec3,
    radius: f64,
    height: f64,
    nearest: f64,
    start_degrees: Option<f64>,
}

impl Ring {
    /// The camera `turned` degrees round, the ring having begun from `began`: on `began`'s bearing from the focus (or
    /// `start_degrees`), looking at the focus; easing over its first second from `began`, `u` seconds in. Where a wall
    /// hides the focus the camera rises over it (up to 480 units more), else tries nearer circles, else comes in front of
    /// the wall, no nearer than `nearest`.
    fn at(&self, began: CsView, turned: f64, u: f64, collider: Option<&Collider>) -> CsView {
        let focus = self.focus;
        let bearing = self.start_degrees.unwrap_or_else(|| {
            let d = began.eye - focus;
            if d.x.hypot(d.y) < 1.0 { began.yaw + 180.0 } else { d.y.atan2(d.x).to_degrees() }
        });
        let a = (bearing + turned).to_radians();
        let radius = self.radius.max(self.nearest);
        let round = DVec3::new(a.cos() * radius, a.sin() * radius, 0.0);
        let mut eye = focus + round + DVec3::Z * self.height;
        if let Some(c) = collider {
            // the asked-for circle first, rising over walls; then nearer ones, down to what the throw left
            let scales = [1.0, 0.8, 0.6, 0.45].into_iter().map(|f| (radius * f).max(self.nearest)).collect::<Vec<_>>();
            let candidates = scales.into_iter().flat_map(|r| (0..=12).map(move |k| (r, f64::from(k) * 40.0)));
            match candidates.map(|(r, rise)| focus + round * (r / radius) + DVec3::Z * (self.height + rise)).find(|e| seen_from(c, *e, focus)) {
                Some(clear) => eye = clear,
                None => {
                    let out = eye - focus;
                    if let Some(d) = c.hit_distance(focus, out.normalize_or_zero(), out.length()) {
                        eye = focus + out.normalize_or_zero() * (d - 16.0).max(self.nearest.min(out.length()));
                    }
                }
            }
        }
        let w = smoothstep((u / EASE).clamp(0.0, 1.0));
        let looked = began.eye + forward(began.pitch, began.yaw) * began.eye.distance(focus).max(1.0);
        let eye = began.eye.lerp(eye, w);
        let (pitch, yaw) = angles_to(eye, looked.lerp(focus, w));
        CsView { eye, pitch, yaw, fov: vertical_fov() }
    }
}

/// Whether a camera at `eye` sees `focus` from the open: 24 units of room every way, so no wall fills its near plane,
/// and open sky for 300 units above it, so it is not in a room the players never enter (whose walls the map's physics
/// leaves out) looking through its window.
fn seen_from(c: &Collider, eye: DVec3, focus: DVec3) -> bool {
    const ROOM: f64 = 24.0;
    c.clear_between(eye, focus) && !c.blocked(eye, DVec3::Z, 300.0) && [DVec3::X, DVec3::NEG_X, DVec3::Y, DVec3::NEG_Y, DVec3::NEG_Z].iter().all(|d| !c.blocked(eye, *d, ROOM))
}

fn smoothstep(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nades_control::scene::{Grenade, SceneThrow};
    use std::sync::Arc;

    fn placed() -> Vec<Placed> {
        vec![(Arc::new(sim::simulate(&SceneThrow::new(Grenade::Flash, [0.0, 0.0, 0.0], -20.0, 0.0), None, DVec3::Z)), 0.5)]
    }

    #[test]
    fn a_follow_lasts_until_the_grenade_goes_off_then_the_orbit_circles_what_it_left() {
        let placed = placed();
        let det = placed[0].0.detonates_at(0.5);
        let moves = [ShotMove::Follow { throw: 0, after_seconds: 0.5 }, ShotMove::Orbit { throw: 0, radius: 300.0, height: 100.0, seconds: 4.0, degrees: 360.0, start_degrees: Some(0.0) }];
        let mut shot = Shot::new(&moves, &placed, 0.0);
        assert!((shot.seconds() - (det + 0.5 + 4.0)).abs() < 1e-9);
        let focus = placed[0].0.stage.focus;
        let mut eyes = Vec::new();
        let mut elapsed = 0.0;
        while elapsed < shot.seconds() {
            eyes.push((elapsed, shot.camera(elapsed, &placed, 0.0, None)));
            elapsed += 1.0 / 30.0;
        }
        // once eased in, the orbit holds its radius and height and looks at the focus, going once round
        let orbiting: Vec<&CsView> = eyes.iter().filter(|(e, _)| *e > det + 0.5 + EASE).map(|(_, c)| c).collect();
        for c in &orbiting {
            let d = c.eye - focus;
            // smoothed from frame to frame, it rides a few units inside the circle
            assert!((d.x.hypot(d.y) - 300.0).abs() < 6.0 && (d.z - 100.0).abs() < 1.0, "{d}");
            assert!(forward(c.pitch, c.yaw).dot((focus - c.eye).normalize()) > 0.9999);
        }
        let bearings: Vec<f64> = orbiting.iter().map(|c| (c.eye.y - focus.y).atan2(c.eye.x - focus.x).to_degrees()).collect();
        let turned: f64 = bearings.windows(2).map(|w| (w[1] - w[0] + 540.0).rem_euclid(360.0) - 180.0).sum();
        assert!(turned > 250.0 && turned < 360.0, "{turned}");
        // the ease starts where the follow left off: no jump between frames
        let jumps = eyes.windows(2).map(|w| w[0].1.eye.distance(w[1].1.eye)).fold(0.0, f64::max);
        assert!(jumps < 60.0, "{jumps}");
    }
}
