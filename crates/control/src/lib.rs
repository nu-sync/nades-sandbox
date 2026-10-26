use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_PNG_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CameraPose {
    /// Player feet in CS2 Hammer units (Z up).
    pub position: [f32; 3],
    /// Degrees; positive looks down. Range -89.9 through 89.9.
    pub pitch_degrees: f32,
    /// Degrees, normalized to [-180, 180).
    pub yaw_degrees: f32,
    /// Height above feet in Hammer units; use zero for an eye position.
    #[serde(default = "standing_eye")]
    pub eye_height_units: f32,
    /// Vertical field of view in degrees, strictly between 1 and 179.
    #[serde(default = "default_fov")]
    pub vertical_fov_degrees: f32,
}

pub const fn standing_eye() -> f32 {
    64.0
}
pub const fn default_fov() -> f32 {
    73.739_8
}

impl CameraPose {
    pub fn validated(mut self) -> Result<Self, ControlError> {
        if !self
            .position
            .iter()
            .chain([
                &self.pitch_degrees,
                &self.yaw_degrees,
                &self.eye_height_units,
                &self.vertical_fov_degrees,
            ])
            .all(|v| v.is_finite())
        {
            return Err(ControlError::new(
                "invalid_camera",
                "Camera values must be finite numbers.",
                false,
            ));
        }
        if !(-89.9..=89.9).contains(&self.pitch_degrees)
            || !(0.0..=128.0).contains(&self.eye_height_units)
            || self.vertical_fov_degrees <= 1.0
            || self.vertical_fov_degrees >= 179.0
        {
            return Err(ControlError::new(
                "invalid_camera",
                "Pitch must be within ±89.9°, eye height 0–128 units, and vertical FOV strictly between 1° and 179°.",
                false,
            ));
        }
        self.yaw_degrees = (self.yaw_degrees + 180.0).rem_euclid(360.0) - 180.0;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub visible: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FrameMetrics {
    /// CPU wall time preparing and submitting frames; not GPU time or refresh interval.
    pub last_cpu_ms: Option<f64>,
    pub median_cpu_ms: Option<f64>,
    pub p95_cpu_ms: Option<f64>,
    pub samples: usize,
    /// None when GPU timestamp measurement is unavailable.
    pub gpu_ms: Option<f64>,
    pub memory: Option<MemoryUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryUsage {
    pub physical_footprint_bytes: u64,
    pub peak_physical_footprint_bytes: u64,
    /// Portion in the graphics ledger, not total GPU allocation or resident CPU bytes.
    pub graphics_ledger_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AppState {
    pub instance_id: String,
    pub map: String,
    pub camera: CameraPose,
    pub scene: SceneSummary,
    pub viewport: Viewport,
    pub scene_viewport: ViewportRect,
    pub frame_id: u64,
    pub revision: u64,
    /// Revision of the last presented frame; may lag applied state while occluded.
    pub presented_revision: u64,
    pub render_time_seconds: f64,
    pub gpu: String,
    pub draws: u32,
    pub triangles: u64,
    pub startup_marks: Vec<(String, f64)>,
    pub pre_main_ms: Option<f64>,
    pub metrics: FrameMetrics,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SceneSummary {
    pub camera_mode: String,
    pub loading: bool,
    pub notice: Option<String>,
    pub lineup_id: Option<String>,
    pub throw_id: Option<String>,
    pub timeline_seconds: Option<f64>,
    pub playing: bool,
    pub active_effects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewportRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Default for SceneSummary {
    fn default() -> Self {
        Self {
            camera_mode: "fly".into(),
            loading: false,
            notice: None,
            lineup_id: None,
            throw_id: None,
            timeline_seconds: None,
            playing: false,
            active_effects: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ControlError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl ControlError {
    pub fn new(code: &str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ControlError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlRequest {
    pub id: u64,
    /// Requests must not begin applying after this UNIX-millisecond deadline.
    pub deadline_unix_ms: Option<u64>,
    pub command: AppCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum AppCommand {
    GetState,
    SetCamera {
        camera: CameraPose,
        expected_revision: Option<u64>,
    },
    Capture {
        include_ui: bool,
        expected_revision: Option<u64>,
    },
    Shutdown,
    Cancel {
        request_id: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlOutput {
    State {
        state: AppState,
    },
    Capture {
        state: AppState,
        png_base64: String,
        include_ui: bool,
        width: u32,
        height: u32,
    },
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMessage {
    Ready {
        protocol_version: u32,
        state: AppState,
    },
    /// Child-authoritative disposition after its control input closes.
    Detached { ready: bool },
    Reply {
        id: u64,
        result: Result<ControlOutput, ControlError>,
    },
}

pub fn validate_map_id(map: &str) -> Result<(), ControlError> {
    if map.starts_with("de_")
        && map.len() > 3
        && map.len() <= 64
        && map.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        Ok(())
    } else {
        Err(ControlError::new(
            "invalid_map",
            "Use a map ID such as de_cache, without a path or file extension.",
            false,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_defaults_and_yaw_are_explicit() {
        let pose: CameraPose = serde_json::from_str(
            r#"{"position":[100,200,10],"pitch_degrees":30,"yaw_degrees":450}"#,
        )
        .unwrap();
        let actual = pose.validated().unwrap();
        assert_eq!(actual.yaw_degrees, 90.0);
        assert_eq!(actual.eye_height_units, 64.0);
        assert_eq!(actual.vertical_fov_degrees, 73.739_8);
    }

    #[test]
    fn rejects_camera_degeneracy_and_map_paths() {
        let pose = CameraPose {
            position: [0.0; 3],
            pitch_degrees: 90.0,
            yaw_degrees: 0.0,
            eye_height_units: 64.0,
            vertical_fov_degrees: 73.739_8,
        };
        assert_eq!(pose.validated().unwrap_err().code, "invalid_camera");
        for map in [
            "de_",
            "../de_cache",
            "de_cache.pack",
            "de_cache/../../secret",
            "de_cache;echo",
        ] {
            assert_eq!(validate_map_id(map).unwrap_err().code, "invalid_map");
        }
        validate_map_id("de_cache").unwrap();
    }
}
