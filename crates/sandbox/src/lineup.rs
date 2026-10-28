//! A lineup's video as jumpthrow.pro's lineup videos play it (S34): where to stand, the aim, a close-up of what the
//! crosshair lines up with, the throw, the camera after the grenade through the air down to where it lands, the effect
//! blooming, then one slow turn around it. The crosshair shows until the grenade leaves the hand.
//!
//! The chase and the turn are ports of show-me-the-nades' Orbit view (`web/app/src/scene/flight.ts`, its D29): the chase
//! rides the grenade's own path a little behind and above it, easing back before it goes off; the turn circles the
//! effect clockwise at a distance that frames it and outside the effect, rising over walls, lingering where the effect
//! shows and hurrying where it does not. Every pose is a function of the time into the video.

use glam::DVec3;
use nades_scene::camera::{AIM_FOV, View as CsView, angles_to, forward, vertical_fov};
use nades_scene::collider::Collider;

use crate::sim::Built;

/// The phases' lengths before scaling, seconds: the spot, the aim, the aim point, and the hold on the aim before the
/// release.
const SPOT: f64 = 2.0;
const AIM: f64 = 2.0;
const ZOOM: f64 = 2.2;
const THROW: f64 = 1.6;
/// The field of view the spot is shown with, wide.
const WIDE_FOV: f64 = 100.0;
/// How long the eye takes to leave the thrower's view and fall in behind the grenade.
const LEAVE: f64 = 0.5;
/// How far behind the grenade the chase rides its path, and above it; over the last SETTLE seconds it eases back to
/// SETTLED_LAG so it is not inside what blooms where the grenade lies.
const LAG: f64 = 70.0;
const SETTLED_LAG: f64 = 150.0;
const SETTLE: f64 = 0.6;
const LIFT: f64 = 12.0;
/// The chase's smoothing, σ in seconds, of the eye and of where it looks.
const SMOOTH_EYE: f64 = 0.08;
const SMOOTH_LOOK: f64 = 0.14;
const RATE: f64 = 120.0;
/// Once it goes off: how long the eye takes to pull out to where it watches the bloom, and to turn to the effect.
const PULL: f64 = 1.3;
const TURN_TO: f64 = 0.9;
/// The share of the turn spent speeding up, and again slowing down.
const RAMP: f64 = 0.2;
const BEARINGS: usize = 72;
const CLEARANCE: f64 = 24.0;

/// How a kind of grenade's result is framed: the turn's radius, how clear of the effect's points it keeps, how long
/// the bloom is watched and the turn takes.
struct Framing {
    radius: f64,
    clear: f64,
    bloom: f64,
    turn: f64,
}

fn framing(b: &Built) -> Framing {
    let s = &b.stage;
    if s.cloud.is_some() {
        Framing { radius: 400.0, clear: 100.0, bloom: 4.5, turn: 8.0 }
    } else if let Some(fire) = &s.fire {
        // a fire burns about seven seconds: the turn ends a second before it dies
        let lasts = fire.lasts();
        Framing { radius: 320.0, clear: 40.0, bloom: 2.5, turn: (lasts - 3.5).clamp(4.0, 8.0) }
    } else {
        Framing { radius: 360.0, clear: 0.0, bloom: 1.5, turn: 5.0 }
    }
}

/// The whole video's moves, laid out.
pub struct Sequence {
    thrower: CsView,
    /// Video seconds where each phase ends: spot, aim, aim point, release, detonation, bloom, turn (the end).
    spot: f64,
    aim: f64,
    zoom: f64,
    release: f64,
    detonate: f64,
    bloom: f64,
    end: f64,
    /// The throw's own clock: seconds from its start to the release, and to its detonation.
    released_at: f64,
    start: f64,
    air: f64,
    chase_eyes: Vec<DVec3>,
    chase_looks: Vec<DVec3>,
    raw_looks: Vec<DVec3>,
    focus: DVec3,
    /// Points that show the effect ([`effect_marks`]).
    marks: Vec<DVec3>,
    circle: Circle,
    /// The turn, or none when nowhere round the effect is clear: the camera stays where the chase ended.
    sweep: Option<Sweep>,
    pull: Option<Pull>,
    turn: f64,
    /// From the release on, RATE a second: how far each pose moves off what its near plane would cut.
    shifts: Vec<DVec3>,
}

impl Sequence {
    /// The video of `b` (a throw starting at `start` on the scene's clock), framed against the map's `collider`;
    /// `length` stretches or shrinks every phase but the flight to make the video that long.
    pub fn new(b: &Built, start: f64, collider: Option<&Collider>, length: Option<f64>) -> Self {
        let f = framing(b);
        let air = (b.path.detonation.t - b.path.release.t).max(0.2);
        let base = SPOT + AIM + ZOOM + THROW + f.bloom + f.turn;
        let k = length.map_or(1.0, |l| ((l - air) / base).clamp(0.3, 3.0));
        let (spot, aim, zoom, release) = (SPOT * k, (SPOT + AIM) * k, (SPOT + AIM + ZOOM) * k, (SPOT + AIM + ZOOM + THROW) * k);
        let detonate = release + air;
        let bloom = detonate + f.bloom * k;
        let turn = f.turn * k;
        let thrower = b.stage.thrower;

        // the chase, sampled from release to detonation and smoothed
        let points = &b.path.points;
        let track = Track::new(points);
        let n = ((air * RATE).ceil() as usize + 1).max(2);
        let (mut eyes, mut looks) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for i in 0..n {
            let t = b.path.release.t + i as f64 / (n - 1) as f64 * air;
            let to_go = b.path.detonation.t - t;
            let lag = lerp(LAG, SETTLED_LAG, smoothstep((SETTLE - to_go) / SETTLE));
            let grenade = track.at(t);
            let eye_at = |lag: f64| {
                let behind = track.point_at((track.arc_at(t) - lag).max(0.0));
                let up = collider.and_then(|c| c.hit_distance(behind, DVec3::Z, LIFT + CLEARANCE));
                let lift = up.map_or(LIFT, |u| (u - CLEARANCE).clamp(0.0, LIFT));
                behind + DVec3::Z * lift
            };
            // round a corner the eye closes in on the grenade until it sees it again
            let eye = CLOSER.iter().map(|k| eye_at(lag * k)).find(|e| collider.is_none_or(|c| chase_fits(c, *e, grenade))).unwrap_or_else(|| collider.map_or(eye_at(lag), |c| nudge(c, eye_at(lag))));
            eyes.push(eye);
            looks.push(grenade);
        }
        let mut chase_eyes = smooth(&eyes, SMOOTH_EYE * RATE);
        // where the smoothing cuts a corner into the map, the eye keeps to the grenade's own path
        if let Some(c) = collider {
            for i in 0..n {
                if !chase_fits(c, chase_eyes[i], looks[i]) {
                    chase_eyes[i] = if chase_fits(c, eyes[i], looks[i]) { eyes[i] } else { nudge(c, chase_eyes[i]) };
                }
            }
        }
        let chase_looks = smooth(&looks, SMOOTH_LOOK * RATE);
        let raw_looks = looks;

        let focus = b.stage.focus;
        let body: Vec<DVec3> = b.stage.body.clone();
        let circle = Circle::new(focus, f.radius, collider, &body, f.clear);
        let end_eye = chase_eyes[chase_eyes.len() - 1];
        let away = end_eye - focus;
        let chased = if away.x.hypot(away.y) > 32.0 { away.y.atan2(away.x) } else { (thrower.eye.y - focus.y).atan2(thrower.eye.x - focus.x) };
        let chased = (chased / std::f64::consts::TAU * BEARINGS as f64).rem_euclid(BEARINGS as f64);
        let seen = |k: f64| collider.is_none_or(|c| pull_clear(c, end_eye, circle.at(k)));
        // once round, clockwise, from the bearing nearest the chase's end that it can reach in a straight line
        // (and where the bloom shows from), so the bloom is watched from where it shows
        let marks = effect_marks(focus, &body);
        let shows = |k: f64| collider.is_none_or(|c| eye_fits(c, focus, &marks, &body, f.clear, circle.at(k)));
        let near = || (0..=BEARINGS / 2).flat_map(|d| [chased.round() - d as f64, chased.round() + d as f64]);
        let from = near().find(|&k| seen(k) && shows(k)).or_else(|| near().find(|&k| shows(k))).or_else(|| near().find(|&k| seen(k))).unwrap_or(chased.round());
        let sweep = Some(Sweep { from, steps: -(BEARINGS as f64) });
        let pull = sweep.as_ref().map(|s| circle.at(s.at(0.0))).map(|to| collider.map_or(Pull::Straight(to), |c| Pull::plan(c, end_eye, to)));
        let mut sequence = Self { thrower, spot, aim, zoom, release, detonate, bloom, end: bloom + turn, released_at: b.path.release.t, start, air, chase_eyes, chase_looks, raw_looks, focus, marks, circle, sweep, pull, turn, shifts: Vec::new() };
        // last, every pose of the chase and the pull moved off the surfaces its near plane would cut, eased
        if let Some(c) = collider {
            let n = ((bloom - release) * RATE).ceil() as usize + 1;
            let shifts: Vec<DVec3> = (0..n)
                .map(|i| {
                    let (view, _) = sequence.pose(release + i as f64 / RATE);
                    clear_of_near(c, &view) - view.eye
                })
                .collect();
            let eased = smooth(&shifts, 0.04 * RATE);
            // easing must not weaken a shift: each keeps the larger of the two
            sequence.shifts = shifts.iter().zip(&eased).map(|(a, b)| if a.length() > b.length() { *a } else { *b }).collect();
        }
        sequence
    }

    /// Seconds the video lasts.
    pub fn seconds(&self) -> f64 {
        self.end
    }

    /// The scene's clock at `v` seconds into the video: held at the throw's start until it is thrown, then running.
    pub fn scene_time(&self, v: f64) -> f64 {
        self.start + (v - (self.release - self.released_at)).max(0.0)
    }

    /// Seconds into the video of one of its moments.
    pub fn moment(&self, m: nades_control::video::Moment) -> f64 {
        use nades_control::video::Moment;
        match m {
            Moment::Spot => self.spot * 0.6,
            Moment::Aim => (self.spot + self.aim) / 2.0 + 0.4,
            Moment::AimPoint => self.zoom - 0.3,
            Moment::Flight => self.release + self.air / 2.0,
            Moment::Detonation => self.detonate,
            Moment::Bloom => self.bloom - 0.2,
            Moment::Result => self.bloom + self.turn / 2.0,
        }
    }

    /// Whether the crosshair shows: until the grenade leaves the hand.
    pub fn crosshair(&self, v: f64) -> bool {
        v < self.release
    }

    /// The camera `v` seconds into the video.
    pub fn camera(&self, v: f64) -> CsView {
        self.shot(v).0
    }

    /// The camera and its look target, moved off the surfaces its near plane would cut.
    fn shot(&self, v: f64) -> (CsView, Option<DVec3>) {
        let (mut view, target) = self.pose(v);
        if v >= self.release && v < self.bloom && !self.shifts.is_empty() {
            let f = ((v - self.release) * RATE).clamp(0.0, (self.shifts.len() - 1) as f64);
            let i = (f.floor() as usize).min(self.shifts.len().saturating_sub(2));
            // easing out over the pull's last half second into the turn, which is let be
            let shift = self.shifts[i].lerp(self.shifts[(i + 1).min(self.shifts.len() - 1)], f - i as f64) * smoothstep((self.bloom - v) / 0.5);
            if shift != DVec3::ZERO {
                let look = view.eye + forward(view.pitch, view.yaw) * 100.0;
                view.eye += shift;
                (view.pitch, view.yaw) = angles_to(view.eye, look);
            }
        }
        (view, target)
    }

    /// The camera `v` seconds into the video, and what it looks at once the grenade is thrown.
    fn pose(&self, v: f64) -> (CsView, Option<DVec3>) {
        let t = self.thrower;
        let aim = |fov: f64| CsView { fov, ..t };
        if v < self.spot {
            // where to stand: wide, looking a little down past the feet along the throw's way
            return (CsView { pitch: t.pitch.max(28.0), fov: WIDE_FOV, ..t }, None);
        }
        if v < self.aim {
            let w = smoothstep((v - self.spot) / 0.9);
            return (CsView { pitch: lerp(t.pitch.max(28.0), t.pitch, w), fov: lerp(WIDE_FOV, vertical_fov(), w), ..t }, None);
        }
        if v < self.zoom {
            return (aim(lerp(vertical_fov(), AIM_FOV, smoothstep((v - self.aim) / 0.7))), None);
        }
        if v < self.release {
            return (aim(lerp(AIM_FOV, vertical_fov(), smoothstep((v - self.zoom) / 0.5))), None);
        }
        if v < self.detonate {
            let (eye, look) = self.chase(v - self.release);
            let (pitch, yaw) = angles_to(eye, look);
            return (CsView { eye, pitch, yaw, fov: vertical_fov() }, Some(self.grenade(v - self.release)));
        }
        let (end_eye, end_look) = self.chase(self.air);
        let since = v - self.detonate;
        let (eye, look) = match (&self.sweep, &self.pull) {
            (Some(sweep), Some(pull)) if v >= self.bloom => {
                let u = ((v - self.bloom) / self.turn).clamp(0.0, 1.0);
                (self.circle.at(sweep.at(turn_ease(u))), self.focus)
            }
            // close on the effect as it grows: out to where the turn begins, turning to the effect
            (Some(_), Some(pull)) => (pull.at(end_eye, 1.0 - (1.0 - (since / PULL).min(1.0)).powi(2)), end_look.lerp(self.focus, smoothstep(since / TURN_TO))),
            _ => (end_eye, end_look.lerp(self.focus, smoothstep(since / TURN_TO))),
        };
        let (pitch, yaw) = angles_to(eye, look);
        (CsView { eye, pitch, yaw, fov: vertical_fov() }, Some(self.focus))
    }

    /// The chase's and the pull's frames, at `fps`, checked against what the map draws (`sight`): the video seconds
    /// where the near plane cuts into a surface, and where a surface stands between the eye and the grenade or the
    /// effect. Before the release the eye is the player's own; the turn may pass through walls (S36).
    pub fn audit(&self, sight: &Collider, fps: f64) -> Audit {
        let first = (self.release * fps).ceil() as usize;
        let frames = (self.bloom * fps).ceil() as usize;
        let mut audit = Audit { frames: frames - first.min(frames), ..Audit::default() };
        for i in first..frames {
            let v = i as f64 / fps;
            let (view, target) = self.shot(v);
            if near_plane_cut(sight, &view) {
                audit.clipped.push(v);
            }
            let seen = match target {
                Some(at) if at == self.focus => sees_effect(sight, &self.marks, view.eye),
                Some(at) => sight.clear_between(view.eye, at),
                None => true,
            };
            if !seen {
                audit.hidden.push(v);
            }
        }
        audit
    }

    /// Where the grenade is `since` seconds after the release.
    fn grenade(&self, since: f64) -> DVec3 {
        let n = self.chase_looks.len();
        self.raw_looks[((since / self.air * (n - 1) as f64).round() as usize).min(n - 1)]
    }

    /// The chase `since` seconds after the release: out of the thrower's eye onto the grenade's path, turning from the
    /// aim to the grenade.
    fn chase(&self, since: f64) -> (DVec3, DVec3) {
        let n = self.chase_eyes.len();
        let f = (since / self.air * (n - 1) as f64).clamp(0.0, (n - 1) as f64);
        let i = (f.floor() as usize).min(n - 2);
        let eye = self.chase_eyes[i].lerp(self.chase_eyes[i + 1], f - i as f64);
        let look = self.chase_looks[i].lerp(self.chase_looks[i + 1], f - i as f64);
        let w = smoothstep(since / LEAVE);
        let from = self.thrower.eye.lerp(eye, w);
        let toward = look - from;
        let dist = toward.length().max(1.0);
        let dir = forward(self.thrower.pitch, self.thrower.yaw).lerp(toward / dist, w).normalize_or(DVec3::X);
        (from, from + dir * dist)
    }
}

/// How the eye goes from the chase's end to where the turn begins: straight, up and then across, or a cut.
enum Pull {
    Straight(DVec3),
    Rising(DVec3, DVec3),
    Cut(DVec3),
}

impl Pull {
    fn plan(c: &Collider, from: DVec3, to: DVec3) -> Self {
        if pull_clear(c, from, to) {
            return Self::Straight(to);
        }
        let up = DVec3::new(from.x, from.y, from.z.max(to.z));
        if pull_clear(c, from, up) && pull_clear(c, up, to) { Self::Rising(up, to) } else { Self::Cut(to) }
    }

    /// The eye `w` (0–1) of the way from `from`.
    fn at(&self, from: DVec3, w: f64) -> DVec3 {
        match *self {
            Self::Straight(to) => from.lerp(to, w),
            Self::Rising(up, to) => {
                let (a, b) = (from.distance(up), up.distance(to));
                let s = w * (a + b);
                if s < a { from.lerp(up, s / a.max(1e-9)) } else { up.lerp(to, ((s - a) / b.max(1e-9)).min(1.0)) }
            }
            Self::Cut(to) => to,
        }
    }
}

/// Whether the eye can move from `a` to `b` without its near plane meeting what the map draws: nothing within
/// CLEARANCE / 2 of the line between them (the line itself and four offset alongside it).
fn pull_clear(c: &Collider, a: DVec3, b: DVec3) -> bool {
    let d = b - a;
    let len = d.length();
    if len < 1.0 {
        return true;
    }
    let dir = d / len;
    let side = dir.cross(DVec3::Z).normalize_or(DVec3::X);
    let up = side.cross(dir);
    let r = CLEARANCE / 2.0;
    [DVec3::ZERO, side * r, -side * r, up * r, -up * r].iter().all(|o| !c.blocked(a + *o, dir, len))
}

/// Fourteen directions round a point: the axes and the corners of a cube.
const AROUND: [[f64; 3]; 14] = [
    [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0],
    [0.577, 0.577, 0.577], [0.577, 0.577, -0.577], [0.577, -0.577, 0.577], [0.577, -0.577, -0.577],
    [-0.577, 0.577, 0.577], [-0.577, 0.577, -0.577], [-0.577, -0.577, 0.577], [-0.577, -0.577, -0.577],
];

/// The shares of the chase's lag it tries, nearest last, to keep the grenade in sight round a corner.
const CLOSER: [f64; 6] = [1.0, 0.7, 0.5, 0.35, 0.25, 0.15];
/// How far from every surface the chase's eye keeps: more than the near plane's corners reach.
const NEAR_ROOM: f64 = 10.0;

/// Whether the chase's eye at `eye` is clear of the map by NEAR_ROOM and sees the grenade at `grenade`.
fn chase_fits(c: &Collider, eye: DVec3, grenade: DVec3) -> bool {
    AROUND.iter().all(|d| !c.blocked(eye, DVec3::from_array(*d), NEAR_ROOM)) && c.clear_between(eye, grenade)
}

/// `eye` moved away from the surfaces within NEAR_ROOM of it, where the grenade's path runs that close to a wall.
fn nudge(c: &Collider, mut eye: DVec3) -> DVec3 {
    for _ in 0..6 {
        let push: DVec3 = AROUND
            .iter()
            .map(|d| DVec3::from_array(*d))
            .filter_map(|d| c.hit_distance(eye, d, NEAR_ROOM).map(|h| -d * (NEAR_ROOM - h)))
            .sum();
        if push.length() < 0.5 {
            break;
        }
        eye += push * 0.5;
    }
    eye
}

/// The eye of `view` moved off every surface its near plane or NEAR_ROOM round it would meet: unchanged where nothing
/// is that near.
fn clear_of_near(c: &Collider, view: &CsView) -> DVec3 {
    let mut eye = view.eye;
    for _ in 0..8 {
        let corners = near_corners(&CsView { eye, ..*view });
        let push: DVec3 = AROUND
            .iter()
            .map(|d| DVec3::from_array(*d) * NEAR_ROOM)
            .chain(corners.iter().map(|c| *c * 1.5))
            .filter_map(|ray| {
                let len = ray.length();
                c.hit_distance(eye, ray / len, len).map(|h| -ray / len * (len - h + 0.5))
            })
            .sum();
        if push.length() < 0.25 {
            break;
        }
        eye += push * 0.5;
    }
    eye
}

/// The near plane's middle and corners from the eye of `view` (16:9).
fn near_corners(view: &CsView) -> [DVec3; 5] {
    let f = forward(view.pitch, view.yaw);
    let right = f.cross(DVec3::Z).normalize_or(DVec3::Y);
    let up = right.cross(f);
    let h = NEAR * (view.fov.to_radians() / 2.0).tan();
    let w = h * 16.0 / 9.0;
    [(0.0, 0.0), (-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)].map(|(x, y)| f * NEAR + right * (w * x) + up * (h * y))
}

/// A video's frames checked against what the map draws.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Audit {
    pub frames: usize,
    /// Video seconds where the near plane cuts into a surface: the camera is in a wall.
    pub clipped: Vec<f64>,
    /// Video seconds where a surface hides the grenade or the effect from the eye.
    pub hidden: Vec<f64>,
}

/// The renderer's near plane, units.
const NEAR: f64 = 4.0;

/// Whether the near plane of `view` (16:9) cuts into a surface: a ray from the eye to its middle or a corner meets one.
fn near_plane_cut(c: &Collider, view: &CsView) -> bool {
    near_corners(view).iter().any(|corner| {
        let d = corner.length();
        c.blocked(view.eye, *corner / d, d)
    })
}

/// The flight by its clock and by its own length.
struct Track {
    times: Vec<f64>,
    points: Vec<DVec3>,
    arc: Vec<f64>,
}

impl Track {
    fn new(samples: &[(f64, DVec3)]) -> Self {
        let times: Vec<f64> = samples.iter().map(|(t, _)| *t).collect();
        let points: Vec<DVec3> = samples.iter().map(|(_, p)| *p).collect();
        let mut arc = vec![0.0];
        for i in 1..points.len() {
            arc.push(arc[i - 1] + points[i].distance(points[i - 1]));
        }
        Self { times, points, arc }
    }

    fn along(&self, keys: &[f64], v: f64) -> DVec3 {
        if self.points.is_empty() {
            return DVec3::ZERO;
        }
        let i = below(keys, v);
        if i >= self.points.len() - 1 {
            return self.points[self.points.len() - 1];
        }
        let span = keys[i + 1] - keys[i];
        self.points[i].lerp(self.points[i + 1], if span > 0.0 { ((v - keys[i]) / span).clamp(0.0, 1.0) } else { 0.0 })
    }

    fn at(&self, t: f64) -> DVec3 {
        self.along(&self.times, t)
    }

    fn point_at(&self, s: f64) -> DVec3 {
        self.along(&self.arc, s)
    }

    fn arc_at(&self, t: f64) -> f64 {
        let i = below(&self.times, t);
        if i >= self.points.len().saturating_sub(1) {
            return self.arc.last().copied().unwrap_or(0.0);
        }
        let span = self.times[i + 1] - self.times[i];
        let f = if span > 0.0 { ((t - self.times[i]) / span).clamp(0.0, 1.0) } else { 0.0 };
        lerp(self.arc[i], self.arc[i + 1], f)
    }
}

/// The turn: one level circle round the effect at the frame's radius, the same height all the way round, so it turns
/// smoothly; the walls it passes through are let be (the user's call, S36). Its height is the lowest up to 42° at
/// which most bearings see the effect from a clear place ([`eye_fits`]).
struct Circle {
    focus: DVec3,
    radius: f64,
    height: f64,
}

/// The heights a turn may take, as elevations at the frame's radius, degrees.
const HEIGHTS: [f64; 6] = [10.0, 16.0, 22.0, 28.0, 35.0, 42.0];

impl Circle {
    fn new(focus: DVec3, radius: f64, map: Option<&Collider>, body: &[DVec3], clear: f64) -> Self {
        use rayon::prelude::*;
        let heights: Vec<f64> = HEIGHTS.iter().map(|e| radius * e.to_radians().tan()).collect();
        let Some(c) = map else { return Self { focus, radius, height: heights[2] } };
        let marks = effect_marks(focus, body);
        let seen = |h: f64| (0..BEARINGS).into_par_iter().filter(|&k| eye_fits(c, focus, &marks, body, clear, Self { focus, radius, height: h }.at(k as f64))).count();
        let counts: Vec<usize> = heights.iter().map(|&h| seen(h)).collect();
        let most = counts.iter().copied().max().unwrap_or(0);
        // the lowest within a few bearings of the best
        let pick = counts.iter().position(|&n| n + 4 >= most).unwrap_or(2);
        Self { focus, radius, height: heights[pick] }
    }

    /// The eye at a bearing counted in steps (0 to BEARINGS round the circle).
    fn at(&self, k: f64) -> DVec3 {
        let a = k / BEARINGS as f64 * std::f64::consts::TAU;
        self.focus + DVec3::new(self.radius * a.cos(), self.radius * a.sin(), self.height)
    }
}

/// Points that show the effect: its middle, and for a cloud or a fire its points furthest out each way and its top.
fn effect_marks(focus: DVec3, body: &[DVec3]) -> Vec<DVec3> {
    let mut marks = vec![focus];
    for d in [DVec3::X, DVec3::NEG_X, DVec3::Y, DVec3::NEG_Y, DVec3::Z, DVec3::new(1.0, 1.0, 0.0), DVec3::new(1.0, -1.0, 0.0), DVec3::new(-1.0, 1.0, 0.0), DVec3::new(-1.0, -1.0, 0.0)] {
        if let Some(p) = body.iter().max_by(|a, b| (**a - focus).dot(d).total_cmp(&(**b - focus).dot(d))) {
            // a little inside the edge, so it is the effect seen and not the wall it touches
            marks.push(focus.lerp(*p, 0.7));
        }
    }
    marks
}

/// Whether the effect shows from `eye`: its middle, or two of its marks, with nothing the map draws in between.
fn sees_effect(c: &Collider, marks: &[DVec3], eye: DVec3) -> bool {
    c.clear_between(eye, marks[0]) || marks[1..].iter().filter(|m| c.clear_between(eye, **m)).take(2).count() == 2
}

/// Whether an eye at `eye` is a place to watch the effect from: the map under it, the effect in sight
/// ([`sees_effect`]), CLEARANCE from every surface, and `clear` of each of the effect's points.
fn eye_fits(c: &Collider, _focus: DVec3, marks: &[DVec3], body: &[DVec3], clear: f64, eye: DVec3) -> bool {
    body.iter().all(|p| p.distance_squared(eye) >= clear * clear)
        && AROUND.iter().all(|d| !c.blocked(eye, DVec3::from_array(*d), CLEARANCE))
        && sees_effect(c, marks, eye)
        && c.floor_below(eye, 4000.0).is_some()
}

/// The turn: from its first eye along the circle's steps, `steps` of them, forwards or backwards, at an even pace.
struct Sweep {
    from: f64,
    steps: f64,
}

impl Sweep {
    /// The step `share` (0–1) of the way through the turn.
    fn at(&self, share: f64) -> f64 {
        self.from + self.steps * share.clamp(0.0, 1.0)
    }
}


/// How far through the turn it is at `u`: speeding up over the first RAMP of it, steady, then slowing.
fn turn_ease(u: f64) -> f64 {
    let t = u.clamp(0.0, 1.0);
    let v = 1.0 / (1.0 - RAMP);
    if t < RAMP {
        v * t * t / (2.0 * RAMP)
    } else if t > 1.0 - RAMP {
        1.0 - v * (1.0 - t).powi(2) / (2.0 * RAMP)
    } else {
        v * (t - RAMP / 2.0)
    }
}

/// The last index of a sorted list whose value is at most `v`, or 0.
fn below(sorted: &[f64], v: f64) -> usize {
    sorted.partition_point(|x| *x <= v).saturating_sub(1)
}

/// A track smoothed by a Gaussian of `sigma` samples, its ends held.
fn smooth(track: &[DVec3], sigma: f64) -> Vec<DVec3> {
    let r = (sigma * 3.0).ceil() as i64;
    let weights: Vec<f64> = (-r..=r).map(|j| (-((j * j) as f64) / (2.0 * sigma * sigma)).exp()).collect();
    (0..track.len())
        .map(|k| {
            let (mut sum, mut total) = (DVec3::ZERO, 0.0);
            for j in -r..=r {
                let p = track[(k as i64 + j).clamp(0, track.len() as i64 - 1) as usize];
                let w = weights[(j + r) as usize];
                sum += p * w;
                total += w;
            }
            sum / total
        })
        .collect()
}



fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn smoothstep(x: f64) -> f64 {
    let t = x.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim;
    use nades_control::scene::{Grenade, SceneThrow};

    /// Lineups on packed maps, every frame checked against what the map draws: `cargo test --release -p nades-sandbox
    /// lineup_probe -- --ignored --nocapture`. `NADES_PROBE_CASES` names a JSON-lines file of other cases
    /// (`{"map", "grenade", "line", "movement", "speed"}`).
    #[test]
    #[ignore = "needs the packs"]
    fn lineup_probe() {
        use nades_control::scene::{Movement, parse_console};
        let cases = [
            ("de_mirage", Grenade::Smoke, "setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0", Movement::Jumpthrow, Some(30.0)),
            ("de_inferno", Grenade::Smoke, "setpos 457.29 1833.34 136.03; setang -0.79 85.07 0", Movement::Jumpthrow, None),
            ("de_inferno", Grenade::Flash, "setpos 1322.07 221.88 138.03; setang -8.35 -23.14 0", Movement::Stand, None),
            ("de_inferno", Grenade::He, "setpos 152.83 851.23 80.49; setang 0.12 90.10 0", Movement::Stand, None),
            ("de_inferno", Grenade::Molotov, "setpos 479.05 2016.97 136.03; setang -41.02 90.36 0", Movement::Stand, None),
            ("de_inferno", Grenade::Incendiary, "setpos 1947.66 -187.86 260.03; setang 12.99 -147.77 0", Movement::Stand, None),
            ("de_inferno", Grenade::Decoy, "setpos 782.64 2124.34 136.03; setang 12.85 -155.33 0", Movement::Stand, None),
        ];
        let mut cases: Vec<(String, Grenade, String, Movement, Option<f64>)> = cases.iter().map(|(m, g, l, mv, sp)| (m.to_string(), *g, l.to_string(), *mv, *sp)).collect();
        if let Some(file) = std::env::var_os("NADES_PROBE_CASES") {
            cases = std::fs::read_to_string(file)
                .unwrap()
                .lines()
                .map(|l| {
                    let v: serde_json::Value = serde_json::from_str(l).unwrap();
                    let movement = serde_json::from_value(v["movement"].clone()).unwrap();
                    (v["map"].as_str().unwrap().to_string(), Grenade::parse(v["grenade"].as_str().unwrap()).unwrap(), v["line"].as_str().unwrap().to_string(), movement, v["speed"].as_f64())
                })
                .collect();
        }
        let (mut frames, mut bad, mut turned, mut lineups) = (0, 0, 0.0, 0);
        let spans = |v: &[f64]| {
            v.iter().fold(Vec::<(f64, f64)>::new(), |mut acc, &x| {
                match acc.last_mut() {
                    Some(l) if x - l.1 < 0.05 => l.1 = x,
                    _ => acc.push((x, x)),
                }
                acc
            })
        };
        let mut maps = std::collections::HashMap::new();
        for (map, grenade, line, movement, speed) in cases {
            let (physics, sight) = maps.entry(map.clone()).or_insert_with(|| (crate::assets::collider(&map).unwrap(), crate::assets::sight(&map).unwrap()));
            let (feet, pitch, yaw) = parse_console(&line).unwrap();
            let t = SceneThrow { movement, speed, ..SceneThrow::new(grenade, feet, pitch, yaw) };
            let b = sim::simulate(&t, Some(physics), DVec3::Z);
            let started = std::time::Instant::now();
            let s = Sequence::new(&b, 0.0, Some(sight), None);
            let planned = started.elapsed().as_secs_f64() * 1e3;
            let a = s.audit(sight, 60.0);
            lineups += 1;
            frames += a.frames;
            bad += a.clipped.len() + a.hidden.len();
            turned += s.sweep.as_ref().map_or(0.0, |w| w.steps.abs() * 5.0);
            let turn = s.sweep.as_ref().map_or("none".to_string(), |w| format!("{:.0}° from {:.0}", w.steps.abs() * 5.0, w.from * 5.0));
            let pull = match s.pull { Some(Pull::Straight(_)) => "straight", Some(Pull::Rising(..)) => "rising", Some(Pull::Cut(_)) => "cut", None => "none" };
            println!(
                "{map} {grenade:?}: planned {planned:.0} ms, turn {turn}, height {:.0}, radius {:.0}, pull {pull}; {} frames: clipped {} hidden {} (detonate {:.2} bloom {:.2})",
                s.circle.height,
                s.circle.radius,
                a.frames,
                a.clipped.len(),
                a.hidden.len(),
                s.detonate,
                s.bloom
            );
            for &v in a.clipped.iter().take(3) {
                let (view, target) = s.shot(v);
                let near: Vec<String> = AROUND.iter().map(|d| sight.hit_distance(view.eye, DVec3::from_array(*d), 30.0).map_or("-".into(), |h| format!("{h:.0}"))).collect();
                let i = ((v - s.release) / s.air * (s.chase_eyes.len() - 1) as f64).round() as usize;
                println!("  at {v:.2}: eye {:.0?} pitch {:.0} grenade {:.0?} room {near:?} chase sample fits {}", view.eye, view.pitch, target, s.chase_eyes.get(i).is_some_and(|e| chase_fits(sight, *e, s.raw_looks[i])));
            }
            if !a.clipped.is_empty() || !a.hidden.is_empty() {
                println!("  clipped {:?}\n  hidden {:?}", spans(&a.clipped), spans(&a.hidden));
            }
        }
        println!("{lineups} lineups, {frames} frames: {bad} clipped or hidden; the turn {:.0}° on average", turned / lineups as f64);
    }

    #[test]
    fn the_video_shows_the_spot_the_aim_the_flight_and_the_result_with_the_crosshair_until_the_release() {
        let b = sim::simulate(&SceneThrow::new(Grenade::Flash, [0.0, 0.0, 0.0], -20.0, 90.0), None, DVec3::Z);
        let s = Sequence::new(&b, 0.0, None, None);
        let air = b.path.detonation.t - b.path.release.t;
        assert!((s.seconds() - (SPOT + AIM + ZOOM + THROW + air + 1.5 + 5.0)).abs() < 1e-9);
        // the spot is wide, the aim point zoomed, the release the thrower's own view
        assert_eq!(s.camera(1.0).fov, WIDE_FOV);
        assert!((s.camera(SPOT + AIM + 1.5).fov - AIM_FOV).abs() < 1e-9);
        let at_release = s.camera(s.release - 1e-6);
        assert!(at_release.eye.distance(b.stage.thrower.eye) < 1e-9 && (at_release.fov - vertical_fov()).abs() < 1e-9);
        assert!(s.crosshair(s.release - 0.01) && !s.crosshair(s.release + 0.01));
        // the scene's clock waits for the throw, then runs: the grenade leaves the hand at the release
        assert_eq!(s.scene_time(1.0), 0.0);
        assert!((s.scene_time(s.release) - b.path.release.t).abs() < 1e-9);
        // the chase trails the grenade; at the end the camera looks at what it left
        let mid = s.camera(s.release + air / 2.0);
        let grenade = Track::new(&b.path.points).at(b.path.release.t + air / 2.0);
        assert!(mid.eye.distance(grenade) < 160.0, "{}", mid.eye.distance(grenade));
        let last = s.camera(s.seconds());
        assert!(forward(last.pitch, last.yaw).dot((b.stage.focus - last.eye).normalize()) > 0.999);
        // a longer video stretches the phases, not the flight
        let long = Sequence::new(&b, 0.0, None, Some(40.0));
        assert!((long.seconds() - 40.0).abs() < 1e-6 && (long.detonate - long.release - air).abs() < 1e-9);
    }
}
