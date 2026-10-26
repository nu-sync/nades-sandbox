//! CS2's view, so a lineup shot sees what the thrower saw, and the choice of camera for a result shot: a port of
//! show-me-the-nades' `web/app/src/scene/camera.ts`, in the same doubles, so both pick the same camera.
//!
//! World units are Hammer units with z up. Pitch is positive looking down; yaw turns from +x toward +y. CS2 renders 90°
//! horizontally at 4:3 and keeps that vertical angle on wider screens, so the vertical field of view is
//! 2·atan(tan(45°)·3/4) ≈ 73.74°. The eye is 64 units above the feet standing and 46 crouched.

use glam::DVec3;

/// CS2's vertical field of view, in degrees.
pub fn vertical_fov() -> f64 {
    2.0 * (std::f64::consts::FRAC_PI_4.tan() * 0.75).atan() * 180.0 / std::f64::consts::PI
}
/// About 3× the normal view: the zoom lineup sites use for the aim point.
pub const AIM_FOV: f64 = 28.0;
pub const EYE_STANDING: f64 = 64.0;
pub const EYE_CROUCHED: f64 = 46.0;

fn rad(deg: f64) -> f64 {
    deg * std::f64::consts::PI / 180.0
}

fn deg(r: f64) -> f64 {
    r * 180.0 / std::f64::consts::PI
}

/// Unit view direction for CS angles.
pub fn forward(pitch: f64, yaw: f64) -> DVec3 {
    DVec3::new(rad(pitch).cos() * rad(yaw).cos(), rad(pitch).cos() * rad(yaw).sin(), -rad(pitch).sin())
}

/// CS angles (pitch, yaw) that look from one point at another.
pub fn angles_to(from: DVec3, to: DVec3) -> (f64, f64) {
    let d = to - from;
    (-deg(d.z.atan2(hypot2(d.x, d.y))), deg(d.y.atan2(d.x)))
}

/// Where the eye is for someone standing at `feet`; a partial duck lowers it in proportion.
pub fn eye_at(feet: DVec3, duck: Option<f64>) -> DVec3 {
    DVec3::new(feet.x, feet.y, feet.z + EYE_STANDING - (EYE_STANDING - EYE_CROUCHED) * duck.unwrap_or(0.0).clamp(0.0, 1.0))
}

/// Math.hypot of three, as camera.ts's `distance` computes it.
pub fn distance(a: DVec3, b: DVec3) -> f64 {
    hypot3(a.x - b.x, a.y - b.y, a.z - b.z)
}

/// `Math.hypot(x, y, z)`, as JavaScript engines compute it, so distances match the web view's to the last bit.
pub fn hypot3(x: f64, y: f64, z: f64) -> f64 {
    hypot(&[x, y, z])
}

/// `Math.hypot(x, y)`.
pub fn hypot2(x: f64, y: f64) -> f64 {
    hypot(&[x, y])
}

/// V8's and JavaScriptCore's `Math.hypot`: a Kahan-compensated sum of each magnitude over the largest, squared, times
/// the largest (libm's `hypot` can differ in the last bit).
fn hypot(values: &[f64]) -> f64 {
    let max = values.iter().map(|v| v.abs()).fold(0.0f64, f64::max);
    if max == 0.0 || !max.is_finite() {
        return max;
    }
    let mut sum = 0.0f64;
    let mut compensation = 0.0f64;
    for v in values {
        let r = v.abs() / max;
        let summand = r * r - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}

/// A camera: an eye with CS angles and a vertical field of view in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub eye: DVec3,
    pub pitch: f64,
    pub yaw: f64,
    pub fov: f64,
}

/// An opponent's sample while the effect was up (`scene_viewers`).
#[derive(Debug, Clone, PartialEq)]
pub struct Viewer {
    pub tick: i32,
    pub player: Option<String>,
    pub feet: DVec3,
    pub pitch: Option<f64>,
    pub yaw: Option<f64>,
}

impl From<&nades_data::Viewer> for Viewer {
    fn from(v: &nades_data::Viewer) -> Self {
        Self { tick: v.tick, player: v.player.clone(), feet: DVec3::new(v.pos[0].into(), v.pos[1].into(), v.pos[2].into()), pitch: v.pitch.map(f64::from), yaw: v.yaw.map(f64::from) }
    }
}

/// Whose view a result shot is.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Opponent { player: Option<String>, tick: i32 },
    /// The enemy a flash blinded longest, as it went off.
    Victim { seconds: f64 },
    /// A fallback looking at the target.
    Overview,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShotCamera {
    pub eye: DVec3,
    pub pitch: f64,
    pub yaw: f64,
    /// The share of the effect's points the map leaves in sight from the eye, when there was a collider to ask.
    pub seen: Option<f64>,
    pub source: Source,
}

impl ShotCamera {
    pub fn looking(eye: DVec3, at: DVec3, source: Source) -> Self {
        let (pitch, yaw) = angles_to(eye, at);
        Self { eye, pitch, yaw, seen: None, source }
    }
}

/// Distances from the target that frame a result well. Across the web view's smokes, 89% had an opponent 250–1500
/// units away looking within 20° of the cloud while it was up; the median was about 800. A fire is small and on the
/// ground, so it is framed closer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Framing {
    pub near: f64,
    pub far: f64,
    pub ideal: f64,
    /// How far the eye must stay from the effect's nearest part, so the shot shows it in its surroundings.
    pub clear: f64,
}

pub const SMOKE_FRAMING: Framing = Framing { near: 250.0, far: 1200.0, ideal: 500.0, clear: 150.0 };
pub const FIRE_FRAMING: Framing = Framing { near: 200.0, far: 800.0, ideal: 400.0, clear: 80.0 };

/// A view must see at least this share of the target's points; a doorway's sliver of a smoke is not a result.
pub const MIN_SEEN: f64 = 0.5;
/// Seeing all of the target is worth as much as looking straight at it.
const SEEN_WEIGHT: f64 = 2.0;

/// The view an opponent had of the target, preferring one who was looking toward it, from beyond it (the side the
/// thrower wanted to block), at a comfortable distance, seeing as much of it as possible; `seen` answers which share of
/// the target's points the map leaves in sight from an eye, and `gap` how far the eye is from the effect's nearest
/// part. When nobody fits, an overview from the thrower's side.
pub fn pick_result_camera(viewers: &[Viewer], target: DVec3, from: DVec3, mut seen: impl FnMut(DVec3) -> f64, framing: Framing, gap: impl Fn(DVec3) -> f64) -> ShotCamera {
    struct Candidate<'a> {
        score: f64,
        viewer: &'a Viewer,
        eye: DVec3,
    }
    let mut candidates = Vec::new();
    for v in viewers {
        let eye = eye_at(v.feet, Some(0.0));
        let d = distance(eye, target);
        if d < framing.near || d > framing.far {
            continue;
        }
        let (tp, ty) = angles_to(eye, target);
        // 1 when they looked straight at it, 0 when they looked 90° or more away
        let looked = match (v.pitch, v.yaw) {
            (Some(p), Some(y)) => forward(p, y).dot(forward(tp, ty)).max(0.0),
            _ => 0.5,
        };
        let beyond = if (eye.x - target.x) * (from.x - target.x) + (eye.y - target.y) * (from.y - target.y) < 0.0 { 0.5 } else { 0.0 };
        candidates.push(Candidate { score: looked * 2.0 + beyond - (d - framing.ideal).abs() / 500.0, viewer: v, eye });
    }
    // line of sight is the costly part: visit the best first, and stop once seeing everything could not win
    candidates.sort_by(|a, b| (b.score - a.score).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut best: Option<(f64, ShotCamera)> = None;
    for c in &candidates {
        if best.as_ref().is_some_and(|(score, _)| c.score + SEEN_WEIGHT <= *score) {
            break;
        }
        if gap(c.eye) < framing.clear {
            continue;
        }
        let share = seen(c.eye);
        if share < MIN_SEEN {
            continue;
        }
        let score = c.score + share * SEEN_WEIGHT;
        if best.as_ref().is_some_and(|(b, _)| score <= *b) {
            continue;
        }
        best = Some((score, ShotCamera::looking(c.eye, target, Source::Opponent { player: c.viewer.player.clone(), tick: c.viewer.tick })));
    }
    if let Some((_, camera)) = best {
        return camera;
    }
    let back = (from.x - target.x, from.y - target.y);
    let len = hypot2(back.0, back.1);
    let len = if len == 0.0 { 1.0 } else { len };
    let eye = DVec3::new(target.x + back.0 / len * 550.0, target.y + back.1 / len * 550.0, target.z + 420.0);
    ShotCamera::looking(eye, target, Source::Overview)
}

const SPREAD: [[f64; 3]; 14] = [
    [1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, -1.0],
    [1.0, 1.0, 1.0],
    [1.0, 1.0, -1.0],
    [1.0, -1.0, 1.0],
    [1.0, -1.0, -1.0],
    [-1.0, 1.0, 1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [-1.0, -1.0, -1.0],
];

/// Points that stand for a whole effect when testing what a view sees of it: the centre, plus the effect's outermost
/// point in each of 14 directions, pulled `pull` of the way back to the centre so they sit in its body rather than on
/// its ragged edge.
pub fn spread_points(points: &[DVec3], centre: DVec3, pull: f64) -> Vec<DVec3> {
    let mut picked: Vec<usize> = Vec::new();
    for dir in SPREAD {
        let mut far = None;
        let mut reach = 0.0;
        for (k, p) in points.iter().enumerate() {
            let r = dir[0] * (p.x - centre.x) + dir[1] * (p.y - centre.y) + dir[2] * (p.z - centre.z);
            if r > reach {
                reach = r;
                far = Some(k);
            }
        }
        if let Some(k) = far
            && !picked.contains(&k)
        {
            picked.push(k);
        }
    }
    let lerp = |p: DVec3| DVec3::new(p.x + (centre.x - p.x) * pull, p.y + (centre.y - p.y) * pull, p.z + (centre.z - p.z) * pull);
    std::iter::once(centre).chain(picked.into_iter().map(|k| lerp(points[k]))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The numbers camera.ts's own tests (web/app/test/camera.test.ts) hold.
    #[test]
    fn cs2s_view_and_angles() {
        assert!((vertical_fov() - 73.739_795_291_688).abs() < 1e-9);
        let f = forward(0.0, 90.0);
        assert!(close(f.x, 0.0) && close(f.y, 1.0) && close(f.z, 0.0));
        // positive pitch looks down
        assert!(forward(30.0, 0.0).z < 0.0);
        let (p, y) = angles_to(DVec3::ZERO, DVec3::new(100.0, 100.0, -100.0));
        assert!(close(y, 45.0) && (p - 35.264_389_682_754_654).abs() < 1e-9);
        assert_eq!(eye_at(DVec3::new(1.0, 2.0, 3.0), None).z, 67.0);
        assert_eq!(eye_at(DVec3::new(1.0, 2.0, 3.0), Some(1.0)).z, 49.0);
        assert_eq!(eye_at(DVec3::new(1.0, 2.0, 3.0), Some(0.5)).z, 58.0);
        assert_eq!(hypot3(3.0, 4.0, 12.0), 13.0);
    }

    #[test]
    fn an_opponent_looking_at_the_target_from_beyond_it_is_chosen() {
        let target = DVec3::new(0.0, 0.0, 0.0);
        let thrower = DVec3::new(-1000.0, 0.0, 0.0);
        let viewer = |x: f64, y: f64, yaw: f64, tick: i32| Viewer { tick, player: Some(format!("p{tick}")), feet: DVec3::new(x, y, -64.0), pitch: Some(0.0), yaw: Some(yaw) };
        // one beyond the target looking back at it, one on the thrower's side looking away, one too far
        let viewers = [viewer(-500.0, 0.0, 180.0, 1), viewer(500.0, 0.0, 180.0, 2), viewer(2000.0, 0.0, 180.0, 3)];
        let shot = pick_result_camera(&viewers, target, thrower, |_| 1.0, SMOKE_FRAMING, |_| f64::INFINITY);
        assert_eq!(shot.source, Source::Opponent { player: Some("p2".into()), tick: 2 });
        assert!(close(shot.eye.x, 500.0) && close(shot.eye.z, 0.0));
        assert!(close(shot.yaw.abs(), 180.0));
        // nobody sees it: an overview from the thrower's side
        let shot = pick_result_camera(&viewers, target, thrower, |_| 0.2, SMOKE_FRAMING, |_| f64::INFINITY);
        assert_eq!(shot.source, Source::Overview);
        assert!(close(shot.eye.x, -550.0) && close(shot.eye.z, 420.0));
    }

    #[test]
    fn spread_points_take_the_centre_and_the_outermost_points_pulled_in() {
        let points = [DVec3::new(10.0, 0.0, 0.0), DVec3::new(-10.0, 0.0, 0.0), DVec3::new(0.0, 0.0, 20.0)];
        let spread = spread_points(&points, DVec3::ZERO, 0.5);
        assert_eq!(spread[0], DVec3::ZERO);
        assert_eq!(spread[1..], [DVec3::new(5.0, 0.0, 0.0), DVec3::new(-5.0, 0.0, 0.0), DVec3::new(0.0, 0.0, 10.0)]);
        assert_eq!(spread_points(&[], DVec3::ONE, 0.35), [DVec3::ONE]);
    }
}
