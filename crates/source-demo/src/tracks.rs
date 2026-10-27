//! What a grenade leaves over time: where it flew, the voxel log a smoke networks so clients can simulate its cloud,
//! and the flames a fire spread to. Ported from show-me-the-nades' `tracks.rs`, where it is checked against real demos.

use awpy::entity::field_path::FieldPath;
use awpy::{Entity, Serializer};

use crate::extract::{boolean, get, num, uint, vec3};

/// Ticks between stored projectile positions.
pub const PATH_EVERY: i32 = 2;
/// Length of the networked `m_VoxelFrameData` array.
pub const VOXEL_CAPACITY: usize = 3072;
/// Flames one inferno networks.
pub const MAX_FLAMES: usize = 64;
/// Closer than this to the last sample counts as not having moved.
const AT_REST: f32 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathPoint {
    pub tick: i32,
    pub pos: [f32; 3],
}

/// One smoke's voxel log as networked, with the point its grid is centred on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmokeVoxels {
    /// The tick the first frame arrived (`m_nVoxelFrameDataSize` first non-zero).
    pub first_tick: i32,
    /// `m_vSmokeDetonationPos`.
    pub detonation: [f32; 3],
    pub log: Vec<u8>,
}

/// A flame a fire spread to, and the ticks it first and last burned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flame {
    pub slot: usize,
    pub pos: [f32; 3],
    pub normal: Option<[f32; 3]>,
    pub first_tick: i32,
    pub last_tick: i32,
}

fn moved(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2) >= AT_REST * AT_REST
}

/// Adds a point when `PATH_EVERY` ticks have passed and the grenade has moved, so a resting smoke or decoy adds nothing
/// for the rest of its life.
pub fn sample_path(path: &mut Vec<PathPoint>, tick: i32, pos: [f32; 3]) {
    match path.last() {
        Some(last) if tick - last.tick < PATH_EVERY || !moved(last.pos, pos) => {}
        _ => path.push(PathPoint { tick, pos }),
    }
}

/// Ends a path where the projectile last was, unless it already ends there.
pub fn finish_path(path: &mut Vec<PathPoint>, tick: i32, pos: [f32; 3]) {
    if path.last().is_none_or(|last| last.tick < tick && moved(last.pos, pos)) {
        path.push(PathPoint { tick, pos });
    }
}

pub struct SmokeKeys {
    size: Option<u64>,
    detonation: Option<u64>,
    bytes: Vec<Option<u64>>,
}

impl SmokeKeys {
    pub fn new(s: &Serializer) -> Self {
        SmokeKeys {
            size: s.resolve_field_key("m_nVoxelFrameDataSize"),
            detonation: s.resolve_field_key("m_vSmokeDetonationPos"),
            bytes: (0..VOXEL_CAPACITY).map(|i| s.resolve_field_key(&format!("m_VoxelFrameData.{i}"))).collect(),
        }
    }

    /// Appends what the server added to the log since the last read. The entity carries every element from its
    /// baseline, so only `m_nVoxelFrameDataSize` says how much is in use.
    pub fn read(&self, e: &Entity, tick: i32, out: &mut Option<SmokeVoxels>) {
        let size = num(get(e, self.size)).map_or(0, |n| n as usize).min(VOXEL_CAPACITY);
        if size == 0 || out.as_ref().is_some_and(|v| v.log.len() >= size) {
            return;
        }
        let v = out.get_or_insert_with(|| SmokeVoxels { first_tick: tick, ..Default::default() });
        if let Some(d) = vec3(get(e, self.detonation)) {
            v.detonation = d;
        }
        for key in &self.bytes[v.log.len()..size] {
            v.log.push(uint(get(e, *key)).unwrap_or(0) as u8);
        }
    }
}

/// Keys for every element of a fixed array. Each element has the array's own name, so names cannot address them; their
/// field paths differ only in the last component.
fn elements(s: &Serializer, name: &str, n: usize) -> Vec<Option<u64>> {
    let Some(first) = s.resolve_field_key(&format!("{name}.{name}")) else {
        return vec![None; n];
    };
    let path = FieldPath::unpack(first);
    (0..n)
        .map(|i| {
            let mut p = path;
            p.data[p.last] = i16::try_from(i).ok()?;
            p.try_pack()
        })
        .collect()
}

pub struct InfernoKeys {
    count: Option<u64>,
    pos: Vec<Option<u64>>,
    normal: Vec<Option<u64>>,
    burning: Vec<Option<u64>>,
}

impl InfernoKeys {
    pub fn new(s: &Serializer) -> Self {
        InfernoKeys {
            count: s.resolve_field_key("m_fireCount"),
            pos: elements(s, "m_firePositions", MAX_FLAMES),
            normal: elements(s, "m_BurnNormal", MAX_FLAMES),
            burning: elements(s, "m_bFireIsBurning", MAX_FLAMES),
        }
    }

    /// Notes which flames burn at `tick`.
    pub fn read(&self, e: &Entity, tick: i32, flames: &mut Vec<Option<Flame>>) {
        flames.resize(MAX_FLAMES, None);
        let count = num(get(e, self.count)).map_or(0, |n| n as usize).min(MAX_FLAMES);
        for (i, slot) in flames.iter_mut().enumerate().take(count) {
            if boolean(get(e, self.burning[i])) != Some(true) {
                continue;
            }
            match slot {
                Some(f) => f.last_tick = tick,
                None => {
                    if let Some(pos) = vec3(get(e, self.pos[i])) {
                        *slot = Some(Flame { slot: i, pos, normal: vec3(get(e, self.normal[i])), first_tick: tick, last_tick: tick });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_keeps_a_point_every_two_ticks_while_moving_and_ends_where_it_stopped() {
        let mut path = vec![PathPoint { tick: 100, pos: [0.0; 3] }];
        for (tick, x) in [(101, 5.0), (102, 10.0), (104, 10.2), (106, 30.0), (107, 31.0)] {
            sample_path(&mut path, tick, [x, 0.0, 0.0]);
        }
        finish_path(&mut path, 107, [31.0, 0.0, 0.0]);
        assert_eq!(path.iter().map(|p| (p.tick, p.pos[0])).collect::<Vec<_>>(), [(100, 0.0), (102, 10.0), (106, 30.0), (107, 31.0)]);
        finish_path(&mut path, 107, [31.0, 0.0, 0.0]);
        assert_eq!(path.len(), 4, "already ends there");
    }
}
