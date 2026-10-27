//! One throw turned into what the sandbox draws and where it can look from: the thrower's view at release, a zoom on
//! the aim point, a result shot of what the grenade left from where an opponent actually stood while it was up, and a
//! free orbit around it. A port of show-me-the-nades' `web/app/src/scene/stage.ts`, with the flight's clock from
//! `render/effects.ts` and the moments `SceneView.tsx` shows each view at, built from a throw's facts, its tracks and
//! what nades-model derives of them.

use glam::{DQuat, DVec3};
use nades_model::{Derived, Effect, Grenade, Throw, Tracks};

use crate::camera::{AIM_FOV, FIRE_FRAMING, Framing, MIN_SEEN, SMOKE_FRAMING, ShotCamera, Source, View, Viewer, angles_to, eye_at, pick_result_camera, spread_points, vertical_fov};
use crate::collider::Collider;
use crate::fire::{Fire, FireKind, build_fire, flames_of};
use crate::smoke::{Cloud, smoke_cloud};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewMode {
    /// The thrower's eye at release, with the crosshair.
    Lineup,
    /// The same eye zoomed on the aim point.
    Aim,
    /// An opponent's eye, with the effect as the round had it.
    Result,
    /// Anywhere around the effect, by dragging.
    Orbit,
}

impl ViewMode {
    pub const ALL: [ViewMode; 4] = [ViewMode::Lineup, ViewMode::Aim, ViewMode::Result, ViewMode::Orbit];

    pub fn name(self) -> &'static str {
        match self {
            ViewMode::Lineup => "Lineup",
            ViewMode::Aim => "Aim",
            ViewMode::Result => "Result",
            ViewMode::Orbit => "Orbit",
        }
    }
}

/// The recorded flight with its clock: where the grenade was at each tick, and the tick it went off.
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

/// A line from a flash to a player it blinded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlindLine {
    pub from: DVec3,
    pub eye: DVec3,
    /// 0..1: how long they were blind, of 3 s.
    pub strength: f64,
    pub enemy: bool,
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
    pub throw_id: String,
    pub grenade: Grenade,
    pub lineup: View,
    pub aim: View,
    /// The flight, drawn in result and orbit shots; a lineup shot shows the thrower's view without it.
    pub flight: Option<Flight>,
    pub cloud: Option<Cloud>,
    pub fire: Option<Fire>,
    pub burst: Option<Burst>,
    pub blinded: Vec<BlindLine>,
    /// What a result shot looks at: the cloud, the fire, or the landing.
    pub focus: DVec3,
    /// Points spread over the effect (the focus first); a result shot must see enough of them.
    pub targets: Vec<DVec3>,
    /// Every voxel of the cloud or every flame; a result shot keeps its eye clear of them.
    pub body: Vec<DVec3>,
    /// For a flash: the eye of the enemy it blinded longest, whose view is the result shot, and for how long.
    pub victim: Option<(DVec3, f64)>,
    /// Seconds after detonation a result shot shows, how long the effect lasts, and when the grenade left the hand
    /// (the moment the lineup and aim views show).
    pub result_time: f64,
    pub duration: f64,
    pub release_time: f64,
    /// The tick it went off: its time 0 (a replay's clock for it).
    pub detonate_tick: i32,
    /// What the stage could not draw, said plainly (e.g. a throw ingested before tracks existed).
    pub missing: Vec<&'static str>,
}

fn three(v: [f32; 3]) -> DVec3 {
    DVec3::new(v[0].into(), v[1].into(), v[2].into())
}

/// The thrower's view at release: their eye where they stood (nades-model's rule), their angles, CS2's field of view.
/// Without the thrower's pawn, the eye is the grenade's start.
pub fn lineup_view(t: &Throw, d: &Derived) -> View {
    let eye = nades_model::derive::eye(t, d.technique.as_ref()).map_or_else(|| three(t.start), DVec3::from_array);
    let angle = |a: Option<f32>| a.map_or(0.0, f64::from);
    let pawn = t.thrower.as_ref();
    View { eye, pitch: angle(pawn.and_then(|p| p.pitch)), yaw: angle(pawn.and_then(|p| p.yaw)), fov: vertical_fov() }
}

/// The tick the grenade went off, the flight's clock: one that never did goes off when its airtime ran out.
fn goes_off(t: &Throw, d: &Derived) -> i32 {
    t.detonation.map_or_else(|| t.tick.0 + (64.0 * f64::from(d.airtime.unwrap_or(1.0)).max(0.2) + 0.5).floor() as i32, |(at, _)| at.0)
}

/// A player a flash blinded, where they stood and whether an enemy, longest first (those without a position cannot be
/// drawn).
struct Blinded {
    pos: [f32; 3],
    seconds: f32,
    enemy: bool,
}

fn blinded_by(t: &Throw, tracks: &Tracks) -> Vec<Blinded> {
    let Effect::Flash(blinds) = &tracks.effect else { return Vec::new() };
    let mut out: Vec<Blinded> = blinds.iter().filter(|b| b.seconds > 0.0).filter_map(|b| Some(Blinded { pos: b.pos?, seconds: b.seconds, enemy: b.side.is_none_or(|s| s != t.side) })).collect();
    out.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    out
}

pub fn build_stage(t: &Throw, tracks: &Tracks, d: &Derived) -> Stage {
    let lineup = lineup_view(t, d);
    let mut missing = Vec::new();
    let land = three(d.landing.unwrap_or(t.start));
    let detonate = t.detonation.map_or(t.tick.0, |(at, _)| at.0);
    let mut focus = land + DVec3::new(0.0, 0.0, 32.0);
    let (mut cloud, mut fire, mut burst, mut blinded, mut victim) = (None, None, None, Vec::new(), None);
    let mut result_time = 0.05;
    let mut body: Vec<DVec3> = Vec::new();
    let mut duration = if d.effect_recorded { f64::from(d.effect_seconds) } else { 2.0 };

    let flight = (tracks.flight.len() > 1).then(|| Flight::new(tracks.flight.iter().map(|(_, p)| three(*p)).collect(), tracks.flight.iter().map(|(tick, _)| tick.0).collect(), goes_off(t, d)));
    if flight.is_none() {
        missing.push("flight path");
    }

    match (t.grenade, &tracks.effect) {
        (Grenade::Smoke, effect) => {
            let voxels = if let Effect::Smoke(Some(v)) = effect { Some(v) } else { None };
            // the grid is centred on m_vSmokeDetonationPos, else on where it went off
            cloud = voxels.and_then(|v| smoke_cloud(&v.bytes, three(v.origin.or(t.detonation.map(|(_, at)| at))?)));
            match &cloud {
                Some(c) => {
                    focus = c.focus();
                    result_time = c.settled_seconds();
                    body = c.voxels();
                }
                None => missing.push("smoke cloud"),
            }
        }
        (Grenade::Molotov | Grenade::Incendiary, effect) => {
            let flames = if let Effect::Fire(flames) = effect { flames.as_slice() } else { &[] };
            if !flames.is_empty() {
                let f = build_fire(flames_of(flames, detonate), FireKind::of(t.grenade)).expect("flames to build from");
                // the fire is a carpet a foot deep: a result shot looks at the flame nearest the fire's middle, since
                // the middle of a fire that ran down a floor is inside a wall
                let centre = DVec3::from_array(f.centre);
                let middle = flames.iter().map(|f| three(f.pos)).fold(None, |a: Option<DVec3>, p| match a {
                    Some(a) if centre.distance_squared(p) >= centre.distance_squared(a) => Some(a),
                    _ => Some(p),
                });
                let middle = middle.expect("flames");
                focus = DVec3::new(middle.x, middle.y, middle.z + 12.0);
                body = flames.iter().map(|f| three(f.pos) + DVec3::new(0.0, 0.0, 12.0)).collect();
                result_time = f.fullest_at();
                // the scrubber runs on past the fire, through the flecks and steam it leaves
                duration = f.lasts();
                fire = Some(f);
            } else if d.effect_recorded {
                missing.push("fire spread");
            }
        }
        (Grenade::Flash | Grenade::He, _) => {
            let flash = t.grenade == Grenade::Flash;
            burst = Some(Burst { flash, at: land });
            let caught = blinded_by(t, tracks);
            if flash && !caught.is_empty() {
                blinded = caught.iter().map(|b| BlindLine { from: land, eye: three(b.pos) + DVec3::new(0.0, 0.0, 64.0), strength: (f64::from(b.seconds) / 3.0).min(1.0), enemy: b.enemy }).collect();
                if let Some(worst) = caught.iter().find(|b| b.enemy) {
                    victim = Some((eye_at(three(worst.pos), Some(0.0)), f64::from(worst.seconds)));
                }
                // frame the flash and the players it caught
                let points: Vec<DVec3> = std::iter::once(land).chain(caught.iter().map(|b| three(b.pos) + DVec3::new(0.0, 0.0, 40.0))).collect();
                let sum = points.iter().fold(DVec3::ZERO, |a, b| a + *b);
                focus = sum * (1.0 / points.len() as f64);
            }
        }
        (Grenade::Decoy, _) => {}
    }

    Stage {
        throw_id: t.id.0.clone(),
        grenade: t.grenade,
        aim: View { fov: AIM_FOV, ..lineup },
        lineup,
        flight,
        cloud,
        fire,
        burst,
        blinded,
        targets: spread_points(&body, focus, 0.35),
        focus,
        body,
        victim,
        result_time,
        duration,
        release_time: -f64::from(d.airtime.unwrap_or(1.0)).max(0.2),
        detonate_tick: goes_off(t, d),
        missing,
    }
}

impl Stage {
    /// The scrubber's span: from release to the effect's end (or a second after it went off).
    pub fn timeline(&self) -> (f64, f64) {
        (self.release_time, self.duration.max(1.0))
    }

    /// The moment each view opens at: the release for the thrower's views, the effect at its fullest for the result,
    /// and the release for the orbit, which then plays.
    pub fn opens_at(&self, view: ViewMode) -> f64 {
        match view {
            ViewMode::Lineup | ViewMode::Aim | ViewMode::Orbit => self.release_time,
            ViewMode::Result => self.result_time,
        }
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

    /// The result shot: an opponent's view of the focus with nothing in between. When no opponent had one, a spot on
    /// the floor around the focus that sees it, preferring the far side from the thrower.
    pub fn result_camera(&self, viewers: &[Viewer], collider: Option<&Collider>) -> ShotCamera {
        let target = self.focus;
        if let Some((eye, seconds)) = self.victim {
            // what the player it blinded longest saw as it went off
            let at = self.flight.as_ref().map_or(target, |f| f.points[f.points.len() - 1]);
            return ShotCamera::looking(eye, at, Source::Victim { seconds });
        }
        let framing: Framing = if self.grenade.is_fire() { FIRE_FRAMING } else { SMOKE_FRAMING };
        let seen = |eye: DVec3| collider.map_or(1.0, |c| seen_share(c, eye, &self.targets));
        let gap = |eye: DVec3| nearest(eye, &self.body);
        let mut shot = pick_result_camera(viewers, target, self.lineup.eye, seen, framing, gap);
        if !matches!(shot.source, Source::Opponent { .. })
            && let Some(c) = collider
            && let Some(standing) = standing_view(c, self.focus, self.lineup.eye, |eye| if gap(eye) < framing.clear { 0.0 } else { seen(eye) })
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
    use nades_model::technique::{Click, Movement};
    use nades_model::{Blind, Flame, Pawn, Player, Side, Technique, ThrowId, Tick, fixture};

    /// A throw released in the air at 1000 from a take-off at (10, 20, 30), its pawn's origin at (1, 2, 3), looking at
    /// pitch -10, yaw 90, gone off at 1100 where it landed: its facts, a three-point flight, and its derivations.
    fn throw(grenade: Grenade) -> (Throw, Tracks, Derived) {
        let mut t = fixture::throw("t", "d", grenade, Side::T, [12.0, 22.0, 94.0]);
        t.tick = Tick(1000);
        t.player = Player { name: Some("ropz".into()), steamid: None };
        t.thrower = Some(Pawn { origin: [1.0, 2.0, 3.0], takeoff: Some([10.0, 20.0, 30.0]), pitch: Some(-10.0), yaw: Some(90.0), on_ground: Some(false), duck: Some(0.0), ..Default::default() });
        t.detonation = Some((Tick(1100), [500.0, 600.0, 0.0]));
        let tracks = Tracks { flight: vec![(Tick(1000), [12.0, 22.0, 94.0]), (Tick(1050), [250.0, 300.0, 300.0]), (Tick(1100), [500.0, 600.0, 0.0])], effect: Effect::Explosion };
        let d = Derived::of(&t, None);
        (t, tracks, d)
    }

    fn crouched(d: &mut Derived) {
        d.technique = Some(Technique { movement: Movement::Stand, click: Some(Click::Left), crouched: true });
    }

    #[test]
    fn the_lineup_is_the_throwers_eye_where_they_stood() {
        let (mut t, tracks, mut d) = throw(Grenade::He);
        let v = lineup_view(&t, &d);
        assert_eq!((v.eye, v.pitch, v.yaw), (DVec3::new(10.0, 20.0, 94.0), -10.0, 90.0));
        // crouched, the eye is 46 up; without a take-off point, from the origin; without the pawn, the release point
        crouched(&mut d);
        assert_eq!(lineup_view(&t, &d).eye.z, 76.0);
        t.thrower.as_mut().unwrap().takeoff = None;
        assert_eq!(lineup_view(&t, &d).eye, DVec3::new(1.0, 2.0, 49.0));
        t.thrower = None;
        assert_eq!(lineup_view(&t, &d).eye, DVec3::new(12.0, 22.0, 94.0));
        let (t, _, d) = throw(Grenade::He);
        let stage = build_stage(&t, &tracks, &d);
        assert_eq!((stage.aim.fov, stage.aim.eye), (AIM_FOV, stage.lineup.eye));
        assert_eq!(stage.release_time, -1.5625);
        assert_eq!(stage.opens_at(ViewMode::Result), 0.05);
        assert_eq!(stage.duration, 2.0, "an HE has no recorded effect");
    }

    #[test]
    fn the_flight_plays_until_it_goes_off_and_marks_its_bounce() {
        let (t, tracks, d) = throw(Grenade::He);
        let stage = build_stage(&t, &tracks, &d);
        let flight = stage.flight.as_ref().unwrap();
        assert_eq!(flight.marks().len(), 3, "release, the turn at the top, the stop");
        // in the hand at release; halfway up at 50 ticks before detonation; gone once it goes off
        assert!(flight.grenade_at(-100.0 / 64.0).is_none());
        let (mid, _) = flight.grenade_at(-50.0 / 64.0).unwrap();
        assert!(mid.distance(DVec3::new(250.0, 300.0, 300.0)) < 1e-9);
        assert!(flight.grenade_at(0.0).is_none());
        assert!(stage.light_at(0.1).is_some_and(|l| l.intensity > 0.0) && stage.light_at(0.3).is_none());
        let still = Tracks { flight: Vec::new(), ..tracks };
        assert_eq!(build_stage(&t, &still, &d).missing, ["flight path"]);
    }

    #[test]
    fn a_flash_shows_the_enemy_it_blinded_longest() {
        let (t, mut tracks, d) = throw(Grenade::Flash);
        let blind = |x: f32, seconds: f32, side: Side| Blind { player: Player::default(), side: Some(side), pos: Some([x, 600.0, 0.0]), seconds };
        tracks.effect = Effect::Flash(vec![blind(900.0, 0.5, Side::Ct), blind(600.0, 1.0, Side::T), Blind { pos: None, ..blind(0.0, 3.0, Side::Ct) }]);
        let stage = build_stage(&t, &tracks, &d);
        assert_eq!(stage.victim, Some((DVec3::new(900.0, 600.0, 64.0), 0.5)));
        assert_eq!(stage.blinded.len(), 2, "one without a position is not drawn");
        assert!(!stage.blinded[0].enemy && stage.blinded[1].enemy, "longest first; a teammate is no enemy");
        let shot = stage.result_camera(&[], None);
        assert_eq!(shot.source, Source::Victim { seconds: 0.5 });
        // it looks at where the flash went off: the flight's end
        assert!((shot.yaw - 180.0).abs() < 1e-9);
        assert!(stage.focus.distance(DVec3::new(2000.0 / 3.0, 600.0, 80.0 / 3.0)) < 1e-9, "{}", stage.focus);
    }

    #[test]
    fn a_fire_frames_the_flame_nearest_its_middle() {
        let (mut t, mut tracks, _) = throw(Grenade::Molotov);
        t.end = Some(Tick(1100 + 7 * 64));
        let d = Derived::of(&t, None);
        assert!(d.effect_recorded && d.effect_seconds == 7.0);
        tracks.effect = Effect::Fire([(400.0, 1100), (500.0, 1110), (650.0, 1150)].iter().enumerate().map(|(slot, &(x, first))| Flame { slot: slot as u32, pos: [x, 600.0, 0.0], normal: Some([0.0, 0.0, 1.0]), first: Tick(first), last: Tick(1500) }).collect());
        let stage = build_stage(&t, &tracks, &d);
        assert_eq!(stage.focus, DVec3::new(500.0, 600.0, 12.0));
        assert_eq!(stage.body.len(), 3);
        assert!(stage.duration > 7.0);
        // nobody watched: without a collider, an overview from the thrower's side
        let shot = stage.result_camera(&[], None);
        assert_eq!(shot.source, Source::Overview);
        assert!(stage.light_at(1.0).is_some_and(|l| l.intensity > 900.0));
        // an incendiary is a fire too, framed as one; one that burned without its flames says so
        t.grenade = Grenade::Incendiary;
        assert!(build_stage(&t, &tracks, &d).fire.is_some());
        tracks.effect = Effect::Fire(Vec::new());
        assert_eq!(build_stage(&t, &tracks, &d).missing, ["fire spread"]);
        assert_eq!(build_stage(&t, &tracks, &d).throw_id, ThrowId("t".into()).0);
    }

    #[test]
    fn colours_are_linear() {
        let [r, g, b] = hex(0xff8000);
        assert_eq!(r, 1.0);
        assert!((g - 0.2158605).abs() < 1e-6 && b == 0.0);
        assert_eq!(nade_color(Grenade::Incendiary), nade_color(Grenade::Molotov));
    }
}
