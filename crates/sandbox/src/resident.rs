//! Every pool map on the GPU at once, within a budget (S19). The maps not shown are kept here, each with what it holds
//! on the GPU ([`Map::gpu_bytes`]); when a map would not fit, the one shown least recently is let go. After the first
//! frame the other packed maps are opened in the background, one at a time and only while the next one fits, so that
//! picking any of them is a swap: the window shows it in the next frame.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use anyhow::Result;
use nades_pack::Pack;
use nades_pack::scene::Scene;
use nades_render::View;
use nades_render::map::{FrameEffects, Map};

/// The packs opened in this process, kept mapped for as long as it runs: the GPU reads their own pages, and opening a
/// map again (after it was let go) maps nothing new.
pub fn open_cached(path: &Path) -> Result<(&'static Pack, Scene)> {
    static OPEN: std::sync::Mutex<Vec<(PathBuf, &'static Pack)>> = std::sync::Mutex::new(Vec::new());
    let known = OPEN.lock().expect("packs").iter().find(|(p, _)| p == path).map(|(_, pack)| *pack);
    match known {
        Some(pack) => Ok((pack, pack.scene()?)),
        None => {
            let (pack, scene) = crate::open_pack(path)?;
            OPEN.lock().expect("packs").push((path.to_path_buf(), pack));
            Ok((pack, scene))
        }
    }
}

/// Opens `path`'s map on the GPU on a thread of its own, below the window's priority, and answers once its textures
/// are on the GPU and it has been drawn twice offscreen ([`warm`]) from `camera` (the one it was last shown from), or
/// from the middle of its draws.
pub fn load(device: &wgpu::Device, queue: &wgpu::Queue, path: PathBuf, camera: Option<crate::state::Camera>, wake: Option<Arc<dyn Fn() + Send + Sync>>) -> Receiver<Result<Map>> {
    let (device, queue) = (device.clone(), queue.clone());
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("map".into())
        .spawn(move || {
            background_priority();
            let loaded = open_cached(&path).map(|(pack, scene)| {
                let map = Map::new(&device, &queue, pack, &scene, crate::SURFACE_FORMAT, &|_| {});
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
                let view = camera.map_or_else(|| middle(&scene), |(eye, pitch, yaw, fov)| View::from_cs(eye.map(|v| v as f32), pitch as f32, yaw as f32, fov as f32));
                warm(&device, &queue, &map, &view);
                map
            });
            let _ = send.send(loaded);
            if let Some(wake) = wake {
                wake();
            }
        })
        .expect("a map thread");
    receive
}

/// Draws `map` twice offscreen at the window's usual size. A map's first two frames cost the GPU 30–100 ms more than
/// the rest (the driver's work on first use, mostly whatever the camera; S21): paid here, not in the frame that shows
/// it.
pub fn warm(device: &wgpu::Device, queue: &wgpu::Queue, map: &Map, view: &View) {
    draw_offscreen(device, queue, map, view, &FrameEffects::default(), 2, true);
}

/// Draws `map` `frames` times offscreen from `view` with `effects`, at the window's usual size; `wait` for each on the
/// GPU (a loader thread does; the window's thread does not).
pub fn draw_offscreen(device: &wgpu::Device, queue: &wgpu::Queue, map: &Map, view: &View, effects: &FrameEffects, frames: u32, wait: bool) {
    let (width, height) = crate::lineups::view_size(crate::WINDOW, crate::EXPECTED_SCALE);
    let targets = map.targets(device, queue, width, height);
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("warm-up"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::SURFACE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT,
        view_formats: &[],
    });
    let output = output.create_view(&Default::default());
    for k in 0..frames {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("warm-up") });
        map.render(queue, &mut encoder, &targets, view, k as f32 / 60.0, effects);
        map.present(
            &mut encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("warm-up"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &output, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(crate::CLEAR), store: wgpu::StoreOp::Discard } })],
                ..Default::default()
            }),
            &targets,
        );
        let index = queue.submit([encoder.finish()]);
        if wait {
            let _ = device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None });
        }
    }
}

/// A camera in the middle of the map's draws, looking across it: where a map with no camera of its own is warmed from.
fn middle(scene: &Scene) -> View {
    let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
    for d in &scene.draws {
        lo = lo.min(glam::Vec3::from_array(d.bounds.min));
        hi = hi.max(glam::Vec3::from_array(d.bounds.max));
    }
    let eye = if lo.x <= hi.x { (lo + hi) / 2.0 } else { glam::Vec3::ZERO };
    View { eye, rotation: glam::Quat::IDENTITY, fov_y: nades_render::view::VERTICAL_FOV_DEGREES.to_radians(), near: 4.0 * nades_render::view::INCH, far: 30_000.0 * nades_render::view::INCH }
}

/// Below the window's thread: macOS runs it on what the window leaves (its utility class).
fn background_priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: sets the calling thread's own scheduling class
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

/// How much a pack's map is expected to hold on the GPU before it is opened: its file, and a tenth more for what the
/// GPU allocates above the blocks (Metal: 3–15%).
pub fn expected_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len() + m.len() / 10)
}

/// Of the maps kept (name, bytes held, when last shown), the ones to let go, least recently shown first, so that
/// `needed` more fits within `budget`.
pub fn to_let_go<'a>(kept: impl IntoIterator<Item = (&'a str, u64, u64)>, needed: u64, budget: u64) -> Vec<String> {
    let mut kept: Vec<(&str, u64, u64)> = kept.into_iter().collect();
    kept.sort_by_key(|&(_, _, used)| used);
    let mut held: u64 = kept.iter().map(|&(_, bytes, _)| bytes).sum();
    let mut out = Vec::new();
    for (name, bytes, _) in kept {
        if held + needed <= budget {
            break;
        }
        held -= bytes;
        out.push(name.to_string());
    }
    out
}

/// A map kept on the GPU while another is shown.
struct Kept {
    name: String,
    map: Map,
    /// When it was last shown (a counter).
    used: u64,
}

/// The maps kept on the GPU beside the shown one, and the one being opened in the background.
pub struct Resident {
    /// What the maps may hold on the GPU together, the shown one included.
    pub budget: u64,
    kept: Vec<Kept>,
    tick: u64,
    /// Maps still to open in the background, in order.
    queue: VecDeque<String>,
    /// The map being opened in the background, and since when.
    loading: Option<(String, Receiver<Result<Map>>, Instant)>,
    /// Background opens so far: map, ms, bytes.
    pub opened: Vec<(String, f64, u64)>,
    /// The camera each map was last shown from (state.rs): what it is warmed from.
    pub cameras: std::collections::BTreeMap<String, crate::state::Camera>,
}

impl Resident {
    pub fn new(budget: u64) -> Self {
        Self { budget, kept: Vec::new(), tick: 0, queue: VecDeque::new(), loading: None, opened: Vec::new(), cameras: Default::default() }
    }

    /// What the kept maps hold on the GPU.
    pub fn held(&self) -> u64 {
        self.kept.iter().map(|k| k.map.gpu_bytes).sum()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.kept.iter().any(|k| k.name == name)
    }

    /// The kept maps' names.
    pub fn names(&self) -> Vec<String> {
        self.kept.iter().map(|k| k.name.clone()).collect()
    }

    /// The map being opened in the background.
    pub fn loading(&self) -> Option<&str> {
        self.loading.as_ref().map(|(name, _, _)| name.as_str())
    }

    /// Whether nothing is left to open in the background.
    pub fn settled(&self) -> bool {
        self.loading.is_none() && self.queue.is_empty()
    }

    /// `name`'s map, kept.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Map> {
        self.kept.iter_mut().find(|k| k.name == name).map(|k| &mut k.map)
    }

    /// Takes `name`'s map out to show it.
    pub fn take(&mut self, name: &str) -> Option<Map> {
        let at = self.kept.iter().position(|k| k.name == name)?;
        Some(self.kept.swap_remove(at).map)
    }

    /// Keeps `map` (just shown, or just opened) beside the shown one, which holds `shown` bytes; returns the maps let go
    /// to make room, least recently shown first. A map that alone does not fit beside the shown one is not kept.
    pub fn keep(&mut self, name: String, map: Map, shown: u64) -> Vec<String> {
        self.tick += 1;
        self.kept.retain(|k| k.name != name);
        let needed = map.gpu_bytes;
        let mut gone = to_let_go(self.kept.iter().map(|k| (k.name.as_str(), k.map.gpu_bytes, k.used)), needed + shown, self.budget);
        self.kept.retain(|k| !gone.contains(&k.name));
        if self.held() + needed + shown <= self.budget {
            self.kept.push(Kept { name, map, used: self.tick });
        } else {
            gone.push(name);
        }
        gone
    }

    /// Lets go of kept maps, least recently shown first, until `needed` more fits beside the shown map's `shown` bytes;
    /// returns their names.
    pub fn make_room(&mut self, needed: u64, shown: u64) -> Vec<String> {
        let gone = to_let_go(self.kept.iter().map(|k| (k.name.as_str(), k.map.gpu_bytes, k.used)), needed + shown, self.budget);
        self.kept.retain(|k| !gone.contains(&k.name));
        gone
    }

    /// Opens these maps in the background, in this order, after anything already asked for.
    pub fn preload(&mut self, maps: impl IntoIterator<Item = String>) {
        for map in maps {
            if !self.queue.contains(&map) && !self.contains(&map) && self.loading() != Some(map.as_str()) {
                self.queue.push_back(map);
            }
        }
    }

    /// Starts opening the next map in line that fits beside the shown one (`shown`, which holds `shown_bytes`), unless
    /// one is being opened. A map that would not fit is passed over: it opens when picked.
    pub fn next(&mut self, gpu: &nades_render::Gpu, shown: &str, shown_bytes: u64, wake: Option<Arc<dyn Fn() + Send + Sync>>) {
        if self.loading.is_some() {
            return;
        }
        while let Some(name) = self.queue.pop_front() {
            if name == shown || self.contains(&name) {
                continue;
            }
            let path = crate::pack_path(Some(&name));
            if self.held() + shown_bytes + expected_bytes(&path) > self.budget {
                continue;
            }
            self.loading = Some((name.clone(), load(&gpu.device, &gpu.queue, path, self.cameras.get(&name).copied(), wake), Instant::now()));
            return;
        }
    }

    /// The map opened in the background, once it is on the GPU: its name, the map (or why not), how long it took.
    pub fn arrived(&mut self) -> Option<(String, Result<Map>, f64)> {
        let (_, answer, _) = self.loading.as_ref()?;
        let result = answer.try_recv().ok()?;
        let (name, _, since) = self.loading.take().expect("loading");
        let ms = since.elapsed().as_secs_f64() * 1e3;
        if let Ok(map) = &result {
            self.opened.push((name.clone(), ms, map.gpu_bytes));
        }
        Some((name, result, ms))
    }
}

/// The budget the maps share: `--budget-mb`, else `NADES_SANDBOX_BUDGET_MB`, else what the GPU allows
/// ([`nades_render::memory::budget`]).
pub fn budget(gpu: &nades_render::Gpu, asked_mb: Option<u64>) -> u64 {
    let asked = asked_mb.or_else(|| std::env::var("NADES_SANDBOX_BUDGET_MB").ok().and_then(|v| v.parse().ok())).map(|mb| mb << 20);
    nades_render::memory::budget(asked, nades_render::memory::recommended_working_set(&gpu.device))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_least_recently_shown_maps_go_first_until_the_new_one_fits() {
        let kept = [("de_mirage", 300, 5), ("de_cache", 1_200, 2), ("de_nuke", 600, 9), ("de_dust2", 500, 1)];
        // 2,600 held: 600 more fits 3,200, nothing goes
        assert!(to_let_go(kept, 600, 3_200).is_empty());
        // 600 more within 2,800: dust2 (shown longest ago) goes, then cache
        assert_eq!(to_let_go(kept, 600, 2_800), ["de_dust2"]);
        assert_eq!(to_let_go(kept, 1_000, 2_800), ["de_dust2", "de_cache"]);
        // more than the budget can hold: everything goes
        assert_eq!(to_let_go(kept, 5_000, 2_800).len(), 4);
        assert!(to_let_go([], 5_000, 2_800).is_empty());
    }

    #[test]
    fn a_pack_is_expected_to_hold_a_tenth_more_than_its_file() {
        let dir = std::env::temp_dir().join(format!("nades-resident-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("de_test.pack");
        std::fs::write(&path, vec![0u8; 1000]).unwrap();
        assert_eq!(expected_bytes(&path), 1100);
        assert_eq!(expected_bytes(&dir.join("none.pack")), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
