//! What the sandbox reopens on: the map and camera last shown, and each map's last camera, in
//! `~/.cache/nades/sandbox/state.json` (`NADES_SANDBOX_STATE` names another file). The camera is kept so the first frame
//! opens where the last run left off.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Remembered {
    pub map: String,
    /// The CS2 camera shown: eye (Hammer units), pitch, yaw, vertical field of view (degrees).
    pub camera: Option<Camera>,
    /// Each map's last camera, so any map opened before opens where it was left.
    #[serde(default)]
    pub cameras: BTreeMap<String, Camera>,
}

/// A CS2 camera: eye (Hammer units), pitch, yaw, vertical field of view (degrees).
pub type Camera = ([f64; 3], f64, f64, f64);

impl Remembered {
    /// The camera `map` was last shown from.
    pub fn camera_on(&self, map: &str) -> Option<Camera> {
        if self.map == map { self.camera.or_else(|| self.cameras.get(map).copied()) } else { self.cameras.get(map).copied() }
    }
}

pub fn path() -> PathBuf {
    match std::env::var_os("NADES_SANDBOX_STATE") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache/nades/sandbox/state.json"),
    }
}

/// The remembered state, or None on a first run (or a file this build cannot read).
pub fn load() -> Option<Remembered> {
    serde_json::from_slice(&std::fs::read(path()).ok()?).ok()
}

/// Writes the state, quietly: losing it only costs the next start its first view.
pub fn save(state: &Remembered) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(state) {
        let temporary = path.with_extension("json.tmp");
        if std::fs::write(&temporary, json).is_ok() {
            let _ = std::fs::rename(temporary, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_reads_back_as_written() {
        let cameras = BTreeMap::from([("de_cache".to_string(), ([9.0, 8.0, 7.0], 1.0, 2.0, 73.7))]);
        let state = Remembered { map: "de_inferno".into(), camera: Some(([1.0, 2.0, 3.0], 4.0, 5.0, 73.7)), cameras };
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<Remembered>(&json).unwrap(), state);
        // each map opens where it was left
        assert_eq!(state.camera_on("de_inferno").unwrap().0, [1.0, 2.0, 3.0]);
        assert_eq!(state.camera_on("de_cache").unwrap().0, [9.0, 8.0, 7.0]);
        assert_eq!(state.camera_on("de_nuke"), None);
        // a file from an older build still reads, with what it lacks left out
        assert_eq!(serde_json::from_str::<Remembered>(r#"{"map":"de_cache"}"#).unwrap().map, "de_cache");
        // and one from the lineup sandbox, with what it no longer keeps left out
        assert_eq!(serde_json::from_str::<Remembered>(r#"{"map":"de_cache","lineup":"x","throw":"t","view":"result"}"#).unwrap().map, "de_cache");
    }
}
