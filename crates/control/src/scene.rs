//! The scene file: a map and the throws placed on it, each with its start time, as JSON. It is the simulator's one
//! generic port: the app saves and opens it, its MCP tools read and write it, and other applications (a lineup
//! library, show-me-the-nades) can write one to show their throws here.
//!
//! ```json
//! {"format": "nades-sandbox-scene", "version": 1, "map": "de_mirage",
//!  "throws": [{"grenade": "smoke", "feet": [-1216.0, -1048.0, -167.97], "pitch": -24.5, "yaw": 132.0,
//!              "crouch": false, "movement": "jumpthrow", "click": "left", "start": 0.0}]}
//! ```
//!
//! Positions are the player's feet in Hammer units (z up); angles are degrees as CS2's `setang` takes them (pitch
//! positive looking down); `start` is seconds on the scene's clock when the throw begins. `crouch`, `movement`, `click`
//! and `start` may be left out (standing, left click, at 0); `label` is the author's name for a throw. A reader takes
//! any version it knows and refuses a newer one: a change of meaning is a new version.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ControlError, validate_map_id};

pub const FORMAT: &str = "nades-sandbox-scene";
pub const VERSION: u32 = 1;
/// The most throws a scene holds.
pub const MAX_THROWS: usize = 64;
/// The latest start a throw may have, in seconds.
pub const MAX_START: f64 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Grenade {
    Smoke,
    Flash,
    He,
    Molotov,
    Incendiary,
    Decoy,
}

impl Grenade {
    pub const ALL: [Grenade; 6] = [Grenade::Smoke, Grenade::Flash, Grenade::He, Grenade::Molotov, Grenade::Incendiary, Grenade::Decoy];

    pub fn name(self) -> &'static str {
        match self {
            Grenade::Smoke => "smoke",
            Grenade::Flash => "flash",
            Grenade::He => "HE",
            Grenade::Molotov => "molotov",
            Grenade::Incendiary => "incendiary",
            Grenade::Decoy => "decoy",
        }
    }
}

/// How the player moves as they let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Movement {
    #[default]
    Stand,
    Walk,
    Run,
    Jumpthrow,
    RunJumpthrow,
}

impl Movement {
    pub const ALL: [Movement; 5] = [Movement::Stand, Movement::Walk, Movement::Run, Movement::Jumpthrow, Movement::RunJumpthrow];

    pub fn name(self) -> &'static str {
        match self {
            Movement::Stand => "stand",
            Movement::Walk => "walk",
            Movement::Run => "run",
            Movement::Jumpthrow => "jump-throw",
            Movement::RunJumpthrow => "run jump-throw",
        }
    }
}

/// Left is a full throw, middle (both buttons) a medium one, right an underhand lob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Click {
    #[default]
    Left,
    Middle,
    Right,
}

impl Click {
    pub const ALL: [Click; 3] = [Click::Left, Click::Middle, Click::Right];

    pub fn name(self) -> &'static str {
        match self {
            Click::Left => "left",
            Click::Middle => "middle",
            Click::Right => "right",
        }
    }
}

/// One throw placed in a scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneThrow {
    pub grenade: Grenade,
    /// The player's feet in Hammer units (z up); for a jump-throw, where they take off.
    pub feet: [f64; 3],
    /// Degrees, positive looking down; within ±89.
    pub pitch: f64,
    /// Degrees.
    pub yaw: f64,
    #[serde(default)]
    pub crouch: bool,
    #[serde(default)]
    pub movement: Movement,
    #[serde(default)]
    pub click: Click,
    /// Seconds on the scene's clock when the throw begins, 0–120.
    #[serde(default)]
    pub start: f64,
    /// The author's name for the throw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl SceneThrow {
    pub fn new(grenade: Grenade, feet: [f64; 3], pitch: f64, yaw: f64) -> Self {
        Self { grenade, feet, pitch, yaw, crouch: false, movement: Movement::Stand, click: Click::Left, start: 0.0, label: None }
    }

    /// The throw with its yaw in [-180, 180), or why it cannot be thrown.
    pub fn validated(mut self) -> Result<Self, ControlError> {
        let invalid = |message: &str| ControlError::new("invalid_throw", message, false);
        if !self.feet.iter().chain([&self.pitch, &self.yaw, &self.start]).all(|v| v.is_finite()) {
            return Err(invalid("Throw values must be finite numbers."));
        }
        if self.feet.iter().any(|v| v.abs() > 65_536.0) {
            return Err(invalid("Feet must be within ±65536 units."));
        }
        if !(-89.0..=89.0).contains(&self.pitch) {
            return Err(invalid("Pitch must be within ±89°."));
        }
        if !(0.0..=MAX_START).contains(&self.start) {
            return Err(invalid("Start must be 0–120 seconds."));
        }
        if self.label.as_ref().is_some_and(|l| l.chars().count() > 120) {
            return Err(invalid("A label holds at most 120 characters."));
        }
        self.yaw = (self.yaw + 180.0).rem_euclid(360.0) - 180.0;
        Ok(self)
    }
}

/// A map and the throws placed on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    /// Always "nades-sandbox-scene".
    #[serde(default = "format")]
    pub format: String,
    /// The format's version: 1.
    #[serde(default = "version")]
    pub version: u32,
    /// A map ID such as de_mirage.
    pub map: String,
    #[serde(default)]
    pub throws: Vec<SceneThrow>,
}

fn format() -> String {
    FORMAT.into()
}

fn version() -> u32 {
    VERSION
}

impl Scene {
    pub fn new(map: &str) -> Self {
        Self { format: FORMAT.into(), version: VERSION, map: map.into(), throws: Vec::new() }
    }

    /// The scene with every throw checked, or why it cannot be shown.
    pub fn validated(mut self) -> Result<Self, ControlError> {
        let invalid = |message: String| ControlError::new("invalid_scene", message, false);
        if self.format != FORMAT {
            return Err(invalid(format!("Not a scene file: format is {:?}, not {FORMAT:?}.", self.format)));
        }
        if self.version == 0 || self.version > VERSION {
            return Err(invalid(format!("Scene version {} is not one this build reads (1 to {VERSION}).", self.version)));
        }
        validate_map_id(&self.map)?;
        if self.throws.len() > MAX_THROWS {
            return Err(invalid(format!("A scene holds at most {MAX_THROWS} throws.")));
        }
        self.throws = self.throws.into_iter().map(SceneThrow::validated).collect::<Result<_, _>>()?;
        self.version = VERSION;
        Ok(self)
    }

    /// Reads a scene file's text.
    pub fn parse(text: &str) -> Result<Self, ControlError> {
        let scene: Scene = serde_json::from_str(text).map_err(|e| ControlError::new("invalid_scene", format!("Not a scene file: {e}"), false))?;
        scene.validated()
    }

    /// The scene file's text.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a scene serializes")
    }

    /// When the scene's last throw begins.
    pub fn last_start(&self) -> f64 {
        self.throws.iter().map(|t| t.start).fold(0.0, f64::max)
    }
}

/// The views of a throw the app can show, beside the free camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewName {
    /// The fly camera.
    #[default]
    Free,
    /// The thrower's eye as they let go.
    Thrower,
    /// Behind the grenade in flight, then over what it left.
    Follow,
    /// Around what the throw left; it opens on a result shot that sees it.
    Orbit,
}

impl ViewName {
    pub const ALL: [ViewName; 4] = [ViewName::Free, ViewName::Thrower, ViewName::Follow, ViewName::Orbit];

    pub fn name(self) -> &'static str {
        match self {
            ViewName::Free => "free",
            ViewName::Thrower => "thrower",
            ViewName::Follow => "follow",
            ViewName::Orbit => "orbit",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name().eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_reads_back_as_written_and_fills_its_defaults() {
        let mut scene = Scene::new("de_mirage");
        let mut smoke = SceneThrow::new(Grenade::Smoke, [-1216.0, -1048.0, -167.97], -24.5, 132.0);
        smoke.movement = Movement::Jumpthrow;
        scene.throws.push(smoke);
        scene.throws.push(SceneThrow { start: 2.5, label: Some("window".into()), ..SceneThrow::new(Grenade::Molotov, [1.0, 2.0, 3.0], 10.0, -90.0) });
        let text = scene.to_json();
        assert!(text.contains("\"format\": \"nades-sandbox-scene\"") && text.contains("\"movement\": \"jumpthrow\""));
        assert_eq!(Scene::parse(&text).unwrap(), scene);
        let short = Scene::parse(r#"{"map":"de_cache","throws":[{"grenade":"he","feet":[0,0,0],"pitch":0,"yaw":270}]}"#).unwrap();
        let t = &short.throws[0];
        assert_eq!((t.movement, t.click, t.crouch, t.start, t.yaw), (Movement::Stand, Click::Left, false, 0.0, -90.0));
        assert_eq!(short.version, VERSION);
        assert_eq!(scene.last_start(), 2.5);
    }

    #[test]
    fn a_scene_says_what_is_wrong_with_it() {
        let code = |text: &str| Scene::parse(text).unwrap_err();
        assert!(code(r#"{"format":"other","map":"de_cache"}"#).message.contains("format"));
        assert!(code(r#"{"version":2,"map":"de_cache"}"#).message.contains("version 2"));
        assert_eq!(code(r#"{"map":"../x"}"#).code, "invalid_map");
        assert_eq!(code(r#"{"map":"de_cache","throws":[{"grenade":"smoke","feet":[0,0,0],"pitch":95,"yaw":0}]}"#).code, "invalid_throw");
        assert_eq!(code(r#"{"map":"de_cache","throws":[{"grenade":"smoke","feet":[0,0,0],"pitch":0,"yaw":0,"start":-1}]}"#).code, "invalid_throw");
        assert_eq!(code(r#"{"map":"de_cache","throws":[{"grenade":"rock","feet":[0,0,0],"pitch":0,"yaw":0}]}"#).code, "invalid_scene");
        assert_eq!(code("not json").code, "invalid_scene");
        assert_eq!(ViewName::parse("Orbit"), Some(ViewName::Orbit));
    }
}
