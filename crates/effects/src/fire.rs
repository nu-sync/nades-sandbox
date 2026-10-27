//! A molotov's or incendiary's spread, simulated on the collider: CS2's inferno grows a tree of flames out from where
//! the grenade broke, each a step from an earlier one, over floors and stairs but not through walls or off drops, no
//! closer to another than a minimum spacing and no further than a range from the first. The numbers are measured from
//! recorded fires (S33).

use glam::DVec3;
use nades_scene::collider::Collider;
use nades_scene::fire::{FireKind, Flame};

/// What differs between the two grenades' fires.
#[derive(Debug, Clone, PartialEq)]
pub struct Spread {
    pub flames: usize,
    /// No flame lies further than this from the first, horizontally.
    pub range: f64,
    /// A child lands this far (horizontally) from its parent, scaled by 0.6–1.
    pub step: f64,
    /// No two flames closer than this.
    pub spacing: f64,
    /// Seconds after detonation each flame catches, in order (the recorded medians).
    pub catches: Vec<f64>,
    /// Seconds after detonation they all go out.
    pub lasts: f64,
}

impl Spread {
    pub fn of(kind: FireKind) -> Self {
        match kind {
            FireKind::Molotov => Self {
                flames: 16,
                range: 150.0,
                step: 80.0,
                spacing: 46.0,
                catches: vec![0.0, 0.12, 0.22, 0.22, 0.33, 0.33, 0.42, 0.52, 0.53, 0.62, 0.72, 0.83, 1.03, 1.12, 1.33, 1.53],
                lasts: 7.0,
            },
            FireKind::Incendiary => Self {
                flames: 16,
                range: 110.0,
                step: 60.0,
                spacing: 34.0,
                catches: vec![0.0, 0.03, 0.05, 0.06, 0.06, 0.08, 0.08, 0.09, 0.11, 0.12, 0.14, 0.16, 0.17, 0.19, 0.2, 0.23],
                lasts: 5.48,
            },
        }
    }
}

/// How far up a flame climbs to a child (stairs, a kerb) and how far down it follows a slope or step before the drop
/// stops it.
pub const CLIMB: f64 = 40.0;
pub const DROP: f64 = 56.0;
/// Height above the floor the spread tests walls at.
const WAIST: f64 = 12.0;
/// How far below the detonation the first flame finds its floor.
pub const FIRST_DROP: f64 = 160.0;
/// Candidate directions tried before the spread gives up.
const ATTEMPTS: usize = 400;

/// SplitMix64 over a seed: the spread's draws, the same for the same detonation every time.
struct Draws(u64);

impl Draws {
    fn of(at: DVec3) -> Self {
        let q = |v: f64| (v * 8.0).round() as i64 as u64;
        Self(q(at.x).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ q(at.y).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ q(at.z).wrapping_mul(0x1656_67B1_9E37_79F9))
    }

    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The floor's normal at `p`, from the floor heights a little around it (straight up where those are missing).
fn normal_at(p: DVec3, collider: &Collider) -> DVec3 {
    let h = |dx: f64, dy: f64| collider.floor_below(DVec3::new(p.x + dx, p.y + dy, p.z + 8.0), 24.0);
    match (h(6.0, 0.0), h(-6.0, 0.0), h(0.0, 6.0), h(0.0, -6.0)) {
        (Some(xp), Some(xm), Some(yp), Some(ym)) => DVec3::new((xm - xp) / 12.0, (ym - yp) / 12.0, 1.0).normalize(),
        _ => DVec3::Z,
    }
}

/// The flames a fire of `kind` spreads to from `detonation`. With no floor within reach below it, none.
pub fn flames(kind: FireKind, detonation: DVec3, collider: &Collider) -> Vec<Flame> {
    flames_with(&Spread::of(kind), detonation, collider)
}

/// The flames a fire spreading by `spread` reaches from `detonation`.
pub fn flames_with(spread: &Spread, detonation: DVec3, collider: &Collider) -> Vec<Flame> {
    let Some(floor) = collider.floor_below(detonation + DVec3::Z, FIRST_DROP + 1.0) else { return Vec::new() };
    let first = DVec3::new(detonation.x, detonation.y, floor);
    let mut at = vec![first];
    let mut draws = Draws::of(detonation);
    for parent in 0..ATTEMPTS {
        if at.len() >= spread.flames {
            break;
        }
        // each flame in turn spreads a child, the oldest first
        let from = at[parent % at.len()];
        let angle = draws.next() * std::f64::consts::TAU;
        let length = spread.step * (0.6 + 0.4 * draws.next());
        let dir = DVec3::new(angle.cos(), angle.sin(), 0.0);
        let target = from + dir * length;
        if (target - first).truncate().length() > spread.range {
            continue;
        }
        let Some(z) = collider.floor_below(DVec3::new(target.x, target.y, from.z + CLIMB + WAIST), CLIMB + WAIST + DROP) else { continue };
        let child = DVec3::new(target.x, target.y, z);
        // a wall between the parent and the child stops it: the way is open at the parent's waist, or, up stairs, at
        // the child's
        let open_at = |height: f64| !collider.blocked(DVec3::new(from.x, from.y, height + WAIST), dir, length);
        if !(open_at(from.z) || (z > from.z && open_at(z))) {
            continue;
        }
        if at.iter().any(|p| p.distance(child) < spread.spacing) {
            continue;
        }
        at.push(child);
    }
    at.iter()
        .enumerate()
        .map(|(k, &p)| Flame {
            pos: p.to_array(),
            normal: normal_at(p, collider).to_array(),
            start: spread.catches[k.min(spread.catches.len() - 1)],
            end: spread.lasts,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voxels::collider_of;
    use nades_scene::smoke::{CELLS, GRID, VOXEL};

    const ORIGIN: DVec3 = DVec3::new(-320.0, -320.0, -320.0);

    /// A collider of voxels, solid where `solid(x, y, z)` (voxel indices, the grenade's above voxel 16).
    fn world(solid: impl Fn(i32, i32, i32) -> bool) -> Collider {
        let cells: Vec<u8> = (0..CELLS).map(|c| u8::from(solid((c % GRID) as i32, ((c / GRID) % GRID) as i32, (c / (GRID * GRID)) as i32))).collect();
        collider_of(ORIGIN, &cells)
    }

    fn x_of(f: &Flame) -> f64 {
        f.pos[0]
    }

    #[test]
    fn an_open_floor_burns_sixteen_flames_within_range() {
        let floor = world(|_, _, z| z < 16);
        let det = DVec3::new(0.0, 0.0, 10.0);
        let fire = flames(FireKind::Molotov, det, &floor);
        assert_eq!(fire.len(), 16);
        assert!(fire.iter().all(|f| f.pos[2].abs() < 1e-6 && DVec3::from_array(f.pos).truncate().length() <= 150.0 + 1e-9));
        assert!(fire.iter().all(|f| f.end == 7.0 && (f.normal[2] - 1.0).abs() < 1e-9));
        let spacing = fire.iter().enumerate().flat_map(|(i, a)| fire[i + 1..].iter().map(move |b| DVec3::from_array(a.pos).distance(DVec3::from_array(b.pos)))).fold(f64::INFINITY, f64::min);
        assert!(spacing >= 46.0, "{spacing}");
        let incendiary = flames(FireKind::Incendiary, det, &floor);
        assert!(incendiary.iter().all(|f| DVec3::from_array(f.pos).length() <= 110.0 + 1e-9 && f.end == 5.48));
        assert_eq!(flames(FireKind::Molotov, det, &floor), fire, "the same fire for the same detonation");
    }

    #[test]
    fn a_wall_stops_the_spread() {
        // a wall 40 units to the +x side, floor-to-ceiling
        let walled = world(|x, _, z| z < 16 || x == 18);
        let fire = flames(FireKind::Molotov, DVec3::new(0.0, 0.0, 10.0), &walled);
        assert!(fire.len() >= 8);
        assert!(fire.iter().all(|f| x_of(f) < 2.0 * VOXEL), "{:?}", fire.iter().map(x_of).collect::<Vec<_>>());
    }

    #[test]
    fn a_drop_stops_it_and_stairs_do_not() {
        // the floor ends 40 units to the +x side, 160 units above the ground below
        let ledge = world(|x, _, z| z < 8 || (z < 16 && x < 18));
        let fire = flames(FireKind::Molotov, DVec3::new(0.0, 0.0, 10.0), &ledge);
        assert!(fire.iter().all(|f| f.pos[2].abs() < 1e-6), "{:?}", fire.iter().map(|f| f.pos).collect::<Vec<_>>());
        // stairs up to +x: a 20-unit step every 40 units
        let stairs = world(|x, _, z| z < 16 + ((x - 16).max(0) / 2).min(4));
        let fire = flames(FireKind::Molotov, DVec3::new(0.0, 0.0, 10.0), &stairs);
        assert!(fire.iter().any(|f| f.pos[2] >= 40.0), "{:?}", fire.iter().map(|f| f.pos).collect::<Vec<_>>());
    }

    #[test]
    fn no_floor_below_means_no_fire() {
        let floor = world(|_, _, z| z < 4);
        assert!(flames(FireKind::Molotov, DVec3::new(0.0, 0.0, 10.0), &floor).is_empty());
    }
}
