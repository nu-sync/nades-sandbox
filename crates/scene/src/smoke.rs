//! A CS2 smoke's cloud, rebuilt from the voxel log its projectile networks, and the volume CS2's smoke shader reads:
//! ports of show-me-the-nades' `web/app/src/scene/smoke.ts` (the cloud) and `render/volumes.ts` (the volume and the
//! light the map lets reach each voxel), number for number. The log is decoded by nades-model's port of the game's
//! encoding (`nades_model::smoke`).
//!
//! Voxels are 20 units on a 32³ grid whose minimum corner is the detonation point minus 320 on every axis. A brick is
//! 4³ voxels; its index is a Morton code over the 8³ brick grid and each mask bit a Morton code inside the brick, x
//! lowest. A set bit is world geometry, not smoke. The cloud spreads from the emitters one voxel per frame through
//! open voxels and cannot enter a brick before the frame that sent it; voxels that still touch a brick the server never
//! sent are the front, not the cloud, and are left out.

use glam::DVec3;
use rayon::prelude::*;
use nades_model::smoke::{Emitter, decode};

use crate::collider::Collider;

pub const GRID: usize = 32;
pub const VOXEL: f64 = 20.0;
pub const HALF_SPAN: f64 = (GRID as f64 * VOXEL) / 2.0;
pub const CELLS: usize = GRID * GRID * GRID;
/// Frames arrive every 7 ticks at 64 tick.
pub const FRAME_SECONDS: f64 = 7.0 / 64.0;
/// Arrival is a frame number; NEVER marks a voxel outside the cloud.
pub const NEVER: u8 = 255;
/// The whole cloud fades over its last seconds.
const FADE_OUT: f64 = 2.5;

/// Row-major voxel index, x fastest: the layout of the 32³ volume texture.
pub fn cell(x: usize, y: usize, z: usize) -> usize {
    x + GRID * (y + GRID * z)
}

fn coords(c: usize) -> (usize, usize, usize) {
    (c % GRID, (c / GRID) % GRID, c / (GRID * GRID))
}

const SIX: [(i32, i32, i32); 6] = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];

/// The neighbour of `c` one step along `d`, when it is in the grid.
fn step(c: usize, d: (i32, i32, i32)) -> Option<usize> {
    let (x, y, z) = coords(c);
    let (nx, ny, nz) = (x as i32 + d.0, y as i32 + d.1, z as i32 + d.2);
    let inside = |v: i32| (0..GRID as i32).contains(&v);
    (inside(nx) && inside(ny) && inside(nz)).then(|| cell(nx as usize, ny as usize, nz as usize))
}

/// Splits a Morton code into its x, y and z components, x in the lowest bit.
fn deinterleave(code: u32, bits: u32) -> (usize, usize, usize) {
    let (mut x, mut y, mut z) = (0, 0, 0);
    for b in 0..bits {
        x |= ((code >> (3 * b)) & 1) << b;
        y |= ((code >> (3 * b + 1)) & 1) << b;
        z |= ((code >> (3 * b + 2)) & 1) << b;
    }
    (x as usize, y as usize, z as usize)
}

/// What the log's frames say about the grid.
pub struct Grid {
    /// 1 where a sent brick marks world geometry.
    pub solid: Vec<u8>,
    /// 1 inside a brick the server sent.
    pub sent: Vec<u8>,
    /// Per voxel: the frame its brick was sent, or NEVER.
    pub sent_frame: Vec<u8>,
    /// The first frame's emitters, as the simulation starts from them.
    pub emitters: Vec<Emitter>,
    pub frames: usize,
    /// First frame that says the smoke is fading, or the frame count when none does.
    pub fading_from: usize,
}

/// The grid a log describes, or None when it does not decode.
pub fn build_grid(log: &[u8]) -> Option<Grid> {
    let frames = decode(log).ok()?;
    let mut solid = vec![0u8; CELLS];
    let mut sent = vec![0u8; CELLS];
    let mut sent_frame = vec![NEVER; CELLS];
    let mut emitters: Vec<Emitter> = Vec::new();
    let mut fading_from = frames.len();
    for (i, frame) in frames.iter().enumerate() {
        if emitters.is_empty() && !frame.emitters.is_empty() {
            emitters = frame.emitters.clone();
        }
        if frame.fading && fading_from == frames.len() {
            fading_from = i;
        }
        for &(index, mask) in &frame.bricks {
            let (bx, by, bz) = deinterleave(u32::from(index) & 511, 3);
            for bit in 0..64u32 {
                let (vx, vy, vz) = deinterleave(bit, 2);
                let c = cell(bx * 4 + vx, by * 4 + vy, bz * 4 + vz);
                if sent[c] == 0 {
                    sent_frame[c] = (NEVER - 1).min(i.min(255) as u8);
                }
                sent[c] = 1;
                if (mask >> bit) & 1 == 1 {
                    solid[c] = 1;
                }
            }
        }
    }
    Some(Grid { solid, sent, sent_frame, emitters, frames: frames.len(), fading_from })
}

/// A smoke's cloud on its grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Cloud {
    /// World position of the grid's minimum corner.
    pub origin: DVec3,
    /// 1 where world geometry is, as the log's bricks say.
    pub solid: Vec<u8>,
    /// Per voxel: the frame the cloud reached it, counted from the first frame, or NEVER.
    pub arrival: Vec<u8>,
    /// Voxels in the cloud.
    pub count: usize,
    /// Inclusive voxel bounds of the cloud.
    pub bounds: ([usize; 3], [usize; 3]),
    /// Seconds the cloud stays full before it starts to fade.
    pub hold_seconds: f64,
}

/// Spreads from the emitters one voxel per frame through open voxels, never into a brick before the frame that sent
/// it: a bucket queue over frame numbers, since every arrival is a small integer. None when nothing fills.
pub fn fill_cloud(grid: &Grid, detonation: DVec3) -> Option<Cloud> {
    let open = |c: usize| grid.sent[c] == 1 && grid.solid[c] == 0;
    let mut reached = vec![NEVER; CELLS];
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); NEVER as usize];
    let offer = |c: usize, frame: u8, reached: &mut Vec<u8>, buckets: &mut Vec<Vec<usize>>| {
        let at = frame.max(grid.sent_frame[c]);
        if at < reached[c] {
            reached[c] = at;
            if let Some(bucket) = buckets.get_mut(at as usize) {
                bucket.push(c);
            }
        }
    };
    for e in &grid.emitters {
        let [x, y, z] = e.cell.map(usize::from);
        if x < GRID && y < GRID && z < GRID && open(cell(x, y, z)) {
            offer(cell(x, y, z), 0, &mut reached, &mut buckets);
        }
    }
    for frame in 0..NEVER {
        let bucket = std::mem::take(&mut buckets[frame as usize]);
        for &c in &bucket {
            // a stale entry: it was reached sooner
            if reached[c] != frame {
                continue;
            }
            for d in SIX {
                if let Some(n) = step(c, d)
                    && open(n)
                    && frame + 1 < NEVER
                {
                    offer(n, frame + 1, &mut reached, &mut buckets);
                }
            }
        }
    }
    // the front: a voxel next to a brick the server never looked into had not settled there
    let front = |c: usize| SIX.iter().any(|&d| step(c, d).is_some_and(|n| grid.sent[n] == 0));
    let mut arrival = vec![NEVER; CELLS];
    let mut count = 0;
    let (mut min, mut max) = ([GRID; 3], [0usize; 3]);
    for z in 0..GRID {
        for y in 0..GRID {
            for x in 0..GRID {
                let c = cell(x, y, z);
                if reached[c] == NEVER || front(c) {
                    continue;
                }
                arrival[c] = reached[c];
                count += 1;
                for (a, v) in [x, y, z].into_iter().enumerate() {
                    min[a] = min[a].min(v);
                    max[a] = max[a].max(v);
                }
            }
        }
    }
    (count > 0).then(|| Cloud {
        origin: DVec3::new(detonation.x - HALF_SPAN, detonation.y - HALF_SPAN, detonation.z - HALF_SPAN),
        solid: grid.solid.clone(),
        arrival,
        count,
        bounds: (min, max),
        hold_seconds: grid.fading_from as f64 * FRAME_SECONDS,
    })
}

/// The cloud a smoke's log describes, or None when the log holds nothing to fill from.
pub fn smoke_cloud(log: &[u8], detonation: DVec3) -> Option<Cloud> {
    let grid = build_grid(log)?;
    if grid.emitters.is_empty() {
        return None;
    }
    fill_cloud(&grid, detonation)
}

impl Cloud {
    /// Voxels the cloud had reached `seconds` after its first frame.
    pub fn count_at(&self, seconds: f64) -> usize {
        let frame = seconds / FRAME_SECONDS;
        self.arrival.iter().filter(|&&a| a != NEVER && f64::from(a) <= frame).count()
    }

    pub fn voxel_center(&self, x: usize, y: usize, z: usize) -> DVec3 {
        DVec3::new(self.origin.x + (x as f64 + 0.5) * VOXEL, self.origin.y + (y as f64 + 0.5) * VOXEL, self.origin.z + (z as f64 + 0.5) * VOXEL)
    }

    /// Centres of the voxels the cloud fills, z slowest.
    pub fn voxels(&self) -> Vec<DVec3> {
        let mut out = Vec::with_capacity(self.count);
        for z in 0..GRID {
            for y in 0..GRID {
                for x in 0..GRID {
                    if self.arrival[cell(x, y, z)] != NEVER {
                        out.push(self.voxel_center(x, y, z));
                    }
                }
            }
        }
        out
    }

    /// Middle of the cloud, a little below its centre of mass, where a camera should look.
    pub fn focus(&self) -> DVec3 {
        let mut sum = DVec3::ZERO;
        let mut zs = Vec::new();
        for z in 0..GRID {
            for y in 0..GRID {
                for x in 0..GRID {
                    if self.arrival[cell(x, y, z)] == NEVER {
                        continue;
                    }
                    sum += self.voxel_center(x, y, z);
                    zs.push(z);
                }
            }
        }
        if zs.is_empty() {
            return self.origin + DVec3::splat(320.0);
        }
        // three's divideScalar multiplies by the reciprocal
        sum *= 1.0 / zs.len() as f64;
        zs.sort_unstable();
        sum.z = self.origin.z + (zs[(zs.len() as f64 * 0.35).floor() as usize] as f64 + 0.5) * VOXEL;
        sum
    }

    /// 1 while the cloud holds, falling to 0 over its last seconds.
    pub fn fade_at(&self, seconds: f64) -> f64 {
        (1.0 - (seconds - self.hold_seconds) / FADE_OUT).clamp(0.0, 1.0)
    }

    /// Seconds after detonation when the cloud has bloomed and before it creeps or fades.
    pub fn settled_seconds(&self) -> f64 {
        (self.hold_seconds - FADE_OUT).clamp(1.5, 4.0)
    }

    /// World-space box around every voxel the cloud ever fills, padded for the blur and the shader's warp.
    pub fn bounding_box(&self) -> (DVec3, DVec3) {
        let (min, max) = self.bounds;
        let pad = 20.0 + 16.0 + 4.0;
        let o = self.origin;
        (
            DVec3::new(o.x + min[0] as f64 * 20.0 - pad, o.y + min[1] as f64 * 20.0 - pad, o.z + min[2] as f64 * 20.0 - pad),
            DVec3::new(o.x + (max[0] + 1) as f64 * 20.0 + pad, o.y + (max[1] + 1) as f64 * 20.0 + pad, o.z + (max[2] + 1) as f64 * 20.0 + pad),
        )
    }

    fn near_cloud(&self, x: usize, y: usize, z: usize) -> bool {
        for dz in -1i32..=1 {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                    let inside = |v: i32| (0..GRID as i32).contains(&v);
                    if inside(nx) && inside(ny) && inside(nz) && self.arrival[cell(nx as usize, ny as usize, nz as usize)] != NEVER {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Per voxel, 0..1: how much open sky it sees, and whether the sun reaches it, past the map's geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct VoxelLight {
    pub sky: Vec<f32>,
    pub sun: Vec<f32>,
}

/// Seven directions into the upper sky: straight up and a ring at 45°.
fn sky_rays() -> [DVec3; 7] {
    let mut rays = [DVec3::Z; 7];
    for (i, ray) in rays.iter_mut().enumerate().skip(1) {
        let a = (i - 1) as f64 * std::f64::consts::PI / 3.0;
        #[expect(clippy::approx_constant, reason = "Preserve the web volumes.ts sky directions exactly.")]
        let v = DVec3::new(a.cos(), a.sin(), std::f64::consts::SQRT_2 / 1.4142);
        // three's normalize: divide by the length (a square root of summed squares)
        let length = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        *ray = v * (1.0 / length);
    }
    rays
}

/// The light each voxel of the cloud (and a voxel's margin around it) gets from the map: rays from its centre to the
/// sun and into the sky, against the collider, on every core.
pub fn voxel_light(cloud: &Cloud, toward_sun: DVec3, collider: &Collider) -> VoxelLight {
    let rays = sky_rays();
    let lit: Vec<(f32, f32)> = (0..CELLS)
        .into_par_iter()
        .map(|c| {
            let (x, y, z) = coords(c);
            if cloud.arrival[c] == NEVER && !cloud.near_cloud(x, y, z) {
                return (1.0, 1.0);
            }
            let p = cloud.voxel_center(x, y, z);
            let sun = if collider.blocked(p, toward_sun, 6000.0) { 0.0 } else { 1.0 };
            let open = rays.iter().filter(|d| !collider.blocked(p, **d, 3000.0)).count();
            // f32, as the Float32Array the web view keeps it in
            ((open as f64 / rays.len() as f64) as f32, sun)
        })
        .collect();
    VoxelLight { sky: lit.iter().map(|l| l.0).collect(), sun: lit.iter().map(|l| l.1).collect() }
}

/// How many voxels from open air the cloud reaches full density.
const AIR_RAMP: f64 = 2.5;

/// Each cloud voxel's density from how far it lies from open air, walking through the cloud: 1/AIR_RAMP beside an
/// open voxel (or the grid's edge), full AIR_RAMP voxels in. Walls and floors are not open air.
fn toward_air(cloud: &Cloud, occ: &[f32]) -> Vec<f32> {
    let mut steps = vec![f32::INFINITY; CELLS];
    let mut queue = Vec::new();
    for c in 0..CELLS {
        if occ[c] == 0.0 {
            continue;
        }
        let open = SIX.iter().any(|&d| match step(c, d) {
            None => true,
            Some(n) => occ[n] == 0.0 && cloud.solid[n] == 0,
        });
        if open {
            steps[c] = 1.0;
            queue.push(c);
        }
    }
    let mut head = 0;
    while head < queue.len() {
        let c = queue[head];
        head += 1;
        for d in SIX {
            let Some(n) = step(c, d) else { continue };
            if occ[n] == 0.0 || steps[n] <= steps[c] + 1.0 {
                continue;
            }
            steps[n] = steps[c] + 1.0;
            queue.push(n);
        }
    }
    (0..CELLS).map(|c| if occ[c] != 0.0 { (f64::from(steps[c]) / AIR_RAMP).min(1.0) as f32 } else { 0.0 }).collect()
}

/// Separable [1,2,1]/4 blur along x, y and z, each pass kept in f32 as the web view's Float32Arrays are.
fn blur3(src: &[f32]) -> Vec<f32> {
    let mut a = src.to_vec();
    for stride in [1, GRID, GRID * GRID] {
        let b: Vec<f32> = (0..CELLS)
            .map(|c| {
                let i = (c / stride) % GRID;
                let prev = if i > 0 { f64::from(a[c - stride]) } else { 0.0 };
                let next = if i < GRID - 1 { f64::from(a[c + stride]) } else { 0.0 };
                ((f64::from(a[c]) * 2.0 + prev + next) / 4.0) as f32
            })
            .collect();
        a = b;
    }
    a
}

/// JavaScript's Math.round for the non-negative numbers the volume holds.
fn round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// The cloud as one RGBA 32³ volume the smoke shader animates by itself:
/// - R density: full inside, falling over the AIR_RAMP voxels nearest open air, but not where the cloud meets a wall
///   or floor, and carried one voxel into the solid voxels it touches, so trilinear sampling does not thin it against
///   surfaces; then a [1,2,1] blur;
/// - G arrival in 1/40 s after detonation, blurred only among filled voxels so the front is not delayed;
/// - B sky visibility past the map's geometry, when `light` is given;
/// - A sun visibility past the map's geometry.
///
/// Without `light`, roofs come from the log's own bricks and the sun is assumed to reach everywhere.
pub fn smoke_volume(cloud: &Cloud, light: Option<&VoxelLight>) -> Vec<u8> {
    let mut occ = vec![0f32; CELLS];
    let mut arrive = vec![0f32; CELLS];
    for c in 0..CELLS {
        let a = cloud.arrival[c];
        if a == NEVER {
            continue;
        }
        occ[c] = 1.0;
        arrive[c] = (f64::from(a) * FRAME_SECONDS).min(6.3) as f32;
    }
    let mut sky = vec![0f32; CELLS];
    for y in 0..GRID {
        for x in 0..GRID {
            let mut roof = false;
            for z in (0..GRID).rev() {
                let c = cell(x, y, z);
                if cloud.solid[c] != 0 {
                    roof = true;
                }
                sky[c] = match light {
                    Some(l) => l.sky[c],
                    None if roof => 0.35,
                    None => 1.0,
                };
            }
        }
    }
    let mut density = toward_air(cloud, &occ);
    // carried into the solid voxels the cloud touches, with the arrival of the cloud voxel it came from
    for c in 0..CELLS {
        if cloud.solid[c] == 0 {
            continue;
        }
        for d in SIX {
            let Some(n) = step(c, d) else { continue };
            if occ[n] == 0.0 || density[n] <= density[c] {
                continue;
            }
            density[c] = density[n];
            arrive[c] = arrive[n];
        }
    }
    let blurred = blur3(&density);
    let weighted = blur3(&arrive.iter().zip(&density).map(|(a, d)| (f64::from(*a) * f64::from(*d)) as f32).collect::<Vec<_>>());
    let sun = light.map(|l| blur3(&l.sun));
    let mut out = vec![0u8; CELLS * 4];
    for c in 0..CELLS {
        let d = f64::from(blurred[c]);
        out[c * 4] = round(d.min(1.0) * 255.0) as u8;
        out[c * 4 + 1] = if d > 0.001 { round(f64::from(weighted[c]) / d * 40.0).min(254.0) as u8 } else { 255 };
        out[c * 4 + 2] = round(f64::from(sky[c]).min(1.0) * 255.0) as u8;
        out[c * 4 + 3] = match &sun {
            Some(s) => round(f64::from(s[c]).min(1.0) * 255.0) as u8,
            None => 255,
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(number: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = number.to_le_bytes().to_vec();
        out.extend((payload.len() as u16).to_le_bytes());
        out.extend(payload);
        out
    }

    /// Morton code of a brick (8³ grid) or of a voxel inside one (4³), x lowest.
    fn interleave(x: u32, y: u32, z: u32, bits: u32) -> u32 {
        (0..bits).map(|b| (((x >> b) & 1) << (3 * b)) | (((y >> b) & 1) << (3 * b + 1)) | (((z >> b) & 1) << (3 * b + 2))).sum()
    }

    /// A log whose first frame sends one emitter at (16, 16, 16) and the 2×2×2 bricks around it (voxels 12–19), the
    /// lowest layer of them solid (a floor at z = 12); the second frame sends one more brick in a corner the cloud
    /// cannot reach; the fourth says it is fading.
    fn log() -> Vec<u8> {
        let mut bricks = Vec::new();
        for (bx, by, bz) in [(3, 3, 3), (4, 3, 3), (3, 4, 3), (4, 4, 3), (3, 3, 4), (4, 3, 4), (3, 4, 4), (4, 4, 4)] {
            let mut mask = 0u64;
            if bz == 3 {
                for vx in 0..4 {
                    for vy in 0..4 {
                        mask |= 1 << interleave(vx, vy, 0, 2);
                    }
                }
            }
            bricks.push((interleave(bx, by, bz, 3) as u16, mask));
        }
        let payload = |fading: u8, emitters: &[[u8; 3]], bricks: &[(u16, u64)]| {
            let mut p = vec![fading, u8::from(!emitters.is_empty()) | if bricks.is_empty() { 0 } else { 2 }];
            if !emitters.is_empty() {
                p.push(emitters.len() as u8);
                for e in emitters {
                    p.extend([e[0], e[1], e[2], 5]);
                    p.extend(0f32.to_le_bytes());
                }
            }
            if !bricks.is_empty() {
                p.extend((bricks.len() as u16).to_le_bytes());
                for (index, mask) in bricks {
                    p.extend(index.to_le_bytes());
                    p.extend(mask.to_le_bytes());
                }
            }
            p.push(0);
            p
        };
        let mut log = frame(0, &payload(0, &[[16, 16, 16]], &bricks));
        log.extend(frame(1, &payload(0, &[], &[(interleave(0, 0, 0, 3) as u16, 0)])));
        log.extend(frame(2, &payload(0, &[], &[])));
        log.extend(frame(3, &payload(1, &[], &[])));
        log
    }

    #[test]
    fn the_cloud_fills_the_sent_open_voxels_from_its_emitters() {
        let grid = build_grid(&log()).unwrap();
        assert_eq!((grid.frames, grid.fading_from, grid.emitters.len()), (4, 3, 1));
        assert_eq!(grid.solid.iter().filter(|&&s| s == 1).count(), 64);
        let cloud = smoke_cloud(&log(), DVec3::new(1000.0, 2000.0, 100.0)).unwrap();
        assert_eq!(cloud.origin, DVec3::new(680.0, 1680.0, -220.0));
        assert_eq!(cloud.hold_seconds, 3.0 * FRAME_SECONDS);
        // the emitter's voxel at frame 0, a neighbour at 1; the floor is never smoke
        assert_eq!(cloud.arrival[cell(16, 16, 16)], 0);
        assert_eq!(cloud.arrival[cell(17, 16, 16)], 1);
        assert_eq!(cloud.arrival[cell(16, 16, 12)], NEVER);
        // the bricks' outer layer touches unsent voxels: the front, left out; the corner brick is out of reach
        assert_eq!(cloud.arrival[cell(12, 16, 16)], NEVER);
        assert_eq!(cloud.arrival[cell(1, 1, 1)], NEVER);
        assert_eq!(grid.sent_frame[cell(1, 1, 1)], 1);
        assert_eq!(cloud.bounds, ([13, 13, 13], [18, 18, 18]));
        // 6×6 columns over the floor, z 13–18 (the bricks' open part less the front)
        assert_eq!(cloud.count, 6 * 6 * 6);
        assert_eq!(cloud.count_at(0.0), 1);
        let focus = cloud.focus();
        assert!((focus.x - (680.0 + 16.0 * 20.0)).abs() < 1e-9 && (focus.y - (1680.0 + 16.0 * 20.0)).abs() < 1e-9, "{focus}");
        assert_eq!(cloud.voxels().len(), cloud.count);
    }

    #[test]
    fn a_log_without_emitters_or_that_does_not_decode_has_no_cloud() {
        assert!(smoke_cloud(&frame(0, &[0, 0, 0]), DVec3::ZERO).is_none());
        assert!(smoke_cloud(&frame(0, &[0, 0, 0, 7]), DVec3::ZERO).is_none());
    }

    #[test]
    fn the_volume_ramps_toward_air_and_carries_into_the_floor() {
        let cloud = smoke_cloud(&log(), DVec3::ZERO).unwrap();
        let volume = smoke_volume(&cloud, None);
        let at = |x, y, z, k| volume[cell(x, y, z) * 4 + k];
        // full in the middle, thinner at the edge, carried into the floor below
        assert!(at(16, 16, 16, 0) > at(13, 16, 16, 0));
        assert!(at(16, 16, 12, 0) > 0);
        // arrival in 1/40 s: the emitter's voxel first
        assert!(at(16, 16, 16, 1) < at(18, 18, 18, 1));
        assert_eq!(at(0, 0, 0, 1), 255);
        // no light given: open sky above the floor, a roof's shade under it, the sun everywhere
        assert_eq!((at(16, 16, 20, 2), at(16, 16, 5, 2), at(16, 16, 16, 3)), (255, 89, 255));
        assert_eq!(cloud.fade_at(0.0), 1.0);
        assert_eq!(cloud.settled_seconds(), 1.5);
    }

    #[test]
    fn the_light_is_cast_against_the_collider() {
        let cloud = smoke_cloud(&log(), DVec3::ZERO).unwrap();
        // a roof just above the cloud, over half of it
        let roof = Collider::new(vec![[[-400.0, -400.0, 100.0], [0.0, -400.0, 100.0], [0.0, 400.0, 100.0]], [[-400.0, -400.0, 100.0], [0.0, 400.0, 100.0], [-400.0, 400.0, 100.0]]]);
        let light = voxel_light(&cloud, DVec3::Z, &roof);
        let x_under = (0..GRID).find(|&x| cloud.voxel_center(x, 0, 0).x < -20.0 && cloud.voxel_center(x, 0, 0).x > -100.0).unwrap();
        let under = cell(x_under.max(13), 16, 16);
        let open = cell(18, 16, 16);
        assert!(cloud.voxel_center(18, 16, 16).x > 0.0);
        assert_eq!((light.sun[under], light.sun[open]), (0.0, 1.0));
        assert!(light.sky[under] < light.sky[open]);
        // far from the cloud nothing is cast
        assert_eq!((light.sun[cell(0, 0, 0)], light.sky[cell(0, 0, 0)]), (1.0, 1.0));
    }
}
