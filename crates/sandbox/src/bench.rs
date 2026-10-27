//! `--bench-maps`: each map in turn, picked as the shell's selector picks it, timed from the pick to the frame that
//! shows it, with the frames drawn in between, the switching frame's own time and the process's memory once it shows.
//! `--bench-maps all` picks every packed map, the shown one last, after waiting until the background has opened every
//! map that fits.

use std::time::Instant;

use crate::app::{Sandbox, packed_maps};

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

    /// The map being picked and timed, if one is.
    pub fn picking(&self) -> Option<&str> {
        self.asked.as_ref().map(|a| a.map.as_str())
    }

    /// One step, after a frame (`drew`: one was drawn, taking `frame_ms` of the window's time); the report once every
    /// map has been shown.
    pub fn step(&mut self, app: &mut Sandbox, shown_map: &str, drew: bool, frame_ms: f64) -> Option<String> {
        if self.settle && self.settled_ms.is_none() {
            if drew {
                self.opening.push(frame_ms);
            }
            // `preload` still set: the other maps have not started opening (the window is not shown yet)
            if app.preload || !app.resident.settled() {
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
                    asked.parts = app.frame_parts.clone();
                }
            }
            if app.switching() || shown_map != asked.map {
                return None;
            }
            let memory = nades_render::memory::footprint().unwrap_or_default();
            let held = app.resident.held() + app.shown_bytes;
            self.done.push(serde_json::json!({
                "map": asked.map,
                "ms": asked.at.elapsed().as_secs_f64() * 1e3,
                "frames": asked.frames,
                "frame_ms": asked.frame_ms,
                "parts": asked.parts.iter().map(|(part, ms)| serde_json::json!([part, (ms * 100.0).round() / 100.0])).collect::<Vec<_>>(),
                "resident": asked.resident,
                "held_mb": nades_render::memory::mb(held),
                "footprint_mb": memory.now_mb(),
                "graphics_mb": nades_render::memory::mb(memory.graphics),
            }));
            self.asked = None;
        }
        match self.plan.first().cloned() {
            Some(next) => {
                self.plan.remove(0);
                let resident = app.resident.contains(&next);
                self.asked = Some(Asked { map: next.clone(), at: Instant::now(), frames: 0, resident, frame_ms: 0.0, parts: Vec::new() });
                app.ask_map(next);
                None
            }
            None => {
                let memory = nades_render::memory::footprint().unwrap_or_default();
                let mut opening = self.opening.clone();
                opening.sort_by(f64::total_cmp);
                let at = |q: f64| opening.get(((opening.len().max(1) - 1) as f64 * q).round() as usize).copied();
                let opened: Vec<serde_json::Value> = app.resident.opened.iter().map(|(map, ms, bytes)| serde_json::json!({ "map": map, "ms": ms, "mb": nades_render::memory::mb(*bytes) })).collect();
                Some(
                    serde_json::json!({
                        "maps": self.done,
                        "settled_ms": self.settled_ms,
                        "while_opening": { "frames": opening.len(), "median_ms": at(0.5), "p95_ms": at(0.95), "max_ms": at(1.0) },
                        "opened": opened,
                        "resident": app.resident.names(),
                        "budget_mb": nades_render::memory::mb(app.resident.budget),
                        "held_mb": nades_render::memory::mb(app.resident.held() + app.shown_bytes),
                        "footprint_mb": memory.now_mb(),
                        "peak_footprint_mb": nades_render::memory::mb(memory.peak),
                    })
                    .to_string(),
                )
            }
        }
    }
}
