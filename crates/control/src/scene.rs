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

/// The spot and view a CS2 console string puts a player in: `setpos x y z; setang pitch yaw roll`, as `getpos` prints
/// it and lineup sites post it: either order, with or without the roll, with or without semicolons, `setpos_exact` too,
/// other words around it ignored.
pub fn parse_console(text: &str) -> Result<([f64; 3], f64, f64), ControlError> {
    let invalid = |message: String| ControlError::new("invalid_console", message, false);
    let words: Vec<String> = text.split(|c: char| c.is_whitespace() || c == ';' || c == ',' || c == '"').filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect();
    let numbers_after = |k: usize| -> Vec<f64> { words[k + 1..].iter().map_while(|w| w.parse::<f64>().ok()).collect() };
    let (mut feet, mut angles) = (None, None);
    for (k, word) in words.iter().enumerate() {
        let numbers = numbers_after(k);
        match word.as_str() {
            "setpos" | "setpos_exact" => match numbers.as_slice() {
                [x, y, z, ..] => feet = Some([*x, *y, *z]),
                _ => return Err(invalid(format!("`{word}` needs three numbers (x y z), in {text:?}."))),
            },
            "setang" | "setang_exact" => match numbers.as_slice() {
                [pitch, yaw, ..] => angles = Some((*pitch, *yaw)),
                _ => return Err(invalid(format!("`{word}` needs two numbers (pitch yaw), in {text:?}."))),
            },
            _ => {}
        }
    }
    match (feet, angles) {
        (Some(feet), Some((pitch, yaw))) => Ok((feet, pitch, yaw)),
        _ => Err(invalid(format!("A lineup needs both `setpos x y z` and `setang pitch yaw`; found {}{} in {text:?}.", if feet.is_some() { "setpos" } else { "no setpos" }, if angles.is_some() { " and setang" } else { " and no setang" }))),
    }
}

impl Grenade {
    /// A grenade as people name it: smoke, flash(bang), he / frag / hegrenade, molotov / molly, incendiary / inc,
    /// decoy, in any case.
    pub fn parse(name: &str) -> Option<Self> {
        let n: String = name.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        Some(match n.as_str() {
            "smoke" | "smokegrenade" | "smokes" => Grenade::Smoke,
            "flash" | "flashbang" | "flashes" | "popflash" => Grenade::Flash,
            "he" | "hegrenade" | "frag" | "grenade" | "nade" => Grenade::He,
            "molotov" | "molly" | "molo" | "molotovs" | "mollies" => Grenade::Molotov,
            "incendiary" | "inc" | "incgrenade" | "incendiarygrenade" | "incen" => Grenade::Incendiary,
            "decoy" | "decoygrenade" => Grenade::Decoy,
            _ => return None,
        })
    }
}

/// How a lineup is thrown, read from the words people use: "jumpthrow", "jump throw", "run jumpthrow", "running
/// jump throw", "walk", "run", "standing", "crouch" / "duck", "left click" / "lmb", "right click" / "rmb", "middle",
/// "both" / "lmb+rmb". Words it does not know are left alone; what it found is said in `understood`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Technique {
    pub movement: Movement,
    pub click: Click,
    pub crouch: bool,
    /// The words it read, in order.
    pub understood: Vec<String>,
}

impl Technique {
    pub fn parse(text: &str) -> Self {
        let t = text.to_ascii_lowercase().replace(['-', '_', '+', '/', '(', ')', ',', ';', '.'], " ");
        let words: Vec<&str> = t.split_whitespace().collect();
        let has = |w: &[&str]| words.iter().any(|x| w.contains(x));
        let joined = words.join(" ");
        let jump = has(&["jumpthrow", "jumpthrown", "jt", "jumpbind"]) || joined.contains("jump throw") || has(&["jump"]);
        let run = has(&["run", "running", "runthrow", "runjumpthrow", "w", "forward", "moving"]) || joined.contains("run jump");
        let walk = has(&["walk", "walking", "shift"]);
        let mut understood = Vec::new();
        let movement = match (jump || has(&["runjumpthrow"]), run || has(&["runjumpthrow"]), walk) {
            (true, true, _) => Movement::RunJumpthrow,
            (true, false, _) => Movement::Jumpthrow,
            (false, true, _) => Movement::Run,
            (false, false, true) => Movement::Walk,
            _ => Movement::Stand,
        };
        if movement != Movement::Stand || has(&["stand", "standing", "still"]) {
            understood.push(movement.name().to_string());
        }
        let left = has(&["left", "lmb", "m1", "mouse1", "attack", "full", "long"]);
        let right = has(&["right", "rmb", "m2", "mouse2", "attack2", "underhand", "lob", "short"]);
        let click = if has(&["middle", "both", "medium", "mid"]) || (left && right) {
            Click::Middle
        } else if right {
            Click::Right
        } else {
            Click::Left
        };
        if left || right || has(&["middle", "both", "medium", "mid"]) {
            understood.push(format!("{} click", click.name()));
        }
        let crouch = has(&["crouch", "crouched", "crouching", "duck", "ducked", "ducking", "ctrl"]);
        if crouch {
            understood.push("crouch".into());
        }
        Self { movement, click, crouch, understood }
    }
}

/// A lineup as people post it online, for one call: the map, the grenade, the console line (`setpos …; setang …`),
/// and how it is thrown, in words or as fields (fields win over words).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineupInput {
    /// The map: de_mirage, or just mirage.
    pub map: String,
    /// smoke, flash, he, molotov (molly), incendiary (inc), decoy.
    pub grenade: String,
    /// The lineup's console line, as posted: `setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0` (semicolons,
    /// roll and `_exact` optional; other words around it are ignored, and technique words in it are read too).
    pub lineup: String,
    /// How it is thrown, in words: "jumpthrow", "run jumpthrow", "walk", "crouch", "right click", "middle click",
    /// "jumpthrow + left click"… Standing, left click, not crouched when nothing says otherwise.
    #[serde(default)]
    pub technique: Option<String>,
    #[serde(default)]
    pub movement: Option<Movement>,
    #[serde(default)]
    pub click: Option<Click>,
    #[serde(default)]
    pub crouch: Option<bool>,
}

impl LineupInput {
    /// The map's ID and the throw, with the technique as it was read.
    pub fn throw(&self) -> Result<(String, SceneThrow, Technique), ControlError> {
        let map = if self.map.starts_with("de_") || self.map.starts_with("cs_") || self.map.starts_with("ar_") { self.map.to_ascii_lowercase() } else { format!("de_{}", self.map.to_ascii_lowercase()) };
        validate_map_id(&map)?;
        let grenade = Grenade::parse(&self.grenade).ok_or_else(|| ControlError::new("invalid_grenade", format!("Not a grenade: {:?}. Use smoke, flash, he, molotov, incendiary or decoy.", self.grenade), false))?;
        let (feet, pitch, yaw) = parse_console(&self.lineup)?;
        let mut technique = Technique::parse(&format!("{} {}", self.technique.as_deref().unwrap_or_default(), strip_console(&self.lineup)));
        if let Some(m) = self.movement {
            technique.movement = m;
        }
        if let Some(c) = self.click {
            technique.click = c;
        }
        if let Some(c) = self.crouch {
            technique.crouch = c;
        }
        let throw = SceneThrow { grenade, feet, pitch, yaw, crouch: technique.crouch, movement: technique.movement, click: technique.click, start: 0.0, label: None }.validated()?;
        Ok((map, throw, technique))
    }
}

/// A lineup line without its commands and numbers, for its technique words.
fn strip_console(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c == ';').filter(|w| !w.is_empty() && w.parse::<f64>().is_err() && !w.to_ascii_lowercase().starts_with("set")).collect::<Vec<_>>().join(" ")
}

/// A throw as an agent or another application gives it: the grenade, the spot and view either as a CS2 console string
/// (`setpos …; setang …`) or as feet, pitch and yaw, and how it is thrown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThrowInput {
    pub grenade: Grenade,
    /// `setpos x y z; setang pitch yaw 0`, as CS2's console takes it: the player's feet and view. Give this, or feet,
    /// pitch and yaw.
    #[serde(default)]
    pub console: Option<String>,
    /// The player's feet in Hammer units (Z up), as `setpos` takes them.
    #[serde(default)]
    pub feet: Option<[f64; 3]>,
    /// Degrees, positive looking down, as `setang` takes it.
    #[serde(default)]
    pub pitch: Option<f64>,
    #[serde(default)]
    pub yaw: Option<f64>,
    /// Crouched as it is thrown.
    #[serde(default)]
    pub crouch: bool,
    /// stand (default), walk, run, jumpthrow, run_jumpthrow.
    #[serde(default)]
    pub movement: Movement,
    /// left (default: a full throw), middle (both buttons: medium), right (an underhand lob).
    #[serde(default)]
    pub click: Click,
    /// Seconds on the scene's clock when the throw begins (0–120, default 0).
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub label: Option<String>,
}

impl ThrowInput {
    /// The scene's throw, or why it cannot be one.
    pub fn throw(self) -> Result<SceneThrow, ControlError> {
        let (feet, pitch, yaw) = match (&self.console, self.feet, self.pitch, self.yaw) {
            (Some(text), None, None, None) => parse_console(text)?,
            (None, Some(feet), Some(pitch), Some(yaw)) => (feet, pitch, yaw),
            _ => return Err(ControlError::new("invalid_throw", "Give the spot as `console` (setpos …; setang …) or as feet, pitch and yaw, not both.", false)),
        };
        SceneThrow { grenade: self.grenade, feet, pitch, yaw, crouch: self.crouch, movement: self.movement, click: self.click, start: self.start, label: self.label }.validated()
    }
}

impl From<SceneThrow> for ThrowInput {
    fn from(t: SceneThrow) -> Self {
        Self { grenade: t.grenade, console: None, feet: Some(t.feet), pitch: Some(t.pitch), yaw: Some(t.yaw), crouch: t.crouch, movement: t.movement, click: t.click, start: t.start, label: t.label }
    }
}

/// A scene as an agent gives it: a map and throws, each a [`ThrowInput`] (a scene file reads as one too).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneInput {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub version: Option<u32>,
    /// A map ID such as de_mirage.
    pub map: String,
    #[serde(default)]
    pub throws: Vec<ThrowInput>,
}

impl SceneInput {
    pub fn scene(self) -> Result<Scene, ControlError> {
        let throws = self.throws.into_iter().map(ThrowInput::throw).collect::<Result<Vec<_>, _>>()?;
        Scene { format: self.format.unwrap_or_else(format), version: self.version.unwrap_or(VERSION), map: self.map, throws }.validated()
    }
}

/// A point of a flight: seconds on the scene's clock, and where.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FlightPoint {
    pub t: f64,
    pub at: [f64; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal: Option<[f64; 3]>,
}

/// What a throw did, as the simulator worked it out. Times are seconds on the scene's clock; positions Hammer units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThrowReport {
    /// The throw's index in the scene.
    pub throw: usize,
    pub grenade: Grenade,
    pub start: f64,
    /// When and where it leaves the hand, and its velocity then (units a second).
    pub release: FlightPoint,
    pub velocity: [f64; 3],
    /// Every time it hits something, with the surface's normal.
    pub bounces: Vec<FlightPoint>,
    /// Where it first comes down on a floor (a surface facing up), if it does before it goes off.
    pub landing: Option<FlightPoint>,
    /// When and where it goes off (a flash or HE in the air has no surface).
    pub detonation: FlightPoint,
    /// Seconds from the release to the detonation.
    pub flight_seconds: f64,
    /// How long what it leaves lasts after it goes off, and where a camera should look at it.
    pub effect_seconds: f64,
    pub effect_focus: [f64; 3],
    pub smoke_voxels: Option<usize>,
    pub fire_flames: Option<usize>,
    /// The console commands that put a player in this throw's spot and view.
    pub practice: String,
    /// How long the simulation took (ms).
    pub ms: f64,
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

    #[test]
    fn a_console_string_is_a_throws_spot_and_view() {
        let input = |json: &str| serde_json::from_str::<ThrowInput>(json).unwrap().throw();
        let t = input(r#"{"grenade":"smoke","console":"setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0","movement":"jumpthrow"}"#).unwrap();
        assert_eq!((t.feet, t.pitch, t.yaw, t.movement, t.click, t.crouch), ([1296.0, 32.0, -167.97], -28.89, -166.0, Movement::Jumpthrow, Click::Left, false));
        assert_eq!(parse_console("setang 10 20;setpos_exact 1 2 3").unwrap(), ([1.0, 2.0, 3.0], 10.0, 20.0));
        assert_eq!(input(r#"{"grenade":"he","feet":[0,0,0],"pitch":0,"yaw":90}"#).unwrap().yaw, 90.0);
        assert_eq!(input(r#"{"grenade":"he","console":"setpos 1 2 3"}"#).unwrap_err().code, "invalid_console");
        assert_eq!(input(r#"{"grenade":"he","console":"setpos 1 2 3; setang 0 0","feet":[0,0,0]}"#).unwrap_err().code, "invalid_throw");
        assert_eq!(input(r#"{"grenade":"he","console":"noclip"}"#).unwrap_err().code, "invalid_console");
        // as lineup sites post them: no semicolon, no roll, extra spaces, quotes
        assert_eq!(parse_console("setpos  -1.5 2 3   setang 10 20").unwrap(), ([-1.5, 2.0, 3.0], 10.0, 20.0));
        assert_eq!(parse_console("\"setpos_exact 1 2 3;setang_exact 4 5 0.000000\"").unwrap(), ([1.0, 2.0, 3.0], 4.0, 5.0));
        assert!(parse_console("setpos 1 2; setang 4 5").unwrap_err().message.contains("three numbers"));
        let scene: SceneInput = serde_json::from_str(r#"{"map":"de_mirage","throws":[{"grenade":"flash","console":"setpos 1 2 3; setang 0 0 0"}]}"#).unwrap();
        assert_eq!(scene.scene().unwrap().throws[0].feet, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_lineup_as_posted_online_is_one_throw() {
        let lineup = |json: &str| serde_json::from_str::<LineupInput>(json).unwrap().throw();
        let (map, t, how) = lineup(r#"{"map":"mirage","grenade":"Smoke","lineup":"setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0","technique":"jumpthrow, left click"}"#).unwrap();
        assert_eq!((map.as_str(), t.grenade, t.movement, t.click, t.crouch), ("de_mirage", Grenade::Smoke, Movement::Jumpthrow, Click::Left, false));
        assert_eq!(how.understood, ["jump-throw", "left click"]);
        // the technique may sit in the lineup line itself, and fields win over words
        let (_, t, _) = lineup(r#"{"map":"de_inferno","grenade":"molly","lineup":"setpos 1 2 3; setang 4 5 0 (run jumpthrow, crouch)"}"#).unwrap();
        assert_eq!((t.grenade, t.movement, t.crouch), (Grenade::Molotov, Movement::RunJumpthrow, true));
        let (_, t, _) = lineup(r#"{"map":"nuke","grenade":"flashbang","lineup":"setpos 1 2 3 setang 4 5","technique":"RMB","click":"middle"}"#).unwrap();
        assert_eq!((t.grenade, t.click, t.movement), (Grenade::Flash, Click::Middle, Movement::Stand));
        assert_eq!(Technique::parse("right click lob").click, Click::Right);
        assert_eq!(Technique::parse("LMB+RMB").click, Click::Middle);
        assert_eq!(Technique::parse("walk").movement, Movement::Walk);
        assert_eq!(Technique::parse("W + jumpthrow").movement, Movement::RunJumpthrow);
        assert_eq!(lineup(r#"{"map":"mirage","grenade":"rock","lineup":"setpos 1 2 3; setang 4 5"}"#).unwrap_err().code, "invalid_grenade");
    }
}
