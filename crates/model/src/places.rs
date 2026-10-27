//! Landing callouts, learned from where players stood: every pawn carries the place it is in (`m_szLastPlaceName`).
//! Place-labelled samples of living players fill 32-unit cells, each taking the place seen there most often. A point
//! takes the place of the nearest cell, with vertical distance counted half (a grenade bursting in the air still names
//! the floor below) and nothing beyond 320 units. Coverage grows with every demo.
//!
//! show-me-the-nades' `places.rs`, in the same f32 arithmetic, with its test.

use std::collections::{BTreeMap, HashMap};

use crate::facts::Vec3;

pub const CELL: f32 = 32.0;
/// Vertical distance counts half.
const Z_WEIGHT: f32 = 0.5;
/// Points farther than this from any cell get no callout.
pub const MAX_DISTANCE: f32 = 320.0;

pub fn cell_of(p: Vec3) -> [i32; 3] {
    [(p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32, (p[2] / CELL).floor() as i32]
}

fn key(x: f32, y: f32, z: f32) -> [f32; 3] {
    [x, y, z * Z_WEIGHT]
}

/// The buckets cells are found by: as wide as the farthest lookup, so a hit is in a bucket next to the point's.
fn bucket(k: [f32; 3]) -> [i64; 3] {
    k.map(|v| (f64::from(v) / f64::from(MAX_DISTANCE)).floor() as i64)
}

/// A map's cells and the place each one names.
#[derive(Debug, Default)]
pub struct Callouts {
    /// Cell centres in lookup space (z counted half), with the index of their place's name.
    cells: Vec<([f32; 3], usize)>,
    names: Vec<String>,
    buckets: HashMap<[i64; 3], Vec<usize>>,
}

impl Callouts {
    /// The cells `points` fill: per cell the place seen most often, ties to the name first in order.
    pub fn learn<'a>(points: impl IntoIterator<Item = (Vec3, &'a str)>) -> Self {
        let mut counts: BTreeMap<[i32; 3], BTreeMap<&str, u64>> = BTreeMap::new();
        for (p, place) in points {
            if place.is_empty() || !p.iter().all(|v| v.is_finite()) {
                continue;
            }
            *counts.entry(cell_of(p)).or_default().entry(place).or_default() += 1;
        }
        Self::from_cells(counts.into_iter().filter_map(|(c, names)| {
            let mut best: Option<(&str, u64)> = None;
            for (name, n) in names {
                if best.is_none_or(|(_, m)| n > m) {
                    best = Some((name, n));
                }
            }
            best.map(|(name, _)| (c, name.to_string()))
        }))
    }

    /// From `(cell, place)` pairs, using the cells' centres.
    pub fn from_cells(cells: impl IntoIterator<Item = ([i32; 3], String)>) -> Self {
        let mut out = Callouts::default();
        let mut name_of: HashMap<String, usize> = HashMap::new();
        for (c, name) in cells {
            let centre = |v: i32| (v as f32 + 0.5) * CELL;
            let k = key(centre(c[0]), centre(c[1]), centre(c[2]));
            let next = out.names.len();
            let n = *name_of.entry(name.clone()).or_insert(next);
            if n == next {
                out.names.push(name);
            }
            out.buckets.entry(bucket(k)).or_default().push(out.cells.len());
            out.cells.push((k, n));
        }
        out
    }

    pub fn cells(&self) -> usize {
        self.cells.len()
    }

    /// The place of the nearest cell, if one is within reach; of two cells as near, the one learned first.
    pub fn lookup(&self, p: Vec3) -> Option<&str> {
        let q = key(p[0], p[1], p[2]);
        let [x, y, z] = bucket(q);
        let mut best: Option<(f32, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &i in self.buckets.get(&[x + dx, y + dy, z + dz]).into_iter().flatten() {
                        let g = self.cells[i].0;
                        let d = ((g[0] - q[0]).powi(2) + (g[1] - q[1]).powi(2) + (g[2] - q[2]).powi(2)).sqrt();
                        if best.is_none_or(|(b, j)| d < b || (d == b && i < j)) {
                            best = Some((d, i));
                        }
                    }
                }
            }
        }
        let (d, i) = best?;
        (d <= MAX_DISTANCE).then(|| self.names[self.cells[i].1].as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_cell_names_the_point() {
        let idx = Callouts::from_cells(vec![(cell_of([100.0, 100.0, 0.0]), "LongA".into()), (cell_of([1000.0, 100.0, 0.0]), "Highway".into())]);
        assert_eq!(idx.lookup([120.0, 90.0, 10.0]), Some("LongA"));
        assert_eq!(idx.lookup([980.0, 130.0, 200.0]), Some("Highway"));
        assert_eq!(idx.lookup([5000.0, 5000.0, 0.0]), None);
    }

    #[test]
    fn a_cell_takes_its_commonest_place_and_height_counts_half() {
        let samples = [([10.0, 10.0, 0.0], "Mid"), ([12.0, 11.0, 0.0], "Mid"), ([11.0, 12.0, 0.0], "Catwalk"), ([700.0, 10.0, 0.0], "")];
        let idx = Callouts::learn(samples.iter().map(|(p, n)| (*p, *n)));
        assert_eq!(idx.cells(), 1, "an empty place name is no label");
        assert_eq!(idx.lookup([16.0, 16.0, 0.0]), Some("Mid"));
        // 600 units up is 300 in lookup space: still the floor below; 300 across and 300 up is not
        assert_eq!(idx.lookup([16.0, 16.0, 616.0]), Some("Mid"));
        assert_eq!(idx.lookup([316.0, 16.0, 300.0]), None);
        // a tie goes to the name first in order
        let tie = Callouts::learn([([0.0, 0.0, 0.0], "B"), ([1.0, 1.0, 0.0], "A")]);
        assert_eq!(tie.lookup([0.0; 3]), Some("A"));
    }

    #[test]
    fn lookups_match_a_scan_of_every_cell() {
        let cells: Vec<([i32; 3], String)> = (0..300).map(|i| ([(i * 7) % 61 - 30, (i * 13) % 47 - 20, (i % 5) - 2], format!("P{}", i % 9))).collect();
        let idx = Callouts::from_cells(cells.clone());
        for i in 0..500 {
            let p = [((i * 37) % 2400) as f32 - 1200.0, ((i * 53) % 1800) as f32 - 900.0, ((i * 11) % 400) as f32 - 200.0];
            let q = key(p[0], p[1], p[2]);
            let scanned = cells
                .iter()
                .map(|(c, name)| {
                    let centre = |v: i32| (v as f32 + 0.5) * CELL;
                    let g = key(centre(c[0]), centre(c[1]), centre(c[2]));
                    (((g[0] - q[0]).powi(2) + (g[1] - q[1]).powi(2) + (g[2] - q[2]).powi(2)).sqrt(), name.as_str())
                })
                .fold(None, |best: Option<(f32, &str)>, (d, n)| if best.is_none_or(|(b, _)| d < b) { Some((d, n)) } else { best })
                .filter(|(d, _)| *d <= MAX_DISTANCE)
                .map(|(_, n)| n);
            assert_eq!(idx.lookup(p), scanned, "at {p:?}");
        }
    }
}
