//! CS2 cameras (Hammer units, z up, CS angles) as renderer cameras, and the orbit around a throw's result.
//!
//! The orbit opens on a view it is given (the thrower's at release, or the result shot); the first drag turns it onto
//! the effect, and from then on it circles it as three.js's OrbitControls do (show-me-the-nades' `render/engine.ts`
//! `setOrbit`): from straight down to a little past level, 80 to 5,000 units out.

use glam::DVec3;
use nades_render::View;
use nades_scene::camera::{View as CsView, angles_to, vertical_fov};
#[cfg(test)]
use nades_scene::camera::forward;

/// A renderer camera from a CS2 eye (Hammer units), angles and vertical field of view (degrees).
pub fn cs_view(eye: DVec3, pitch: f64, yaw: f64, fov: f64) -> View {
    View::from_cs([eye.x as f32, eye.y as f32, eye.z as f32], pitch as f32, yaw as f32, fov as f32)
}

pub fn of(v: &CsView) -> View {
    cs_view(v.eye, v.pitch, v.yaw, v.fov)
}

/// The free orbit: around `target`, at a distance, azimuth (from +x toward +y) and polar angle (from straight up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    pub target: DVec3,
    /// Before the first drag: the eye and angles it opened on.
    pub opened: Option<CsView>,
    pub distance: f64,
    pub azimuth: f64,
    pub polar: f64,
}

/// OrbitControls' bounds as the web view sets them.
const MIN_DISTANCE: f64 = 80.0;
const MAX_DISTANCE: f64 = 5000.0;
const MAX_POLAR: f64 = std::f64::consts::PI * 0.55;

impl Orbit {
    /// Opening on `from` (the thrower's view), circling `target` once dragged.
    pub fn new(target: DVec3, from: CsView) -> Self {
        let mut orbit = Self { target, opened: Some(from), distance: 0.0, azimuth: 0.0, polar: 0.0 };
        orbit.take(from.eye);
        orbit.opened = Some(from);
        orbit
    }

    /// The spherical coordinates of `eye` about the target, within the bounds.
    fn take(&mut self, eye: DVec3) {
        let d = eye - self.target;
        self.distance = d.length().clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.azimuth = d.y.atan2(d.x);
        self.polar = (d.z / d.length().max(1e-9)).clamp(-1.0, 1.0).acos().clamp(1e-3, MAX_POLAR);
        self.opened = None;
    }

    pub fn eye(&self) -> DVec3 {
        self.target + DVec3::new(self.polar.sin() * self.azimuth.cos(), self.polar.sin() * self.azimuth.sin(), self.polar.cos()) * self.distance
    }

    /// Turns by a drag of (dx, dy) points across a view `height` points tall (a full height is a full turn).
    pub fn drag(&mut self, dx: f64, dy: f64, height: f64) {
        if let Some(opened) = self.opened {
            self.take(opened.eye);
        }
        let turn = 2.0 * std::f64::consts::PI / height.max(1.0);
        self.azimuth -= dx * turn;
        self.polar = (self.polar - dy * turn).clamp(1e-3, MAX_POLAR);
    }

    /// Nearer for a positive scroll, further for a negative one.
    pub fn zoom(&mut self, steps: f64) {
        if let Some(opened) = self.opened {
            self.take(opened.eye);
        }
        self.distance = (self.distance * 0.95f64.powf(steps)).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    pub fn view(&self) -> CsView {
        if let Some(opened) = self.opened {
            return opened;
        }
        let eye = self.eye();
        let (pitch, yaw) = angles_to(eye, self.target);
        CsView { eye, pitch, yaw, fov: vertical_fov() }
    }
}

/// Where a CS2 camera looks.
#[cfg(test)]
pub fn looking(v: &CsView) -> DVec3 {
    forward(v.pitch, v.yaw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: DVec3, b: DVec3) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn the_orbit_opens_on_the_lineup_and_turns_onto_the_effect() {
        let lineup = CsView { eye: DVec3::new(0.0, 0.0, 64.0), pitch: 0.0, yaw: 0.0, fov: vertical_fov() };
        let mut orbit = Orbit::new(DVec3::new(500.0, 0.0, 64.0), lineup);
        assert_eq!(orbit.view(), lineup);
        orbit.drag(0.0, 0.0, 900.0);
        // after a drag it looks at the target from where it stood
        let v = orbit.view();
        assert!(close(v.eye, DVec3::new(0.0, 0.0, 64.0)));
        assert!(close(looking(&v), DVec3::X));
        // a full height's drag is a full turn
        orbit.drag(900.0, 0.0, 900.0);
        assert!(close(orbit.eye(), DVec3::new(0.0, 0.0, 64.0)));
        orbit.drag(225.0, 0.0, 900.0);
        assert!(close(orbit.eye(), DVec3::new(500.0, 500.0, 64.0)));
        // never under the floor's level by much, never nearer than 80
        orbit.drag(0.0, -10_000.0, 900.0);
        assert!(orbit.eye().z >= 64.0 - 500.0 * (0.05 * std::f64::consts::PI).sin() - 1e-6);
        orbit.zoom(1000.0);
        assert!((orbit.distance - MIN_DISTANCE).abs() < 1e-9);
    }

    #[test]
    fn a_cs_view_looks_where_its_angles_say() {
        let v = cs_view(DVec3::new(100.0, 200.0, 64.0), 30.0, 90.0, 73.74);
        let expected = nades_render::View::from_setpos([100.0, 200.0, 0.0], 30.0, 90.0, 64.0);
        assert!((v.eye - expected.eye).length() < 1e-5);
        assert!((v.forward() - expected.forward()).length() < 1e-5);
    }
}
