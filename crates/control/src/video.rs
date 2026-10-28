use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::scene::{Scene, ViewName};
use crate::{CameraPose, ControlError};

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
    /// The scene to play: its map and throws.
    pub scene: Scene,
    /// Seconds on the scene's clock where the clip starts (each throw begins at its own start; default 0).
    #[serde(default)]
    pub start_seconds: f64,
    /// Where it ends; with a `shot`, omit it to end with the shot.
    #[serde(default)]
    pub end_seconds: Option<f64>,
    #[serde(default = "width")]
    pub width: u32,
    #[serde(default = "height")]
    pub height: u32,
    #[serde(default = "fps")]
    pub fps: u32,
    /// A fixed camera; omit to follow `view` of `throw`.
    #[serde(default)]
    pub camera: Option<CameraPose>,
    /// Ordered poses for a moving camera; mutually exclusive with camera.
    #[serde(default)]
    pub camera_keyframes: Vec<CameraKeyframe>,
    /// Without a camera: thrower, follow (the default) or orbit, of `throw`.
    #[serde(default)]
    pub view: Option<ViewName>,
    /// The throw the view is of (its index; the first by default).
    #[serde(default)]
    pub throw: Option<usize>,
    /// A scripted camera: moves played one after another from the clip's start, the scene's clock running with them
    /// (e.g. follow the throw to its detonation, then orbit 360° around what it left). Exclusive of camera, keyframes
    /// and view. With none of them, the clip is the lineup video of the first throw (`{"move": "lineup"}`).
    #[serde(default)]
    pub shot: Vec<ShotMove>,
    /// Draw the throws' flight paths (off by default: the camera follows the grenade itself).
    #[serde(default)]
    pub arcs: bool,
    /// Jittered frames each smoke frame averages (1–16, default 4; a still averages 16).
    #[serde(default)]
    pub samples: Option<u32>,
    /// Frames to keep as PNGs as well, each at a named moment of a lineup video or seconds into the clip.
    #[serde(default)]
    pub stills: Vec<Still>,
    /// The most the MP4 may weigh, megabytes (1–500, default 25: under the 30 MB many chat apps take); the encoder's
    /// bitrate is capped to fit it.
    #[serde(default)]
    pub max_megabytes: Option<f64>,
}

/// The size an MP4 keeps under when none is asked for, megabytes.
pub const DEFAULT_MAX_MEGABYTES: f64 = 25.0;

/// A lineup video's camera checked frame by frame against what the map draws (S36), from the release on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CameraAudit {
    /// Frames checked.
    pub frames: usize,
    /// Seconds into the video where the near plane cuts into a surface (the camera in a wall).
    pub clipped: Vec<f64>,
    /// Seconds into the video where a surface hides the grenade or the effect from the camera.
    pub hidden: Vec<f64>,
}

/// A frame of the clip kept as a PNG at `path` (absolute, ending in .png).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Still {
    pub at: StillAt,
    pub path: String,
}

/// When a still is taken: a moment of a lineup video, or seconds into the clip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum StillAt {
    Moment(Moment),
    Seconds(f64),
}

/// The moments of a lineup video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Moment {
    /// Where to stand, wide.
    Spot,
    /// The aim, with the crosshair.
    Aim,
    /// The close-up of what the crosshair lines up with.
    AimPoint,
    /// Halfway through the flight.
    Flight,
    /// As it goes off.
    Detonation,
    /// The effect grown, before the turn.
    Bloom,
    /// Halfway round the turn.
    Result,
}

impl Moment {
    pub const ALL: [Moment; 7] = [Moment::Spot, Moment::Aim, Moment::AimPoint, Moment::Flight, Moment::Detonation, Moment::Bloom, Moment::Result];

    pub fn name(self) -> &'static str {
        match self {
            Moment::Spot => "spot",
            Moment::Aim => "aim",
            Moment::AimPoint => "aim_point",
            Moment::Flight => "flight",
            Moment::Detonation => "detonation",
            Moment::Bloom => "bloom",
            Moment::Result => "result",
        }
    }
}

/// One move of a scripted camera.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "move", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShotMove {
    /// The lineup video of `throw`, as jumpthrow.pro's play: where to stand, the aim with the crosshair, a close-up of
    /// what it lines up with, the throw, the camera after the grenade through the air to where it lands, the effect
    /// blooming, one slow turn round it. Its phases scale with the flight and the effect; `seconds` makes the whole
    /// video that long instead. The scene's clock waits for the throw.
    Lineup {
        throw: usize,
        #[serde(default)]
        seconds: Option<f64>,
    },
    /// Behind `throw`'s grenade from now until it goes off (and `after_seconds` more, looking at what it left).
    Follow {
        throw: usize,
        #[serde(default)]
        after_seconds: f64,
    },
    /// A horizontal circle around what `throw` left (its smoke's middle, its fire, where it went off): `radius` units
    /// out, `height` units above that point, turning `degrees` (360 by default; negative turns clockwise) in `seconds`,
    /// always looking at it. It starts on the bearing the camera already has, unless `start_degrees` (0 = east, +x;
    /// 90 = north, +y) is given, and eases in from the previous camera over its first second. A wall between the point
    /// and the camera brings the camera in front of the wall.
    Orbit {
        throw: usize,
        #[serde(default = "radius")]
        radius: f64,
        #[serde(default = "orbit_height")]
        height: f64,
        #[serde(default = "orbit_seconds")]
        seconds: f64,
        #[serde(default = "full_turn")]
        degrees: f64,
        #[serde(default)]
        start_degrees: Option<f64>,
    },
    /// The thrower's eye as `throw` leaves the hand, for `seconds`.
    Thrower {
        throw: usize,
        #[serde(default = "hold_seconds")]
        seconds: f64,
    },
    /// A fixed camera for `seconds`.
    Hold { camera: CameraPose, seconds: f64 },
}

fn radius() -> f64 {
    350.0
}
fn orbit_height() -> f64 {
    120.0
}
fn orbit_seconds() -> f64 {
    8.0
}
fn full_turn() -> f64 {
    360.0
}
fn hold_seconds() -> f64 {
    2.0
}

impl ShotMove {
    /// The throw it is about, if any.
    pub fn throw(&self) -> Option<usize> {
        match self {
            ShotMove::Lineup { throw, .. } | ShotMove::Follow { throw, .. } | ShotMove::Orbit { throw, .. } | ShotMove::Thrower { throw, .. } => Some(*throw),
            ShotMove::Hold { .. } => None,
        }
    }

    /// Its length when it has one of its own (a follow lasts as long as the flight).
    pub fn seconds(&self) -> Option<f64> {
        match self {
            ShotMove::Follow { .. } => None,
            ShotMove::Lineup { seconds, .. } => *seconds,
            ShotMove::Orbit { seconds, .. } | ShotMove::Thrower { seconds, .. } | ShotMove::Hold { seconds, .. } => Some(*seconds),
        }
    }
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
        self.scene = self.scene.validated()?;
        if self.throw.is_some_and(|k| k >= self.scene.throws.len()) || (self.camera.is_none() && self.camera_keyframes.is_empty() && self.scene.throws.is_empty()) {
            return Err(invalid("Name a throw of the scene for the view, or give a camera."));
        }
        if !self.shot.is_empty() {
            if self.camera.is_some() || !self.camera_keyframes.is_empty() || self.view.is_some() {
                return Err(invalid("A shot is the whole camera: leave camera, camera_keyframes and view out."));
            }
            for m in &mut self.shot {
                if m.throw().is_some_and(|k| k >= self.scene.throws.len()) {
                    return Err(invalid("A shot's move names a throw the scene does not have."));
                }
                if m.seconds().is_some_and(|s| !s.is_finite() || s <= 0.0 || s > 60.0) {
                    return Err(invalid("A move lasts more than 0 and at most 60 seconds."));
                }
                match m {
                    ShotMove::Follow { after_seconds, .. } if !after_seconds.is_finite() || !(0.0..=30.0).contains(after_seconds) => return Err(invalid("after_seconds is 0–30.")),
                    ShotMove::Orbit { radius, height, degrees, start_degrees, .. } if ![*radius, *height, *degrees, start_degrees.unwrap_or(0.0)].iter().all(|v| v.is_finite()) || !(16.0..=4000.0).contains(radius) || height.abs() > 2000.0 || degrees.abs() > 3600.0 => {
                        return Err(invalid("An orbit's radius is 16–4000 units, its height within ±2000, its turn within ±3600°."));
                    }
                    ShotMove::Hold { camera, .. } => *camera = camera.clone().validated()?,
                    _ => {}
                }
            }
            if self.shot.iter().filter_map(ShotMove::seconds).sum::<f64>() > 60.0 {
                return Err(invalid("A shot lasts at most 60 seconds."));
            }
        } else if self.camera.is_none() && self.camera_keyframes.is_empty() && self.view.is_none() && !self.scene.throws.is_empty() {
            self.shot = vec![ShotMove::Lineup { throw: self.throw.unwrap_or(0), seconds: None }];
        } else if self.end_seconds.is_none() {
            return Err(invalid("Give end_seconds, or a shot the clip ends with."));
        }
        if self.samples.is_some_and(|n| !(1..=16).contains(&n)) {
            return Err(invalid("samples is 1–16."));
        }
        if self.max_megabytes.is_some_and(|m| !m.is_finite() || !(1.0..=500.0).contains(&m)) {
            return Err(invalid("max_megabytes is 1–500."));
        }
        for still in &self.stills {
            let path = std::path::Path::new(&still.path);
            if !path.is_absolute() || path.extension().is_none_or(|e| e != "png") || matches!(still.at, StillAt::Seconds(s) if !s.is_finite() || s < 0.0) {
                return Err(invalid("A still's path is absolute and ends in .png; its seconds are 0 or more."));
            }
        }
        let end = self.end_seconds.unwrap_or(self.start_seconds + 1.0);
        let duration = end - self.start_seconds;
        if !self.start_seconds.is_finite() || self.start_seconds < 0.0
            || !end.is_finite()
            || duration <= 0.0
            || duration > 60.0
        {
            return Err(invalid(
                "Start at 0 s or later; the clip's duration must be positive and no more than 60 seconds.",
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

    /// Frames from the start to `end` (the request's end, or the end its shot resolves to).
    pub fn frames_until(&self, end: f64) -> u32 {
        ((end - self.start_seconds) * f64::from(self.fps)).ceil().max(1.0) as u32
    }

    /// Its frames, when its end is given.
    pub fn frame_count(&self) -> Option<u32> {
        self.end_seconds.map(|end| self.frames_until(end))
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
            scene: Scene::new("de_dust2"),
            start_seconds: 1.0,
            end_seconds: Some(3.0),
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
            view: None,
            throw: None,
            shot: Vec::new(),
            arcs: false,
            samples: None,
            stills: Vec::new(),
            max_megabytes: None,
        }
        .validated()
        .unwrap();
        assert_eq!(request.frame_count(), Some(60));
        assert_eq!(request.camera_at(1.0).unwrap().yaw_degrees, -180.0);
    }
    #[test]
    fn rejects_invalid_clip_sizes_and_ambiguous_camera_inputs() {
        let json = serde_json::json!({"scene":{"map":"de_dust2","throws":[{"grenade":"smoke","feet":[0,0,0],"pitch":0,"yaw":0}]},"start_seconds":0.0,"end_seconds":2.0,"width":1279});
        let error = serde_json::from_value::<VideoRequest>(json)
            .unwrap()
            .validated()
            .unwrap_err();
        assert_eq!(error.code, "invalid_video");
        let json = serde_json::json!({"scene":{"map":"de_dust2","throws":[{"grenade":"smoke","feet":[0,0,0],"pitch":0,"yaw":0}]},"start_seconds":0.0,"end_seconds":2.0,"camera":{"position":[0,0,0],"pitch_degrees":0,"yaw_degrees":0},"camera_keyframes":[{"at_seconds":0,"camera":{"position":[1,0,0],"pitch_degrees":0,"yaw_degrees":0}}]});
        let error = serde_json::from_value::<VideoRequest>(json)
            .unwrap()
            .validated()
            .unwrap_err();
        assert_eq!(error.code, "invalid_video");
    }

    #[test]
    fn absent_camera_follows_a_throw_of_the_scene() {
        let json = serde_json::json!({"scene":{"map":"de_dust2","throws":[{"grenade":"smoke","feet":[0,0,0],"pitch":0,"yaw":0}]},"start_seconds":0.0,"end_seconds":2.0});
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
            (1280, 720, 30, Some(60))
        );
        assert!(request.camera_at(0.5).is_none());
        // a view needs a throw to be of, and one the scene has
        let json = serde_json::json!({"scene":{"map":"de_dust2"},"start_seconds":0.0,"end_seconds":2.0});
        assert_eq!(serde_json::from_value::<VideoRequest>(json).unwrap().validated().unwrap_err().code, "invalid_video");
        let json = serde_json::json!({"scene":{"map":"de_dust2","throws":[{"grenade":"he","feet":[0,0,0],"pitch":0,"yaw":0}]},"start_seconds":0.0,"end_seconds":2.0,"throw":1});
        assert_eq!(serde_json::from_value::<VideoRequest>(json).unwrap().validated().unwrap_err().code, "invalid_video");
    }

    #[test]
    fn a_shot_follows_then_orbits_and_ends_the_clip() {
        let scene = serde_json::json!({"map":"de_mirage","throws":[{"grenade":"smoke","feet":[0,0,0],"pitch":0,"yaw":0}]});
        let json = serde_json::json!({"scene":scene,"shot":[{"move":"follow","throw":0,"after_seconds":1},{"move":"orbit","throw":0}]});
        let request = serde_json::from_value::<VideoRequest>(json).unwrap().validated().unwrap();
        assert_eq!(request.shot[1], ShotMove::Orbit { throw: 0, radius: 350.0, height: 120.0, seconds: 8.0, degrees: 360.0, start_degrees: None });
        assert_eq!(request.frame_count(), None, "a shot's end is resolved against the simulation");
        assert_eq!(request.frames_until(10.0), 300);
        let bad = |shot: serde_json::Value| serde_json::from_value::<VideoRequest>(serde_json::json!({"scene":scene,"shot":shot})).unwrap().validated().unwrap_err().code;
        assert_eq!(bad(serde_json::json!([{"move":"orbit","throw":1}])), "invalid_video");
        assert_eq!(bad(serde_json::json!([{"move":"orbit","throw":0,"radius":5}])), "invalid_video");
        assert_eq!(bad(serde_json::json!([{"move":"orbit","throw":0,"seconds":61}])), "invalid_video");
        let both = serde_json::json!({"scene":scene,"view":"orbit","shot":[{"move":"follow","throw":0}]});
        assert_eq!(serde_json::from_value::<VideoRequest>(both).unwrap().validated().unwrap_err().code, "invalid_video");
        // with no camera at all, a throw's lineup video
        let plain = serde_json::from_value::<VideoRequest>(serde_json::json!({"scene":scene})).unwrap().validated().unwrap();
        assert_eq!(plain.shot, [ShotMove::Lineup { throw: 0, seconds: None }]);
        let stills = serde_json::json!({"scene":scene,"stills":[{"at":"aim_point","path":"/tmp/a.png"},{"at":2.5,"path":"/tmp/b.png"}]});
        let stills = serde_json::from_value::<VideoRequest>(stills).unwrap().validated().unwrap().stills;
        assert_eq!((stills[0].at, stills[1].at), (StillAt::Moment(Moment::AimPoint), StillAt::Seconds(2.5)));
        let relative = serde_json::json!({"scene":scene,"stills":[{"at":"aim","path":"a.png"}]});
        assert_eq!(serde_json::from_value::<VideoRequest>(relative).unwrap().validated().unwrap_err().code, "invalid_video");
    }
}
