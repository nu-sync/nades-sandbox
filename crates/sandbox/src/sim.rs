//! The simulator: a scene's throw through nades-physics (its release, its flight with the bounces, where and when it
//! goes off) and nades-effects (the smoke's grid, filled by the scene's `fill_cloud`; the fire's flames, drawn by the
//! scene's `build_fire`), made into a stage the renderer draws. A throw is simulated once per spot, angles and
//! technique; its start time only places it on the scene's clock.
//!
//! What a frame of the scene draws is put together here too ([`show`]), for the window, `--render` and videos alike:
//! every throw at its own time (seconds after its detonation), the arcs, the smokes and the strongest light.

use std::sync::Arc;
use std::time::Instant;

use glam::DVec3;
use nades_control::scene::{Click, Grenade, Movement, SceneThrow};
use nades_render::Gpu;
use nades_render::map::{FrameEffects, Map, PointLight};
use nades_render::smoke::SmokeFrame;
use nades_render::throw::Arc as ArcMesh;
use nades_scene::camera::{ShotCamera, View as CsView, angles_to, vertical_fov};
use nades_scene::collider::Collider;
use nades_scene::smoke::{fill_cloud, smoke_volume, voxel_light};
use nades_scene::stage::{Simulated, Stage, build_stage, nade_color};

pub fn grenade(g: Grenade) -> nades_physics::Grenade {
    match g {
        Grenade::Smoke => nades_physics::Grenade::Smoke,
        Grenade::Flash => nades_physics::Grenade::Flash,
        Grenade::He => nades_physics::Grenade::He,
        Grenade::Molotov => nades_physics::Grenade::Molotov,
        Grenade::Incendiary => nades_physics::Grenade::Incendiary,
        Grenade::Decoy => nades_physics::Grenade::Decoy,
    }
}

pub fn drawn(g: Grenade) -> nades_scene::stage::Grenade {
    match g {
        Grenade::Smoke => nades_scene::stage::Grenade::Smoke,
        Grenade::Flash => nades_scene::stage::Grenade::Flash,
        Grenade::He => nades_scene::stage::Grenade::He,
        Grenade::Molotov => nades_scene::stage::Grenade::Molotov,
        Grenade::Incendiary => nades_scene::stage::Grenade::Incendiary,
        Grenade::Decoy => nades_scene::stage::Grenade::Decoy,
    }
}

/// A scene's throw as the physics takes it.
pub fn physics_throw(t: &SceneThrow) -> nades_physics::Throw {
    nades_physics::Throw {
        grenade: grenade(t.grenade),
        feet: t.feet,
        pitch: t.pitch,
        yaw: t.yaw,
        crouch: t.crouch,
        movement: match t.movement {
            Movement::Stand => nades_physics::Movement::Stand,
            Movement::Walk => nades_physics::Movement::Walk,
            Movement::Run => nades_physics::Movement::Run,
            Movement::Jumpthrow => nades_physics::Movement::Jumpthrow,
            Movement::RunJumpthrow => nades_physics::Movement::RunJumpthrow,
        },
        click: match t.click {
            Click::Left => nades_physics::Click::Left,
            Click::Middle => nades_physics::Click::Middle,
            Click::Right => nades_physics::Click::Right,
        },
    }
}

/// What a throw's simulation depends on: everything but its start and label.
pub fn key(t: &SceneThrow) -> String {
    let t = SceneThrow { start: 0.0, label: None, ..t.clone() };
    format!("{:?}", (t.grenade, t.feet.map(f64::to_bits), t.pitch.to_bits(), t.yaw.to_bits(), t.crouch, t.movement, t.click))
}

/// The thrower's eye as they set up the throw: their feet, 64 up standing, 46 crouched.
pub fn thrower_eye(t: &SceneThrow) -> CsView {
    let eye = nades_physics::eye(&physics_throw(t));
    CsView { eye, pitch: t.pitch, yaw: t.yaw, fov: vertical_fov() }
}

/// The flight alone, for the throw tool's preview arc: where it flies and where it goes off.
pub fn preview(t: &SceneThrow, collider: Option<&Collider>) -> nades_physics::Path {
    let throw = physics_throw(t);
    nades_physics::fly(&nades_physics::release(&throw), throw.grenade, collider)
}

/// One throw simulated: its flight, its stage, its smoke's lit volume, the result shot over what it left.
pub struct Built {
    pub throw: SceneThrow,
    pub path: nades_physics::Path,
    pub stage: Stage,
    /// 32³ RGBA texels, lit against the collider.
    pub volume: Option<Vec<u8>>,
    pub result: ShotCamera,
    /// A number for the throw's smoke on the GPU.
    pub id: u64,
    /// How long the simulation took, flight and effect (ms).
    pub ms: f64,
}

impl Built {
    /// Seconds on the scene's clock when it goes off, for a throw starting at `start`.
    pub fn detonates_at(&self, start: f64) -> f64 {
        start + self.path.detonation.t
    }

    /// Seconds after its detonation at `t` on the scene's clock.
    pub fn local(&self, start: f64, t: f64) -> f64 {
        t - self.detonates_at(start)
    }

    /// When what it leaves is gone, on the scene's clock.
    pub fn ends_at(&self, start: f64) -> f64 {
        self.detonates_at(start) + self.stage.duration
    }

    /// The arc of its flight.
    pub fn arc(&self, strong: bool) -> ArcMesh {
        let flight = self.stage.flight.as_ref();
        ArcMesh { points: flight.map(|f| f.points.clone()).unwrap_or_default(), marks: flight.map(|f| f.marks()).unwrap_or_default(), color: nade_color(self.stage.grenade), strong }
    }
}

/// Simulates `t` against the map's `collider` (without one the grenade flies through walls and leaves no smoke or
/// fire), its smoke lit from `toward_sun`.
pub fn simulate(t: &SceneThrow, collider: Option<&Collider>, toward_sun: DVec3) -> Built {
    use std::hash::{Hash, Hasher};
    let started = Instant::now();
    let throw = physics_throw(t);
    let release = nades_physics::release(&throw);
    let path = nades_physics::fly(&release, throw.grenade, collider);
    let det = path.detonation;
    let cloud = match (t.grenade, collider) {
        (Grenade::Smoke, Some(c)) => fill_cloud(&nades_effects::smoke_grid(det.pos, c), det.pos),
        _ => None,
    };
    let flames = match (t.grenade, collider) {
        (Grenade::Molotov | Grenade::Incendiary, Some(c)) => nades_effects::fire_flames(det.pos, det.normal, c),
        _ => Vec::new(),
    };
    let stage = build_stage(Simulated { grenade: drawn(t.grenade), thrower: thrower_eye(t), flight: path.points.clone(), release: release.t, detonation: (det.t, det.pos), cloud, flames });
    let volume = stage.cloud.as_ref().map(|cloud| {
        let light = collider.map(|c| voxel_light(cloud, toward_sun, c));
        smoke_volume(cloud, light.as_ref())
    });
    let result = stage.result_camera(collider);
    let mut hasher = std::hash::DefaultHasher::new();
    key(t).hash(&mut hasher);
    Built { throw: t.clone(), path, stage, volume, result, id: hasher.finish(), ms: started.elapsed().as_secs_f64() * 1e3 }
}

/// The views of a throw as CS2 cameras, at `t` on the scene's clock.
pub mod views {
    use super::*;
    use crate::views::Orbit;
    use nades_control::scene::ViewName;

    /// The result shot over what the throw left.
    pub fn result(b: &Built) -> CsView {
        CsView { eye: b.result.eye, pitch: b.result.pitch, yaw: b.result.yaw, fov: vertical_fov() }
    }

    /// Behind the grenade while it flies, looking where it goes; once it has gone off, from where it was a moment
    /// before, over what it left. With the collider, the camera stays on the grenade's side of any wall.
    pub fn follow(b: &Built, local: f64, collider: Option<&Collider>) -> CsView {
        let flight = b.stage.flight.as_ref();
        let points = &b.path.points;
        let at = |tau: f64| -> DVec3 {
            let when = b.path.detonation.t + tau;
            let i = points.partition_point(|(s, _)| *s <= when).clamp(1, points.len().max(2) - 1);
            match (points.get(i - 1), points.get(i)) {
                (Some(&(s0, p0)), Some(&(s1, p1))) if s1 > s0 => p0.lerp(p1, ((when - s0) / (s1 - s0)).clamp(0.0, 1.0)),
                (Some(&(_, p)), _) => p,
                _ => b.stage.detonation,
            }
        };
        if flight.is_none() || points.len() < 2 {
            return result(b);
        }
        // the grenade a tenth of a second on, and where it was a third of a second ago: the camera trails it
        let local = local.max(b.stage.release_time);
        let (now, ahead) = (at(local.min(0.0)), at((local + 0.1).min(0.0)));
        let behind = at((local - 0.35).max(b.stage.release_time));
        let mut back = now - behind;
        back.z = back.z.max(-40.0);
        let back = if back.length() < 1.0 { (now - b.stage.thrower.eye).normalize_or(DVec3::X) } else { back.normalize() };
        let mut eye = now - back * 140.0 + DVec3::Z * 30.0;
        if let Some(c) = collider {
            let away = eye - now;
            if let Some(d) = c.hit_distance(now, away.normalize_or_zero(), away.length()) {
                eye = now + away.normalize_or_zero() * (d - 8.0).max(4.0);
            }
        }
        let look = if local < 0.0 { ahead + (ahead - now) * 1.5 } else { b.stage.focus };
        let (pitch, yaw) = angles_to(eye, look);
        CsView { eye, pitch, yaw, fov: vertical_fov() }
    }

    /// `view` of `b` at its local time, `orbit` once it moves.
    pub fn camera(view: ViewName, b: &Built, local: f64, orbit: Option<&Orbit>, collider: Option<&Collider>) -> Option<CsView> {
        match view {
            ViewName::Free => None,
            ViewName::Thrower => Some(b.stage.thrower),
            ViewName::Follow => Some(follow(b, local, collider)),
            ViewName::Orbit => Some(orbit.map_or_else(|| result(b), Orbit::view)),
        }
    }

    /// The orbit around what `b` left, opening on its result shot.
    pub fn orbit(b: &Built) -> Orbit {
        Orbit::new(b.stage.focus, result(b))
    }
}

/// A throw on the scene's clock: what it built and when it starts.
pub type Placed = (Arc<Built>, f64);

/// What `throws` show at `t` on the scene's clock, readied on `map` for a frame from `eye` into targets of `size`:
/// each grenade in flight, its burst, its fire, its smoke, and the strongest light on the map.
pub fn show(gpu: &Gpu, map: &mut Map, throws: &[Placed], t: f64, eye: &nades_render::View, size: (u32, u32), accumulated: u32) -> FrameEffects {
    let moment: Vec<(&Stage, f64)> = throws.iter().map(|(b, start)| (&b.stage, b.local(*start, t))).collect();
    map.throw.set_moment(&gpu.device, &gpu.queue, &moment, eye, size);
    let clouds: Vec<(u64, &nades_scene::smoke::Cloud, &[u8], Option<SmokeFrame>)> = throws
        .iter()
        .filter_map(|(b, start)| {
            let (cloud, volume) = (b.stage.cloud.as_ref()?, b.volume.as_deref()?);
            let local = b.local(*start, t);
            let frame = (local > 0.0 && local < b.stage.duration).then(|| SmokeFrame { seconds: local as f32, fade: cloud.fade_at(local) as f32, accumulated });
            Some((b.id, cloud, volume, frame))
        })
        .collect();
    map.composite.set_clouds(&gpu.device, &gpu.queue, &clouds);
    let light = moment
        .iter()
        .filter_map(|(stage, local)| stage.light_at(*local))
        .max_by(|a, b| a.intensity.total_cmp(&b.intensity))
        .map(|l| PointLight { at: glam::Vec3::new(l.at.x as f32, l.at.y as f32, l.at.z as f32), color: l.color.map(|c| c as f32), intensity: l.intensity as f32, distance: l.distance as f32, decay: l.decay as f32 });
    FrameEffects { light, accumulated }
}

/// Whether a smoke shows at `t`: a still picture of it averages jittered frames.
pub fn smoking(throws: &[Placed], t: f64) -> bool {
    throws.iter().any(|(b, start)| b.stage.cloud.is_some() && (0.0..b.stage.duration).contains(&b.local(*start, t)))
}

/// What shows at `t`, each once: flight, smoke, fire, flash, he.
pub fn active(throws: &[Placed], t: f64) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (b, start) in throws {
        let local = b.local(*start, t);
        let s = &b.stage;
        let on = |name: &str, out: &mut Vec<String>| {
            if !out.iter().any(|o| o == name) {
                out.push(name.into());
            }
        };
        if local < 0.0 && t >= start + b.path.release.t {
            on("flight", &mut out);
        }
        if s.cloud.is_some() && (0.0..s.duration).contains(&local) {
            on("smoke", &mut out);
        }
        if s.fire.as_ref().is_some_and(|f| (0.0..f.lasts()).contains(&local)) {
            on("fire", &mut out);
        }
        if let Some(burst) = s.burst
            && burst.on(local) > 0.0
        {
            on(if burst.flash { "flash" } else { "he" }, &mut out);
        }
    }
    out
}

/// Where the scene's clock runs to: the last effect's end (at least a second).
pub fn end(throws: &[Placed]) -> f64 {
    throws.iter().map(|(b, start)| b.ends_at(*start)).fold(1.0, f64::max)
}

/// The point on the map under the cursor: a ray from `camera` through `(x, y)` of a view `size` points across (0..1
/// each way from its top left), to the first surface the collider has.
pub fn pick(collider: &Collider, camera: &CsView, (x, y): (f64, f64), aspect: f64) -> Option<DVec3> {
    let forward = nades_scene::camera::forward(camera.pitch, camera.yaw);
    let right = forward.cross(DVec3::Z).normalize_or(DVec3::Y);
    let up = right.cross(forward);
    let half = (camera.fov.to_radians() / 2.0).tan();
    let dir = (forward + right * (2.0 * x - 1.0) * half * aspect + up * (1.0 - 2.0 * y) * half).normalize();
    collider.hit_distance(camera.eye, dir, 20_000.0).map(|d| camera.eye + dir * d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_throw_is_simulated_on_its_own_clock_and_placed_on_the_scenes() {
        let smoke = SceneThrow::new(Grenade::He, [0.0, 0.0, 0.0], -10.0, 0.0);
        let b = simulate(&smoke, None, DVec3::Z);
        let det = b.path.detonation.t;
        assert!(det > 0.0 && b.stage.flight.is_some());
        assert_eq!(b.detonates_at(2.0), 2.0 + det);
        assert!((b.local(2.0, 2.0 + det + 0.1) - 0.1).abs() < 1e-9);
        // the same spot, angles and technique are one simulation, whatever the start or label
        let later = SceneThrow { start: 5.0, label: Some("x".into()), ..smoke.clone() };
        assert_eq!(key(&later), key(&smoke));
        assert_ne!(key(&SceneThrow { crouch: true, ..smoke.clone() }), key(&smoke));
        let placed = vec![(Arc::new(b), 2.0)];
        assert_eq!(active(&placed, 2.0 + det + 0.01), ["he"]);
        assert!(active(&placed, 2.0 + det - 0.2).contains(&"flight".to_string()));
        assert!(active(&placed, 0.5).is_empty(), "nothing before it is thrown");
        assert!(end(&placed) > 2.0 + det);
        assert!(!smoking(&placed, 3.0));
    }

    #[test]
    fn the_follow_view_trails_the_grenade_and_ends_over_what_it_left() {
        let b = simulate(&SceneThrow::new(Grenade::Flash, [0.0, 0.0, 0.0], -20.0, 90.0), None, DVec3::Z);
        let flying = views::follow(&b, -0.5, None);
        // behind it, so looking roughly the way it was thrown (+y)
        assert!(nades_scene::camera::forward(flying.pitch, flying.yaw).y > 0.5, "{flying:?}");
        let after = views::follow(&b, 1.0, None);
        let towards = (b.stage.focus - after.eye).normalize();
        assert!(nades_scene::camera::forward(after.pitch, after.yaw).dot(towards) > 0.99);
    }

    #[test]
    fn a_ray_through_the_middle_of_the_view_hits_what_the_camera_faces() {
        let floor = Collider::new(vec![[[-1000.0, -1000.0, 0.0], [1000.0, -1000.0, 0.0], [0.0, 1000.0, 0.0]]]);
        let camera = CsView { eye: DVec3::new(0.0, 0.0, 64.0), pitch: 45.0, yaw: 0.0, fov: vertical_fov() };
        let at = pick(&floor, &camera, (0.5, 0.5), 16.0 / 9.0).unwrap();
        assert!(at.distance(DVec3::new(64.0, 0.0, 0.0)) < 1e-6, "{at}");
        // further up the screen is further away
        assert!(pick(&floor, &camera, (0.5, 0.3), 16.0 / 9.0).unwrap().x > 64.0);
    }
}
