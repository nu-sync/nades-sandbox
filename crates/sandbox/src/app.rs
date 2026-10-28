//! The simulator's state and what it asks of the threads: the maps (the shown one, the ones kept on the GPU beside it,
//! one being opened), the game's effect files, the camera each map was last shown from, and the scene (scene.rs) with
//! its simulations. The window (main.rs) draws it and feeds it input.

use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::DVec3;
use nades_control::SceneSummary;
use nades_control::scene::{Scene, ViewName};
use nades_render::Gpu;
use nades_render::map::{FrameEffects, Map};
use nades_scene::camera::View as CsView;

use crate::collider::SharedCollider;
use crate::resident::{self, Resident};
use crate::scene::SceneState;
use crate::shell::{self, Action};
use crate::{sim, state};

pub use crate::assets::packed_maps;

pub fn cs_view((eye, pitch, yaw, fov): state::Camera) -> CsView {
    CsView { eye: DVec3::from_array(eye), pitch, yaw, fov }
}

pub fn camera_of(v: &CsView) -> state::Camera {
    ([v.eye.x, v.eye.y, v.eye.z], v.pitch, v.yaw, v.fov)
}

/// The 3D view's size in pixels for a window of `window` points at `scale`: the whole window, or the window less the
/// shell's panels when the shell shows.
pub fn view_size(window: (f64, f64), scale: f64) -> (u32, u32) {
    let (panels_x, panels_y) = if shell::shown() { (f64::from(shell::LEFT + shell::RIGHT), f64::from(shell::TOP + shell::BOTTOM)) } else { (0.0, 0.0) };
    let w = (window.0 - panels_x).max(64.0);
    let h = (window.1 - panels_y).max(64.0);
    ((w * scale).round() as u32, (h * scale).round() as u32)
}

/// The light the map lends a smoke: towards its sun, in Hammer units' axes.
pub fn toward_sun(map: &Map) -> DVec3 {
    let s = map.toward_sun();
    DVec3::new(s.x.into(), s.y.into(), s.z.into())
}

/// A CS2 camera as app control's pose: the eye itself (no eye height), its angles and field of view.
pub fn pose_of(v: &CsView) -> nades_control::CameraPose {
    nades_control::CameraPose { position: [v.eye.x as f32, v.eye.y as f32, v.eye.z as f32], pitch_degrees: v.pitch as f32, yaw_degrees: v.yaw as f32, eye_height_units: 0.0, vertical_fov_degrees: v.fov as f32 }
}

/// A map being loaded for the map selector: its name, and the thread's answer.
type MapLoad = (String, std::sync::mpsc::Receiver<anyhow::Result<Map>>);

/// What the window adds to the map: the maps kept on the GPU, a map being loaded, the game's effect files, the shell's
/// state, and the 3D view's place between the shell's panels.
pub struct Sandbox {
    pub map: String,
    pub maps: Vec<String>,
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
    /// A map picked that was not on the GPU, loaded for the pick.
    loading: Option<MapLoad>,
    /// Where the last frame's time went on the window's thread (ms by part), for the maps bench.
    pub frame_parts: Vec<(&'static str, f64)>,
    /// The shown map's collider, built after the first frame.
    pub collider: SharedCollider,
    pub toward_sun: DVec3,
    /// Each map's last camera, as remembered (state.rs).
    pub cameras: std::collections::BTreeMap<String, state::Camera>,
    /// The camera the window draws from, until the person or a view moves it.
    pub remembered: Option<CsView>,
    pub notice: Option<String>,
    pub first_frame_ms: Option<f64>,
    /// Bench and screenshot runs draw from the bench camera, as M2's did.
    pub bench: bool,
    /// The camera was taken by hand (the fly keys) or by app control.
    pub free: bool,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    copied_at: Option<Instant>,
    /// The scene on the shown map, and the other maps' kept.
    pub scene: SceneState,
    /// A camera to jump to once (standing on a picked spot).
    jump: Option<CsView>,
    /// The cursor over the window (points), and the orbit's drag from where.
    cursor: Option<egui::Pos2>,
    dragging: Option<egui::Pos2>,
    /// What the arcs on the GPU were built for.
    arcs_key: Option<crate::scene::ArcsKey>,
    /// What the last frame showed, and how many jittered frames of it are averaged.
    last_key: Option<[u64; 4]>,
    pub accumulated: u32,
}

impl Sandbox {
    pub fn new(map: &str, remembered: Option<state::Remembered>, bench: bool, rect: egui::Rect) -> Self {
        let camera = remembered.as_ref().and_then(|r| r.camera_on(map)).map(cs_view);
        let scenes = remembered.as_ref().map(|r| r.scenes.clone()).unwrap_or_default();
        let cameras = remembered.map(|r| r.cameras).unwrap_or_default();
        Self {
            map: map.to_string(),
            maps: packed_maps(),
            rect,
            actions: Vec::new(),
            effects: None,
            effects_files: None,
            resident: Resident::new(0),
            preload: !bench,
            shown_bytes: 0,
            picked: None,
            loading: None,
            frame_parts: Vec::new(),
            collider: SharedCollider::default(),
            toward_sun: DVec3::Z,
            cameras,
            remembered: camera,
            notice: None,
            first_frame_ms: None,
            bench,
            free: camera.is_none(),
            wake: None,
            copied_at: None,
            scene: SceneState::new(map, scenes),
            jump: None,
            cursor: None,
            dragging: None,
            arcs_key: None,
            last_key: None,
            accumulated: 0,
        }
    }

    /// After the first frame: the map's collider, and the game's flipbooks and smoke noise, on threads of their own.
    pub fn after_first_frame(&mut self, map: &Map, wake: Arc<dyn Fn() + Send + Sync>) {
        self.wake = Some(wake.clone());
        self.shown_bytes = map.gpu_bytes;
        self.build_collider();
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
        self.resident.cameras = self.cameras.clone();
        let looked_at: Vec<String> = self.cameras.keys().cloned().collect();
        self.resident.preload(looked_at.into_iter().chain(self.maps.iter().cloned()).filter(|m| m != name));
        self.resident.next(gpu, name, self.shown_bytes, self.wake.clone());
    }

    fn build_collider(&mut self) {
        let wake = self.wake.clone();
        self.collider = SharedCollider::build(self.map.clone(), move || {
            if let Some(wake) = &wake {
                wake();
            }
        });
    }

    /// Takes what the threads have finished; true when the picture may change.
    pub fn receive(&mut self, gpu: &Gpu, map: &mut Map, name: &mut String) -> bool {
        let mut changed = self.scene.receive();
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
                Ok(new) => {
                    let old = std::mem::replace(map, new);
                    let from = std::mem::replace(name, loading);
                    self.keep(from, old, map.gpu_bytes);
                    self.switch_to(gpu, map, name);
                }
                Err(e) => self.notice = Some(format!("{loading}: {e:#}")),
            }
            changed = true;
        }
        // a map opened in the background: kept on the GPU; then the next one
        if let Some((opened, result, ms)) = self.resident.arrived() {
            match result {
                Ok(new) => {
                    eprintln!("resident: {opened} on the GPU in {ms:.0} ms, {:.0} MB", nades_render::memory::mb(new.gpu_bytes));
                    self.keep(opened.clone(), new, map.gpu_bytes);
                    if self.picked.as_deref() == Some(opened.as_str()) {
                        self.picked = None;
                        self.switch_resident(gpu, map, name, &opened);
                    }
                }
                Err(e) => {
                    eprintln!("resident: {opened}: {e:#}");
                    if self.picked.as_deref() == Some(opened.as_str()) {
                        self.picked = None;
                        self.notice = Some(format!("{opened}: {e:#}"));
                    }
                }
            }
            self.resident.next(gpu, name, map.gpu_bytes, self.wake.clone());
            changed = true;
        }
        changed
    }

    /// Keeps a map no longer shown on the GPU.
    fn keep(&mut self, name: String, map: Map, shown: u64) {
        self.resident.keep(name, map, shown);
    }

    /// Shows `to`, kept on the GPU: a swap of maps, drawn in the frame that asked for it. False when `to` is not kept.
    pub fn switch_resident(&mut self, gpu: &Gpu, map: &mut Map, name: &mut String, to: &str) -> bool {
        let Some(new) = self.resident.take(to) else { return false };
        let old = std::mem::replace(map, new);
        let from = std::mem::replace(name, to.to_string());
        // a pick of another map, still on its way, is passed over
        self.loading = None;
        self.keep(from, old, map.gpu_bytes);
        self.switch_to(gpu, map, name);
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
        self.resident.make_room(resident::expected_bytes(&path), map.gpu_bytes);
        let receive = resident::load(&gpu.device, &gpu.queue, path, self.cameras.get(&name).copied(), self.wake.clone());
        self.loading = Some((name, receive));
    }

    /// The new map is on the GPU: the game's effects, its collider, and the camera it was last shown from.
    fn switch_to(&mut self, gpu: &Gpu, map: &mut Map, name: &str) {
        self.install_effects(gpu, map);
        self.shown_bytes = map.gpu_bytes;
        self.note_camera_of(&self.map.clone());
        self.map = name.to_string();
        self.toward_sun = toward_sun(map);
        self.remembered = self.cameras.get(name).copied().map(cs_view);
        self.free = true;
        self.jump = self.remembered;
        self.build_collider();
        self.scene.switch_map(name);
        self.arcs_key = None;
    }

    /// Notes the camera shown on `map`, for the next time it opens.
    fn note_camera_of(&mut self, map: &str) {
        if let Some(c) = self.remembered {
            self.cameras.insert(map.to_string(), camera_of(&c));
        }
    }

    /// The camera the person moved by hand, noted as this map's.
    pub fn moved(&mut self, camera: CsView) {
        self.remembered = Some(camera);
    }

    /// Writes what to reopen on.
    pub fn remember(&mut self) {
        self.note_camera_of(&self.map.clone());
        state::save(&state::Remembered { map: self.map.clone(), camera: self.remembered.map(|c| camera_of(&c)), cameras: self.cameras.clone(), scenes: self.scene.all() });
    }

    /// What the app-control layer reports of the scene.
    pub fn summary(&self) -> SceneSummary {
        let s = &self.scene;
        let placed = s.placed();
        SceneSummary {
            camera_mode: if self.free { "free".into() } else { s.view.name().into() },
            loading: self.switching() || !s.ready(),
            notice: self.notice.clone().or_else(|| s.notice.clone()),
            throws: s.scene.throws.len(),
            selected_throw: s.selected,
            timeline_seconds: s.time,
            timeline_end_seconds: s.end(),
            playing: s.playing,
            active_effects: sim::active(&placed, s.time),
        }
    }

    /// The scene shown, as a scene file has it.
    pub fn scene(&self) -> Scene {
        self.scene.scene.clone()
    }

    /// Shows `scene`: on this map at once, on another once that map is up (picked as the map selector picks it).
    pub fn set_scene(&mut self, scene: Scene) {
        let map = scene.map.clone();
        self.scene.set(scene);
        if map != self.map {
            self.actions.push(Action::Map(map));
        }
    }

    /// Moves the scene's clock, and plays or pauses it.
    pub fn set_time(&mut self, seconds: Option<f64>, playing: Option<bool>) {
        if let Some(t) = seconds {
            self.scene.time = t.clamp(0.0, self.scene.end().max(t.min(nades_control::scene::MAX_START + 60.0)));
        }
        if let Some(on) = playing {
            self.scene.play(on);
        } else if seconds.is_some() {
            self.scene.playing = false;
        }
    }

    /// Shows a view of a throw, or the free camera.
    pub fn set_view(&mut self, view: ViewName, throw: Option<usize>) {
        if view == ViewName::Free {
            self.scene.view = ViewName::Free;
            self.free = true;
        } else {
            self.scene.show_view(view, throw);
            self.free = false;
        }
    }

    /// Opens the scene file at `path` (dropped on the window).
    pub fn open_file(&mut self, path: &std::path::Path) {
        if let Some(scene) = self.scene.read(Some(path)) {
            self.set_scene(scene);
        }
    }

    /// The shell, laid out over `ui`; what it asks for is applied by [`Sandbox::update`].
    pub fn ui(&mut self, ui: &mut egui::Ui, timings: String) {
        if !shell::shown() {
            self.rect = ui.max_rect();
            return;
        }
        let loading = self.loading.as_ref().map(|(n, _)| n.clone()).or_else(|| self.picked.clone());
        let practice = self.scene.selected.and_then(|k| self.scene.scene.throws.get(k)).map(|t| nades_physics::practice_command(&sim::physics_throw(t)));
        let status = shell::Status { map: &self.map, maps: &self.maps, notice: self.notice.as_deref(), timings, loading_map: loading.as_deref(), copied: self.copied_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)), scene: Some(&mut self.scene), practice, free: self.free };
        let (actions, rect) = shell::show(ui, status);
        self.actions.extend(actions);
        self.rect = rect;
    }

    /// Applies the shell's actions (a map on the GPU is shown in this frame), asks for the throws not simulated yet,
    /// follows the aim with the preview arc, and plays the clock on by `dt`.
    pub fn update(&mut self, gpu: &Gpu, map: &mut Map, name: &mut String, ctx: &egui::Context, dt: f64) {
        for action in std::mem::take(&mut self.actions) {
            let camera = self.remembered;
            let s = &mut self.scene;
            match action {
                Action::Map(to) if to == *name => {}
                Action::Map(to) => self.pick_map(gpu, map, name, to),
                Action::Copy(text) => {
                    self.copied_at = Some(Instant::now());
                    ctx.copy_text(text);
                }
                Action::Tool(tool) => s.tool = tool,
                Action::Throw => {
                    if let Some(camera) = camera.filter(|_| self.free) {
                        let t = s.tool.throw_from(&camera);
                        s.add(t);
                    } else {
                        s.notice = Some("Throw from the free camera: press 1 or Free first.".into());
                    }
                }
                Action::Select(k) => {
                    s.selected = Some(k);
                    if s.view != ViewName::Free && !self.free {
                        s.show_view(s.view, Some(k));
                    }
                }
                Action::Edit(k, t) => s.edit(k, t),
                Action::Reaim(k) => {
                    if let (Some(camera), Some(old)) = (camera.filter(|_| self.free), s.scene.throws.get(k).cloned()) {
                        let mut t = s.tool.throw_from(&camera);
                        (t.grenade, t.movement, t.click, t.crouch, t.start, t.label) = (old.grenade, old.movement, old.click, old.crouch, old.start, old.label);
                        let height = if t.crouch { nades_scene::camera::EYE_CROUCHED } else { nades_scene::camera::EYE_STANDING };
                        t.feet = (camera.eye - DVec3::Z * height).to_array();
                        s.edit(k, t);
                    } else {
                        s.notice = Some("Re-aim from the free camera: press 1 or Free first.".into());
                    }
                }
                Action::Duplicate(k) => s.duplicate(k),
                Action::Delete(k) => s.delete(k),
                Action::View(view) => self.set_view(view, None),
                Action::Time(t) => self.set_time(Some(t), None),
                Action::Play(on) => s.play(on),
                Action::Arcs(on) => s.arcs = on,
                Action::Save => s.save(),
                Action::Open => {
                    if let Some(scene) = s.read(None) {
                        self.set_scene(scene);
                    }
                }
                Action::Clear => {
                    let map = s.scene.map.clone();
                    s.set(Scene::new(&map));
                }
            }
        }
        if self.free && self.scene.view != ViewName::Free {
            self.scene.view = ViewName::Free;
            self.scene.orbit = None;
        }
        if self.wake.is_some() {
            self.scene.simulate(&self.collider, self.toward_sun, self.wake.clone());
        }
        if let Some(camera) = self.remembered.filter(|_| self.free) {
            self.scene.aim(&camera, &self.collider);
        } else {
            self.scene.preview = None;
        }
        self.scene.advance(dt);
    }

    /// Picks `to` as the map selector does (for the maps bench and app control).
    pub fn ask_map(&mut self, to: String) {
        self.actions.push(Action::Map(to));
    }

    /// The camera this frame draws from: a spot just picked to stand on, the view of a throw; none when the person or
    /// app control has the camera.
    pub fn camera(&mut self) -> Option<CsView> {
        if let Some(jump) = self.jump.take() {
            return Some(jump);
        }
        if self.bench || self.free {
            return None;
        }
        self.scene.camera(self.collider.get().as_deref()).or(self.remembered)
    }

    /// Readies what the frame from `eye` into targets of `size` draws of the scene; None when the picture is already
    /// final (every jittered smoke frame averaged in) and nothing needs drawing.
    pub fn frame(&mut self, gpu: &Gpu, map: &mut Map, eye: &nades_render::View, size: (u32, u32), force: bool) -> Option<FrameEffects> {
        let key = self.scene.arcs_key();
        if self.arcs_key.as_ref() != Some(&key) {
            map.throw.set_arcs(&gpu.device, &self.scene.arcs());
            self.arcs_key = Some(key);
        }
        let placed = self.scene.placed();
        let t = self.scene.time;
        // a still picture averages its smokes' jittered frames; anything that moves starts it over
        let q = eye.rotation;
        let camera = [eye.eye.x, eye.eye.y, eye.eye.z, q.x, q.y, q.z, eye.fov_y].iter().fold(0u64, |h, v| h.rotate_left(7) ^ u64::from(v.to_bits()));
        let shown = [camera, t.to_bits(), u64::from(size.0) << 32 | u64::from(size.1), self.scene.revision ^ (placed.len() as u64) << 48 ^ map.id << 56];
        self.accumulated = if self.last_key == Some(shown) { (self.accumulated + 1).min(nades_render::smoke::ACCUMULATE) } else { 0 };
        self.last_key = Some(shown);
        if !force && self.accumulated >= nades_render::smoke::ACCUMULATE && sim::smoking(&placed, t) {
            return None;
        }
        Some(sim::show(gpu, map, &placed, t, eye, size, self.accumulated))
    }

    /// Whether frames must keep coming: the clock playing, a smoke still averaging, throws being simulated, a copy note
    /// fading, a map loading.
    pub fn animating(&self) -> bool {
        let smoke = sim::smoking(&self.scene.placed(), self.scene.time) && self.accumulated < nades_render::smoke::ACCUMULATE;
        self.scene.playing || smoke || self.copied_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) || self.loading.is_some() || self.picked.is_some()
    }

    /// Whether a map is on its way to being shown.
    pub fn switching(&self) -> bool {
        self.loading.is_some() || self.picked.is_some()
    }

    /// Takes a window event before the camera does: a click on the map to stand there, the orbit's drag and wheel;
    /// true when it took it.
    pub fn input(&mut self, event: &winit::event::WindowEvent, over_shell: bool, scale: f64) -> bool {
        use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
        let orbiting = !self.free && self.scene.view == ViewName::Orbit && self.scene.orbit.is_some();
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let at = egui::pos2((position.x / scale) as f32, (position.y / scale) as f32);
                self.cursor = Some(at);
                let Some(from) = self.dragging else { return false };
                self.dragging = Some(at);
                if let Some(orbit) = &mut self.scene.orbit {
                    orbit.drag(f64::from(at.x - from.x), f64::from(at.y - from.y), f64::from(self.rect.height()));
                    self.scene.playing = false;
                }
                true
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } if self.scene.tool.picking && !over_shell => {
                self.stand_at_cursor();
                true
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } if orbiting || self.dragging.is_some() => {
                self.dragging = (*state == ElementState::Pressed && !over_shell).then_some(self.cursor).flatten();
                self.dragging.is_some()
            }
            WindowEvent::MouseWheel { delta, .. } if orbiting && !over_shell => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                    MouseScrollDelta::PixelDelta(p) => p.y / 20.0,
                };
                if let Some(orbit) = &mut self.scene.orbit {
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

    /// Stands the camera on the point of the map under the cursor (its floor, when it is a wall), looking as it looked.
    fn stand_at_cursor(&mut self) {
        let (Some(cursor), Some(camera), Some(collider)) = (self.cursor, self.remembered, self.collider.get()) else {
            self.scene.notice = Some("The map's collider is not built yet.".into());
            return;
        };
        let r = self.rect;
        let at = ((f64::from(cursor.x - r.min.x) / f64::from(r.width())), (f64::from(cursor.y - r.min.y) / f64::from(r.height())));
        let Some(hit) = sim::pick(&collider, &camera, at, f64::from(r.width() / r.height())) else { return };
        let floor = collider.floor_below(hit + DVec3::Z * 8.0, 400.0).unwrap_or(hit.z);
        let height = if self.scene.tool.crouch { nades_scene::camera::EYE_CROUCHED } else { nades_scene::camera::EYE_STANDING };
        let eye = DVec3::new(hit.x, hit.y, floor + height);
        self.jump = Some(CsView { eye, ..camera });
        self.remembered = self.jump;
        self.free = true;
        self.scene.tool.picking = false;
    }

    /// The shell's line of timings: the first frame, the maps on the GPU.
    pub fn timings(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ms) = self.first_frame_ms {
            parts.push(format!("first frame {ms:.0} ms"));
        }
        let held = self.resident.held() + self.shown_bytes;
        let maps = self.resident.names().len() + 1;
        let (ready, all) = (self.scene.placed().len(), self.scene.scene.throws.len());
        if ready < all {
            parts.push(format!("simulating {}/{all}", all - ready));
        }
        if !self.collider.built() && self.wake.is_some() {
            parts.push("building the collider".into());
        }
        parts.push(format!("{maps} map{} on the GPU, {:.1} of {:.1} GB", if maps == 1 { "" } else { "s" }, held as f64 / 1e9, self.resident.budget as f64 / 1e9));
        parts.join(" · ")
    }
}
