//! Lineups: throws of one kind of grenade on one map that start from nearly the same point and land in nearly the same
//! place. Throws are taken in order (`played_at`, `demo_id`, `tick`, `throw_id`); a throw joins the earliest group
//! whose first throw started within 48 units of its start and landed within 150 units of its landing, or else starts a
//! group. The same facts always give the same groups, and a group's id is its place in the order groups were started.
//!
//! This is show-me-the-nades' lineup registry (`lineups.rs`) run over one set of facts in one pass, with its tests.

use std::collections::HashMap;

use crate::facts::{Grenade, LineupId, Vec3};

/// Starts closer than this (units) can be the same lineup.
pub const START_TOLERANCE: f32 = 48.0;
/// Landings closer than this (units) can be the same lineup.
pub const LANDING_TOLERANCE: f32 = 150.0;

fn distance(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// A group's first throw: where it started and landed.
struct Anchor {
    id: LineupId,
    start: Vec3,
    landing: Vec3,
}

/// Groups being formed. Anchors are kept in cells of the start point, larger than the start tolerance, so a throw
/// looks only at the anchors in its cell's neighbourhood.
#[derive(Default)]
pub struct Grouping {
    anchors: HashMap<(Grenade, [i64; 3]), Vec<Anchor>>,
    started: u32,
}

const CELL: f64 = 64.0;

fn cell_of(p: Vec3) -> [i64; 3] {
    p.map(|v| (f64::from(v) / CELL).floor() as i64)
}

impl Grouping {
    /// The group of the next throw in order: the earliest whose anchor fits, else a new one.
    pub fn assign(&mut self, grenade: Grenade, start: Vec3, landing: Vec3) -> LineupId {
        let kind = grenade.kind();
        let [x, y, z] = cell_of(start);
        let mut earliest: Option<LineupId> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.anchors.get(&(kind, [x + dx, y + dy, z + dz])) else { continue };
                    // in each cell the anchors are in the order they were started: the first fit is that cell's earliest
                    if let Some(a) = list.iter().find(|a| distance(a.start, start) < START_TOLERANCE && distance(a.landing, landing) < LANDING_TOLERANCE)
                        && earliest.is_none_or(|e| a.id < e)
                    {
                        earliest = Some(a.id);
                    }
                }
            }
        }
        if let Some(id) = earliest {
            return id;
        }
        self.started += 1;
        let id = LineupId(self.started);
        self.anchors.entry((kind, [x, y, z])).or_default().push(Anchor { id, start, landing });
        id
    }

    /// How many groups have been started.
    pub fn started(&self) -> u32 {
        self.started
    }
}

/// Every throw's group, for throws given in order as (grenade, start, landing).
pub fn group(throws: impl IntoIterator<Item = (Grenade, Vec3, Vec3)>) -> Vec<LineupId> {
    let mut grouping = Grouping::default();
    throws.into_iter().map(|(g, start, landing)| grouping.assign(g, start, landing)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_identical_throws_share_a_lineup() {
        let ids = group([(Grenade::Smoke, [1305.6, 1333.2, 1800.6], [-410.0, 41.0, 1666.0]), (Grenade::Smoke, [1305.6, 1332.2, 1800.6], [-413.0, 42.0, 1666.0])]);
        assert_eq!(ids, [LineupId(1), LineupId(1)]);
    }

    #[test]
    fn same_spot_different_landing_is_a_different_lineup() {
        let ids = group([(Grenade::Smoke, [1306.6, 1585.5, 1780.7], [-20.0, 1357.0, 1614.0]), (Grenade::Smoke, [1306.9, 1587.0, 1781.1], [-232.0, 1424.0, 1614.0])]);
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn grenade_kinds_are_separate_and_a_fire_is_one_kind() {
        let ids = group([
            (Grenade::Smoke, [0.0; 3], [500.0, 0.0, 0.0]),
            (Grenade::Flash, [0.0; 3], [500.0, 0.0, 0.0]),
            (Grenade::Molotov, [0.0; 3], [500.0, 0.0, 0.0]),
            (Grenade::Incendiary, [1.0, 0.0, 0.0], [510.0, 0.0, 0.0]),
        ]);
        assert_eq!(ids, [LineupId(1), LineupId(2), LineupId(3), LineupId(3)]);
    }

    #[test]
    fn a_throw_joins_the_earliest_group_that_fits_not_the_nearest() {
        // x = 40 joins the group started at 0; x = 70 is too far from it and starts its own, which is nearer to x = 40;
        // x = 30 fits both and joins the earlier
        let ids = group([0.0, 40.0, 70.0, 30.0].map(|x| (Grenade::Smoke, [x, 0.0, 0.0], [0.0; 3])));
        assert_eq!(ids, [LineupId(1), LineupId(1), LineupId(2), LineupId(1)]);
    }

    #[test]
    fn the_tolerances_are_strict_and_hold_across_cells() {
        // 47.9 and 149.9 apart: the same lineup, though the starts fall in different cells; 48 and 150 apart: not
        let ids = group([
            (Grenade::He, [63.0, 0.0, 0.0], [0.0; 3]),
            (Grenade::He, [110.9, 0.0, 0.0], [149.9, 0.0, 0.0]),
            (Grenade::He, [-63.0, 0.0, 0.0], [0.0; 3]),
            (Grenade::He, [-15.0, 0.0, 0.0], [0.0; 3]),
            (Grenade::He, [500.0, 0.0, 0.0], [0.0; 3]),
            (Grenade::He, [500.0, 0.0, 0.0], [150.0, 0.0, 0.0]),
        ]);
        assert_eq!(ids, [LineupId(1), LineupId(1), LineupId(2), LineupId(3), LineupId(4), LineupId(5)]);
    }

    #[test]
    fn grouping_matches_a_scan_of_every_anchor() {
        // show-me-the-nades' registry: the earliest anchor within both tolerances, by a scan of all of them
        let throws: Vec<(Grenade, Vec3, Vec3)> = (0..400)
            .map(|i| {
                let k = (i % 23) as f32;
                let wobble = ((i * 7919) % 97) as f32;
                (Grenade::Smoke, [k * 37.0 + wobble * 0.6, (i % 5) as f32 * 31.0, 0.0], [k * 90.0 + wobble, wobble * 1.3, 0.0])
            })
            .collect();
        let mut anchors: Vec<(Vec3, Vec3)> = Vec::new();
        let scanned: Vec<LineupId> = throws
            .iter()
            .map(|&(_, start, landing)| match anchors.iter().position(|&(s, l)| distance(s, start) < START_TOLERANCE && distance(l, landing) < LANDING_TOLERANCE) {
                Some(k) => LineupId(k as u32 + 1),
                None => {
                    anchors.push((start, landing));
                    LineupId(anchors.len() as u32)
                }
            })
            .collect();
        assert_eq!(group(throws), scanned);
    }
}
