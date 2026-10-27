//! The simulator's state and what it asks of the threads: the maps (the shown one, the ones kept on the GPU beside it,
//! one being opened), the game's effect files, and the camera each map was last shown from. The window (main.rs) draws
//! it and feeds it input.

use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::DVec3;
use nades_control::SceneSummary;
use nades_render::Gpu;
use nades_render::map::{FrameEffects, Map};
use nades_scene::camera::View as CsView;

use crate::collider::SharedCollider;
use crate::resident::{self, Resident};
use crate::shell::{self, Action};
use crate::state;

pub use crate::assets::packed_maps;

pub fn cs_view((eye, pitch, yaw, fov): state::Camera) -> CsView {
    CsView { eye: DVec3::from_array(eye), pitch, yaw, fov }
}

pub fn camera_of(v: &CsView) -> state::Camera {
    ([v.eye.x, v.eye.y, v.eye.z], v.pitch, v.yaw, v.fov)
}

/// The 3D view's size in pixels for a window of `window` points at `scale`: the window less the shell's panels.
pub fn view_size(window: (f64, f64), scale: f64) -> (u32, u32) {
    let w = (window.0 - f64::from(shell::LEFT) - f64::from(shell::RIGHT)).max(64.0);
    let h = (window.1 - f64::from(shell::TOP) - f64::from(shell::BOTTOM)).max(64.0);
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
}

impl Sandbox {
    pub fn new(map: &str, remembered: Option<state::Remembered>, bench: bool, rect: egui::Rect) -> Self {
        let camera = remembered.as_ref().and_then(|r| r.camera_on(map)).map(cs_view);
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
        let mut changed = false;
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
        self.free = self.remembered.is_none();
        self.build_collider();
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
        state::save(&state::Remembered { map: self.map.clone(), camera: self.remembered.map(|c| camera_of(&c)), cameras: self.cameras.clone() });
    }

    /// What the app-control layer reports of the scene.
    pub fn summary(&self) -> SceneSummary {
        SceneSummary { camera_mode: "fly".into(), loading: self.loading.is_some() || self.picked.is_some(), notice: self.notice.clone(), ..Default::default() }
    }

    /// The shell, laid out over `ui`; what it asks for is applied by [`Sandbox::update`].
    pub fn ui(&mut self, ui: &mut egui::Ui, timings: String) {
        let loading = self.loading.as_ref().map(|(n, _)| n.clone()).or_else(|| self.picked.clone());
        let status = shell::Status { map: &self.map, maps: &self.maps, notice: self.notice.as_deref(), timings, loading_map: loading.as_deref(), copied: self.copied_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) };
        let (actions, rect) = shell::show(ui, status);
        self.actions.extend(actions);
        self.rect = rect;
    }

    /// Applies the shell's actions (a map on the GPU is shown in this frame).
    pub fn update(&mut self, gpu: &Gpu, map: &mut Map, name: &mut String, ctx: &egui::Context, _dt: f64) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                Action::Map(to) => self.pick_map(gpu, map, name, to),
                Action::Copy(text) => {
                    self.copied_at = Some(Instant::now());
                    ctx.copy_text(text);
                }
            }
        }
    }

    /// Picks `to` as the map selector does (for the maps bench and app control).
    pub fn ask_map(&mut self, to: String) {
        self.actions.push(Action::Map(to));
    }

    /// The camera this frame draws from, unless the person or app control took it.
    pub fn camera(&self) -> Option<CsView> {
        if self.bench || self.free { None } else { self.remembered }
    }

    /// Readies what the frame draws of the scene's throws.
    pub fn frame(&mut self, _gpu: &Gpu, _map: &mut Map, _eye: &nades_render::View, _size: (u32, u32), _force: bool) -> Option<FrameEffects> {
        Some(FrameEffects::default())
    }

    /// Whether frames must keep coming: a copy note fading, a map loading.
    pub fn animating(&self) -> bool {
        self.copied_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) || self.loading.is_some() || self.picked.is_some()
    }

    /// Whether a map is on its way to being shown.
    pub fn switching(&self) -> bool {
        self.loading.is_some() || self.picked.is_some()
    }

    /// Takes a window event before the camera does; true when it took it.
    pub fn input(&mut self, _event: &winit::event::WindowEvent, _over_shell: bool, _scale: f64) -> bool {
        false
    }

    /// The shell's line of timings: the first frame, the maps on the GPU.
    pub fn timings(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ms) = self.first_frame_ms {
            parts.push(format!("first frame {ms:.0} ms"));
        }
        let held = self.resident.held() + self.shown_bytes;
        let maps = self.resident.names().len() + 1;
        parts.push(format!("{maps} map{} on the GPU, {:.1} of {:.1} GB", if maps == 1 { "" } else { "s" }, held as f64 / 1e9, self.resident.budget as f64 / 1e9));
        parts.join(" · ")
    }
}
