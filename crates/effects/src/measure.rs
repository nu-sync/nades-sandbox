//! How alike two smokes or two fires are, as the effects are measured against recorded ones (S33).

use nades_scene::fire::Flame;
use nades_scene::smoke::{Cloud, FRAME_SECONDS, NEVER};

/// The voxels a cloud holds `seconds` after its first frame (every voxel it ever holds for None).
pub fn cloud_at(cloud: Option<&Cloud>, seconds: Option<f64>) -> Vec<bool> {
    let limit = seconds.map_or(f64::from(NEVER) - 1.0, |s| s / FRAME_SECONDS);
    match cloud {
        Some(c) => c.arrival.iter().map(|&a| a != NEVER && f64::from(a) <= limit).collect(),
        None => vec![false; nades_scene::smoke::CELLS],
    }
}

/// Intersection over union of two voxel sets: 1 when both are empty.
pub fn iou(a: &[bool], b: &[bool]) -> f64 {
    let (mut both, mut either) = (0usize, 0usize);
    for (&x, &y) in a.iter().zip(b) {
        both += usize::from(x && y);
        either += usize::from(x || y);
    }
    if either == 0 { 1.0 } else { both as f64 / either as f64 }
}

/// Floor a flame covers: a disc this wide around it, on surfaces within `FLAME_HEIGHT` of its height.
pub const FLAME_RADIUS: f64 = 40.0;
pub const FLAME_HEIGHT: f64 = 48.0;
const RASTER: f64 = 4.0;

/// Of the floor `a`'s flames burning at `t` cover (all of them for None), the share `b`'s flames burning then cover
/// too. None when none of `a`'s burn then.
pub fn coverage(a: &[Flame], b: &[Flame], t: Option<f64>) -> Option<f64> {
    let live = |f: &&Flame| t.is_none_or(|t| f.start <= t && t < f.end);
    let a: Vec<&Flame> = a.iter().filter(live).collect();
    let b: Vec<&Flame> = b.iter().filter(live).collect();
    if a.is_empty() {
        return None;
    }
    let r2 = FLAME_RADIUS * FLAME_RADIUS;
    let covers = |f: &Flame, x: f64, y: f64, z: f64| (f.pos[0] - x).powi(2) + (f.pos[1] - y).powi(2) <= r2 && (f.pos[2] - z).abs() < FLAME_HEIGHT;
    // raster points under a's flames, each counted once: at its first flame's height
    let mut points: Vec<(i64, i64, f64)> = Vec::new();
    let steps = (FLAME_RADIUS / RASTER).ceil() as i64;
    for f in &a {
        let (cx, cy) = ((f.pos[0] / RASTER).round() as i64, (f.pos[1] / RASTER).round() as i64);
        for i in cx - steps..=cx + steps {
            for j in cy - steps..=cy + steps {
                let (x, y) = (i as f64 * RASTER, j as f64 * RASTER);
                if covers(f, x, y, f.pos[2]) && !points.iter().any(|&(pi, pj, z)| (pi, pj) == (i, j) && (z - f.pos[2]).abs() < FLAME_HEIGHT) {
                    points.push((i, j, f.pos[2]));
                }
            }
        }
    }
    let covered = points.iter().filter(|&&(i, j, z)| b.iter().any(|f| covers(f, i as f64 * RASTER, j as f64 * RASTER, z))).count();
    Some(covered as f64 / points.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flame(x: f64, y: f64) -> Flame {
        Flame { pos: [x, y, 0.0], normal: [0.0, 0.0, 1.0], start: 0.0, end: 7.0 }
    }

    #[test]
    fn coverage_is_the_shared_share_of_the_floor() {
        let a = [flame(0.0, 0.0)];
        assert_eq!(coverage(&a, &a, None), Some(1.0));
        assert_eq!(coverage(&a, &[flame(500.0, 0.0)], None), Some(0.0));
        // two discs one radius apart share about 39% of each
        let half = coverage(&a, &[flame(FLAME_RADIUS, 0.0)], None).unwrap();
        assert!((half - 0.391).abs() < 0.02, "{half}");
        // a flame on the floor below is not this floor
        assert_eq!(coverage(&a, &[Flame { pos: [0.0, 0.0, -100.0], ..a[0] }], None), Some(0.0));
        assert_eq!(coverage(&a, &a, Some(8.0)), None);
    }

    #[test]
    fn iou_of_voxel_sets() {
        assert_eq!(iou(&[true, true, false], &[true, false, false]), 0.5);
        assert_eq!(iou(&[false], &[false]), 1.0);
    }
}
