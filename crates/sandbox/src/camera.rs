//! The camera: `bench/camera.json` at start-up (glTF space: metres, y up), then a fly camera: WASD to move (Shift
//! faster, E and Q up and down), the right mouse button held to look.

use glam::{Quat, Vec3};
use nades_control::CameraPose;
use nades_render::View;
use winit::keyboard::KeyCode;

pub struct Camera {
    pub position: Vec3,
    rotation: Quat,
    fov_degrees: f32,
    angles: [f32; 2],
    near: f32,
    far: f32,
    eye_height: f32,
}

impl Camera {
    /// The bench camera, which every screenshot and timing run starts from.
    pub fn bench() -> Self {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../../../bench/camera.json"))
                .expect("bench/camera.json is JSON");
        let number =
            |v: &serde_json::Value| v.as_f64().expect("bench/camera.json holds numbers") as f32;
        let list = |key: &str| -> Vec<f32> {
            json[key]
                .as_array()
                .map(|a| a.iter().map(number).collect())
                .unwrap_or_default()
        };
        let (p, q) = (list("position"), list("quaternion_xyzw"));
        let rotation = Quat::from_xyzw(q[0], q[1], q[2], q[3]).normalize();
        let forward = rotation * Vec3::NEG_Z;
        let source_forward = Vec3::new(forward.z, forward.x, forward.y);
        Self {
            position: Vec3::new(p[0], p[1], p[2]),
            rotation,
            angles: [
                (-source_forward.z)
                    .atan2(source_forward.truncate().length())
                    .to_degrees(),
                source_forward.y.atan2(source_forward.x).to_degrees(),
            ],
            fov_degrees: number(&json["vertical_fov_degrees"]),
            near: number(&json["near"]),
            far: number(&json["far"]),
            eye_height: 64.0,
        }
    }

    /// What the renderer draws from.
    pub fn view(&self) -> View {
        View {
            eye: self.position,
            rotation: self.rotation,
            fov_y: self.fov_degrees.to_radians(),
            near: self.near,
            far: self.far,
        }
    }

    pub fn set_pose(&mut self, pose: &CameraPose) {
        self.angles = [pose.pitch_degrees, normalize_yaw(pose.yaw_degrees)];
        let view = View::from_setpos(
            pose.position,
            self.angles[0],
            self.angles[1],
            pose.eye_height_units,
        );
        self.position = view.eye;
        self.rotation = view.rotation;
        self.fov_degrees = pose.vertical_fov_degrees;
        self.near = view.near;
        self.far = view.far;
        self.eye_height = pose.eye_height_units;
    }

    pub fn pose(&self) -> CameraPose {
        let source =
            Vec3::new(self.position.z, self.position.x, self.position.y) / nades_render::view::INCH;
        CameraPose {
            position: [source.x, source.y, source.z - self.eye_height],
            pitch_degrees: self.angles[0],
            yaw_degrees: self.angles[1],
            eye_height_units: self.eye_height,
            vertical_fov_degrees: self.fov_degrees,
        }
    }
}

fn normalize_yaw(yaw: f32) -> f32 {
    if (-180.0..180.0).contains(&yaw) {
        yaw
    } else {
        (yaw + 180.0).rem_euclid(360.0) - 180.0
    }
}

/// What the fly camera's controls hold.
#[derive(Default)]
pub struct Fly {
    /// forward, back, left, right, up, down
    held: [bool; 6],
    fast: bool,
    pub looking: bool,
}

/// Metres per second (CS2 runs at 250 units a second, 6.35 m), and radians per point of mouse travel.
const SPEED: f32 = 6.35;
const LOOK: f32 = 0.0025;

impl Fly {
    pub fn key(&mut self, code: KeyCode, down: bool) {
        let at = match code {
            KeyCode::KeyW => 0,
            KeyCode::KeyS => 1,
            KeyCode::KeyA => 2,
            KeyCode::KeyD => 3,
            KeyCode::KeyE => 4,
            KeyCode::KeyQ => 5,
            KeyCode::ShiftLeft | KeyCode::ShiftRight => return self.fast = down,
            _ => return,
        };
        self.held[at] = down;
    }

    /// Whether the camera moves on its own between inputs, so frames must keep coming.
    pub fn moving(&self) -> bool {
        self.held.iter().any(|&h| h)
    }

    /// Turns the camera by a mouse movement; looking around drops any roll.
    pub fn look(&self, camera: &mut Camera, dx: f64, dy: f64) {
        camera.angles[0] = (camera.angles[0] + (dy as f32 * LOOK).to_degrees())
            .clamp(-1.55_f32.to_degrees(), 1.55_f32.to_degrees());
        camera.angles[1] = normalize_yaw(camera.angles[1] - (dx as f32 * LOOK).to_degrees());
        camera.rotation =
            View::from_setpos([0.0; 3], camera.angles[0], camera.angles[1], 0.0).rotation;
    }

    /// Moves the camera for `dt` seconds of the keys held.
    pub fn step(&self, camera: &mut Camera, dt: f32) {
        let axes = [
            camera.rotation * Vec3::NEG_Z,
            camera.rotation * Vec3::Z,
            camera.rotation * Vec3::NEG_X,
            camera.rotation * Vec3::X,
            Vec3::Y,
            Vec3::NEG_Y,
        ];
        let direction: Vec3 = axes
            .iter()
            .zip(self.held)
            .filter(|(_, held)| *held)
            .map(|(axis, _)| *axis)
            .sum();
        camera.position +=
            direction.normalize_or_zero() * SPEED * if self.fast { 4.0 } else { 1.0 } * dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_console_pose_controls_the_live_camera_and_reads_back() {
        let pose = CameraPose {
            position: [100.0, 200.0, 10.0],
            pitch_degrees: 30.0,
            yaw_degrees: 90.0,
            eye_height_units: 46.0,
            vertical_fov_degrees: 28.0,
        };
        let mut camera = Camera::bench();
        camera.set_pose(&pose);
        assert!((camera.position - Vec3::new(5.08, 1.4224, 2.54)).length() < 0.00001);
        let forward = camera.view().forward();
        assert!((forward - Vec3::new(0.866_025_4, -0.5, 0.0)).length() < 0.00001);
        let actual = camera.pose();
        for (actual, expected) in actual.position.into_iter().zip(pose.position) {
            assert!((actual - expected).abs() < 0.001);
        }
        assert!((actual.pitch_degrees - 30.0).abs() < 0.001);
        assert!((actual.yaw_degrees - 90.0).abs() < 0.001);
        assert!((actual.vertical_fov_degrees - 28.0).abs() < 0.001);
        assert_eq!(actual.eye_height_units, 46.0);
    }

    #[test]
    fn near_vertical_poses_remain_valid_and_round_trip_without_losing_the_angle() {
        for pitch in [-89.9_f32, -89.8, -89.5, 89.5, 89.8, 89.9] {
            for yaw in [
                -179.0_f32, -135.0, -90.0, -45.0, 0.0, 45.0, 90.0, 135.0, 179.0,
            ] {
                let pose = CameraPose {
                    position: [100.0, 200.0, 10.0],
                    pitch_degrees: pitch,
                    yaw_degrees: yaw,
                    eye_height_units: 64.0,
                    vertical_fov_degrees: 73.739_8,
                }
                .validated()
                .unwrap();
                let mut camera = Camera::bench();
                camera.set_pose(&pose);
                let actual = camera.pose();
                assert!(
                    (actual.pitch_degrees - pitch).abs() < 0.001,
                    "{pose:?} -> {actual:?}"
                );
                assert!(
                    (actual.yaw_degrees - yaw).abs() < 0.001,
                    "{pose:?} -> {actual:?}"
                );
                let (p, y) = (pitch.to_radians(), yaw.to_radians());
                let expected_forward = Vec3::new(p.cos() * y.sin(), -p.sin(), p.cos() * y.cos());
                assert!((camera.view().forward() - expected_forward).length() < 0.000001);
                actual.validated().unwrap();
            }
        }
    }

    /// The bench camera looks where camera.json says: its own position projects to nothing, and a point straight ahead
    /// lands in the middle of the screen with a depth between the planes.
    #[test]
    fn the_bench_camera_looks_down_its_own_axis() {
        let camera = Camera::bench();
        assert!((camera.position - Vec3::new(-3.988_445, 42.571_194, 29.617_913)).length() < 1e-4);
        let view = camera.view();
        let ahead = camera.position + camera.rotation * Vec3::new(0.0, 0.0, -10.0);
        let clip = view.view_proj(1.6) * ahead.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1e-4 && ndc.y.abs() < 1e-4, "{ndc}");
        // reversed depth: nearer is larger, the near plane is 1 and the far plane 0
        assert!(ndc.z > 0.0 && ndc.z < 1.0);
        let near = view.view_proj(1.6)
            * (camera.position + camera.rotation * Vec3::new(0.0, 0.0, -camera.near)).extend(1.0);
        let far = view.view_proj(1.6)
            * (camera.position + camera.rotation * Vec3::new(0.0, 0.0, -camera.far)).extend(1.0);
        assert!((near.z / near.w - 1.0).abs() < 1e-4 && (far.z / far.w).abs() < 1e-4);
    }

    #[test]
    fn flying_forward_moves_along_the_view_and_looking_keeps_the_horizon_level() {
        let mut camera = Camera::bench();
        let start = camera.position;
        let mut fly = Fly::default();
        fly.key(KeyCode::KeyW, true);
        fly.step(&mut camera, 0.5);
        let moved = camera.position - start;
        assert!((moved.length() - SPEED * 0.5).abs() < 1e-4);
        assert!(moved.normalize().dot(camera.rotation * Vec3::NEG_Z) > 0.9999);
        fly.look(&mut camera, 100.0, -40.0);
        // no roll: the camera's right axis stays horizontal
        assert!((camera.rotation * Vec3::X).y.abs() < 1e-5);
    }
}
