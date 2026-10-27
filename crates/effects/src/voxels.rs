//! The map's geometry on a smoke's 32³ grid, from rays against the collider: a voxel holds geometry where a surface
//! passes within `pad` of it.

use glam::DVec3;
use rayon::prelude::*;
use nades_scene::collider::Collider;
use nades_scene::smoke::{CELLS, GRID, VOXEL, cell};

/// Lines per voxel side along each axis: 8 finds 92–98% of the game's solid voxels at 99% precision (S33).
pub const SAMPLES: usize = 8;

/// Per voxel of the grid whose minimum corner is `origin`: 1 where a collider surface passes through the voxel grown
/// by `pad` on every side, found by casting `samples`² lines per voxel face along every axis.
pub fn surface(origin: DVec3, collider: &Collider, samples: usize, pad: f64) -> Vec<u8> {
    let lines = GRID * samples;
    let span = GRID as f64 * VOXEL;
    let range = |p: f64| -> std::ops::RangeInclusive<usize> {
        let lo = ((p - pad) / VOXEL).floor().max(0.0) as usize;
        let hi = (((p + pad) / VOXEL).floor() as usize).min(GRID - 1);
        lo..=hi
    };
    let hits: Vec<Vec<usize>> = (0..3 * lines * lines)
        .into_par_iter()
        .map(|k| {
            let (axis, rest) = (k / (lines * lines), k % (lines * lines));
            let (u, v) = (rest % lines, rest / lines);
            let at = |i: usize| (i as f64 + 0.5) * VOXEL / samples as f64;
            let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
            let mut start = [0.0; 3];
            start[a1] = at(u);
            start[a2] = at(v);
            start[axis] = -pad - 1.0;
            let mut dir = DVec3::ZERO;
            dir[axis] = 1.0;
            let from = origin + DVec3::from_array(start);
            let far = span + 2.0 * pad + 2.0;
            let mut out = Vec::new();
            let mut travelled = 0.0;
            while travelled < far {
                let Some(d) = collider.hit_distance(from + dir * travelled, dir, far - travelled) else { break };
                let t = travelled + d + start[axis];
                if t > -pad && t < span + pad {
                    for i in range(t) {
                        for j in range(at(u)) {
                            for l in range(at(v)) {
                                let mut idx = [0usize; 3];
                                (idx[axis], idx[a1], idx[a2]) = (i, j, l);
                                out.push(cell(idx[0], idx[1], idx[2]));
                            }
                        }
                    }
                }
                travelled += d + 0.01;
            }
            out
        })
        .collect();
    let mut solid = vec![0u8; CELLS];
    for c in hits.into_iter().flatten() {
        solid[c] = 1;
    }
    solid
}

/// A collider of the grid's solid voxels (their faces that meet an open voxel or the grid's edge), for checks that
/// cannot carry a map's own collider: the committed fixture and tests.
pub fn collider_of(origin: DVec3, solid: &[u8]) -> Collider {
    let mut triangles = Vec::new();
    let solid_at = |p: [i32; 3]| p.iter().all(|v| (0..GRID as i32).contains(v)) && solid[cell(p[0] as usize, p[1] as usize, p[2] as usize)] == 1;
    for z in 0..GRID as i32 {
        for y in 0..GRID as i32 {
            for x in 0..GRID as i32 {
                if !solid_at([x, y, z]) {
                    continue;
                }
                for axis in 0..3 {
                    for side in [0, 1] {
                        let mut n = [x, y, z];
                        n[axis] += if side == 1 { 1 } else { -1 };
                        if solid_at(n) {
                            continue;
                        }
                        // the face's four corners, on the plane `axis` = its side of the voxel
                        let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
                        let corner = |u: i32, v: i32| -> [f32; 3] {
                            let mut c = [x, y, z];
                            c[axis] += side;
                            c[a1] += u;
                            c[a2] += v;
                            std::array::from_fn(|k| (origin[k] + f64::from(c[k]) * VOXEL) as f32)
                        };
                        let (p00, p10, p11, p01) = (corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1));
                        triangles.push([p00, p10, p11]);
                        triangles.push([p00, p11, p01]);
                    }
                }
            }
        }
    }
    Collider::new(triangles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_collider_voxelizes_back_to_its_voxels() {
        // a floor and a pillar on it
        let solid: Vec<u8> = (0..CELLS)
            .map(|c| {
                let (x, y, z) = (c % GRID, (c / GRID) % GRID, c / (GRID * GRID));
                u8::from(z < 10 || (12..15).contains(&x) && (12..14).contains(&y) && z < 20)
            })
            .collect();
        let origin = DVec3::new(-320.0, -320.0, -320.0);
        let back = surface(origin, &collider_of(origin, &solid), SAMPLES, 0.0);
        // a face on a voxel boundary marks the voxel on one side of it: the floor's top at every column, nothing
        // above the pillar
        for y in 0..GRID {
            for x in 0..GRID {
                let pillar = (12..15).contains(&x) && (12..14).contains(&y);
                assert!(pillar || back[cell(x, y, 9)] == 1 || back[cell(x, y, 10)] == 1, "{x} {y}");
                assert!((21..GRID).all(|z| back[cell(x, y, z)] == 0), "{x} {y} {:?}", (0..GRID).filter(|&z| back[cell(x, y, z)] == 1).collect::<Vec<_>>());
            }
        }
        assert_eq!(back[cell(13, 12, 15)], 1, "the pillar's side");
        assert_eq!(back[cell(5, 5, 15)], 0, "open air");
    }
}
