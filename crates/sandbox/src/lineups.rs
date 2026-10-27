//! The lineup sandbox's state and what it asks of the threads: the map's lineups and filters, the selected lineup and
//! its built stage, the view, the moment shown and the orbit. The window (main.rs) draws it and feeds it input.
//!
//! Also the throws' paths outside the window: a throw's stage built once for `--render --throw`, and the cameras of a
//! list of throws for `--cameras` (bench/cameras.py compares them with the web view's).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use glam::DVec3;
use nades_control::SceneSummary;
use nades_model::{Grenade, Library as MapLibrary, Lineup, LineupId, LineupQuery, Revision, ThrowId, ThrowSource};
use nades_render::map::{FrameEffects, Map, PointLight};
use nades_render::smoke::SmokeFrame;
use nades_render::Gpu;
use nades_scene::camera::{Source, View as CsView};
use nades_scene::collider::Collider;
use nades_scene::stage::{Stage, ViewMode, build_stage};

use crate::library::{Filter, Library, Prepared, Reply, Request, SharedCollider, read_library, read_stage};
use crate::resident::{self, Resident};
use crate::shell::Action;
use crate::views::{self, Orbit};
use crate::{shell, state};

pub use crate::assets::packed_maps;

fn cs_view((eye, pitch, yaw, fov): state::Camera) -> CsView {
    CsView { eye: DVec3::from_array(eye), pitch, yaw, fov }
}

pub fn view_mode(name: &str) -> Option<ViewMode> {
    ViewMode::ALL.into_iter().find(|v| v.name().eq_ignore_ascii_case(name))
}

/// The 3D view's size in pixels for a window of `window` points at `scale`: the window less the shell's panels.
pub fn view_size(window: (f64, f64), scale: f64) -> (u32, u32) {
    let w = (window.0 - f64::from(shell::LEFT) - f64::from(shell::RIGHT)).max(64.0);
    let h = (window.1 - f64::from(shell::TOP) - f64::from(shell::BOTTOM)).max(64.0);
    ((w * scale).round() as u32, (h * scale).round() as u32)
}

/// The light the map lends the throw: towards its sun, in Hammer units' axes.
pub fn toward_sun(map: &Map) -> DVec3 {
    let s = map.toward_sun();
    DVec3::new(s.x.into(), s.y.into(), s.z.into())
}

/// A throw's facts, tracks and viewpoints, read once, and its stage built against the map's collider.
pub fn prepare(source: &dyn ThrowSource, map: &str, throw: &str, toward_sun: DVec3) -> Result<Prepared> {
    let library = read_library(source, map)?;
    let collider = crate::assets::collider(map).map_err(|e| eprintln!("collider: {e:#}")).ok();
    Prepared::read(source, &library, &ThrowId(throw.into()), collider.as_ref(), toward_sun)?.with_context(|| format!("no throw {throw} on {map}"))
}

/// The map a throw is on.
pub fn map_of(source: &dyn ThrowSource, throw: &str) -> Result<String> {
    Ok(source.map_of(&ThrowId(throw.into()))?.with_context(|| format!("no throw {throw}"))?.0)
}

/// What a frame draws of `stage` at `t`: its smoke (when it shows) and its light on the map.
pub fn frame_effects(stage: Option<&Stage>, t: f64, accumulated: u32) -> FrameEffects {
    let Some(stage) = stage else { return FrameEffects::default() };
    let smoke = stage.cloud.as_ref().filter(|_| t > 0.0).map(|cloud| SmokeFrame { seconds: t as f32, fade: cloud.fade_at(t) as f32, accumulated });
    let light = stage.light_at(t).map(|l| PointLight {
        at: glam::Vec3::new(l.at.x as f32, l.at.y as f32, l.at.z as f32),
        color: l.color.map(|c| c as f32),
        intensity: l.intensity as f32,
        distance: l.distance as f32,
        decay: l.decay as f32,
    });
    FrameEffects { smoke, light, accumulated }
}

/// Shows `prepared` on `map`: its flight, fire and bursts, its smoke's volume.
pub fn show_stage(gpu: &Gpu, map: &mut Map, prepared: Option<&Prepared>) {
    map.throw.set_stage(&gpu.device, prepared.map(|p| &p.stage));
    let cloud = prepared.and_then(|p| Some((p.stage.cloud.as_ref()?, p.volume.as_deref()?)));
    map.composite.set_cloud(&gpu.queue, cloud);
}

/// For each `{"throw": id}` in `cases` (a JSON list, with its `"map"` when known), the cameras its views open with,
/// one JSON line each.
pub fn print_cameras(source: &dyn ThrowSource, cases: &Path) -> Result<()> {
    let list: serde_json::Value = serde_json::from_slice(&std::fs::read(cases).with_context(|| format!("reading {}", cases.display()))?)?;
    let mut libraries: HashMap<String, Arc<MapLibrary>> = HashMap::new();
    let mut colliders: HashMap<String, Option<Collider>> = HashMap::new();
    for case in list.as_array().context("a list of cases")? {
        let Some(id) = case.get("throw").and_then(|t| t.as_str()) else { continue };
        let map = match case.get("map").and_then(|m| m.as_str()) {
            Some(map) => map.to_string(),
            None => match source.map_of(&ThrowId(id.into()))? {
                Some(map) => map.0,
                None => continue,
            },
        };
        let library = match libraries.get(&map) {
            Some(library) => library.clone(),
            None => {
                let library = read_library(source, &map)?;
                libraries.insert(map.clone(), library.clone());
                library
            }
        };
        let Some(facts) = read_stage(source, &library, &ThrowId(id.into()))? else { continue };
        let collider = colliders.entry(map.clone()).or_insert_with(|| crate::assets::collider(&map).map_err(|e| eprintln!("collider: {e:#}")).ok());
        let started = Instant::now();
        let stage = build_stage(&facts.throw, &facts.tracks, &facts.derived);
        let viewers: Vec<nades_scene::camera::Viewer> = facts.viewpoints.iter().map(nades_scene::camera::Viewer::from).collect();
        let result = stage.result_camera(&viewers, collider.as_ref());
        let ms = started.elapsed().as_secs_f64() * 1e3;
        let camera = |v: &CsView| serde_json::json!({ "eye": [v.eye.x, v.eye.y, v.eye.z], "pitch": v.pitch, "yaw": v.yaw, "fov": v.fov });
        let (kind, player, tick) = match &result.source {
            Source::Opponent { player, tick } => ("opponent", player.clone(), Some(*tick)),
            Source::Victim { .. } => ("victim", None, None),
            Source::Overview => ("overview", None, None),
        };
        let line = serde_json::json!({
            "throw": facts.throw.id.0,
            "map": map,
            "nade": facts.throw.grenade.name(),
            "lineup": camera(&stage.lineup),
            "aim": camera(&stage.aim),
            "result": { "eye": [result.eye.x, result.eye.y, result.eye.z], "pitch": result.pitch, "yaw": result.yaw, "fov": nades_scene::camera::vertical_fov(), "kind": kind, "player": player, "tick": tick, "seen": result.seen },
            "orbit": camera(&views::camera(ViewMode::Orbit, &stage, &result, None)),
            "focus": [stage.focus.x, stage.focus.y, stage.focus.z],
            "result_time": stage.result_time,
            "duration": stage.duration,
            "viewers": viewers.len(),
            "missing": stage.missing,
            "collider": collider.is_some(),
            "ms": ms,
        });
        println!("{line}");
    }
    Ok(())
}

/// What the window does for an action the sandbox cannot do itself.
pub enum Effect {
    LoadMap(String),
    Copy(String),
}

/// The lineups, the selection and the moment shown: everything but the window and the GPU.
pub struct Sandbox {
    pub library: Library,
    pub maps: Vec<String>,
    pub filter: Filter,
    pub lineups: Vec<Lineup>,
    /// The shown map's facts and rules the list came from (the details of its lineups' throws).
    pub facts: Option<Arc<MapLibrary>>,
    pub teams: Vec<String>,
    pub players: Vec<String>,
    pub search: String,
    /// The selected lineup's id.
    pub selected: Option<LineupId>,
    /// The throw the last run showed: its lineup is selected when the remembered id is not listed.
    pub remembered_throw: Option<ThrowId>,
    /// Built stages, by throw id.
    pub prepared: HashMap<ThrowId, Arc<Prepared>>,
    pub view: ViewMode,
    pub time: f64,
    pub playing: bool,
    pub orbit: Option<Orbit>,
    pub collider: SharedCollider,
    pub toward_sun: DVec3,
    pub notice: Option<String>,
    /// Why there is nothing to list, when the data says so (no files, a file that does not open).
    pub data_notice: Option<String>,
    pub list_ms: Option<f64>,
    /// The throw whose stage is on the GPU.
    pub shown: Option<ThrowId>,
    /// The camera the last run left, until a stage gives one.
    pub remembered: Option<CsView>,
    /// Each map's last camera, as remembered (state.rs).
    pub cameras: std::collections::BTreeMap<String, state::Camera>,
    pub copied_at: Option<Instant>,
    pub first_frame_ms: Option<f64>,
    /// The data's revision the lists were read at, and when it was last looked at.
    pub revision: Option<Revision>,
    pub data_checked: Instant,
    /// The lineups of the maps kept on the GPU beside the shown one, by map.
    pub kept: HashMap<String, MapLineups>,
}

/// A map's lineups kept while another map is shown (its GPU map kept in [`Resident`]): its list for the filters, the
/// selection, the stages built, its collider and the light it lends a throw. Picking the map puts them back as they were.
pub struct MapLineups {
    pub filter: Filter,
    pub lineups: Vec<Lineup>,
    pub facts: Option<Arc<MapLibrary>>,
    pub teams: Vec<String>,
    pub players: Vec<String>,
    pub selected: Option<LineupId>,
    pub prepared: HashMap<ThrowId, Arc<Prepared>>,
    pub collider: SharedCollider,
    pub toward_sun: DVec3,
    /// When its list for `filter` was read; None while it is being read again.
    pub list_ms: Option<f64>,
}

impl MapLineups {
    pub fn selected_lineup(&self) -> Option<&Lineup> {
        let id = self.selected?;
        self.lineups.iter().find(|l| l.id == id)
    }

    /// Whether picking the map shows a lineup at once: its list read, its selected lineup's stage built (or no lineups).
    pub fn ready(&self) -> bool {
        self.list_ms.is_some() && (self.lineups.is_empty() || self.selected_lineup().is_some_and(|l| self.prepared.contains_key(&l.example)))
    }
}

/// The filters a map carries to another: side, grenade, round time, plant, tag and fewest throws, but not its team or
/// player.
fn carried(filter: &Filter, to: &str) -> Filter {
    Filter { map: to.to_string(), query: LineupQuery { team: None, player: None, ..filter.query.clone() } }
}

impl Sandbox {
    pub fn start(map: &str, remembered: Option<state::Remembered>, library: Library) -> Self {
        let view = remembered.as_ref().and_then(|r| r.view.as_deref()).and_then(view_mode).unwrap_or(ViewMode::Lineup);
        let camera = remembered.as_ref().and_then(|r| r.camera_on(map)).map(cs_view);
        let cameras = remembered.as_ref().map(|r| r.cameras.clone()).unwrap_or_default();
        let revision = library.source.revision().ok();
        let sandbox = Self {
            library,
            maps: packed_maps(),
            filter: Filter::map(map),
            lineups: Vec::new(),
            facts: None,
            teams: Vec::new(),
            players: Vec::new(),
            search: String::new(),
            selected: remembered.as_ref().and_then(|r| r.lineup.as_deref()).and_then(LineupId::parse),
            remembered_throw: remembered.as_ref().and_then(|r| r.throw.clone()).map(ThrowId),
            prepared: HashMap::new(),
            view,
            time: 0.0,
            playing: false,
            orbit: None,
            collider: SharedCollider::default(),
            toward_sun: DVec3::Z,
            notice: None,
            data_notice: None,
            list_ms: None,
            shown: None,
            remembered: camera,
            cameras,
            copied_at: None,
            first_frame_ms: None,
            revision,
            data_checked: Instant::now(),
            kept: HashMap::new(),
        };
        // the list is read once the first frame is on screen (Lineups::after_first_frame): nothing on its way competes
        sandbox
    }

    pub fn selected_lineup(&self) -> Option<&Lineup> {
        let id = self.selected?;
        self.lineups.iter().find(|l| l.id == id)
    }

    pub fn selected_prepared(&self) -> Option<&Arc<Prepared>> {
        self.prepared.get(&self.selected_lineup()?.example)
    }

    /// The selected stage when it is the one on the GPU.
    pub fn shown_prepared(&self) -> Option<&Arc<Prepared>> {
        self.selected_prepared().filter(|p| self.shown.as_ref() == Some(&p.throw.id))
    }

    /// Asks for the listed lineups' stages not built yet, the selected one first, then outward from it.
    pub fn prepare_listed(&self) {
        let at = self.selected.and_then(|id| self.lineups.iter().position(|l| l.id == id)).unwrap_or(0);
        let mut order: Vec<usize> = (0..self.lineups.len()).collect();
        order.sort_by_key(|&k| k.abs_diff(at));
        let mut throws: Vec<ThrowId> = order.into_iter().map(|k| self.lineups[k].example.clone()).filter(|t| !self.prepared.contains_key(t)).collect();
        // the selected lineup and its neighbours now; the rest after what keeps the other maps warm
        let rest = throws.split_off(throws.len().min(8));
        if !throws.is_empty() {
            self.library.send(Request::Prepare { map: self.filter.map.clone(), throws, collider: self.collider.clone(), toward_sun: self.toward_sun });
        }
        if !rest.is_empty() {
            self.library.send_bulk(Request::Prepare { map: self.filter.map.clone(), throws: rest, collider: self.collider.clone(), toward_sun: self.toward_sun });
        }
    }

    pub fn refilter(&mut self, change: impl FnOnce(&mut Filter)) {
        change(&mut self.filter);
        self.notice = None;
        self.library.send(Request::List(self.filter.clone()));
        self.rewarm(false);
    }

    /// The kept maps follow the filters the shown map carries (or, `again`, are all read again): their lists and
    /// selected stages, in the background.
    fn rewarm(&mut self, again: bool) {
        for (map, kept) in &mut self.kept {
            let wanted = carried(&self.filter, map);
            if again || kept.filter != wanted {
                if again {
                    kept.prepared.clear();
                }
                kept.filter = wanted.clone();
                kept.list_ms = None;
                self.library.send_background(Request::List(wanted));
            }
        }
    }

    /// Keeps `map`'s lineups ready beside the shown map's: its list for the carried filters and its selected lineup's
    /// stage, read and built in the background. `collider` is used when the map has none kept yet.
    pub fn keep_warm(&mut self, map: &str, toward_sun: DVec3, collider: impl FnOnce() -> SharedCollider) {
        if map == self.filter.map {
            return;
        }
        let wanted = carried(&self.filter, map);
        let kept = self.kept.entry(map.to_string()).or_insert_with(|| MapLineups { filter: wanted.clone(), lineups: Vec::new(), facts: None, teams: Vec::new(), players: Vec::new(), selected: None, prepared: HashMap::new(), collider: collider(), toward_sun, list_ms: None });
        kept.toward_sun = toward_sun;
        if kept.list_ms.is_none() || kept.filter != wanted {
            kept.filter = wanted.clone();
            kept.list_ms = None;
            self.library.send_background(Request::List(wanted));
        }
    }

    /// Shows `to`'s lineups: the shown map's are kept with it (list, selection, stages, collider) for when it is picked
    /// again, and `to`'s come back as they were kept, or are read. True when `to`'s were kept (its collider with them).
    pub fn switch_map(&mut self, to: &str, toward_sun: DVec3) -> bool {
        self.note_camera();
        let wanted = carried(&self.filter, to);
        let mine = MapLineups {
            filter: std::mem::replace(&mut self.filter, wanted.clone()),
            lineups: std::mem::take(&mut self.lineups),
            facts: self.facts.take(),
            teams: std::mem::take(&mut self.teams),
            players: std::mem::take(&mut self.players),
            selected: self.selected.take(),
            prepared: std::mem::take(&mut self.prepared),
            collider: std::mem::take(&mut self.collider),
            toward_sun: self.toward_sun,
            list_ms: self.list_ms.take(),
        };
        self.kept.insert(mine.filter.map.clone(), mine);
        self.toward_sun = toward_sun;
        self.shown = None;
        self.notice = None;
        self.remembered = self.cameras.get(to).copied().map(cs_view);
        let Some(kept) = self.kept.remove(to) else {
            self.library.send(Request::List(self.filter.clone()));
            return false;
        };
        let current = kept.list_ms.is_some() && kept.filter == wanted;
        (self.lineups, self.facts, self.teams, self.players, self.selected, self.prepared, self.collider) = (kept.lineups, kept.facts, kept.teams, kept.players, kept.selected, kept.prepared, kept.collider);
        if current {
            self.list_ms = kept.list_ms;
            self.prepare_listed();
        } else {
            // read again for the filters now; until then its last list stays up
            self.library.send(Request::List(self.filter.clone()));
        }
        true
    }

    /// Takes the data thread's replies; true when something shown changed.
    pub fn receive(&mut self) -> bool {
        let mut changed = false;
        while let Ok(reply) = self.library.replies.try_recv() {
            changed = true;
            match reply {
                Reply::List { filter, library, lineups, teams, players, ms } => {
                    if filter != self.filter {
                        // a kept map's: its list, then its selected lineup's stage in the background
                        if let Some(kept) = self.kept.get_mut(&filter.map)
                            && kept.filter == filter
                        {
                            (kept.lineups, kept.facts, kept.teams, kept.players, kept.list_ms) = (lineups, Some(library), teams, players, Some(ms));
                            if kept.selected_lineup().is_none() {
                                kept.selected = kept.lineups.first().map(|l| l.id);
                            }
                            if let Some(throw) = kept.selected_lineup().map(|l| l.example.clone()).filter(|t| !kept.prepared.contains_key(t)) {
                                self.library.send_background(Request::Prepare { map: filter.map.clone(), throws: vec![throw], collider: kept.collider.clone(), toward_sun: kept.toward_sun });
                            }
                        }
                        continue;
                    }
                    self.lineups = lineups;
                    self.teams = teams;
                    self.players = players;
                    self.list_ms = Some(ms);
                    if self.selected_lineup().is_none() {
                        // the lineup of the throw the last run showed, if it is listed; else the first
                        let remembered = self.remembered_throw.take().and_then(|t| library.derived(&t)?.lineup).filter(|id| self.lineups.iter().any(|l| l.id == *id));
                        self.selected = remembered.or_else(|| self.lineups.first().map(|l| l.id));
                    }
                    self.facts = Some(library);
                    self.notice = self.lineups.is_empty().then(|| self.data_notice.clone().unwrap_or_else(|| "No lineups match these filters.".into()));
                    self.prepare_listed();
                }
                Reply::Prepared { map, prepared } => {
                    if map == self.filter.map {
                        self.prepared.insert(prepared.throw.id.clone(), prepared);
                    } else if let Some(kept) = self.kept.get_mut(&map) {
                        kept.prepared.insert(prepared.throw.id.clone(), prepared);
                    }
                }
                Reply::Failed(why) => self.notice = Some(why),
            }
        }
        changed
    }

    pub fn select(&mut self, lineup: LineupId) {
        if self.selected == Some(lineup) {
            return;
        }
        self.selected = Some(lineup);
        // not built yet: it goes first
        if let Some(l) = self.selected_lineup()
            && !self.prepared.contains_key(&l.example)
        {
            self.library.send(Request::Prepare { map: self.filter.map.clone(), throws: vec![l.example.clone()], collider: self.collider.clone(), toward_sun: self.toward_sun });
        }
    }

    pub fn step(&mut self, by: i32) {
        if self.lineups.is_empty() {
            return;
        }
        let at = self.selected.and_then(|id| self.lineups.iter().position(|l| l.id == id));
        let next = match at {
            Some(k) => (k as i64 + i64::from(by)).rem_euclid(self.lineups.len() as i64) as usize,
            None => 0,
        };
        self.select(self.lineups[next].id);
    }

    /// The camera now: the view's, from the shown stage; or the one the last run left.
    pub fn camera(&self) -> Option<CsView> {
        match self.shown_prepared() {
            Some(p) => Some(views::camera(self.view, &p.stage, &p.result, self.orbit.as_ref())),
            None => self.remembered,
        }
    }

    /// Notes the camera shown on this map, for the next time it opens.
    pub fn note_camera(&mut self) {
        if let Some(c) = self.camera() {
            self.cameras.insert(self.filter.map.clone(), ([c.eye.x, c.eye.y, c.eye.z], c.pitch, c.yaw, c.fov));
        }
    }

    /// Writes what to reopen on.
    pub fn remember(&mut self) {
        self.note_camera();
        let lineup = self.selected_lineup();
        state::save(&state::Remembered {
            map: self.filter.map.clone(),
            lineup: self.selected.map(|id| id.to_string()),
            throw: lineup.map(|l| l.example.0.clone()),
            view: Some(self.view.name().to_lowercase()),
            camera: self.camera().map(|c| ([c.eye.x, c.eye.y, c.eye.z], c.pitch, c.yaw, c.fov)),
            cameras: self.cameras.clone(),
        });
    }

    /// Applies what the shell asked for; what only the window can do comes back.
    pub fn apply(&mut self, action: Action) -> Option<Effect> {
        match action {
            Action::Map(map) => return Some(Effect::LoadMap(map)),
            Action::Side(side) => self.refilter(|f| f.query.side = side),
            Action::Grenade(grenade) => self.refilter(|f| f.query.grenade = grenade),
            Action::Team(team) => self.refilter(|f| {
                f.query.team = team;
                f.query.player = None;
            }),
            Action::Player(player) => self.refilter(|f| f.query.player = player),
            Action::RoundTime(range) => self.refilter(|f| f.query.round_seconds = range),
            Action::Plant(after) => self.refilter(|f| f.query.after_plant = after),
            Action::MinThrows(min) => self.refilter(|f| f.query.min_throws = min),
            Action::Select(lineup) => self.select(lineup),
            Action::Step(by) => self.step(by),
            Action::View(view) => {
                self.view = view;
                if let Some(p) = self.shown_prepared().cloned() {
                    self.open(&p);
                }
            }
            Action::Time(t) => {
                self.time = t;
                self.playing = false;
            }
            Action::Play(on) => {
                self.playing = on;
                if on && let Some(p) = self.shown_prepared() {
                    let (from, to) = p.stage.timeline();
                    if self.time >= to - 1e-3 {
                        self.time = from;
                    }
                }
            }
            Action::Copy(text) => {
                self.copied_at = Some(Instant::now());
                return Some(Effect::Copy(text));
            }
        }
        None
    }

    /// Opens the view on `p`: its moment, and for the orbit, its first pose (the thrower's view), playing.
    pub fn open(&mut self, p: &Prepared) {
        self.time = p.stage.opens_at(self.view);
        self.orbit = (self.view == ViewMode::Orbit).then(|| Orbit::new(p.stage.focus, p.stage.lineup));
        self.playing = self.view == ViewMode::Orbit;
    }

    /// Plays the scrubber on by `dt` seconds, stopping at the end.
    pub fn advance(&mut self, dt: f64) {
        let Some(end) = self.shown_prepared().map(|p| p.stage.timeline().1) else { return };
        if self.playing {
            self.time = (self.time + dt).min(end);
            if self.time >= end {
                self.playing = false;
            }
        }
    }

    /// What the app-control layer reports of the scene; `loading` when a map is on its way.
    pub fn summary(&self, free: bool, loading: bool) -> SceneSummary {
        let shown = self.shown_prepared();
        let mut effects = Vec::new();
        if let Some(p) = shown {
            let t = self.time;
            if p.stage.flight.is_some() && matches!(self.view, ViewMode::Result | ViewMode::Orbit) {
                effects.push("flight".to_string());
            }
            if p.stage.cloud.is_some() && t > 0.0 {
                effects.push("smoke".into());
            }
            if p.stage.fire.as_ref().is_some_and(|f| t >= 0.0 && t < f.lasts()) {
                effects.push("fire".into());
            }
            if p.stage.burst.is_some_and(|b| b.on(t) > 0.0) {
                effects.push(if p.stage.grenade == Grenade::Flash { "flash".into() } else { "he".into() });
            }
        }
        SceneSummary {
            camera_mode: if free || shown.is_none() { "fly".into() } else { self.view.name().to_lowercase() },
            // a map on its way, the list not read yet, or a lineup picked whose stage is not built yet (unless a notice
            // says why nothing comes)
            loading: loading || (self.notice.is_none() && (self.list_ms.is_none() || (self.selected.is_some() && shown.is_none()))),
            notice: self.notice.clone(),
            lineup_id: self.selected.map(|id| id.to_string()),
            throw_id: shown.map(|p| p.throw.id.0.clone()),
            timeline_seconds: shown.map(|_| self.time),
            playing: self.playing,
            active_effects: effects,
        }
    }

    /// Whether frames must keep coming: a playing scrubber, a copy note fading.
    pub fn animating(&self) -> bool {
        self.playing || self.copied_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2))
    }

    /// Notices new data (the source's revision: a file written again): the list and the stages are read again.
    pub fn check_data(&mut self) {
        if self.data_checked.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.data_checked = Instant::now();
        let revision = self.library.source.revision().ok();
        if revision != self.revision {
            self.revision = revision;
            self.prepared.clear();
            self.shown = None;
            self.refilter(|_| {});
            self.rewarm(true);
        }
    }
}

/// A map being loaded for the map selector: its name, and the thread's answer.
type MapLoad = (String, std::sync::mpsc::Receiver<Result<Map>>);

/// What the window adds for the lineups: the sandbox's state, the 3D view's place between the shell's panels, the
/// game's effect files, the maps kept on the GPU beside the shown one, a map being loaded, and how many jittered
/// frames the shown picture has averaged.
pub struct Lineups {
    pub sandbox: Sandbox,
    /// The 3D view's rectangle in points.
    pub rect: egui::Rect,
    actions: Vec<Action>,
    /// The game's smoke noise and fire flipbooks on the GPU, made once and given to every map.
    effects: Option<(wgpu::Texture, Option<nades_render::throw::GpuBooks>)>,
    effects_files: Option<std::sync::mpsc::Receiver<(Option<nades_render::throw::Flipbooks>, Vec<u8>)>>,
    /// The other maps on the GPU, and the one being opened in the background (resident.rs).
    pub resident: Resident,
    /// Whether the other maps are still to be opened in the background, once the first frame is on screen (not in
    /// benches of one map).
    pub preload: bool,
    /// What the shown map holds on the GPU.
    pub shown_bytes: u64,
    /// A map picked while it was being opened in the background: shown when it arrives.
    picked: Option<String>,
    /// Where the last frame's time went on the window's thread (ms by part), for the maps bench.
    pub frame_parts: Vec<(&'static str, f64)>,
    /// Kept maps drawn once from their lineup's view, with its stage.
    warmed: std::collections::HashSet<String>,
    /// A map picked that was not on the GPU, loaded for the pick.
    loading: Option<MapLoad>,
    /// A map loaded for the selector, held back until its first lineup's stage is built (or a few seconds pass), so it
    /// opens on that lineup rather than on the last map's camera.
    pending: Option<(String, Map, Instant)>,
    last_key: Option<[u32; 9]>,
    pub accumulated: u32,
    dragging: Option<egui::Pos2>,
    /// Bench and screenshot runs draw from the bench camera, as M2's did.
    pub bench: bool,
    /// The camera was taken by hand (the fly keys) or by app control: no lineup view overrides it until one is picked.
    pub free: bool,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Lineups {
    pub fn new(sandbox: Sandbox, bench: bool, rect: egui::Rect) -> Self {
        Self { sandbox, rect, actions: Vec::new(), effects: None, effects_files: None, resident: Resident::new(0), preload: !bench, shown_bytes: 0, picked: None, frame_parts: Vec::new(), warmed: Default::default(), loading: None, pending: None, last_key: None, accumulated: 0, dragging: None, bench, free: false, wake: None }
    }

    /// After the first frame: the map's collider, and the game's flipbooks and smoke noise, on threads of their own.
    pub fn after_first_frame(&mut self, map: &Map, name: &str, wake: Arc<dyn Fn() + Send + Sync>) {
        self.wake = Some(wake.clone());
        self.shown_bytes = map.gpu_bytes;
        self.sandbox.library.send(Request::List(self.sandbox.filter.clone()));
        self.build_collider(name);
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("effects".into())
            .spawn(move || {
                let (noise, _, books) = crate::assets::effects();
                let _ = send.send((books, noise));
                wake();
            })
            .expect("an effects thread");
        self.effects_files = Some(receive);
    }

    /// Once the first frame is on screen (presented, and the window shown): the other packed maps, opened in the
    /// background one at a time, the ones looked at before first. Once only, and not in runs that time one map.
    pub fn open_other_maps(&mut self, gpu: &Gpu, name: &str) {
        if !self.preload || self.wake.is_none() {
            return;
        }
        self.preload = false;
        self.resident.cameras = self.sandbox.cameras.clone();
        let looked_at: Vec<String> = self.sandbox.cameras.keys().cloned().collect();
        self.resident.preload(looked_at.into_iter().chain(self.sandbox.maps.iter().cloned()).filter(|m| m != name));
        self.resident.next(gpu, name, self.shown_bytes, self.wake.clone());
    }

    fn build_collider(&mut self, map: &str) {
        let Some(wake) = self.wake.clone() else { return };
        self.sandbox.collider = SharedCollider::build(map.to_string(), move || wake());
        // stages asked for before the collider existed are asked for again with it
        self.sandbox.prepare_listed();
    }

    /// Takes what the threads have finished; true when the picture may change.
    pub fn receive(&mut self, gpu: &Gpu, map: &mut Map, name: &mut String) -> bool {
        let mut changed = self.sandbox.receive();
        self.warm_kept(gpu);
        if let Some(files) = &self.effects_files
            && let Ok((books, noise)) = files.try_recv()
        {
            self.effects_files = None;
            let noise = nades_render::smoke::noise_texture(&gpu.device, &gpu.queue, &noise);
            self.effects = Some((noise, books.map(|b| b.upload(&gpu.device, &gpu.queue))));
            self.install_effects(gpu, map);
            changed = true;
        }
        if let Some((loading, answer)) = &self.loading
            && let Ok(result) = answer.try_recv()
        {
            let loading = loading.clone();
            self.loading = None;
            match result {
                // its lineups and collider first; the map itself once its first stage is built
                Ok(new) => {
                    if !self.sandbox.switch_map(&loading, toward_sun(&new)) {
                        self.build_collider(&loading);
                    }
                    self.pending = Some((loading, new, Instant::now()));
                }
                Err(e) => self.sandbox.notice = Some(format!("{loading}: {e:#}")),
            }
            changed = true;
        }
        if let Some((_, _, since)) = &self.pending {
            let ready = self.sandbox.selected_prepared().is_some() || (self.sandbox.list_ms.is_some() && self.sandbox.lineups.is_empty());
            if ready || since.elapsed() > Duration::from_secs(5) {
                let (loading, new, _) = self.pending.take().expect("pending");
                let old = std::mem::replace(map, new);
                let from = std::mem::replace(name, loading);
                self.keep(from, old, map.gpu_bytes);
                self.switch_to(gpu, map);
                changed = true;
            }
        }
        // a map opened in the background: kept on the GPU, its lineups warmed; then the next one
        if let Some((opened, result, ms)) = self.resident.arrived() {
            match result {
                Ok(new) => {
                    eprintln!("resident: {opened} on the GPU in {ms:.0} ms, {:.0} MB", nades_render::memory::mb(new.gpu_bytes));
                    let sun = toward_sun(&new);
                    self.keep(opened.clone(), new, map.gpu_bytes);
                    if self.resident.contains(&opened) {
                        let (wake, warming) = (self.wake.clone(), opened.clone());
                        self.sandbox.keep_warm(&opened, sun, move || {
                            SharedCollider::build(warming, move || {
                                if let Some(wake) = &wake {
                                    wake();
                                }
                            })
                        });
                    }
                    if self.picked.as_deref() == Some(opened.as_str()) {
                        self.picked = None;
                        self.switch_resident(gpu, map, name, &opened);
                    }
                }
                Err(e) => {
                    eprintln!("resident: {opened}: {e:#}");
                    if self.picked.as_deref() == Some(opened.as_str()) {
                        self.picked = None;
                        self.sandbox.notice = Some(format!("{opened}: {e:#}"));
                    }
                }
            }
            self.resident.next(gpu, name, map.gpu_bytes, self.wake.clone());
            changed = true;
        }
        changed
    }

    /// Draws a kept map once, offscreen, from the view its lineup opens with and with that lineup's stage, once the stage
    /// is built (and the game's effects are in): its first visit then costs a later visit's frame, not 13.5 ms of the
    /// first use of what that view shows (S21). One map a call, so that the shown map's frames take them one at a time;
    /// submitted, not waited for.
    fn warm_kept(&mut self, gpu: &Gpu) {
        if self.effects.is_none() {
            return;
        }
        for name in self.resident.names() {
            let Some(kept) = self.sandbox.kept.get(&name).filter(|_| !self.warmed.contains(&name)) else { continue };
            let Some(prepared) = kept.selected_lineup().and_then(|l| kept.prepared.get(&l.example)).cloned() else { continue };
            let Some(map) = self.resident.get_mut(&name) else { continue };
            if let Some((noise, books)) = &self.effects {
                map.composite.set_noise(&gpu.device, noise);
                if let Some(books) = books {
                    map.throw.set_flipbooks(&gpu.device, books);
                }
            }
            show_stage(gpu, map, Some(&prepared));
            let view = self.sandbox.view;
            let eye = views::of(&views::camera(view, &prepared.stage, &prepared.result, None));
            let t = prepared.stage.opens_at(view);
            map.throw.flight_visible = matches!(view, ViewMode::Result | ViewMode::Orbit);
            let size = view_size(crate::WINDOW, crate::EXPECTED_SCALE);
            map.throw.set_time(&gpu.device, &gpu.queue, Some(&prepared.stage), t, &eye, size);
            resident::draw_offscreen(&gpu.device, &gpu.queue, map, &eye, &frame_effects(Some(&prepared.stage), t, 0), 1, false);
            self.warmed.insert(name);
            return;
        }
    }

    /// Keeps a map no longer shown on the GPU, and lets go of the lineups of any map that had to go to make room.
    fn keep(&mut self, name: String, map: Map, shown: u64) {
        for gone in self.resident.keep(name, map, shown) {
            self.sandbox.kept.remove(&gone);
            self.warmed.remove(&gone);
        }
    }

    /// Shows `to`, kept on the GPU: a swap of maps and of their lineups, drawn in the frame that asked for it. False
    /// when `to` is not kept.
    pub fn switch_resident(&mut self, gpu: &Gpu, map: &mut Map, name: &mut String, to: &str) -> bool {
        let Some(new) = self.resident.take(to) else { return false };
        let old = std::mem::replace(map, new);
        let from = std::mem::replace(name, to.to_string());
        if !self.sandbox.switch_map(to, toward_sun(map)) {
            self.build_collider(to);
        }
        // a pick of another map, still on its way, is passed over
        self.loading = None;
        self.pending = None;
        self.keep(from, old, map.gpu_bytes);
        self.switch_to(gpu, map);
        true
    }

    /// Gives `map` the game's effects, on the GPU once for every map.
    fn install_effects(&self, gpu: &Gpu, map: &mut Map) {
        if let Some((noise, books)) = &self.effects {
            map.composite.set_noise(&gpu.device, noise);
            if let Some(books) = books {
                map.throw.set_flipbooks(&gpu.device, books);
            }
        }
    }

    /// What the app-control layer reports of the scene.
    pub fn summary(&self) -> SceneSummary {
        self.sandbox.summary(self.free, self.loading.is_some() || self.pending.is_some() || self.picked.is_some())
    }

    /// Whether everything the switch bench needs is in: the game's effects, and the built stages of the `n + 1` lineups
    /// it shows, the selected one and the `n` after it (switching to one not built would ask for its stage then).
    pub fn ready_for(&self, n: usize) -> bool {
        let s = &self.sandbox;
        let at = s.selected.and_then(|id| s.lineups.iter().position(|l| l.id == id)).unwrap_or(0);
        self.effects_files.is_none() && self.wake.is_some() && !s.lineups.is_empty() && s.lineups.iter().cycle().skip(at).take(n + 1).all(|l| s.prepared.contains_key(&l.example))
    }

    /// Picks `name`: shown in this frame when it is on the GPU; else shown when its background open arrives, or loaded
    /// on a thread for the pick, the map shown staying until it is ready.
    fn pick_map(&mut self, gpu: &Gpu, map: &mut Map, shown: &mut String, name: String) {
        if self.switch_resident(gpu, map, shown, &name) {
            return;
        }
        if self.resident.loading() == Some(name.as_str()) {
            self.picked = Some(name);
            return;
        }
        if self.loading.as_ref().is_some_and(|(n, _)| *n == name) {
            return;
        }
        // room for it first, so the maps on the GPU stay within the budget while it loads
        let path = crate::pack_path(Some(&name));
        for gone in self.resident.make_room(resident::expected_bytes(&path), map.gpu_bytes) {
            self.sandbox.kept.remove(&gone);
            self.warmed.remove(&gone);
        }
        let receive = resident::load(&gpu.device, &gpu.queue, path, self.sandbox.cameras.get(&name).copied(), self.wake.clone());
        self.loading = Some((name, receive));
    }

    /// The new map is on the GPU: the game's effects, and its lineup's stage.
    fn switch_to(&mut self, gpu: &Gpu, map: &mut Map) {
        self.install_effects(gpu, map);
        self.shown_bytes = map.gpu_bytes;
        self.sandbox.shown = None;
        self.last_key = None;
        self.free = false;
        self.sync_stage(gpu, map);
    }

    /// The shell, laid out over `ui`; what it asks for is applied by [`Lineups::update`].
    pub fn ui(&mut self, ui: &mut egui::Ui, timings: String) {
        let s = &mut self.sandbox;
        let shown = s.shown_prepared().cloned();
        let timeline = shown.as_ref().map_or((-1.0, 1.0), |p| p.stage.timeline());
        let loading = self.loading.as_ref().map(|(n, _)| n.clone()).or_else(|| self.pending.as_ref().map(|(n, _, _)| n.clone())).or_else(|| self.picked.clone());
        let map = s.filter.map.clone();
        let copied = s.copied_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2));
        let status = shell::Status {
            map: &map,
            maps: &s.maps,
            query: &s.filter.query,
            teams: &s.teams,
            players: &s.players,
            lineups: &s.lineups,
            facts: s.facts.as_deref(),
            search: &mut s.search,
            selected: s.selected,
            prepared: shown.as_deref(),
            view: s.view,
            time: s.time,
            timeline,
            playing: s.playing,
            notice: s.notice.as_deref(),
            timings,
            loading_map: loading.as_deref(),
            copied,
            free: self.free,
        };
        let (actions, rect) = shell::show(ui, status);
        self.actions.extend(actions);
        self.rect = rect;
    }

    /// Applies the shell's actions (a map on the GPU is shown in this frame), puts a newly built selected stage on the
    /// GPU, and plays the scrubber on by `dt`.
    pub fn update(&mut self, gpu: &Gpu, map: &mut Map, name: &mut String, ctx: &egui::Context, dt: f64) {
        for action in std::mem::take(&mut self.actions) {
            if matches!(action, Action::Select(_) | Action::Step(_) | Action::View(_)) {
                self.free = false;
            }
            match self.sandbox.apply(action) {
                Some(Effect::Copy(text)) => ctx.copy_text(text),
                Some(Effect::LoadMap(to)) => self.pick_map(gpu, map, name, to),
                None => {}
            }
        }
        self.sync_stage(gpu, map);
        self.sandbox.advance(dt);
    }

    /// Puts the selected lineup's stage on the GPU when it is built and not shown yet; true when it switched.
    pub fn sync_stage(&mut self, gpu: &Gpu, map: &mut Map) -> bool {
        // the stages are the next map's while it waits to be shown
        if self.pending.is_some() {
            return false;
        }
        let prepared = self.sandbox.selected_prepared().cloned();
        let id = prepared.as_ref().map(|p| p.throw.id.clone());
        // a lineup picked but not built yet keeps the last stage up until it is
        if id == self.sandbox.shown || (prepared.is_none() && self.sandbox.shown.is_some() && self.sandbox.selected.is_some()) {
            return false;
        }
        show_stage(gpu, map, prepared.as_deref());
        if let Some(p) = &prepared {
            self.sandbox.open(p);
            self.sandbox.remembered = None;
        }
        self.sandbox.shown = id;
        true
    }

    /// One step of the switch bench: the next lineup, its stage on the GPU; true when it switched.
    pub fn switch_next(&mut self, gpu: &Gpu, map: &mut Map) -> bool {
        self.free = false;
        self.sandbox.step(1);
        self.sync_stage(gpu, map)
    }

    /// The camera this frame draws from, unless the person or app control took it (`free`): the view's, or the one the
    /// last run left.
    pub fn camera(&self) -> Option<CsView> {
        if self.bench || self.free { None } else { self.sandbox.camera() }
    }

    /// Readies what the frame from `eye` into targets of `size` draws of the throw; None when the picture is already
    /// final (every jittered smoke frame averaged in) and nothing needs drawing.
    pub fn frame(&mut self, gpu: &Gpu, map: &mut Map, eye: &nades_render::View, size: (u32, u32), force: bool) -> Option<FrameEffects> {
        let s = &self.sandbox;
        let shown = s.shown_prepared().cloned();
        let stage = shown.as_ref().map(|p| &p.stage);
        let t = s.time;
        map.throw.flight_visible = matches!(s.view, ViewMode::Result | ViewMode::Orbit);
        // a still picture averages its smoke's jittered frames; anything that moves starts it over
        let q = eye.rotation;
        let key = [eye.eye.x.to_bits(), eye.eye.y.to_bits(), eye.eye.z.to_bits(), q.x.to_bits(), q.y.to_bits(), q.z.to_bits(), eye.fov_y.to_bits(), (t as f32).to_bits(), size.0 ^ (size.1 << 16) ^ s.shown.as_ref().map_or(0, |id| id.0.len() as u32 * 7919)];
        if self.last_key == Some(key) {
            self.accumulated = (self.accumulated + 1).min(nades_render::smoke::ACCUMULATE);
        } else {
            self.accumulated = 0;
            self.last_key = Some(key);
        }
        let smoke = stage.is_some_and(|st| st.cloud.is_some()) && t > 0.0;
        if !force && self.accumulated >= nades_render::smoke::ACCUMULATE && smoke {
            return None;
        }
        map.throw.set_time(&gpu.device, &gpu.queue, stage, t, eye, size);
        Some(frame_effects(stage, t, self.accumulated))
    }

    /// Whether frames must keep coming: a playing scrubber, a smoke still averaging, a copy note fading, a map loading.
    pub fn animating(&self) -> bool {
        let smoke = self.sandbox.shown_prepared().is_some_and(|p| p.stage.cloud.is_some()) && self.sandbox.time > 0.0;
        self.sandbox.animating() || (smoke && self.accumulated < nades_render::smoke::ACCUMULATE) || self.loading.is_some() || self.pending.is_some() || self.picked.is_some()
    }

    /// The orbit's mouse: a left drag turns it, the wheel brings it nearer; true when it took the event.
    pub fn input(&mut self, event: &winit::event::WindowEvent, over_shell: bool, scale: f64) -> bool {
        use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
        let orbiting = !self.free && self.sandbox.view == ViewMode::Orbit && self.sandbox.orbit.is_some() && self.sandbox.shown.is_some();
        match event {
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } if orbiting => {
                self.dragging = (*state == ElementState::Pressed && !over_shell).then_some(egui::Pos2::new(f32::NAN, f32::NAN));
                self.dragging.is_some()
            }
            WindowEvent::CursorMoved { position, .. } if self.dragging.is_some() => {
                let at = egui::pos2((position.x / scale) as f32, (position.y / scale) as f32);
                let from = self.dragging.replace(at).filter(|p| p.x.is_finite()).unwrap_or(at);
                if let Some(orbit) = &mut self.sandbox.orbit {
                    orbit.drag(f64::from(at.x - from.x), f64::from(at.y - from.y), f64::from(self.rect.height()));
                    self.sandbox.playing = false;
                }
                true
            }
            WindowEvent::MouseWheel { delta, .. } if orbiting && !over_shell => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                    MouseScrollDelta::PixelDelta(p) => p.y / 20.0,
                };
                if let Some(orbit) = &mut self.sandbox.orbit {
                    orbit.zoom(steps);
                }
                true
            }
            WindowEvent::Focused(false) => {
                self.dragging = None;
                false
            }
            _ => false,
        }
    }

    /// The shell's line of timings: the first frame, the list's read, the shown stage's build, what is built.
    pub fn timings(&self) -> String {
        let s = &self.sandbox;
        let mut parts = Vec::new();
        if let Some(ms) = s.first_frame_ms {
            parts.push(format!("first frame {ms:.0} ms"));
        }
        if let Some(ms) = s.list_ms {
            parts.push(format!("list {ms:.0} ms"));
        }
        if let Some(p) = s.shown_prepared() {
            parts.push(format!("stage {:.1} ms", p.build_ms));
        }
        parts.push(format!("{}/{} built", s.prepared.len(), s.lineups.len()));
        let held = self.resident.held() + self.shown_bytes;
        let maps = self.resident.names().len() + 1;
        parts.push(format!("{maps} map{} on the GPU, {:.1} of {:.1} GB", if maps == 1 { "" } else { "s" }, held as f64 / 1e9, self.resident.budget as f64 / 1e9));
        parts.join(" · ")
    }
}

/// A CS2 camera as app control's pose: the eye itself (no eye height), its angles and field of view.
pub fn pose_of(v: &CsView) -> nades_control::CameraPose {
    nades_control::CameraPose { position: [v.eye.x as f32, v.eye.y as f32, v.eye.z as f32], pitch_degrees: v.pitch as f32, yaw_degrees: v.yaw as f32, eye_height_units: 0.0, vertical_fov_degrees: v.fov as f32 }
}

/// One throw's view offscreen, as the sandbox shows it, written as a PNG: `view` (the result by default) at `time` (the
/// moment the view opens at by default), its smoke averaged over the jittered frames a still view takes.
pub fn render_throw(source: &dyn ThrowSource, throw: &str, map: Option<&str>, view: Option<ViewMode>, time: Option<f64>, (width, height): (u32, u32), out: &Path) -> Result<()> {
    let map_name = match map {
        Some(map) => map.to_string(),
        None => map_of(source, throw)?,
    };
    let (pack, scene) = resident::open_cached(&crate::pack_path(Some(&map_name)))?;
    let gpu = Gpu::new(nades_render::instance(), None)?;
    if !gpu.capabilities.bindless {
        bail!("{} has no texture binding arrays, which the renderer draws every material through", gpu.name());
    }
    let mut map = Map::new(&gpu.device, &gpu.queue, pack, &scene, crate::OFFSCREEN_FORMAT, &|_| {});
    let targets = map.targets(&gpu.device, &gpu.queue, width, height);
    let prepared = prepare(source, &map_name, throw, toward_sun(&map))?;
    let (noise, _, books) = crate::assets::effects();
    map.composite.set_noise(&gpu.device, &nades_render::smoke::noise_texture(&gpu.device, &gpu.queue, &noise));
    if let Some(books) = books {
        map.throw.set_flipbooks(&gpu.device, &books.upload(&gpu.device, &gpu.queue));
    }
    show_stage(&gpu, &mut map, Some(&prepared));
    let view = view.unwrap_or(ViewMode::Result);
    let t = time.unwrap_or_else(|| prepared.stage.opens_at(view));
    map.throw.flight_visible = matches!(view, ViewMode::Result | ViewMode::Orbit);
    let eye = views::of(&views::camera(view, &prepared.stage, &prepared.result, None));
    eprintln!("{throw} on {map_name}: {} view at {t:.2} s, built in {:.1} ms (light {:.1} ms)", view.name(), prepared.build_ms, prepared.light_ms);
    let output = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let frames = if prepared.stage.cloud.is_some() && t > 0.0 { nades_render::smoke::ACCUMULATE } else { 1 };
    for k in 0..frames {
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render") });
        map.throw.set_time(&gpu.device, &gpu.queue, Some(&prepared.stage), t, &eye, (width, height));
        map.render(&gpu.queue, &mut encoder, &targets, &eye, 0.0, &frame_effects(Some(&prepared.stage), t, k));
        gpu.queue.submit([encoder.finish()]);
    }
    let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render") });
    {
        let target = output.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(crate::CLEAR), store: wgpu::StoreOp::Store } })],
            ..Default::default()
        });
        map.present(&mut pass, &targets);
    }
    let row = crate::row_bytes(width);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("render"), size: u64::from(row * height), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    encoder.copy_texture_to_buffer(
        output.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()]);
    gpu.wait_idle();
    crate::write_png(&gpu, &buffer, width, height, out, [0, 1, 2])
}

/// `--bench-maps`: each map in turn, picked as the shell's selector picks it, timed from the pick to the frame that
/// shows it at its lineup, with the frames drawn in between, the switching frame's own time and the process's memory
/// once it shows. `--bench-maps all` picks every packed map, the shown one last, after waiting until the background has
/// opened every map that fits and warmed its lineups.
pub struct MapsBench {
    plan: Vec<String>,
    /// Wait until the background has settled before the first pick.
    settle: bool,
    started: Instant,
    settled_ms: Option<f64>,
    /// The shown map's frames (ms) while the others were being opened.
    opening: Vec<f64>,
    asked: Option<Asked>,
    done: Vec<serde_json::Value>,
}

/// A pick on its way: the map, when it was picked, the frames drawn since, whether it was on the GPU.
struct Asked {
    map: String,
    at: Instant,
    frames: u32,
    resident: bool,
    frame_ms: f64,
    /// The slowest frame's parts.
    parts: Vec<(&'static str, f64)>,
}

impl MapsBench {
    /// `all` (or `all:n`, n rounds) for every packed map; else the maps named, in order.
    pub fn new(plan: Vec<String>) -> Self {
        let rounds = plan.iter().find_map(|m| m.strip_prefix("all").map(|n| n.trim_start_matches(':').parse().unwrap_or(1)));
        let settle = rounds.is_some();
        let plan = match rounds {
            Some(n) => (0..n).flat_map(|_| packed_maps()).collect(),
            None => plan,
        };
        Self { plan, settle, started: Instant::now(), settled_ms: None, opening: Vec::new(), asked: None, done: Vec::new() }
    }

    /// One step, after a frame (`drew`: one was drawn, taking `frame_ms` of the window's time); the report once every
    /// map has been shown.
    pub fn step(&mut self, lineups: &mut Lineups, shown_map: &str, drew: bool, frame_ms: f64) -> Option<String> {
        if lineups.wake.is_none() || lineups.effects_files.is_some() {
            return None;
        }
        let shows_lineup = |s: &Sandbox| s.shown.is_some() || (s.list_ms.is_some() && s.lineups.is_empty());
        if self.settle && self.settled_ms.is_none() {
            if drew {
                self.opening.push(frame_ms);
            }
            let warm = lineups.resident.names().iter().all(|m| lineups.sandbox.kept.get(m).is_some_and(MapLineups::ready));
            // `preload` still set: the other maps have not started opening (the window is not shown yet)
            if lineups.preload || !(lineups.resident.settled() && warm && shows_lineup(&lineups.sandbox)) {
                return None;
            }
            self.settled_ms = Some(self.started.elapsed().as_secs_f64() * 1e3);
            // each round ends on the shown map, so that every pick is a switch
            let rounds = self.plan.iter().filter(|m| *m == shown_map).count().max(1);
            self.plan.retain(|m| m != shown_map);
            let per_round = self.plan.len() / rounds;
            self.plan = self.plan.chunks(per_round.max(1)).flat_map(|round| round.iter().cloned().chain([shown_map.to_string()])).collect();
        }
        if let Some(asked) = &mut self.asked {
            if drew {
                asked.frames += 1;
                if frame_ms >= asked.frame_ms {
                    asked.frame_ms = frame_ms;
                    asked.parts = lineups.frame_parts.clone();
                }
            }
            let s = &lineups.sandbox;
            let arrived = lineups.loading.is_none() && lineups.pending.is_none() && lineups.picked.is_none() && shown_map == asked.map && shows_lineup(s);
            if !arrived {
                return None;
            }
            let camera = s.camera().map(|c| [c.eye.x, c.eye.y, c.eye.z]);
            let memory = nades_render::memory::footprint().unwrap_or_default();
            let held = lineups.resident.held() + lineups.shown_bytes;
            self.done.push(serde_json::json!({
                "map": asked.map,
                "ms": asked.at.elapsed().as_secs_f64() * 1e3,
                "frames": asked.frames,
                "frame_ms": asked.frame_ms,
                "parts": asked.parts.iter().map(|(part, ms)| serde_json::json!([part, (ms * 100.0).round() / 100.0])).collect::<Vec<_>>(),
                "resident": asked.resident,
                "lineup": s.selected.map(|id| id.to_string()),
                "throw": s.shown.as_ref().map(|t| t.0.clone()),
                "camera": camera,
                "lineups": s.lineups.len(),
                "held_mb": nades_render::memory::mb(held),
                "footprint_mb": memory.now_mb(),
                "graphics_mb": nades_render::memory::mb(memory.graphics),
            }));
            self.asked = None;
        }
        match self.plan.first().cloned() {
            Some(next) => {
                self.plan.remove(0);
                let resident = lineups.resident.contains(&next);
                self.asked = Some(Asked { map: next.clone(), at: Instant::now(), frames: 0, resident, frame_ms: 0.0, parts: Vec::new() });
                lineups.actions.push(Action::Map(next));
                None
            }
            None => {
                let memory = nades_render::memory::footprint().unwrap_or_default();
                let mut opening = self.opening.clone();
                opening.sort_by(f64::total_cmp);
                let at = |q: f64| opening.get(((opening.len().max(1) - 1) as f64 * q).round() as usize).copied();
                let opened: Vec<serde_json::Value> = lineups.resident.opened.iter().map(|(map, ms, bytes)| serde_json::json!({ "map": map, "ms": ms, "mb": nades_render::memory::mb(*bytes) })).collect();
                Some(
                    serde_json::json!({
                        "maps": self.done,
                        "settled_ms": self.settled_ms,
                        "while_opening": { "frames": opening.len(), "median_ms": at(0.5), "p95_ms": at(0.95), "max_ms": at(1.0) },
                        "opened": opened,
                        "resident": lineups.resident.names(),
                        "budget_mb": nades_render::memory::mb(lineups.resident.budget),
                        "held_mb": nades_render::memory::mb(lineups.resident.held() + lineups.shown_bytes),
                        "footprint_mb": memory.now_mb(),
                        "peak_footprint_mb": nades_render::memory::mb(memory.peak),
                    })
                    .to_string(),
                )
            }
        }
    }
}

/// Render a throw as a stream of PNG frames on stdout, with JSON progress on stderr.
pub fn render_video(path: &Path) -> Result<()> {
    use std::io::Write as _;
    use nades_control::video::VideoRequest;
    let request: VideoRequest = serde_json::from_slice(&std::fs::read(path)?)?;
    let request = request.validated().map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
    let (width, height) = (request.width, request.height);
    let (pack, scene) = resident::open_cached(&crate::pack_path(Some(&request.map)))?;
    let gpu = Gpu::new(nades_render::instance(), None)?;
    if !gpu.capabilities.bindless { bail!("{} has no texture binding arrays", gpu.name()); }
    if width > gpu.device.limits().max_texture_dimension_2d || height > gpu.device.limits().max_texture_dimension_2d { bail!("video dimensions exceed GPU limit"); }
    let mut map = Map::new(&gpu.device, &gpu.queue, pack, &scene, crate::OFFSCREEN_FORMAT, &|_| {});
    let targets = map.targets(&gpu.device, &gpu.queue, width, height);
    let prepared = prepare(nades_config::source(&[])?.as_ref(), &request.map, &request.throw_id, toward_sun(&map))?;
    let (noise, _, books) = crate::assets::effects();
    map.composite.set_noise(&gpu.device, &nades_render::smoke::noise_texture(&gpu.device, &gpu.queue, &noise));
    if let Some(books) = books {
        map.throw.set_flipbooks(&gpu.device, &books.upload(&gpu.device, &gpu.queue));
    }
    show_stage(&gpu, &mut map, Some(&prepared));
    map.throw.flight_visible = true;
    let output = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("video frame"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: crate::OFFSCREEN_FORMAT, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row = crate::row_bytes(width);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("video readback"), size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    let stdout = std::io::stdout();
    let mut stream = stdout.lock();
    for frame in 0..request.frame_count() {
        let elapsed = f64::from(frame) / f64::from(request.fps);
        let t = request.start_seconds + elapsed;
        let eye = match request.camera_at(elapsed) {
            Some(camera) => {
                let mut live_camera = crate::camera::Camera::bench();
                live_camera.set_pose(&camera);
                live_camera.view()
            }
            None => views::of(&views::camera(ViewMode::Result, &prepared.stage, &prepared.result, None)),
        };
        let samples = if prepared.stage.cloud.is_some() && t > 0.0 { nades_render::smoke::ACCUMULATE } else { 1 };
        for sample in 0..samples {
            let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("video render") });
            map.throw.set_time(&gpu.device, &gpu.queue, Some(&prepared.stage), t, &eye, (width, height));
            map.render(&gpu.queue, &mut encoder, &targets, &eye, elapsed as f32, &frame_effects(Some(&prepared.stage), t, sample));
            gpu.queue.submit([encoder.finish()]);
        }
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("video present") });
        {
            let view = output.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("video present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(crate::CLEAR), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            map.present(&mut pass, &targets);
        }
        encoder.copy_texture_to_buffer(output.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } }, wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
        gpu.queue.submit([encoder.finish()]);
        stream.write_all(&crate::encode_png(&gpu, &buffer, width, height, [0, 1, 2])?)?;
        eprintln!("{}", serde_json::json!({"event":"video_progress", "frames":frame + 1, "total":request.frame_count()}));
    }
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lineup(id: u32) -> Lineup {
        Lineup {
            id: LineupId(id),
            grenade: Grenade::Smoke,
            side: Some(nades_model::Side::T),
            throws: 3,
            demos: 2,
            from: None,
            to: None,
            technique: None,
            teams: Vec::new(),
            players: Vec::new(),
            example: ThrowId(format!("{id}-throw")),
        }
    }

    /// The fixture `tools/cs2_throws.py` writes, through the file adapter: a file for each call, as tests run at once.
    fn fixture_source() -> Arc<dyn ThrowSource> {
        static WRITTEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("nades-sandbox-lineups-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("fixture-{}.sqlite", WRITTEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let tool = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/cs2_throws.py");
        let ran = ["python3", "python"].iter().find_map(|p| std::process::Command::new(p).arg("-I").arg(&tool).arg("fixture").arg(&path).output().ok()).expect("python3 on PATH");
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
        Arc::new(nades_source_file::FileSource::open(&[path]).unwrap())
    }

    #[test]
    fn a_cs2_throws_files_lineups_are_listed_and_their_throws_prepared() {
        let (woke, wakes) = std::sync::mpsc::channel();
        let library = Library::start(fixture_source(), move || {
            let _ = woke.send(());
        });
        let mut s = Sandbox::start("de_mirage", None, library);
        // no pack for this map: the stages are built without a collider
        s.collider = SharedCollider::build("nades-sandbox-test-no-pack".into(), || {});
        s.library.send(Request::List(s.filter.clone()));
        let started = Instant::now();
        while s.prepared.len() < 7 {
            assert!(started.elapsed() < Duration::from_secs(20), "listed {}, prepared {}, notice {:?}", s.lineups.len(), s.prepared.len(), s.notice);
            let _ = wakes.recv_timeout(Duration::from_millis(200));
            s.receive();
        }
        assert_eq!(s.lineups.iter().map(|l| (l.id.0, l.throws)).collect::<Vec<_>>(), [(1, 2), (3, 2), (2, 1), (4, 1), (5, 1), (6, 1), (7, 1)]);
        assert_eq!((s.selected, s.teams.as_slice(), s.notice.as_deref()), (Some(LineupId(1)), &["MOUZ".to_string(), "Vitality".to_string()][..], None));
        let window = s.selected_prepared().unwrap();
        assert_eq!((window.throw.id.as_str(), window.viewers, window.stage.flight.is_some()), ("m1-window-a", 66, true));
        assert_eq!(window.demo.as_ref().and_then(|d| d.file_name.as_deref()), Some("fixture-1.dem"));
        // the flash's blinded players were placed from the samples nearest it
        let flash = &s.prepared[&ThrowId("m1-flash".into())];
        assert_eq!(flash.stage.blinded.len(), 2);
        assert_eq!(flash.result.source, nades_scene::camera::Source::Victim { seconds: f64::from(2.4f32) });
        // a filter is applied in memory, and the summary names the sandbox's own ids
        s.apply(Action::Grenade(Some(Grenade::Incendiary)));
        let started = Instant::now();
        while s.lineups.len() != 1 {
            assert!(started.elapsed() < Duration::from_secs(10));
            let _ = wakes.recv_timeout(Duration::from_millis(200));
            s.receive();
        }
        assert_eq!(s.lineups[0].id, LineupId(3));
        assert_eq!(s.summary(false, false).lineup_id.as_deref(), Some("3"), "a selection no longer listed gives way to the first listed");
    }

    #[test]
    fn the_switch_bench_waits_for_the_stages_it_shows() {
        let (woke, wakes) = std::sync::mpsc::channel();
        let library = Library::start(fixture_source(), move || {
            let _ = woke.send(());
        });
        let mut s = Sandbox::start("de_mirage", None, library);
        s.collider = SharedCollider::build("nades-sandbox-test-no-pack".into(), || {});
        s.library.send(Request::List(s.filter.clone()));
        let started = Instant::now();
        while s.prepared.len() < 7 {
            assert!(started.elapsed() < Duration::from_secs(20), "listed {}, prepared {}, notice {:?}", s.lineups.len(), s.prepared.len(), s.notice);
            let _ = wakes.recv_timeout(Duration::from_millis(200));
            s.receive();
        }
        let examples: Vec<ThrowId> = s.lineups.iter().map(|l| l.example.clone()).collect();
        // from the sixth of seven lineups, two switches show the seventh, then the first
        s.selected = Some(s.lineups[5].id);
        let mut lineups = Lineups::new(s, false, egui::Rect::NOTHING);
        lineups.wake = Some(Arc::new(|| {}));
        lineups.sandbox.prepared.remove(&examples[1]);
        assert!(lineups.ready_for(2), "the second lineup is not shown, so its stage is not waited for");
        lineups.sandbox.prepared.remove(&examples[0]);
        assert!(!lineups.ready_for(2), "the first is shown after the last");
    }

    #[test]
    fn a_map_left_keeps_its_lineups_for_when_it_is_picked_again() {
        let library = Library::start(Arc::new(nades_model::fixture::Memory::default()), || {});
        let mut s = Sandbox::start("de_cache", None, library);
        s.filter.query.team = Some("Vitality".into());
        s.filter.query.side = Some(nades_model::Side::T);
        s.lineups = vec![lineup(1), lineup(2)];
        s.selected = Some(LineupId(2));
        s.list_ms = Some(90.0);
        // a map not kept: read afresh, with the side carried over and the team left behind
        assert!(!s.switch_map("de_mirage", DVec3::Z));
        assert_eq!((s.filter.map.as_str(), s.filter.query.side, s.filter.query.team.as_deref()), ("de_mirage", Some(nades_model::Side::T), None));
        assert!(s.lineups.is_empty() && s.selected.is_none() && s.list_ms.is_none());
        let kept = &s.kept["de_cache"];
        assert_eq!((kept.lineups.len(), kept.selected), (2, Some(LineupId(2))));
        // back again: as it was left, its team filter reset, so its list is read again
        s.lineups = vec![lineup(1)];
        s.list_ms = Some(80.0);
        assert!(s.switch_map("de_cache", DVec3::X));
        assert_eq!((s.lineups.len(), s.selected, s.toward_sun), (2, Some(LineupId(2)), DVec3::X));
        assert!(s.list_ms.is_none(), "its filter changed (no team), so it is read again");
        assert_eq!(s.kept["de_mirage"].lineups.len(), 1);
        // a kept map whose filter is the one shown opens with its list as it is
        s.list_ms = Some(70.0);
        assert!(s.switch_map("de_mirage", DVec3::Z));
        assert_eq!(s.list_ms, Some(80.0));
        assert!(!s.kept["de_cache"].ready(), "its selected lineup's stage is not built");
    }
}
