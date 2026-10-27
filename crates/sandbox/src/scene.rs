//! The scene in the window: the throws placed on the shown map, simulated in the background (each once per spot,
//! angles and technique), the selected one, the scene's clock, the throw tool and the view shown. Each map keeps its own
//! scene while another is shown.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use glam::DVec3;
use nades_control::scene::{Click, Grenade, Movement, Scene, SceneThrow, ViewName};
use nades_scene::camera::{EYE_CROUCHED, EYE_STANDING, View as CsView};

use crate::collider::SharedCollider;
use crate::sim::{self, Built, Placed};
use crate::views::Orbit;

/// How the next throw is thrown, and whether it is being aimed.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub grenade: Grenade,
    pub movement: Movement,
    pub crouch: bool,
    pub click: Click,
    /// Seconds on the scene's clock it starts at.
    pub start: f64,
    /// The preview arc follows the camera.
    pub preview: bool,
    /// The next click on the map stands the camera there.
    pub picking: bool,
}

impl Default for Tool {
    fn default() -> Self {
        Self { grenade: Grenade::Smoke, movement: Movement::Stand, crouch: false, click: Click::Left, start: 0.0, preview: true, picking: false }
    }
}

impl Tool {
    /// The throw from where `camera` stands (its eye 64 over the feet, 46 crouched), aimed where it looks.
    pub fn throw_from(&self, camera: &CsView) -> SceneThrow {
        let height = if self.crouch { EYE_CROUCHED } else { EYE_STANDING };
        let feet = camera.eye - DVec3::Z * height;
        SceneThrow { grenade: self.grenade, feet: feet.to_array().map(|v| (v * 100.0).round() / 100.0), pitch: camera.pitch.clamp(-89.0, 89.0), yaw: camera.yaw, crouch: self.crouch, movement: self.movement, click: self.click, start: self.start, label: None }
    }
}

/// What decides the arcs drawn: the scene's revision, the selected throw, how many are simulated, whether arcs show,
/// the aimed throw.
pub type ArcsKey = (u64, Option<usize>, usize, bool, Option<String>);

/// A simulation answered: the map, the throw's key, what it built.
type Reply = (String, String, Arc<Built>);

pub struct SceneState {
    pub scene: Scene,
    /// Each other map's scene, kept while this one is shown.
    pub others: HashMap<String, Scene>,
    /// What each throw built, by map and key ([`sim::key`]).
    built: HashMap<(String, String), Arc<Built>>,
    asked: HashSet<(String, String)>,
    send: Sender<Reply>,
    replies: Receiver<Reply>,
    pub selected: Option<usize>,
    /// Seconds on the scene's clock.
    pub time: f64,
    pub playing: bool,
    pub view: ViewName,
    pub orbit: Option<Orbit>,
    pub tool: Tool,
    pub arcs: bool,
    /// The aimed throw's flight while the tool previews it.
    pub preview: Option<(SceneThrow, nades_physics::Path)>,
    /// Counts the scene's changes: what is drawn of it follows.
    pub revision: u64,
    /// The scene file's path in the shell.
    pub file: String,
    pub notice: Option<String>,
}

impl SceneState {
    pub fn new(map: &str, kept: impl IntoIterator<Item = Scene>) -> Self {
        let (send, replies) = std::sync::mpsc::channel();
        let mut others: HashMap<String, Scene> = kept.into_iter().map(|s| (s.map.clone(), s)).collect();
        let scene = others.remove(map).unwrap_or_else(|| Scene::new(map));
        Self { file: default_file(map), scene, others, built: HashMap::new(), asked: HashSet::new(), send, replies, selected: None, time: 0.0, playing: false, view: ViewName::Free, orbit: None, tool: Tool::default(), arcs: true, preview: None, revision: 0, notice: None }
    }

    /// Every map's scene with throws, this one's included.
    pub fn all(&self) -> Vec<Scene> {
        let mut all: Vec<Scene> = self.others.values().chain([&self.scene]).filter(|s| !s.throws.is_empty()).cloned().collect();
        all.sort_by(|a, b| a.map.cmp(&b.map));
        all
    }

    /// Shows `map`'s scene, keeping this one.
    pub fn switch_map(&mut self, map: &str) {
        if self.scene.map == map {
            return;
        }
        let next = self.others.remove(map).unwrap_or_else(|| Scene::new(map));
        let previous = std::mem::replace(&mut self.scene, next);
        self.others.insert(previous.map.clone(), previous);
        self.file = default_file(map);
        self.selected = (!self.scene.throws.is_empty()).then_some(0);
        self.changed();
        self.time = 0.0;
        self.playing = false;
        self.view = ViewName::Free;
        self.orbit = None;
        self.preview = None;
    }

    /// Puts `scene` up in place of its map's (the shown map's, or kept for when its map shows).
    pub fn set(&mut self, scene: Scene) {
        if scene.map == self.scene.map {
            self.scene = scene;
            self.selected = (!self.scene.throws.is_empty()).then_some(0);
            self.time = 0.0;
            self.playing = false;
            self.changed();
        } else {
            self.others.insert(scene.map.clone(), scene);
        }
    }

    pub fn changed(&mut self) {
        self.revision += 1;
        if self.selected.is_some_and(|k| k >= self.scene.throws.len()) {
            self.selected = self.scene.throws.len().checked_sub(1);
        }
    }

    fn key(&self, t: &SceneThrow) -> (String, String) {
        (self.scene.map.clone(), sim::key(t))
    }

    /// What `t` built, once simulated.
    pub fn built(&self, t: &SceneThrow) -> Option<&Arc<Built>> {
        self.built.get(&self.key(t))
    }

    /// The scene's throws that are simulated, on its clock (in the scene's order, those still being simulated left out).
    pub fn placed(&self) -> Vec<Placed> {
        self.scene.throws.iter().filter_map(|t| Some((self.built(t)?.clone(), t.start))).collect()
    }

    /// Whether every throw is simulated.
    pub fn ready(&self) -> bool {
        self.scene.throws.iter().all(|t| self.built(t).is_some())
    }

    /// Simulates the throws not simulated yet, each on the pool, once the collider is built; `wake` when each is done.
    pub fn simulate(&mut self, collider: &SharedCollider, toward_sun: DVec3, wake: Option<Arc<dyn Fn() + Send + Sync>>) {
        for t in &self.scene.throws {
            let key = self.key(t);
            if self.built.contains_key(&key) || !self.asked.insert(key.clone()) {
                continue;
            }
            let (t, collider, send, wake) = (t.clone(), collider.clone(), self.send.clone(), wake.clone());
            rayon::spawn(move || {
                let c = collider.wait_for(std::time::Duration::from_secs(30));
                let built = Arc::new(sim::simulate(&t, c.as_deref(), toward_sun));
                eprintln!("simulated: {:?} from {:?} in {:.1} ms, off at {:.2} s at {:?}", t.grenade, t.feet, built.ms, built.path.detonation.t, built.path.detonation.pos.to_array());
                let _ = send.send((key.0, key.1, built));
                if let Some(wake) = wake {
                    wake();
                }
            });
        }
    }

    /// Takes the simulations done; true when one came.
    pub fn receive(&mut self) -> bool {
        let mut any = false;
        while let Ok((map, key, built)) = self.replies.try_recv() {
            self.asked.remove(&(map.clone(), key.clone()));
            self.built.insert((map, key), built);
            any = true;
        }
        any
    }

    /// Where the clock runs to.
    pub fn end(&self) -> f64 {
        sim::end(&self.placed()).max(self.scene.last_start() + 1.0)
    }

    /// Plays the clock on by `dt` seconds, stopping at the end.
    pub fn advance(&mut self, dt: f64) {
        if self.playing {
            let end = self.end();
            self.time = (self.time + dt).min(end);
            self.playing = self.time < end;
        }
    }

    pub fn play(&mut self, on: bool) {
        if on && self.time >= self.end() - 1e-3 {
            self.time = 0.0;
        }
        self.playing = on;
    }

    /// The throw a view is of: the selected one, else the first.
    pub fn viewed(&self) -> Option<(&SceneThrow, &Arc<Built>)> {
        let k = self.selected.unwrap_or(0);
        let t = self.scene.throws.get(k)?;
        Some((t, self.built(t)?))
    }

    /// Shows `view` of the selected throw (or `throw`, selected first): the thrower's eye at its release, the grenade
    /// followed through its flight, or the orbit over what it left at its fullest.
    pub fn show_view(&mut self, view: ViewName, throw: Option<usize>) {
        if let Some(k) = throw.filter(|k| *k < self.scene.throws.len()) {
            self.selected = Some(k);
        }
        self.view = view;
        self.orbit = None;
        let Some((t, b)) = self.viewed().map(|(t, b)| (t.start, b.clone())) else { return };
        match view {
            ViewName::Free => {}
            ViewName::Thrower => {
                self.time = t + b.path.release.t;
                self.playing = false;
            }
            ViewName::Follow => {
                self.time = t;
                self.playing = true;
            }
            ViewName::Orbit => {
                self.orbit = Some(sim::views::orbit(&b));
                self.time = b.detonates_at(t) + b.stage.result_time;
                self.playing = false;
            }
        }
    }

    /// The camera the view shows now, if a throw's view is up and the throw is simulated.
    pub fn camera(&self, collider: Option<&nades_scene::collider::Collider>) -> Option<CsView> {
        let (t, b) = self.viewed()?;
        sim::views::camera(self.view, b, b.local(t.start, self.time), self.orbit.as_ref(), collider)
    }

    /// Adds `t` and selects it.
    pub fn add(&mut self, t: SceneThrow) -> Option<usize> {
        if self.scene.throws.len() >= nades_control::scene::MAX_THROWS {
            self.notice = Some(format!("A scene holds at most {} throws.", nades_control::scene::MAX_THROWS));
            return None;
        }
        self.scene.throws.push(t);
        self.selected = Some(self.scene.throws.len() - 1);
        self.changed();
        self.selected
    }

    pub fn duplicate(&mut self, k: usize) {
        if let Some(t) = self.scene.throws.get(k).cloned() {
            self.add(t);
        }
    }

    pub fn delete(&mut self, k: usize) {
        if k < self.scene.throws.len() {
            self.scene.throws.remove(k);
            if self.selected == Some(k) && self.view != ViewName::Free {
                self.view = ViewName::Free;
            }
            self.changed();
        }
    }

    /// Replaces throw `k` (its settings, its spot or aim).
    pub fn edit(&mut self, k: usize, t: SceneThrow) {
        if let (Some(slot), Ok(t)) = (self.scene.throws.get_mut(k), t.validated()) {
            *slot = t;
            self.changed();
        }
    }

    /// The tool's aimed flight from `camera`, worked out again when the aim or the tool changed.
    pub fn aim(&mut self, camera: &CsView, collider: &SharedCollider) {
        if !self.tool.preview || self.view != ViewName::Free || !collider.built() {
            self.preview = None;
            return;
        }
        let t = self.tool.throw_from(camera);
        if self.preview.as_ref().is_some_and(|(p, _)| sim::key(p) == sim::key(&t)) {
            return;
        }
        let path = sim::preview(&t, collider.get().as_deref());
        self.preview = Some((t, path));
    }

    /// The arcs to draw: every simulated throw's flight (the selected one strong), and the aimed one's.
    pub fn arcs(&self) -> Vec<nades_render::throw::Arc> {
        let mut arcs = Vec::new();
        if self.arcs {
            for (k, t) in self.scene.throws.iter().enumerate() {
                if let Some(b) = self.built(t) {
                    arcs.push(b.arc(self.selected == Some(k)));
                }
            }
        }
        if let Some((t, path)) = &self.preview {
            let points: Vec<DVec3> = path.points.iter().map(|(_, p)| *p).collect();
            let marks = path.bounces.iter().map(|b| (b.pos, 0.8)).chain(points.first().map(|p| (*p, 1.0))).chain([(path.detonation.pos, 1.4)]).collect();
            arcs.push(nades_render::throw::Arc { points, marks, color: nades_scene::stage::nade_color(sim::drawn(t.grenade)), strong: true });
        }
        arcs
    }

    /// What decides the arcs: when it changes they are built again.
    pub fn arcs_key(&self) -> ArcsKey {
        let built = self.scene.throws.iter().filter(|t| self.built(t).is_some()).count();
        (self.revision, self.selected, built, self.arcs, self.preview.as_ref().map(|(t, _)| sim::key(t)))
    }

    /// Writes the scene file to the path in the shell.
    pub fn save(&mut self) {
        let path = expand(&self.file);
        let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, self.scene.to_json()));
        self.notice = Some(match written {
            Ok(()) => format!("Saved {} throws to {}.", self.scene.throws.len(), path.display()),
            Err(e) => format!("Could not save {}: {e}", path.display()),
        });
    }

    /// Reads the scene file at `path` (or the path in the shell): the scene, or why not.
    pub fn read(&mut self, path: Option<&std::path::Path>) -> Option<Scene> {
        let path = path.map_or_else(|| expand(&self.file), std::path::Path::to_path_buf);
        let read = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| Scene::parse(&text).map_err(|e| e.message));
        match read {
            Ok(scene) => {
                self.notice = Some(format!("Opened {} ({} throws on {}).", path.display(), scene.throws.len(), scene.map));
                self.file = path.display().to_string();
                Some(scene)
            }
            Err(e) => {
                self.notice = Some(format!("Could not open {}: {e}", path.display()));
                None
            }
        }
    }
}

/// Where a map's scene is saved unless another path is typed: `~/nades-scenes/<map>.json`.
fn default_file(map: &str) -> String {
    format!("~/nades-scenes/{map}.json")
}

fn expand(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None => std::path::PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simulated(state: &mut SceneState) {
        state.simulate(&SharedCollider::ready(None), DVec3::Z, None);
        let started = std::time::Instant::now();
        while !state.ready() && started.elapsed().as_secs() < 10 {
            state.receive();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(state.ready());
    }

    #[test]
    fn the_tool_throws_from_where_the_camera_stands_and_looks() {
        let camera = CsView { eye: DVec3::new(100.0, 200.0, 64.0), pitch: -95.0, yaw: 45.0, fov: 73.7 };
        let t = Tool { crouch: true, grenade: Grenade::Flash, ..Tool::default() }.throw_from(&camera);
        assert_eq!((t.feet, t.pitch, t.yaw, t.grenade, t.crouch), ([100.0, 200.0, 18.0], -89.0, 45.0, Grenade::Flash, true));
        assert_eq!(Tool::default().throw_from(&camera).feet, [100.0, 200.0, 0.0]);
    }

    #[test]
    fn throws_are_placed_simulated_edited_and_kept_per_map() {
        let mut s = SceneState::new("de_cache", []);
        let he = SceneThrow::new(Grenade::He, [0.0, 0.0, 0.0], -10.0, 0.0);
        assert_eq!(s.add(he.clone()), Some(0));
        s.add(SceneThrow { start: 1.5, ..SceneThrow::new(Grenade::Flash, [0.0, 0.0, 0.0], -30.0, 90.0) });
        simulated(&mut s);
        assert_eq!(s.placed().len(), 2);
        assert!(s.end() > 1.5);
        // the view of the selected throw: the orbit opens over what it left, at its fullest
        s.show_view(ViewName::Orbit, Some(0));
        assert_eq!(s.selected, Some(0));
        assert!(s.camera(None).is_some() && s.orbit.is_some() && !s.playing);
        s.show_view(ViewName::Follow, None);
        assert!(s.playing && s.time == 0.0);
        s.advance(100.0);
        assert!(!s.playing && (s.time - s.end()).abs() < 1e-9);
        // duplicated, edited, deleted
        s.duplicate(0);
        assert_eq!((s.scene.throws.len(), s.selected), (3, Some(2)));
        s.edit(2, SceneThrow { crouch: true, ..he.clone() });
        assert!(s.scene.throws[2].crouch && !s.ready());
        s.delete(2);
        assert!(s.ready() && s.selected == Some(1));
        // another map's scene is its own, and comes back
        s.switch_map("de_mirage");
        assert!(s.scene.throws.is_empty() && s.scene.map == "de_mirage");
        s.switch_map("de_cache");
        assert_eq!(s.scene.throws.len(), 2);
        assert_eq!(s.all().len(), 1);
        let mut other = Scene::new("de_nuke");
        other.throws.push(he);
        s.set(other);
        assert_eq!((s.scene.map.as_str(), s.all().len()), ("de_cache", 2));
    }

    #[test]
    fn a_scene_file_is_saved_and_opened() {
        let dir = std::env::temp_dir().join(format!("nades-scene-{}", std::process::id()));
        let mut s = SceneState::new("de_cache", []);
        s.add(SceneThrow::new(Grenade::Smoke, [1.0, 2.0, 3.0], 0.0, 0.0));
        s.file = dir.join("a.json").display().to_string();
        s.save();
        let opened = s.read(None).unwrap();
        assert_eq!(opened, s.scene);
        std::fs::write(dir.join("b.json"), "{}").unwrap();
        assert!(s.read(Some(&dir.join("b.json"))).is_none() && s.notice.as_ref().unwrap().contains("Could not open"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
