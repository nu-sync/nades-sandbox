use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CameraPose, ControlError, validate_map_id};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CameraKeyframe {
    /// Seconds from the beginning of the clip.
    pub at_seconds: f64,
    pub camera: CameraPose,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VideoRequest {
    pub map: String,
    pub throw_id: String,
    /// Seconds relative to grenade detonation; negative values show flight.
    pub start_seconds: f64,
    pub end_seconds: f64,
    #[serde(default = "width")]
    pub width: u32,
    #[serde(default = "height")]
    pub height: u32,
    #[serde(default = "fps")]
    pub fps: u32,
    /// A fixed camera; omit to use the recorded throw’s result camera.
    #[serde(default)]
    pub camera: Option<CameraPose>,
    /// Ordered poses for a moving camera; mutually exclusive with camera.
    #[serde(default)]
    pub camera_keyframes: Vec<CameraKeyframe>,
}
fn width() -> u32 {
    1280
}
fn height() -> u32 {
    720
}
fn fps() -> u32 {
    30
}

impl VideoRequest {
    pub fn validated(mut self) -> Result<Self, ControlError> {
        let invalid = |message: &str| ControlError::new("invalid_video", message, false);
        validate_map_id(&self.map)?;
        if self.throw_id.is_empty() || self.throw_id.len() > 200 {
            return Err(invalid("A recorded throw ID is required."));
        }
        let duration = self.end_seconds - self.start_seconds;
        if !self.start_seconds.is_finite()
            || !self.end_seconds.is_finite()
            || duration <= 0.0
            || duration > 60.0
        {
            return Err(invalid(
                "Clip duration must be positive and no more than 60 seconds.",
            ));
        }
        if self.width == 0
            || self.height == 0
            || self.width > 1920
            || self.height > 1080
            || !self.width.is_multiple_of(2)
            || !self.height.is_multiple_of(2)
            || !(1..=60).contains(&self.fps)
            || (duration * f64::from(self.fps)).ceil() > 1800.0
        {
            return Err(invalid(
                "Use even dimensions up to 1920×1080, 1–60 fps, and at most 1800 frames.",
            ));
        }
        if let Some(camera) = self.camera.take() {
            if !self.camera_keyframes.is_empty() {
                return Err(invalid(
                    "Choose a fixed camera or camera keyframes, not both.",
                ));
            }
            self.camera_keyframes.push(CameraKeyframe {
                at_seconds: 0.0,
                camera,
            });
        }
        if self.camera_keyframes.len() > 64 {
            return Err(invalid("Provide at most 64 camera keyframes."));
        }
        let mut previous = -1.0;
        for frame in &mut self.camera_keyframes {
            if !frame.at_seconds.is_finite()
                || frame.at_seconds < 0.0
                || frame.at_seconds > duration
                || frame.at_seconds <= previous
            {
                return Err(invalid(
                    "Camera keyframe times must be increasing and within the clip.",
                ));
            }
            frame.camera = frame.camera.clone().validated()?;
            previous = frame.at_seconds;
        }
        if self
            .camera_keyframes
            .first()
            .is_some_and(|frame| frame.at_seconds != 0.0)
        {
            return Err(invalid("The first camera keyframe must start at zero."));
        }
        Ok(self)
    }

    pub fn frame_count(&self) -> u32 {
        ((self.end_seconds - self.start_seconds) * f64::from(self.fps)).ceil() as u32
    }

    pub fn camera_at(&self, seconds: f64) -> Option<CameraPose> {
        let frames = &self.camera_keyframes;
        if frames.is_empty() {
            return None;
        }
        let Some(next) = frames.iter().position(|frame| frame.at_seconds > seconds) else {
            return frames.last().map(|frame| frame.camera.clone());
        };
        if next == 0 {
            return Some(frames[0].camera.clone());
        }
        let a = &frames[next - 1];
        let b = &frames[next];
        let t = ((seconds - a.at_seconds) / (b.at_seconds - a.at_seconds)) as f32;
        let lerp = |x: f32, y: f32| x + (y - x) * t;
        let yaw_delta =
            (b.camera.yaw_degrees - a.camera.yaw_degrees + 180.0).rem_euclid(360.0) - 180.0;
        Some(CameraPose {
            position: std::array::from_fn(|i| lerp(a.camera.position[i], b.camera.position[i])),
            pitch_degrees: lerp(a.camera.pitch_degrees, b.camera.pitch_degrees),
            yaw_degrees: (a.camera.yaw_degrees + yaw_delta * t + 180.0).rem_euclid(360.0) - 180.0,
            eye_height_units: lerp(a.camera.eye_height_units, b.camera.eye_height_units),
            vertical_fov_degrees: lerp(
                a.camera.vertical_fov_degrees,
                b.camera.vertical_fov_degrees,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moving_camera_crosses_the_yaw_seam_on_the_short_path() {
        let pose = |yaw| CameraPose {
            position: [0.0, 0.0, 0.0],
            pitch_degrees: 0.0,
            yaw_degrees: yaw,
            eye_height_units: 0.0,
            vertical_fov_degrees: 73.0,
        };
        let request = VideoRequest {
            map: "de_dust2".into(),
            throw_id: "throw".into(),
            start_seconds: -1.0,
            end_seconds: 1.0,
            width: 1280,
            height: 720,
            fps: 30,
            camera: None,
            camera_keyframes: vec![
                CameraKeyframe {
                    at_seconds: 0.0,
                    camera: pose(170.0),
                },
                CameraKeyframe {
                    at_seconds: 2.0,
                    camera: pose(-170.0),
                },
            ],
        }
        .validated()
        .unwrap();
        assert_eq!(request.frame_count(), 60);
        assert_eq!(request.camera_at(1.0).unwrap().yaw_degrees, -180.0);
    }
    #[test]
    fn rejects_invalid_clip_sizes_and_ambiguous_camera_inputs() {
        let json = serde_json::json!({"map":"de_dust2","throw_id":"t","start_seconds":-1.0,"end_seconds":1.0,"width":1279});
        let error = serde_json::from_value::<VideoRequest>(json)
            .unwrap()
            .validated()
            .unwrap_err();
        assert_eq!(error.code, "invalid_video");
        let json = serde_json::json!({"map":"de_dust2","throw_id":"t","start_seconds":-1.0,"end_seconds":1.0,"camera":{"position":[0,0,0],"pitch_degrees":0,"yaw_degrees":0},"camera_keyframes":[{"at_seconds":0,"camera":{"position":[1,0,0],"pitch_degrees":0,"yaw_degrees":0}}]});
        let error = serde_json::from_value::<VideoRequest>(json)
            .unwrap()
            .validated()
            .unwrap_err();
        assert_eq!(error.code, "invalid_video");
    }

    #[test]
    fn absent_camera_selects_the_recorded_result_view() {
        let json = serde_json::json!({"map":"de_dust2","throw_id":"t","start_seconds":-1.0,"end_seconds":1.0});
        let request = serde_json::from_value::<VideoRequest>(json)
            .unwrap()
            .validated()
            .unwrap();
        assert_eq!(
            (
                request.width,
                request.height,
                request.fps,
                request.frame_count()
            ),
            (1280, 720, 30, 60)
        );
        assert!(request.camera_at(0.5).is_none());
    }
}
