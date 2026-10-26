//! What a frame is drawn from: the eye, its rotation and the projection (reversed depth), and the same view from the
//! 3D skybox's camera.

use glam::{Mat4, Quat, Vec3, Vec4};

/// CS2's default vertical field of view: 90° across at 4:3.
pub const VERTICAL_FOV_DEGREES: f32 = 73.739_8;
/// Scene units (metres) per Hammer unit.
pub const INCH: f32 = 0.0254;

/// A camera in the scene's frame (glTF space: metres, y up), looking down its −z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub eye: Vec3,
    pub rotation: Quat,
    /// Vertical, radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl View {
    /// The world-to-view matrix.
    pub fn view(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rotation, self.eye).inverse()
    }

    /// The projection for `aspect` (width / height): wgpu's clip space, reversed depth (1 at the near plane, 0 at the
    /// far one).
    pub fn projection(&self, aspect: f32) -> Mat4 {
        // right-handed view space looking down −z; view depth d maps to near (far − d) / ((far − near) d)
        let h = 1.0 / (self.fov_y / 2.0).tan();
        let r = self.near / (self.far - self.near);
        Mat4::from_cols(Vec4::new(h / aspect, 0.0, 0.0, 0.0), Vec4::new(0.0, h, 0.0, 0.0), Vec4::new(0.0, 0.0, r, -1.0), Vec4::new(0.0, 0.0, self.far * r, 0.0))
    }

    /// clip = `view_proj` × world.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    /// A CS2 console position and angles, as `setpos x y z; setang pitch yaw` gives them: `setpos` is the player's
    /// feet, the eye `eye_height` units above (64 standing, 46 crouched); pitch is positive looking down.
    pub fn from_setpos(setpos: [f32; 3], pitch: f32, yaw: f32, eye_height: f32) -> Self {
        Self::from_cs([setpos[0], setpos[1], setpos[2] + eye_height], pitch, yaw, VERTICAL_FOV_DEGREES)
    }

    /// A CS2 eye (Hammer units, z up), its angles (pitch positive looking down, yaw from +x toward +y) and vertical field
    /// of view in degrees, looking with no roll, as the web view's `engine.look` places its camera.
    pub fn from_cs(eye: [f32; 3], pitch: f32, yaw: f32, fov_degrees: f32) -> Self {
        let eye = Vec3::new(eye[1], eye[2], eye[0]) * INCH;
        let (p, y) = (pitch.to_radians(), yaw.to_radians());
        // Source's forward (x, y, z) = (cos p cos y, cos p sin y, −sin p), turned into the scene's frame
        let source_forward = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
        let forward = Vec3::new(source_forward.y, source_forward.z, source_forward.x);
        // look down −z with y up and no roll
        let back = -forward.normalize();
        let right = Vec3::Y.cross(back).normalize();
        let up = back.cross(right);
        let rotation = Quat::from_mat3(&glam::Mat3::from_cols(right, up, back));
        Self { eye, rotation, fov_y: fov_degrees.to_radians(), near: 4.0 * INCH, far: 30_000.0 * INCH }
    }

    /// Where the view direction points.
    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }
}

/// How the 3D skybox sits around the map: its prefab built at 1/scale, drawn through a camera at
/// (eye − reference) / scale + origin (Source 2 Viewer's SkyTransform).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyPlacement {
    pub scale: f32,
    pub origin: Vec3,
    pub reference: Vec3,
    /// How far the skybox reaches from its origin, in its own units.
    pub radius: f32,
}

/// The sky camera's near plane, in the skybox's units: the map covers whatever of it is nearer than this.
pub const SKY_NEAR: f32 = 8.0 * INCH;

impl SkyPlacement {
    /// The eye's view from the skybox's camera.
    pub fn view(&self, eye: &View) -> View {
        let at = (eye.eye - self.reference) / self.scale + self.origin;
        let far = (self.radius + at.distance(self.origin)) * 1.1 + SKY_NEAR;
        View { eye: at, rotation: eye.rotation, fov_y: eye.fov_y, near: SKY_NEAR, far: far.max(SKY_NEAR * 2.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setpos_becomes_the_eye_and_its_angles_the_view() {
        // feet at the origin looking along Source +x, level: the eye is 64 units up (scene y), looking down scene +z
        let v = View::from_setpos([0.0, 0.0, 0.0], 0.0, 0.0, 64.0);
        assert!((v.eye - Vec3::new(0.0, 64.0 * INCH, 0.0)).length() < 1e-5);
        assert!((v.forward() - Vec3::Z).length() < 1e-5, "{}", v.forward());
        // yaw 90° looks along Source +y, scene +x; pitch 30° looks down
        let v = View::from_setpos([100.0, 200.0, 10.0], 30.0, 90.0, 46.0);
        assert!((v.eye - Vec3::new(200.0, 56.0, 100.0) * INCH).length() < 1e-4);
        let f = v.forward();
        assert!((f - Vec3::new(30f32.to_radians().cos(), -30f32.to_radians().sin(), 0.0)).length() < 1e-4, "{f}");
        // no roll: the right axis stays level
        assert!((v.rotation * Vec3::X).y.abs() < 1e-5);
        // straight ahead projects to the middle of the screen, between the planes
        let clip = v.view_proj(16.0 / 9.0) * (v.eye + f * 10.0).extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1e-4 && ndc.y.abs() < 1e-4 && ndc.z > 0.0 && ndc.z < 1.0, "{ndc}");
    }

    #[test]
    fn the_sky_camera_moves_a_scaleth_as_far_as_the_eye() {
        let sky = SkyPlacement { scale: 16.0, origin: Vec3::new(1.0, 2.0, 3.0), reference: Vec3::new(10.0, 0.0, 0.0), radius: 50.0 };
        let eye = View { eye: Vec3::new(26.0, 16.0, 0.0), rotation: Quat::IDENTITY, fov_y: 1.0, near: 0.1, far: 700.0 };
        let v = sky.view(&eye);
        assert!((v.eye - Vec3::new(2.0, 3.0, 3.0)).length() < 1e-6);
        assert_eq!((v.rotation, v.near), (eye.rotation, SKY_NEAR));
        assert!(v.far > 50.0);
    }
}
