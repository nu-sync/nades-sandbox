//! One simulated throw turned into what the sandbox draws and where it can look from: the thrower's view at release,
//! the flight with its clock, and what it leaves (a smoke's cloud, a fire, a flash's or HE's burst), with the point a
//! result view looks at. The flight's clock is `render/effects.ts`'s and the result camera `stage.ts`'s, from
//! show-me-the-nades' web view; the flight and the effects themselves come from the simulator (nades-physics,
//! nades-effects).

use glam::{DQuat, DVec3};

use crate::camera::{FIRE_FRAMING, Framing, MIN_SEEN, SMOKE_FRAMING, ShotCamera, Source, View, angles_to, pick_result_camera, spread_points};
use crate::collider::Collider;
use crate::fire::{Fire, FireKind, Flame, build_fire};
use crate::smoke::Cloud;

/// The grenades, as the scene draws them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Grenade {
    Smoke,
    Flash,
    He,
    Molotov,
    Incendiary,
    Decoy,
}

impl Grenade {
    pub fn is_fire(self) -> bool {
        matches!(self, Grenade::Molotov | Grenade::Incendiary)
    }
}

/// The flight with its clock: where the grenade was at each tick, and the tick it went off.
#[derive(Debug, Clone, PartialEq)]
pub struct Flight {
    pub points: Vec<DVec3>,
    pub ticks: Vec<i32>,
    pub detonate: i32,
    /// The grenade's turn at each point, from upright and facing the way it was thrown.
    spins: Vec<DQuat>,
}

/// How far the grenade is from the hand before it shows in flight: nearer, it would fill the thrower's view.
const OUT_OF_HAND: f64 = 48.0;
/// End over end in flight, a radian every TUMBLE units; rolling once it runs along the floor, as a can of ROLL_RADIUS.
const TUMBLE: f64 = 60.0;
const ROLL_RADIUS: f64 = 2.2;
const ROLLING_SLOPE: f64 = 0.15;
const ROLLING_SPEED: f64 = 350.0;

impl Flight {
    /// From samples in seconds on any clock (one a tick, as the simulator gives them) and the moment it went off.
    pub fn of(samples: &[(f64, DVec3)], detonate: f64) -> Option<Self> {
        let tick = |t: f64| (t * 64.0).round() as i32;
        (samples.len() > 1).then(|| Self::new(samples.iter().map(|(_, p)| *p).collect(), samples.iter().map(|(t, _)| tick(*t)).collect(), tick(detonate)))
    }

    fn new(points: Vec<DVec3>, ticks: Vec<i32>, detonate: i32) -> Self {
        let spins = spin_along(&points, &ticks);
        Self { points, ticks, detonate, spins }
    }

    /// Where it was released, where it bounced (the flight turned sharply in one sample) and where it stopped, with
    /// each mark's size.
    pub fn marks(&self) -> Vec<(DVec3, f64)> {
        let v = &self.points;
        let mut marks = vec![(v[0], 1.0)];
        for i in 1..v.len() - 1 {
            let (a, b) = (v[i] - v[i - 1], v[i + 1] - v[i]);
            if a.length_squared() > 4.0 && b.length_squared() > 4.0 && angle_to(a, b) > 0.6 {
                marks.push((v[i], 0.8));
            }
        }
        marks.push((v[v.len() - 1], 1.4));
        marks
    }

    /// The grenade in flight at `t` seconds after detonation (negative before it): its position and turn, or None
    /// once it has gone off, or while it is still in the thrower's hand.
    pub fn grenade_at(&self, t: f64) -> Option<(DVec3, DQuat)> {
        if t >= 0.0 || self.points.len() != self.ticks.len() {
            return None;
        }
        let tick = f64::from(self.detonate) + t * 64.0;
        let mut i = 0;
        while i < self.ticks.len() - 1 && f64::from(self.ticks[i + 1]) <= tick {
            i += 1;
        }
        let j = (i + 1).min(self.points.len() - 1);
        let span = if i < self.ticks.len() - 1 { f64::from(self.ticks[i + 1] - self.ticks[i]) } else { 0.0 };
        let f = if span > 0.0 { ((tick - f64::from(self.ticks[i])) / span).clamp(0.0, 1.0) } else { 0.0 };
        let at = self.points[i].lerp(self.points[j], f);
        let spin = self.spins[i].slerp(self.spins[j], f);
        (at.distance(self.points[0]) > OUT_OF_HAND).then_some((at, spin))
    }
}

/// THREE.Vector3.angleTo.
fn angle_to(a: DVec3, b: DVec3) -> f64 {
    let denominator = (a.length_squared() * b.length_squared()).sqrt();
    if denominator == 0.0 { std::f64::consts::FRAC_PI_2 } else { (a.dot(b) / denominator).clamp(-1.0, 1.0).acos() }
}

/// Its turn at each point of the flight, from upright and facing the way it was thrown.
fn spin_along(points: &[DVec3], ticks: &[i32]) -> Vec<DQuat> {
    if points.len() < 2 {
        return vec![DQuat::IDENTITY; points.len()];
    }
    let first = points[1] - points[0];
    let mut spins = vec![DQuat::from_axis_angle(DVec3::Z, first.y.atan2(first.x))];
    let mut axis = DVec3::Y;
    for i in 1..points.len() {
        let step = points[i] - points[i - 1];
        let along = step.length();
        let seconds = f64::from((ticks.get(i).copied().unwrap_or(0) - ticks.get(i - 1).copied().unwrap_or(0)).max(1)) / 64.0;
        let rolling = step.z.abs() < ROLLING_SLOPE * along && along / seconds < ROLLING_SPEED;
        let across = DVec3::Z.cross(step);
        if across.length_squared() > 1e-6 {
            axis = across.normalize();
        }
        let turn = DQuat::from_axis_angle(axis, along / if rolling { ROLL_RADIUS } else { TUMBLE });
        spins.push(turn * spins[i - 1]);
    }
    spins
}

/// A flash's or HE's moment of detonation: a burst of light the size of what it does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Burst {
    pub flash: bool,
    pub at: DVec3,
}

impl Burst {
    /// 1 as it goes off, gone a quarter of a second later.
    pub fn on(&self, t: f64) -> f64 {
        if t < 0.0 { 0.0 } else { (1.0 - t / 0.25).max(0.0) }
    }
}

/// A point light an effect puts on the map (three.js's PointLight in the web view): linear colour, intensity in
/// candela, the distance it reaches and how it falls off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    pub at: DVec3,
    pub color: [f64; 3],
    pub intensity: f64,
    pub distance: f64,
    pub decay: f64,
}

/// sRGB hex as three.js turns it into linear light.
fn hex(rgb: u32) -> [f64; 3] {
    [(rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255].map(|c| {
        let c = f64::from(c) / 255.0;
        if c < 0.04045 { c * 0.077_399_380_8 } else { (c * 0.947_867_298_6 + 0.052_132_701_4).powf(2.4) }
    })
}

/// The flight path's colour, by grenade (render/effects.ts NADE_COLORS; a fire is a molotov's colour).
pub fn nade_color(grenade: Grenade) -> [f64; 3] {
    hex(match grenade {
        Grenade::Smoke => 0xd9dde3,
        Grenade::Flash => 0xffe36b,
        Grenade::He => 0xff5a4f,
        Grenade::Molotov | Grenade::Incendiary => 0xff8a2a,
        Grenade::Decoy => 0x9aa6b2,
    })
}

#[derive(Debug, Clone)]
pub struct Stage {
    pub grenade: Grenade,
    /// The thrower's eye and angles at release.
    pub thrower: View,
    pub flight: Option<Flight>,
    pub cloud: Option<Cloud>,
    pub fire: Option<Fire>,
    pub burst: Option<Burst>,
    /// Where it went off.
    pub detonation: DVec3,
    /// What a result view looks at: the cloud, the fire, or the detonation.
    pub focus: DVec3,
    /// Points spread over the effect (the focus first); a result shot must see enough of them.
    pub targets: Vec<DVec3>,
    /// Every voxel of the cloud or every flame; a result shot keeps its eye clear of them.
    pub body: Vec<DVec3>,
    /// Seconds after detonation a result view shows (the effect at its fullest), how long the effect lasts, and when
    /// the grenade left the hand (negative: before it went off).
    pub result_time: f64,
    pub duration: f64,
    pub release_time: f64,
}

/// What a simulated throw gives the stage: the grenade, the thrower's view at release, the flight (seconds and points
/// on the throw's clock), where and when it went off, and its effect (the smoke's cloud, the fire's flames).
pub struct Simulated {
    pub grenade: Grenade,
    pub thrower: View,
    pub flight: Vec<(f64, DVec3)>,
    pub release: f64,
    pub detonation: (f64, DVec3),
    pub cloud: Option<Cloud>,
    pub flames: Vec<Flame>,
}

/// How long a decoy's or a burst's stage runs on after it goes off, with nothing left to draw.
const NO_EFFECT: f64 = 2.0;

pub fn build_stage(s: Simulated) -> Stage {
    let (detonate, land) = s.detonation;
    let flight = Flight::of(&s.flight, detonate);
    let mut focus = land + DVec3::new(0.0, 0.0, 32.0);
    let (mut fire, mut burst) = (None, None);
    let mut result_time = 0.05;
    let mut body: Vec<DVec3> = Vec::new();
    let mut duration = NO_EFFECT;
    match s.grenade {
        Grenade::Smoke => {
            if let Some(c) = &s.cloud {
                focus = c.focus();
                result_time = c.settled_seconds();
                body = c.voxels();
                duration = c.hold_seconds + 2.5;
            }
        }
        Grenade::Molotov | Grenade::Incendiary => {
            let kind = if s.grenade == Grenade::Incendiary { FireKind::Incendiary } else { FireKind::Molotov };
            let points: Vec<DVec3> = s.flames.iter().map(|f| DVec3::from_array(f.pos)).collect();
            if let Some(f) = build_fire(s.flames, kind) {
                // the fire is a carpet a foot deep: a result shot looks at the flame nearest the fire's middle, since
                // the middle of a fire that ran down a floor is inside a wall
                let centre = DVec3::from_array(f.centre);
                let middle = points.iter().copied().min_by(|a, b| centre.distance_squared(*a).total_cmp(&centre.distance_squared(*b))).unwrap_or(centre);
                focus = middle + DVec3::new(0.0, 0.0, 12.0);
                body = points.iter().map(|p| *p + DVec3::new(0.0, 0.0, 12.0)).collect();
                result_time = f.fullest_at();
                // the scrubber runs on past the fire, through the flecks and steam it leaves
                duration = f.lasts();
                fire = Some(f);
            }
        }
        Grenade::Flash | Grenade::He => burst = Some(Burst { flash: s.grenade == Grenade::Flash, at: land }),
        Grenade::Decoy => {}
    }
    Stage {
        grenade: s.grenade,
        thrower: s.thrower,
        flight,
        cloud: s.cloud,
        fire,
        burst,
        detonation: land,
        targets: spread_points(&body, focus, 0.35),
        focus,
        body,
        result_time,
        duration,
        release_time: (s.release - detonate).min(0.0),
    }
}

impl Stage {
    /// The scrubber's span: from release to the effect's end (or a second after it went off).
    pub fn timeline(&self) -> (f64, f64) {
        (self.release_time, self.duration.max(1.0))
    }

    /// The light the effect puts on the map at `t`, if any (the fire's, a flash's or an HE's).
    pub fn light_at(&self, t: f64) -> Option<Light> {
        if let Some(fire) = &self.fire {
            let intensity = 900.0 * fire.light_at(t);
            let c = fire.centre;
            return (intensity > 0.0).then_some(Light { at: DVec3::new(c[0], c[1], c[2] + 32.0), color: hex(0xff9a4a), intensity, distance: 750.0, decay: 1.4 });
        }
        let burst = self.burst?;
        let on = burst.on(t);
        let (color, peak, distance) = if burst.flash { (hex(0xf2f4ff), 2500.0, 1400.0) } else { (hex(0xff8a3c), 4000.0, 900.0) };
        (on > 0.0).then_some(Light { at: burst.at, color, intensity: peak * on, distance, decay: 1.3 })
    }

    /// A result shot: a standing eye on the floor around the focus that sees most of the effect, preferring the far
    /// side from the thrower; without the collider, an overview from the thrower's side.
    pub fn result_camera(&self, collider: Option<&Collider>) -> ShotCamera {
        let framing: Framing = if self.grenade.is_fire() { FIRE_FRAMING } else { SMOKE_FRAMING };
        let seen = |eye: DVec3| collider.map_or(1.0, |c| seen_share(c, eye, &self.targets));
        let gap = |eye: DVec3| nearest(eye, &self.body);
        let mut shot = pick_result_camera(&[], self.focus, self.thrower.eye, seen, framing, gap);
        if let Some(c) = collider
            && let Some(standing) = standing_view(c, self.focus, self.thrower.eye, |eye| if gap(eye) < framing.clear { 0.0 } else { seen(eye) })
        {
            shot = standing;
        }
        shot.seen = collider.map(|_| seen(shot.eye));
        shot
    }
}

/// Distance from `eye` to the nearest of `points`, or infinity when there are none.
fn nearest(eye: DVec3, points: &[DVec3]) -> f64 {
    let mut best = f64::INFINITY;
    for p in points {
        let d = *p - eye;
        best = best.min(d.x * d.x + d.y * d.y + d.z * d.z);
    }
    best.sqrt()
}

/// The share of `targets` the map leaves in sight from `eye`.
fn seen_share(c: &Collider, eye: DVec3, targets: &[DVec3]) -> f64 {
    if targets.is_empty() {
        return 1.0;
    }
    targets.iter().filter(|t| c.clear_between(eye, **t)).count() as f64 / targets.len() as f64
}

/// A standing eye on the floor 350–800 units from the focus that sees most of the effect.
fn standing_view(c: &Collider, focus: DVec3, thrower: DVec3, seen: impl Fn(DVec3) -> f64) -> Option<ShotCamera> {
    let away = {
        let (x, y) = (focus.x - thrower.x, focus.y - thrower.y);
        // THREE.Vector2.normalize: times the reciprocal of the length (or of 1)
        let length = (x * x + y * y).sqrt();
        let inverse = 1.0 / if length == 0.0 { 1.0 } else { length };
        (x * inverse, y * inverse)
    };
    let mut best: Option<(f64, DVec3)> = None;
    for r in [350.0, 500.0, 650.0, 800.0] {
        for k in 0..24 {
            let a = (f64::from(k) / 24.0) * std::f64::consts::PI * 2.0;
            let dir = (a.cos(), a.sin());
            let probe = DVec3::new(focus.x + dir.0 * r, focus.y + dir.1 * r, focus.z + 60.0);
            // the floor the target stands on, not a roof or a walkway above it
            let Some(floor) = c.floor_below(probe, 500.0) else { continue };
            if floor > focus.z + 40.0 || floor < focus.z - 260.0 {
                continue;
            }
            let eye = DVec3::new(probe.x, probe.y, floor + 64.0);
            if !c.clear_between(eye, focus) {
                continue;
            }
            // not under a low ceiling or inside a wall
            if !c.clear_between(eye, eye + DVec3::new(0.0, 0.0, 48.0)) {
                continue;
            }
            let share = seen(eye);
            if share < MIN_SEEN {
                continue;
            }
            let score = share + (dir.0 * away.0 + dir.1 * away.1) * 0.6 - (r - 500.0).abs() / 1000.0;
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, eye));
            }
        }
    }
    let (_, eye) = best?;
    let (pitch, yaw) = angles_to(eye, focus);
    Some(ShotCamera { eye, pitch, yaw, seen: None, source: Source::Overview })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::vertical_fov;

    fn simulated(grenade: Grenade) -> Simulated {
        let thrower = View { eye: DVec3::new(0.0, 0.0, 64.0), pitch: -10.0, yaw: 0.0, fov: vertical_fov() };
        // released 0.1 s into the throw, up over (250, 0, 300) at 50 ticks, down at (500, 0, 0) where it goes off
        let flight = vec![(0.1, DVec3::new(10.0, 0.0, 64.0)), (0.1 + 50.0 / 64.0, DVec3::new(250.0, 0.0, 300.0)), (0.1 + 100.0 / 64.0, DVec3::new(500.0, 0.0, 0.0))];
        Simulated { grenade, thrower, flight, release: 0.1, detonation: (0.1 + 100.0 / 64.0, DVec3::new(500.0, 0.0, 0.0)), cloud: None, flames: Vec::new() }
    }

    #[test]
    fn the_flight_plays_until_it_goes_off_and_marks_its_bounce() {
        let stage = build_stage(simulated(Grenade::He));
        assert_eq!(stage.release_time, -100.0 / 64.0);
        let flight = stage.flight.as_ref().unwrap();
        assert_eq!(flight.marks().len(), 3, "release, the turn at the top, the stop");
        // in the hand at release; halfway up 50 ticks before detonation; gone once it goes off
        assert!(flight.grenade_at(-100.0 / 64.0).is_none());
        let (mid, _) = flight.grenade_at(-50.0 / 64.0).unwrap();
        assert!(mid.distance(DVec3::new(250.0, 0.0, 300.0)) < 1e-9);
        assert!(flight.grenade_at(0.0).is_none());
        assert_eq!(stage.burst, Some(Burst { flash: false, at: DVec3::new(500.0, 0.0, 0.0) }));
        assert!(stage.light_at(0.1).is_some_and(|l| l.intensity > 0.0) && stage.light_at(0.3).is_none());
        assert_eq!((stage.duration, stage.focus), (NO_EFFECT, DVec3::new(500.0, 0.0, 32.0)));
    }

    #[test]
    fn a_fire_frames_the_flame_nearest_its_middle() {
        let mut s = simulated(Grenade::Molotov);
        s.flames = [(400.0, 0.0), (500.0, 0.15), (650.0, 0.8)].iter().map(|&(x, start)| Flame { pos: [x, 0.0, 0.0], normal: [0.0, 0.0, 1.0], start, end: 7.0 }).collect();
        let stage = build_stage(s);
        assert_eq!(stage.focus, DVec3::new(500.0, 0.0, 12.0));
        assert_eq!(stage.body.len(), 3);
        assert!(stage.duration > 7.0);
        assert!(stage.light_at(1.0).is_some_and(|l| l.intensity > 900.0));
        // nobody's floor to stand on without the collider: an overview from the thrower's side
        assert_eq!(stage.result_camera(None).source, Source::Overview);
        // an incendiary is a fire too; a fire with no flames draws none
        let mut s = simulated(Grenade::Incendiary);
        s.flames = vec![Flame { pos: [500.0, 0.0, 0.0], normal: [0.0, 0.0, 1.0], start: 0.0, end: 5.5 }];
        assert_eq!(build_stage(s).fire.map(|f| f.kind), Some(FireKind::Incendiary));
        assert!(build_stage(simulated(Grenade::Molotov)).fire.is_none());
    }

    #[test]
    fn colours_are_linear() {
        let [r, g, b] = hex(0xff8000);
        assert_eq!(r, 1.0);
        assert!((g - 0.2158605).abs() < 1e-6 && b == 0.0);
        assert_eq!(nade_color(Grenade::Incendiary), nade_color(Grenade::Molotov));
    }
}
