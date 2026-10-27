//! The replay (SPEC.md M5, docs/decisions.md S30): one round of one demo played on the map, with every player (where
//! they stand and look, alive or dead, in their side's colour) and every grenade (in flight, then its smoke, fire,
//! flash or HE at its own time). Its data is read and built off the window's thread ([`ReplayData::read`]); a moment
//! is put on the GPU with [`show`]: the players and throws through the map's throw layer, each smoke in the composite.
//!
//! The clock is the demo's: ticks, 64 a second. Each throw's stage keeps its own clock, seconds after it went off, so a
//! replay's moment is every stage at `(tick − its detonation) / 64`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use glam::DVec3;
use nades_model::replay::{self, PlayerState, TICKS};
use nades_model::{Demo, DemoId, Library as MapLibrary, PlayerTrack, Round, Throw, ThrowId, ThrowSource};
use nades_render::map::{FrameEffects, Map, PointLight};
use nades_render::smoke::SmokeFrame;
use nades_render::throw::ReplayPlayer;
use nades_render::{Gpu, View};
use nades_scene::camera::{View as CsView, vertical_fov};
use nades_scene::collider::Collider;
use nades_scene::smoke::{smoke_volume, voxel_light};
use nades_scene::stage::{Stage, build_stage};

use crate::views::cs_view;

/// A throw in a replay: its facts, its stage and its smoke's lit volume.
pub struct ReplayThrow {
    pub throw: Throw,
    pub stage: Stage,
    pub volume: Option<Vec<u8>>,
    /// The composite keeps a smoke's volume on the GPU by this.
    pub key: u64,
    /// Whether it groups into a lineup (a lineup view can open on it).
    pub lineup: bool,
}

/// One round, read and built.
pub struct ReplayData {
    pub demo: Demo,
    pub round: Round,
    /// The ticks it plays over.
    pub start: i32,
    pub end: i32,
    pub tracks: Vec<PlayerTrack>,
    /// Whether the players come from tracks (16 a second), not from the samples (two a second).
    pub smooth: bool,
    pub throws: Vec<ReplayThrow>,
    pub build_ms: f64,
}

/// What a moment shows: every player, and each throw still showing with its stage's time.
pub struct Moment {
    pub players: Vec<PlayerState>,
    /// Index into [`ReplayData::throws`], and seconds after its detonation.
    pub live: Vec<(usize, f64)>,
}

fn key_of(id: &ThrowId) -> u64 {
    let mut h = DefaultHasher::new();
    id.0.hash(&mut h);
    h.finish()
}

impl ReplayData {
    /// `demo`'s round `number` on `library`'s map, from `source`; smokes lit against `collider` when there is one.
    pub fn read(source: &dyn ThrowSource, library: &MapLibrary, demo: &DemoId, number: u32, collider: Option<&Collider>, toward_sun: DVec3) -> Result<Self> {
        let started = Instant::now();
        let demo_facts = library.demo(demo).cloned().with_context(|| format!("no demo {} on this map", demo.0))?;
        let round = library.rounds_of(demo).into_iter().find(|r| r.number == number).cloned().with_context(|| format!("{} has no round {number}", demo_facts.file_name.as_deref().unwrap_or(&demo.0)))?;
        let (Some(freeze_end), Some(end)) = (round.freeze_end, round.end) else { bail!("round {number} has no freeze end or end tick") };
        let mut tracks = source.player_tracks(demo, number)?;
        let smooth = !tracks.is_empty();
        // a player without a track moves between their samples
        for from_samples in replay::tracks_from_samples(&source.samples(demo, freeze_end..=end)?, &round) {
            if !tracks.iter().any(|t| t.player == from_samples.player) {
                tracks.push(from_samples);
            }
        }
        let mut throws = Vec::new();
        for t in replay::round_throws(library.throws().map(|(t, _)| t), &round) {
            let (Some(derived), Some(facts)) = (library.derived(&t.id), source.tracks(&t.id)?) else { continue };
            let stage = build_stage(t, &facts, derived);
            let volume = stage.cloud.as_ref().map(|cloud| smoke_volume(cloud, collider.map(|c| voxel_light(cloud, toward_sun, c)).as_ref()));
            throws.push(ReplayThrow { key: key_of(&t.id), throw: t.clone(), volume, lineup: derived.lineup.is_some(), stage });
        }
        // on past the round's end while a throw of it still shows
        let last = throws.iter().map(|t| t.stage.detonate_tick + (t.stage.timeline().1 * TICKS).ceil() as i32).max();
        let (start, end) = replay::span(&round, last.map(nades_model::Tick)).map(|(a, b)| (a.0, b.0)).context("an empty round")?;
        Ok(Self { demo: demo_facts, round, start, end, tracks, smooth, throws, build_ms: started.elapsed().as_secs_f64() * 1e3 })
    }

    /// Seconds the replay runs.
    pub fn length(&self) -> f64 {
        f64::from(self.end - self.start) / TICKS
    }

    /// The tick `seconds` into the replay.
    pub fn tick_at(&self, seconds: f64) -> f64 {
        f64::from(self.start) + seconds.clamp(0.0, self.length()) * TICKS
    }

    /// Seconds into the replay of `tick`.
    pub fn seconds_at(&self, tick: i32) -> f64 {
        f64::from(tick - self.start) / TICKS
    }

    pub fn at(&self, tick: f64) -> Moment {
        let live = self
            .throws
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let s = (tick - f64::from(t.stage.detonate_tick)) / TICKS;
                let released = f64::from(t.throw.tick.0 - t.stage.detonate_tick) / TICKS;
                (s >= released && s <= t.stage.timeline().1 + 0.25).then_some((i, s))
            })
            .collect();
        Moment { players: replay::players_at(&self.tracks, tick), live }
    }

    /// The players a camera can follow, in the order the shell lists them: T first, then CT, by name.
    pub fn roster(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.tracks.len()).collect();
        order.sort_by_key(|&i| (self.tracks[i].side != nades_model::Side::T, self.tracks[i].player.name.clone().unwrap_or_default().to_lowercase()));
        order
    }

    /// A camera over the moment at `tick`: above the middle of the living players, looking down at them at a slant.
    pub fn overview(&self, tick: f64) -> CsView {
        let mut points: Vec<DVec3> = replay::players_at(&self.tracks, tick).iter().filter(|s| s.alive).map(|s| DVec3::new(s.origin[0].into(), s.origin[1].into(), s.origin[2].into())).collect();
        if points.is_empty() {
            points = self.tracks.iter().flat_map(|t| t.points.iter().step_by(16)).map(|p| DVec3::new(p.origin[0].into(), p.origin[1].into(), p.origin[2].into())).collect();
        }
        let (lo, hi) = points.iter().fold((DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let centre = if points.is_empty() { DVec3::ZERO } else { (lo + hi) * 0.5 };
        // close enough that a player reads, and low enough not to see past the map's edges
        let extent = if points.is_empty() { 1000.0 } else { (hi.x - lo.x).max(hi.y - lo.y).clamp(800.0, 2200.0) };
        let fov = vertical_fov();
        let distance = extent * 0.6 / (fov.to_radians() * 0.5).tan();
        let (pitch, yaw) = (55.0f64, 90.0f64);
        let forward = DVec3::new(pitch.to_radians().cos() * yaw.to_radians().cos(), pitch.to_radians().cos() * yaw.to_radians().sin(), -pitch.to_radians().sin());
        CsView { eye: centre - forward * distance, pitch, yaw, fov }
    }

    /// A camera looking straight down on `centre` from `height` units above it, for checking where things are.
    pub fn top(centre: DVec3, height: f64) -> CsView {
        CsView { eye: centre + DVec3::new(0.0, 0.0, height), pitch: 89.9, yaw: 90.0, fov: vertical_fov() }
    }
}

/// Where `track`'s player looks from at `tick`: their eye, their angles.
pub fn follow(state: &PlayerState) -> CsView {
    CsView { eye: DVec3::new(state.origin[0].into(), state.origin[1].into(), f64::from(state.origin[2]) + nades_scene::camera::EYE_STANDING), pitch: state.pitch.into(), yaw: state.yaw.into(), fov: vertical_fov() }
}

/// A camera behind and above `state`'s player, looking at them along their yaw.
pub fn chase(state: &PlayerState) -> CsView {
    let yaw = f64::from(state.yaw);
    let back = DVec3::new(-yaw.to_radians().cos(), -yaw.to_radians().sin(), 0.0);
    let head = DVec3::new(state.origin[0].into(), state.origin[1].into(), f64::from(state.origin[2]) + 56.0);
    let eye = head + back * 170.0 + DVec3::new(0.0, 0.0, 90.0);
    let d = head - eye;
    let pitch = (-d.z).atan2(d.x.hypot(d.y)).to_degrees();
    CsView { eye, pitch, yaw, fov: vertical_fov() }
}

/// Puts the moment at `tick` on `map`, drawn from `eye` into targets of `size`: the players (not `hidden`, the one a
/// camera looks out of), the throws, the smokes. The frame's effects come back: the brightest burst's or fire's light.
#[allow(clippy::too_many_arguments)]
pub fn show(gpu: &Gpu, map: &mut Map, data: &ReplayData, tick: f64, followed: Option<usize>, hidden: Option<usize>, eye: &View, size: (u32, u32), accumulated: u32) -> FrameEffects {
    show_layers(gpu, map, data, tick, followed, hidden, eye, size, accumulated, (true, true))
}

/// [`show`], with the players and the throws each on or off.
#[allow(clippy::too_many_arguments)]
pub fn show_layers(gpu: &Gpu, map: &mut Map, data: &ReplayData, tick: f64, followed: Option<usize>, hidden: Option<usize>, eye: &View, size: (u32, u32), accumulated: u32, (with_players, with_throws): (bool, bool)) -> FrameEffects {
    let mut moment = data.at(tick);
    if !with_players {
        moment.players.clear();
    }
    if !with_throws {
        moment.live.clear();
    }
    let player_of = |s: &PlayerState| data.tracks.iter().position(|t| t.player == s.player);
    let players: Vec<ReplayPlayer> = moment
        .players
        .iter()
        .filter(|s| hidden.is_none() || player_of(s) != hidden)
        .map(|s| ReplayPlayer { origin: s.origin, pitch: s.pitch, yaw: s.yaw, ct: s.side == nades_model::Side::Ct, alive: s.alive, followed: followed.is_some() && player_of(s) == followed })
        .collect();
    let stages: Vec<(&Stage, f64)> = moment.live.iter().map(|&(i, t)| (&data.throws[i].stage, t)).collect();
    map.throw.set_replay(&gpu.device, &gpu.queue, &stages, &players, eye, size);
    let clouds: Vec<_> = moment
        .live
        .iter()
        .filter_map(|&(i, t)| {
            let th = &data.throws[i];
            let cloud = th.stage.cloud.as_ref().filter(|_| t > 0.0)?;
            Some((th.key, cloud, th.volume.as_deref()?, Some(SmokeFrame { seconds: t as f32, fade: cloud.fade_at(t) as f32, accumulated })))
        })
        .collect();
    map.composite.set_clouds(&gpu.device, &gpu.queue, &clouds);
    let light = stages
        .iter()
        .filter_map(|(s, t)| s.light_at(*t))
        .max_by(|a, b| a.intensity.total_cmp(&b.intensity))
        .map(|l| PointLight { at: glam::Vec3::new(l.at.x as f32, l.at.y as f32, l.at.z as f32), color: l.color.map(|c| c as f32), intensity: l.intensity as f32, distance: l.distance as f32, decay: l.decay as f32 });
    FrameEffects { smoke: None, light, accumulated }
}

/// A demo by its id, an id's start, or its file's name (with or without `.dem`).
pub fn find_demo<'a>(demos: impl IntoIterator<Item = &'a Demo>, wanted: &str) -> Option<&'a Demo> {
    let wanted = wanted.trim_end_matches(".dem");
    demos.into_iter().find(|d| d.id.0 == wanted || (wanted.len() >= 8 && d.id.0.starts_with(wanted)) || d.file_name.as_deref().is_some_and(|f| f.trim_end_matches(".dem") == wanted))
}

/// What `--replay` asks for.
pub struct Request {
    pub demo: String,
    pub round: u32,
    /// Seconds into the round's replay.
    pub time: f64,
    /// The camera: `overview` (the default), `top`, `chase:<player>`, or a player's name to look out of.
    pub camera: Option<String>,
    pub map: Option<String>,
    /// What to draw over the map: `all` (the default), `players`, `effects` or `map` (nothing), for checking a picture
    /// against the facts by difference.
    pub layers: Option<String>,
}

/// Opens the request's demo, its map's library and round.
fn open_request(source: &dyn ThrowSource, request: &Request) -> Result<(String, Arc<MapLibrary>, DemoId)> {
    let maps = match &request.map {
        Some(m) => vec![m.clone()],
        None => source.maps()?.into_iter().map(|m| m.0).collect(),
    };
    for map in maps {
        let library = crate::library::read_library(source, &map)?;
        if let Some(demo) = find_demo(library.demos(), &request.demo) {
            let id = demo.id.clone();
            return Ok((map, library, id));
        }
    }
    bail!("no demo {} in the data", request.demo)
}

/// The camera a request names at `tick`, and the player it looks out of.
fn request_camera(data: &ReplayData, camera: Option<&str>, tick: f64) -> Result<(CsView, Option<usize>, bool)> {
    let player = |name: &str| -> Result<(usize, PlayerState)> {
        let i = data.tracks.iter().position(|t| t.player.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))).with_context(|| format!("no player {name} in the round"))?;
        let state = replay::player_at(&data.tracks[i], tick).with_context(|| format!("{name} is not in the round yet"))?;
        Ok((i, state))
    };
    Ok(match camera {
        None | Some("overview") => (data.overview(tick), None, false),
        Some(chase_of) if chase_of.starts_with("chase:") => {
            let (i, state) = player(&chase_of["chase:".len()..])?;
            (chase(&state), Some(i), true)
        }
        Some("top") => {
            let o = data.overview(tick);
            let forward = DVec3::new(o.pitch.to_radians().cos() * o.yaw.to_radians().cos(), o.pitch.to_radians().cos() * o.yaw.to_radians().sin(), -o.pitch.to_radians().sin());
            // the overview looks at the round's middle from about its height above it
            let centre = o.eye + forward * (o.eye.z / -forward.z).abs().min(5000.0);
            (ReplayData::top(DVec3::new(centre.x, centre.y, 0.0), o.eye.z - centre.z + 600.0), None, false)
        }
        Some(name) => {
            let (i, state) = player(name)?;
            (follow(&state), Some(i), false)
        }
    })
}

/// `--replay … --render out.png`: the round's moment offscreen, its smokes averaged over the jittered frames a still
/// view takes; with `bench` frames, plays the round from that moment instead and reports the frame times.
pub fn render(source: &dyn ThrowSource, request: &Request, (width, height): (u32, u32), out: Option<&Path>, bench: Option<u32>) -> Result<()> {
    let (map_name, library, demo) = open_request(source, request)?;
    let (pack, scene) = crate::resident::open_cached(&crate::pack_path(Some(&map_name)))?;
    let gpu = Gpu::new(nades_render::instance(), None)?;
    if !gpu.capabilities.bindless {
        bail!("{} has no texture binding arrays, which the renderer draws every material through", gpu.name());
    }
    let mut map = Map::new(&gpu.device, &gpu.queue, pack, &scene, crate::OFFSCREEN_FORMAT, &|_| {});
    let targets = map.targets(&gpu.device, &gpu.queue, width, height);
    let toward_sun = crate::lineups::toward_sun(&map);
    let collider = crate::assets::collider(&map_name).ok();
    let data = ReplayData::read(source, &library, &demo, request.round, collider.as_ref(), toward_sun)?;
    let (noise, _, books) = crate::assets::effects();
    map.composite.set_noise(&gpu.device, &nades_render::smoke::noise_texture(&gpu.device, &gpu.queue, &noise));
    if let Some(books) = books {
        map.throw.set_flipbooks(&gpu.device, &books.upload(&gpu.device, &gpu.queue));
    }
    map.ready_replay(&gpu.device, &targets);
    let tick = data.tick_at(request.time);
    eprintln!(
        "{} round {} on {map_name}: {:.1} s, {} players ({}), {} throws, built in {:.0} ms; at {:.2} s (tick {tick:.0})",
        data.demo.file_name.as_deref().unwrap_or(&data.demo.id.0),
        data.round.number,
        data.length(),
        data.tracks.len(),
        if data.smooth { "tracks" } else { "samples" },
        data.throws.len(),
        data.build_ms,
        request.time
    );
    if let Some(frames) = bench {
        return bench_frames(&gpu, &mut map, &targets, &data, request, tick, frames);
    }
    let (camera, looking, chased) = request_camera(&data, request.camera.as_deref(), tick)?;
    let hidden = looking.filter(|_| !chased);
    let eye = cs_view(camera.eye, camera.pitch, camera.yaw, camera.fov);
    let moment = data.at(tick);
    for (i, t) in &moment.live {
        let th = &data.throws[*i];
        eprintln!("  {} {} by {} at {t:+.2} s", th.stage.grenade.name(), th.throw.id.0, th.throw.player.name.as_deref().unwrap_or("?"));
    }
    let layers = match request.layers.as_deref() {
        None | Some("all") => (true, true),
        Some("players") => (true, false),
        Some("effects") => (false, true),
        Some("map") => (false, false),
        Some(other) => bail!("--replay-layers {other}: all, players, effects or map"),
    };
    let frames = if moment.live.iter().any(|(i, t)| data.throws[*i].stage.cloud.is_some() && *t > 0.0) { nades_render::smoke::ACCUMULATE } else { 1 };
    for k in 0..frames {
        let effects = show_layers(&gpu, &mut map, &data, tick, looking, hidden, &eye, (width, height), k, layers);
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("replay") });
        map.render(&gpu.queue, &mut encoder, &targets, &eye, 0.0, &effects);
        gpu.queue.submit([encoder.finish()]);
    }
    let out = out.context("--replay needs --render <file.png> or --bench-replay <frames>")?;
    crate::write_frame(&gpu, &map, &targets, (width, height), out)?;
    // where each player and live throw is, for checking the picture against the facts
    let facts = out.with_extension("json");
    let players: Vec<serde_json::Value> = moment
        .players
        .iter()
        .map(|s| serde_json::json!({"name": s.player.name, "steamid": s.player.steamid.map(|i| i.to_string()), "side": s.side.name(), "origin": s.origin, "pitch": s.pitch, "yaw": s.yaw, "alive": s.alive}))
        .collect();
    let throws: Vec<serde_json::Value> = moment
        .live
        .iter()
        .map(|&(i, t)| {
            let th = &data.throws[i];
            let at = th.stage.flight.as_ref().and_then(|f| f.grenade_at(t)).map(|(p, _)| [p.x, p.y, p.z]);
            serde_json::json!({"id": th.throw.id.0, "grenade": th.stage.grenade.name(), "seconds": t, "in_flight": at, "focus": [th.stage.focus.x, th.stage.focus.y, th.stage.focus.z], "smoke": th.stage.cloud.is_some() && t > 0.0, "fire": th.stage.fire.is_some() && t > 0.0})
        })
        .collect();
    let camera = serde_json::json!({"eye": [camera.eye.x, camera.eye.y, camera.eye.z], "pitch": camera.pitch, "yaw": camera.yaw, "fov": camera.fov, "size": [width, height]});
    std::fs::write(&facts, serde_json::to_vec_pretty(&serde_json::json!({"map": map_name, "demo": data.demo.id.0, "round": data.round.number, "tick": tick, "seconds": request.time, "smooth": data.smooth, "camera": camera, "players": players, "throws": throws}))?)?;
    eprintln!("wrote {} and {}", out.display(), facts.display());
    Ok(())
}

/// `--bench-replay <frames>`: the round played from `tick` at one second a second (60 frames a second), each frame's
/// set-up on the CPU and its GPU time apart, offscreen.
fn bench_frames(gpu: &Gpu, map: &mut Map, targets: &nades_render::map::Targets, data: &ReplayData, request: &Request, tick: f64, frames: u32) -> Result<()> {
    let (camera, looking, chased) = request_camera(data, request.camera.as_deref(), tick)?;
    let hidden = looking.filter(|_| !chased);
    let size = targets.size();
    let mut setups = Vec::new();
    let mut totals = Vec::new();
    let mut most = (0, 0);
    for k in 0..frames + 10 {
        let now = tick + f64::from(k) * TICKS / 60.0;
        let eye_view = match looking {
            Some(i) => replay::player_at(&data.tracks[i], now).map_or(camera, |s| if chased { chase(&s) } else { follow(&s) }),
            None => camera,
        };
        let eye = cs_view(eye_view.eye, eye_view.pitch, eye_view.yaw, eye_view.fov);
        let started = Instant::now();
        let effects = show(gpu, map, data, now, looking, hidden, &eye, size, 0);
        let setup = started.elapsed().as_secs_f64() * 1e3;
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("replay bench") });
        map.render(&gpu.queue, &mut encoder, targets, &eye, 0.0, &effects);
        gpu.queue.submit([encoder.finish()]);
        gpu.wait_idle();
        let total = started.elapsed().as_secs_f64() * 1e3;
        // the first frames warm the GPU's pipelines and the volumes' uploads
        if k >= 10 {
            setups.push(setup);
            totals.push(total);
            let m = data.at(now);
            most = most.max((m.live.len(), m.players.len()));
        }
    }
    let q = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(f64::total_cmp);
        v[((v.len() - 1) as f64 * p) as usize]
    };
    println!(
        "replay bench: {frames} frames at {}x{} from {:.1} s ({} throws live at most): set-up median {:.2} ms, p95 {:.2}; frame (set-up, draw, GPU) median {:.2} ms, p95 {:.2}, worst {:.2}",
        size.0,
        size.1,
        request.time,
        most.0,
        q(&mut setups, 0.5),
        q(&mut setups, 0.95),
        q(&mut totals, 0.5),
        q(&mut totals, 0.95),
        q(&mut totals, 1.0)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nades_model::{Player, Side, Tick, TrackPoint};

    #[test]
    fn a_demo_is_found_by_its_id_its_start_or_its_file() {
        let demo = |id: &str, file: &str| Demo { id: DemoId(id.into()), map: nades_model::MapName("de_x".into()), played_at: None, file_name: Some(file.into()), tags: Vec::new() };
        let demos = [demo("aaaa1111bbbb2222", "first.dem"), demo("cccc3333dddd4444", "second.dem")];
        assert_eq!(find_demo(&demos, "cccc3333").map(|d| d.id.0.as_str()), Some("cccc3333dddd4444"));
        assert_eq!(find_demo(&demos, "first").map(|d| d.id.0.as_str()), Some("aaaa1111bbbb2222"));
        assert_eq!(find_demo(&demos, "second.dem").map(|d| d.id.0.as_str()), Some("cccc3333dddd4444"));
        assert!(find_demo(&demos, "cccc").is_none(), "too short a start to be sure");
    }

    #[test]
    fn a_followed_player_is_seen_from_their_eye() {
        let state = PlayerState { player: Player { name: Some("a".into()), steamid: None }, side: Side::T, origin: [10.0, 20.0, 30.0], pitch: 5.0, yaw: -90.0, alive: true };
        let v = follow(&state);
        assert_eq!((v.eye, v.pitch, v.yaw), (DVec3::new(10.0, 20.0, 30.0 + nades_scene::camera::EYE_STANDING), 5.0, -90.0));
        let track = PlayerTrack { player: state.player.clone(), side: Side::T, died: None, points: vec![TrackPoint { tick: Tick(1), origin: [0.0; 3], pitch: 0.0, yaw: 0.0 }] };
        assert!(replay::player_at(&track, 1.0).is_some());
    }
}
