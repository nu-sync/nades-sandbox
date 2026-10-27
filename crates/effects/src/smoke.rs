//! A smoke's voxel log, simulated (S33): the emitters, and which bricks the server sends when, from a flood of the
//! map's open voxels out of the grenade's voxel. The flood is the server's cloud as far as the log can tell: it fills
//! the cheapest voxels first, a step up costing more than a step sideways, and a brick is sent once the cloud comes
//! within a voxel of it, until the smoke has its bricks.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use nades_model::smoke::Emitter;
use nades_scene::smoke::{CELLS, GRID, Grid, NEVER, cell};

/// The numbers the simulation runs on; `Model::default()` is the one measured against recorded smokes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    /// Voxels the cloud holds once it has spread, and at its first frame: they pace the bricks over time.
    pub volume: f64,
    pub first_volume: f64,
    /// Frames the cloud takes to cover most (1 − 1/e) of the way from its first volume to its full one.
    pub spread_frames: f64,
    /// Bricks the server sends at most, the first the cloud comes near.
    pub bricks: usize,
    /// Cost of a step up and of a step down, against 1 for a step sideways; a diagonal step costs its length.
    pub up: f64,
    pub down: f64,
    /// Emitters: the first this many voxels of the flood that lie on a 40-unit lattice through the grenade's voxel.
    pub emitters: usize,
    pub frames: usize,
    pub fading_from: usize,
}

impl Default for Model {
    fn default() -> Self {
        Self { volume: 1800.0, first_volume: 450.0, spread_frames: 15.0, bricks: 78, up: 1.75, down: 0.75, emitters: 44, frames: 202, fading_from: 174 }
    }
}

/// The voxel the detonation point lies in: the grid's minimum corner is the detonation minus half its span.
pub const CENTRE: [i32; 3] = [GRID as i32 / 2; 3];

fn coords(c: usize) -> [i32; 3] {
    [(c % GRID) as i32, ((c / GRID) % GRID) as i32, (c / (GRID * GRID)) as i32]
}

fn at(p: [i32; 3]) -> Option<usize> {
    p.iter().all(|v| (0..GRID as i32).contains(v)).then(|| cell(p[0] as usize, p[1] as usize, p[2] as usize))
}

fn brick_of(p: [i32; 3]) -> usize {
    (p[0] / 4 + 8 * (p[1] / 4 + 8 * (p[2] / 4))) as usize
}

/// Every open voxel reachable from `start`, cheapest first: Dijkstra over the 26 neighbours, a step costing its
/// length with its vertical part scaled by `up` or `down`, never cutting a solid corner.
fn spread(solid: &[u8], start: usize, model: &Model) -> Vec<usize> {
    let mut steps = Vec::with_capacity(26);
    for dz in -1i32..=1 {
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if (dx, dy, dz) == (0, 0, 0) {
                    continue;
                }
                let v = f64::from(dz) * if dz > 0 { model.up } else { model.down };
                steps.push(([dx, dy, dz], (f64::from(dx * dx + dy * dy) + v * v).sqrt()));
            }
        }
    }
    let mut cost = vec![f64::INFINITY; CELLS];
    let mut done = vec![false; CELLS];
    let mut order = Vec::new();
    let mut heap = BinaryHeap::new();
    cost[start] = 0.0;
    // costs are non-negative, so their bits order as the numbers do; the index breaks ties the same way every run
    heap.push(Reverse((0u64, start)));
    while let Some(Reverse((_, c))) = heap.pop() {
        if done[c] {
            continue;
        }
        done[c] = true;
        order.push(c);
        let p = coords(c);
        for &(d, w) in &steps {
            let Some(n) = at([p[0] + d[0], p[1] + d[1], p[2] + d[2]]) else { continue };
            if solid[n] != 0 || done[n] || cuts_corner(solid, p, d) {
                continue;
            }
            let next = cost[c] + w;
            if next < cost[n] {
                cost[n] = next;
                heap.push(Reverse((next.to_bits(), n)));
            }
        }
    }
    order
}

/// Whether a diagonal step from `p` along `d` passes a solid voxel among the axis steps it is made of.
fn cuts_corner(solid: &[u8], p: [i32; 3], d: [i32; 3]) -> bool {
    d.iter().filter(|&&v| v != 0).count() > 1
        && (0..3).any(|a| {
            let mut q = p;
            q[a] += d[a];
            d[a] != 0 && at(q).is_none_or(|c| solid[c] != 0)
        })
}

/// The open voxel nearest the grenade's, within three voxels.
fn start(solid: &[u8]) -> Option<usize> {
    let mut best: Option<(i32, usize)> = None;
    for dz in -3i32..=3 {
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                let d2 = dx * dx + dy * dy + dz * dz;
                if let Some(c) = at([CENTRE[0] + dx, CENTRE[1] + dy, CENTRE[2] + dz])
                    && solid[c] == 0
                    && best.is_none_or(|b| (d2, c) < b)
                {
                    best = Some((d2, c));
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

/// The log's grid for a smoke whose solid voxels are `solid` (1 = map geometry). A smoke buried in geometry, with no
/// open voxel near the grenade, sends nothing and has no emitters.
pub fn grid(solid: &[u8], model: &Model) -> Grid {
    let mut sent = vec![0u8; CELLS];
    let mut sent_frame = vec![NEVER; CELLS];
    let mut out_solid = vec![0u8; CELLS];
    let Some(start) = start(solid) else {
        return Grid { solid: out_solid, sent, sent_frame, emitters: Vec::new(), frames: model.frames, fading_from: model.fading_from };
    };
    let order = spread(solid, start, model);
    let s = coords(start);
    let emitters = order
        .iter()
        .map(|&c| coords(c))
        .filter(|p| (0..3).all(|a| (p[a] - s[a]).rem_euclid(2) == 0))
        .take(model.emitters)
        .map(|p| Emitter { cell: p.map(|v| v as u8), level: 5, hold: 0.0 })
        .collect();
    // the cloud holds the first V(f) voxels of the flood at frame f; a brick is sent at the first frame the cloud
    // comes within a voxel of it
    let volume_at = |f: f64| model.volume - (model.volume - model.first_volume) * (-f / model.spread_frames).exp();
    let mut brick_frame = [NEVER; 512];
    let (mut frame, mut taken) = (0usize, 0usize);
    for (k, &c) in order.iter().enumerate() {
        while frame < model.frames && (k as f64) >= volume_at(frame as f64) {
            frame += 1;
        }
        if frame >= model.frames.min(usize::from(NEVER)) || taken >= model.bricks {
            break;
        }
        let p = coords(c);
        let lo = p.map(|v| (v - 1).max(0));
        let hi = p.map(|v| (v + 1).min(GRID as i32 - 1));
        for z in [lo[2], hi[2]] {
            for y in [lo[1], hi[1]] {
                for x in [lo[0], hi[0]] {
                    let b = brick_of([x, y, z]);
                    if brick_frame[b] == NEVER {
                        brick_frame[b] = frame as u8;
                        taken += 1;
                    }
                }
            }
        }
    }
    for c in 0..CELLS {
        let f = brick_frame[brick_of(coords(c))];
        if f != NEVER {
            sent[c] = 1;
            sent_frame[c] = f;
            out_solid[c] = solid[c];
        }
    }
    Grid { solid: out_solid, sent, sent_frame, emitters, frames: model.frames, fading_from: model.fading_from }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use nades_scene::smoke::fill_cloud;

    /// A floor under the grenade, and a wall `wall` voxels to its +x side when given.
    fn room(wall: Option<i32>) -> Vec<u8> {
        (0..CELLS)
            .map(|c| {
                let p = coords(c);
                u8::from(p[2] < CENTRE[2] || wall.is_some_and(|w| p[0] >= CENTRE[0] + w))
            })
            .collect()
    }

    #[test]
    fn an_open_floor_grows_a_flat_cloud_on_it() {
        let grid = grid(&room(None), &Model::default());
        let cloud = fill_cloud(&grid, DVec3::ZERO).expect("a cloud");
        let (min, max) = cloud.bounds;
        assert_eq!(min[2], CENTRE[2] as usize, "it sits on the floor");
        assert!(max[0] - min[0] > 2 * (max[2] - min[2]), "wider than tall: {:?}", cloud.bounds);
        assert_eq!(grid.emitters.len(), 44);
        assert!(grid.emitters.iter().all(|e| e.cell[2] >= CENTRE[2] as u8));
    }

    #[test]
    fn a_wall_stops_the_cloud_and_it_spreads_the_other_way() {
        let open = fill_cloud(&grid(&room(None), &Model::default()), DVec3::ZERO).unwrap();
        let walled = fill_cloud(&grid(&room(Some(3)), &Model::default()), DVec3::ZERO).unwrap();
        assert!(walled.bounds.1[0] < (CENTRE[0] + 3) as usize, "nothing past the wall");
        assert!(walled.bounds.0[0] < open.bounds.0[0] || walled.bounds.0[1] < open.bounds.0[1], "{:?} against {:?}", walled.bounds, open.bounds);
    }

    #[test]
    fn bricks_arrive_over_time_and_the_same_smoke_comes_out_the_same() {
        let solid = room(Some(5));
        let a = grid(&solid, &Model::default());
        let frames: std::collections::BTreeSet<u8> = a.sent_frame.iter().copied().filter(|&f| f != NEVER).collect();
        assert!(frames.len() > 3 && frames.first() == Some(&0), "{frames:?}");
        let b = grid(&solid, &Model::default());
        assert_eq!((a.sent_frame, a.emitters), (b.sent_frame, b.emitters));
    }

    #[test]
    fn a_buried_smoke_sends_nothing() {
        let grid = grid(&vec![1u8; CELLS], &Model::default());
        assert!(grid.emitters.is_empty() && grid.sent.iter().all(|&s| s == 0));
    }
}
