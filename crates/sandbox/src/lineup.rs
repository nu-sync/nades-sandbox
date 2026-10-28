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
const ELEVATIONS: [f64; 5] = [18.0, 28.0, 40.0, 54.0, 68.0];
const BEARINGS: usize = 72;
const NEAREST: f64 = 150.0;
const CLEARANCE: f64 = 24.0;
const FURTHER: [f64; 3] = [1.0, 1.3, 1.6];
const HURRY: f64 = 0.4;

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
    focus: DVec3,
    circle: Circle,
    sweep: Sweep,
    turn: f64,
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
            let behind = track.point_at((track.arc_at(t) - lag).max(0.0));
            let up = collider.and_then(|c| c.hit_distance(behind, DVec3::Z, LIFT + CLEARANCE));
            let lift = up.map_or(LIFT, |u| (u - CLEARANCE).clamp(0.0, LIFT));
            eyes.push(behind + DVec3::Z * lift);
            looks.push(track.at(t));
        }
        let chase_eyes = smooth(&eyes, SMOOTH_EYE * RATE);
        let chase_looks = smooth(&looks, SMOOTH_LOOK * RATE);

        let focus = b.stage.focus;
        let body: Vec<DVec3> = b.stage.body.clone();
        let circle = Circle::new(focus, f.radius, collider, &body, f.clear);
        let end_eye = chase_eyes[chase_eyes.len() - 1];
        let away = end_eye - focus;
        let chased = if away.x.hypot(away.y) > 32.0 { away.y.atan2(away.x) } else { (thrower.eye.y - focus.y).atan2(thrower.eye.x - focus.x) };
        let sweep = Sweep::new(&circle, circle.open_near(chased));
        Self { thrower, spot, aim, zoom, release, detonate, bloom, end: bloom + turn, released_at: b.path.release.t, start, air, chase_eyes, chase_looks, focus, circle, sweep, turn }
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
        let t = self.thrower;
        let aim = |fov: f64| CsView { fov, ..t };
        if v < self.spot {
            // where to stand: wide, looking a little down past the feet along the throw's way
            return CsView { pitch: t.pitch.max(28.0), fov: WIDE_FOV, ..t };
        }
        if v < self.aim {
            let w = smoothstep((v - self.spot) / 0.9);
            return CsView { pitch: lerp(t.pitch.max(28.0), t.pitch, w), fov: lerp(WIDE_FOV, vertical_fov(), w), ..t };
        }
        if v < self.zoom {
            return aim(lerp(vertical_fov(), AIM_FOV, smoothstep((v - self.aim) / 0.7)));
        }
        if v < self.release {
            return aim(lerp(AIM_FOV, vertical_fov(), smoothstep((v - self.zoom) / 0.5)));
        }
        if v < self.detonate {
            let (eye, look) = self.chase(v - self.release);
            let (pitch, yaw) = angles_to(eye, look);
            return CsView { eye, pitch, yaw, fov: vertical_fov() };
        }
        let (end_eye, end_look) = self.chase(self.air);
        let since = v - self.detonate;
        let start = self.circle.point(self.sweep.at(0.0));
        let (eye, look) = if v < self.bloom {
            // close on the effect as it grows: out to where the turn begins, turning to the effect
            (end_eye.lerp(start, 1.0 - (1.0 - (since / PULL).min(1.0)).powi(2)), end_look.lerp(self.focus, smoothstep(since / TURN_TO)))
        } else {
            let u = ((v - self.bloom) / self.turn).clamp(0.0, 1.0);
            (self.circle.point(self.sweep.at(turn_ease(u))), self.focus)
        };
        let (pitch, yaw) = angles_to(eye, look);
        CsView { eye, pitch, yaw, fov: vertical_fov() }
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

/// The turn's circle: for each of BEARINGS bearings, how far out and how high, and whether the effect shows from there.
struct Circle {
    focus: DVec3,
    reach: Vec<f64>,
    height: Vec<f64>,
    shown: Vec<f64>,
}

impl Circle {
    /// For each bearing the eye goes `radius` out, or further where the effect reaches further, keeping `clear` of each
    /// of its points: at the lowest elevation the map leaves clear that far (with open sky above, so not in a room the
    /// players never enter); else as near as a wall lets it come, at the elevation with the most room; else through
    /// the walls, from the lowest place with the map under it. Neighbouring bearings then share the nearest and highest
    /// of their choices, eased.
    fn new(focus: DVec3, radius: f64, map: Option<&Collider>, body: &[DVec3], clear: f64) -> Self {
        let on_map = |eye: DVec3| map.is_none_or(|c| c.floor_below(eye, 4000.0).is_some());
        let open_above = |eye: DVec3| map.is_none_or(|c| !c.blocked(eye, DVec3::Z, 300.0));
        let (mut out, mut up, mut ceiling) = (Vec::new(), Vec::new(), Vec::new());
        for k in 0..BEARINGS {
            let a = k as f64 / BEARINGS as f64 * std::f64::consts::TAU;
            let mut pick: Option<(f64, f64)> = None;
            let mut room: Option<(f64, f64)> = None;
            let mut through: Vec<(f64, f64, DVec3)> = Vec::new();
            for deg in ELEVATIONS {
                let e = deg.to_radians();
                let dir = DVec3::new(e.cos() * a.cos(), e.cos() * a.sin(), e.sin());
                let outside = clear_of(focus, dir, body, clear);
                let far = radius.max(outside);
                through.push((far, e, dir));
                let d = map.and_then(|c| c.hit_distance(focus, dir, far + CLEARANCE));
                match d {
                    None => {
                        let eye = focus + dir * far;
                        if !on_map(eye) || !open_above(eye) {
                            continue;
                        }
                        pick = Some((far, e));
                        break;
                    }
                    Some(d) => {
                        let near = d - CLEARANCE;
                        if near >= outside.max(NEAREST) && room.is_none_or(|(r, _)| near > r) && on_map(focus + dir * near) {
                            room = Some((near, e));
                        }
                    }
                }
            }
            let chosen = pick.or(room).unwrap_or_else(|| {
                through
                    .iter()
                    .flat_map(|&(r, e, dir)| FURTHER.iter().map(move |f| (r * f, e, dir)))
                    .find(|&(r, _, dir)| on_map(focus + dir * r))
                    .map_or((through[0].0, through[0].1), |(r, e, _)| (r, e))
            });
            let (r, h) = (chosen.0 * chosen.1.cos(), chosen.0 * chosen.1.sin());
            out.push(r);
            up.push(h);
            let over = map.and_then(|c| c.hit_distance(focus + DVec3::new(r * a.cos(), r * a.sin(), h), DVec3::Z, 4.0 * radius));
            ceiling.push(over.map_or(f64::INFINITY, |o| h + (o - CLEARANCE).max(0.0)));
        }
        let reach = ease(&spread(&out, f64::min));
        let height: Vec<f64> = ease(&spread(&up, f64::max)).iter().zip(&ceiling).map(|(h, c)| h.min(*c)).collect();
        let (mut reach, mut height) = (reach, height);
        let eye_at = |reach: &[f64], height: &[f64], k: usize| {
            let a = k as f64 / BEARINGS as f64 * std::f64::consts::TAU;
            focus + DVec3::new(reach[k] * a.cos(), reach[k] * a.sin(), height[k])
        };
        // a bearing the smoothing drew into the effect goes back out along its own line
        for k in 0..BEARINGS {
            let line = eye_at(&reach, &height, k) - focus;
            let far = line.length();
            let need = clear_of(focus, line / far, body, clear);
            if need > far {
                reach[k] *= need / far;
                height[k] *= need / far;
            }
        }
        let shown = ease(
            &(0..BEARINGS)
                .map(|k| {
                    let Some(c) = map else { return 1.0 };
                    let eye = eye_at(&reach, &height, k);
                    let line = eye - focus;
                    let far = line.length();
                    if on_map(eye) && c.hit_distance(focus, line / far, far).is_none() { 1.0 } else { 0.0 }
                })
                .collect::<Vec<_>>(),
        );
        Self { focus, reach, height, shown }
    }

    fn between(values: &[f64], bearing: f64) -> f64 {
        let f = (bearing / std::f64::consts::TAU * BEARINGS as f64).rem_euclid(BEARINGS as f64);
        let i = f.floor() as usize % BEARINGS;
        lerp(values[i], values[(i + 1) % BEARINGS], f - f.floor())
    }

    fn point(&self, bearing: f64) -> DVec3 {
        let r = Self::between(&self.reach, bearing);
        self.focus + DVec3::new(r * bearing.cos(), r * bearing.sin(), Self::between(&self.height, bearing))
    }

    fn open(&self, bearing: f64) -> f64 {
        Self::between(&self.shown, bearing)
    }

    /// The bearing nearest `bearing` the effect shows from, or `bearing` when it shows from none.
    fn open_near(&self, bearing: f64) -> f64 {
        let step = std::f64::consts::TAU / BEARINGS as f64;
        for k in 0..=BEARINGS / 2 {
            for b in [bearing - k as f64 * step, bearing + k as f64 * step] {
                if self.open(b) >= 0.75 {
                    return b;
                }
            }
        }
        bearing
    }
}

/// The turn's bearing at each share of it (0–1), clockwise from its start and once round, lingering where the effect
/// shows and hurrying where it does not.
struct Sweep {
    start: f64,
    total: Vec<f64>,
}

impl Sweep {
    fn new(circle: &Circle, start: f64) -> Self {
        let step = std::f64::consts::TAU / BEARINGS as f64;
        let weight = |b: f64| HURRY + (1.0 - HURRY) * circle.open(b);
        let mut total = vec![0.0];
        for k in 0..BEARINGS {
            total.push(total[k] + (weight(start - k as f64 * step) + weight(start - (k + 1) as f64 * step)) / 2.0);
        }
        Self { start, total }
    }

    fn at(&self, share: f64) -> f64 {
        let step = std::f64::consts::TAU / BEARINGS as f64;
        let want = share.clamp(0.0, 1.0) * self.total[BEARINGS];
        let k = below(&self.total, want).min(BEARINGS - 1);
        let span = self.total[k + 1] - self.total[k];
        self.start - (k as f64 + if span > 0.0 { (want - self.total[k]) / span } else { 0.0 }) * step
    }
}

/// How far out along `dir` from `from` the eye must be to stay `clear` of every one of `points`.
fn clear_of(from: DVec3, dir: DVec3, points: &[DVec3], clear: f64) -> f64 {
    let mut beyond: f64 = 0.0;
    for p in points {
        let v = *p - from;
        let along = v.dot(dir);
        let aside = v.length_squared() - along * along;
        if aside < clear * clear {
            beyond = beyond.max(along + (clear * clear - aside).sqrt());
        }
    }
    beyond
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

/// Each value replaced by the `pick` of it and its four neighbours either side, around the circle.
fn spread(values: &[f64], pick: fn(f64, f64) -> f64) -> Vec<f64> {
    let n = values.len() as i64;
    (0..n).map(|k| (-4..=4).map(|d| values[(k + d).rem_euclid(n) as usize]).reduce(pick).unwrap_or(0.0)).collect()
}

/// A 1-2-3-4-3-2-1 average around the circle.
fn ease(values: &[f64]) -> Vec<f64> {
    let n = values.len() as i64;
    (0..n).map(|k| [1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0].iter().enumerate().map(|(d, w)| w * values[(k + d as i64 - 3).rem_euclid(n) as usize]).sum::<f64>() / 16.0).collect()
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
