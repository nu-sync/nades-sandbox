//! The map's collider: the map's visual occluders in Hammer units, as awpy reduces the map's physics into its collision
//! mesh (the pack's Collider section, which `nades-pack` makes from the map's own physics the same way, byte for byte),
//! and a BVH over it, as the web view uses since its D26 (`render/mapModel.ts` `collisionCollider`, three-mesh-bvh).
//!
//! The answers are the web view's: each triangle is tested with three.js's watertight ray/triangle test
//! (`Ray.intersectTriangle`, both faces) in doubles, a hit's distance is |point − origin| as three-mesh-bvh measures
//! it, and the queries (`clear_between`, `blocked`, `floor_below`) compare those distances as `mapModel.ts` does. The
//! tree only decides which triangles are tested, never what a test says, so it can differ from three-mesh-bvh's.

use glam::DVec3;
use rayon::prelude::*;

/// A node of the tree: its box, and either its first child (the second follows it) or its triangles.
#[derive(Debug, Clone, Copy)]
struct Node {
    min: [f32; 3],
    max: [f32; 3],
    /// Interior: the first child's index. Leaf: the first triangle's.
    first: u32,
    /// Triangles in a leaf; 0 for an interior node.
    count: u32,
}

/// One BVH over a map's collision triangles.
pub struct Collider {
    /// Each triangle's three corners, in the tree's order.
    triangles: Vec<[[f32; 3]; 3]>,
    nodes: Vec<Node>,
}

/// A leaf holds at most this many triangles.
const LEAF: usize = 4;
const BINS: usize = 12;

impl Collider {
    /// awpy's mesh: "AWMH", a version, the vertex and triangle counts (u32, little-endian), then the vertices
    /// (3 × f32 each) and the triangles (3 × u32 each).
    pub fn from_awmh(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 || &bytes[0..4] != b"AWMH" {
            return None;
        }
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes")) as usize;
        let (vertices, triangles) = (word(8), word(12));
        let positions = bytes.get(16..16 + vertices * 12)?;
        let indices = bytes.get(16 + vertices * 12..16 + vertices * 12 + triangles * 12)?;
        let vertex = |k: usize| -> Option<[f32; 3]> {
            let at = k.checked_mul(12)?;
            let p = positions.get(at..at + 12)?;
            Some([0, 4, 8].map(|o| f32::from_le_bytes(p[o..o + 4].try_into().expect("four bytes"))))
        };
        let tris: Option<Vec<[[f32; 3]; 3]>> = indices
            .chunks_exact(12)
            .map(|t| {
                let corner = |o: usize| u32::from_le_bytes(t[o..o + 4].try_into().expect("four bytes")) as usize;
                Some([vertex(corner(0))?, vertex(corner(4))?, vertex(corner(8))?])
            })
            .collect();
        Some(Self::new(tris?))
    }

    /// The tree over `triangles` (Hammer units).
    pub fn new(mut triangles: Vec<[[f32; 3]; 3]>) -> Self {
        let mut nodes = vec![Node { min: [0.0; 3], max: [0.0; 3], first: 0, count: 0 }];
        if triangles.is_empty() {
            return Self { triangles, nodes };
        }
        let centroids: Vec<[f32; 3]> = triangles.iter().map(|t| std::array::from_fn(|a| (t[0][a] + t[1][a] + t[2][a]) / 3.0)).collect();
        let mut order: Vec<u32> = (0..triangles.len() as u32).collect();
        let tree = build(&triangles, &centroids, &mut order);
        flatten(&tree, &mut nodes, 0);
        // the triangles in the leaves' order, so a leaf reads one run of them
        triangles = order.iter().map(|&k| triangles[k as usize]).collect();
        Self { triangles, nodes }
    }

    pub fn triangles(&self) -> usize {
        self.triangles.len()
    }

    /// The nearest hit along a ray within `far` (`dir` a unit vector): its distance, and the triangle's corners.
    fn first_hit(&self, origin: DVec3, dir: DVec3, far: f64) -> Option<(f64, usize)> {
        if self.triangles.is_empty() {
            return None;
        }
        let inverse = DVec3::ONE / dir;
        let mut best: Option<(f64, usize)> = None;
        let mut stack = [0u32; 64];
        let mut depth = 1;
        while depth > 0 {
            depth -= 1;
            let node = self.nodes[stack[depth] as usize];
            let limit = best.map_or(far, |(d, _)| d.min(far));
            if !slab(&node, origin, inverse, limit) {
                continue;
            }
            if node.count > 0 {
                for k in node.first as usize..(node.first + node.count) as usize {
                    if let Some(d) = intersect(origin, dir, &self.triangles[k])
                        && d <= far
                        && best.is_none_or(|(b, _)| d < b)
                    {
                        best = Some((d, k));
                    }
                }
            } else {
                // the nearer child last, so it is visited first
                let (a, b) = (node.first, node.first + 1);
                let axis_near = |n: u32| {
                    let n = &self.nodes[n as usize];
                    let centre = DVec3::new(f64::from(n.min[0] + n.max[0]), f64::from(n.min[1] + n.max[1]), f64::from(n.min[2] + n.max[2])) * 0.5;
                    (centre - origin).dot(dir)
                };
                let (near, farther) = if axis_near(a) <= axis_near(b) { (a, b) } else { (b, a) };
                stack[depth] = farther;
                stack[depth + 1] = near;
                depth += 2;
            }
        }
        best
    }

    /// How far a ray (`dir` a unit vector) runs before it meets the map, when it does within `far`.
    pub fn hit_distance(&self, origin: DVec3, dir: DVec3, far: f64) -> Option<f64> {
        self.first_hit(origin, dir, far).map(|(d, _)| d)
    }

    /// Whether the map blocks a ray within `far` units (`mapModel.ts` `blocked`: a hit nearer than `far`).
    pub fn blocked(&self, origin: DVec3, dir: DVec3, far: f64) -> bool {
        self.first_hit(origin, dir, far).is_some_and(|(d, _)| d < far)
    }

    /// Whether nothing in the map lies between two points, the last 24 units ignored so a target on a surface counts
    /// (`mapModel.ts` `clearBetween`).
    pub fn clear_between(&self, from: DVec3, to: DVec3) -> bool {
        let dir = to - from;
        // three's Vector3.length: the square root of the summed squares
        let dist = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z).sqrt();
        if dist < 1.0 {
            return true;
        }
        let dir = DVec3::new(dir.x / dist, dir.y / dist, dir.z / dist);
        // blocked by the nearest hit at or before dist − 24; anything further is the target's own surface
        self.first_hit(from, dir, dist - 24.0).is_none()
    }

    /// The height of the floor under a point, searching down `depth` units (`mapModel.ts` `floorBelow`).
    pub fn floor_below(&self, at: DVec3, depth: f64) -> Option<f64> {
        self.hit_distance(at, DVec3::NEG_Z, depth).map(|d| at.z - d)
    }
}

/// Whether a ray (origin, 1/direction) meets a node's box before `far`. The box is padded a little, so rounding never
/// hides a triangle that lies on its face.
fn slab(node: &Node, origin: DVec3, inverse: DVec3, far: f64) -> bool {
    let pad = 1e-3;
    let lo = DVec3::new(f64::from(node.min[0]) - pad, f64::from(node.min[1]) - pad, f64::from(node.min[2]) - pad);
    let hi = DVec3::new(f64::from(node.max[0]) + pad, f64::from(node.max[1]) + pad, f64::from(node.max[2]) + pad);
    let a = (lo - origin) * inverse;
    let b = (hi - origin) * inverse;
    // NaN (a zero direction exactly on a face) counts as inside
    let near = a.x.min(b.x).max(a.y.min(b.y)).max(a.z.min(b.z));
    let farthest = a.x.max(b.x).min(a.y.max(b.y)).min(a.z.max(b.z));
    !(farthest < near.max(0.0) || near > far + 1.0)
}

/// three.js's `Ray.intersectTriangle(a, b, c, false)`: Woop, Benthin and Wald's watertight test, then the distance
/// three-mesh-bvh reports, |origin + dir·t − origin|. None when the ray misses.
fn intersect(origin: DVec3, dir: DVec3, t: &[[f32; 3]; 3]) -> Option<f64> {
    let corner = |k: usize| DVec3::new(f64::from(t[k][0]) - origin.x, f64::from(t[k][1]) - origin.y, f64::from(t[k][2]) - origin.z);
    let (ao, bo, co) = (corner(0), corner(1), corner(2));
    let (dx, dy, dz) = (dir.x, dir.y, dir.z);
    let (adx, ady, adz) = (dx.abs(), dy.abs(), dz.abs());
    // the axis the ray runs along most is kz; kx and ky swap when it runs backwards, keeping the winding
    let (dk, ak, bk, ck) = if adx >= ady && adx >= adz {
        if dx >= 0.0 { ((dy, dz, dx), (ao.y, ao.z, ao.x), (bo.y, bo.z, bo.x), (co.y, co.z, co.x)) } else { ((dz, dy, dx), (ao.z, ao.y, ao.x), (bo.z, bo.y, bo.x), (co.z, co.y, co.x)) }
    } else if ady >= adz {
        if dy >= 0.0 { ((dz, dx, dy), (ao.z, ao.x, ao.y), (bo.z, bo.x, bo.y), (co.z, co.x, co.y)) } else { ((dx, dz, dy), (ao.x, ao.z, ao.y), (bo.x, bo.z, bo.y), (co.x, co.z, co.y)) }
    } else if dz >= 0.0 {
        ((dx, dy, dz), (ao.x, ao.y, ao.z), (bo.x, bo.y, bo.z), (co.x, co.y, co.z))
    } else {
        ((dy, dx, dz), (ao.y, ao.x, ao.z), (bo.y, bo.x, bo.z), (co.y, co.x, co.z))
    };
    let (dkx, dky, dkz) = dk;
    if dkz == 0.0 {
        return None;
    }
    let (sx, sy, sz) = (dkx / dkz, dky / dkz, 1.0 / dkz);
    let (ax, ay) = (ak.0 - sx * ak.2, ak.1 - sy * ak.2);
    let (bx, by) = (bk.0 - sx * bk.2, bk.1 - sy * bk.2);
    let (cx, cy) = (ck.0 - sx * ck.2, ck.1 - sy * ck.2);
    let u = cx * by - cy * bx;
    let v = ax * cy - ay * cx;
    let w = bx * ay - by * ax;
    if (u < 0.0 || v < 0.0 || w < 0.0) && (u > 0.0 || v > 0.0 || w > 0.0) {
        return None;
    }
    let det = u + v + w;
    if det == 0.0 {
        return None;
    }
    let scaled = sz * (u * ak.2 + v * bk.2 + w * ck.2);
    if if det > 0.0 { scaled < 0.0 } else { scaled > 0.0 } {
        return None;
    }
    // Ray.at, then Vector3.distanceTo
    let t = scaled / det;
    let p = DVec3::new(dir.x * t + origin.x, dir.y * t + origin.y, dir.z * t + origin.z);
    let (ex, ey, ez) = (origin.x - p.x, origin.y - p.y, origin.z - p.z);
    Some((ex * ex + ey * ey + ez * ez).sqrt())
}

enum Tree {
    Leaf { start: usize, count: usize, min: [f32; 3], max: [f32; 3] },
    Split { children: Box<(Tree, Tree)>, min: [f32; 3], max: [f32; 3] },
}

fn bounds_of(triangles: &[[[f32; 3]; 3]], order: &[u32]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for &k in order {
        for p in &triangles[k as usize] {
            for (a, v) in p.iter().enumerate() {
                min[a] = min[a].min(*v);
                max[a] = max[a].max(*v);
            }
        }
    }
    (min, max)
}

fn area(min: [f32; 3], max: [f32; 3]) -> f32 {
    let d = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    if d.iter().any(|x| *x < 0.0) { 0.0 } else { d[0] * d[1] + d[1] * d[2] + d[2] * d[0] }
}

/// A binned surface-area split of `order` (a slice of triangle indices, reordered in place); big halves in parallel.
fn build(triangles: &[[[f32; 3]; 3]], centroids: &[[f32; 3]], order: &mut [u32]) -> Tree {
    build_at(triangles, centroids, order, 0)
}

fn build_at(triangles: &[[[f32; 3]; 3]], centroids: &[[f32; 3]], order: &mut [u32], start: usize) -> Tree {
    let (min, max) = bounds_of(triangles, order);
    if order.len() <= LEAF {
        return Tree::Leaf { start, count: order.len(), min, max };
    }
    let mut cmin = [f32::INFINITY; 3];
    let mut cmax = [f32::NEG_INFINITY; 3];
    for &k in order.iter() {
        for a in 0..3 {
            cmin[a] = cmin[a].min(centroids[k as usize][a]);
            cmax[a] = cmax[a].max(centroids[k as usize][a]);
        }
    }
    // the cheapest split over every axis's bins
    let mut best: Option<(f32, usize, f32)> = None;
    for axis in 0..3 {
        let extent = cmax[axis] - cmin[axis];
        if extent <= 0.0 {
            continue;
        }
        let bin_of = |c: f32| (((c - cmin[axis]) / extent * BINS as f32) as usize).min(BINS - 1);
        let mut counts = [0usize; BINS];
        let mut boxes = [([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]); BINS];
        for &k in order.iter() {
            let b = bin_of(centroids[k as usize][axis]);
            counts[b] += 1;
            for p in &triangles[k as usize] {
                for (a, v) in p.iter().enumerate() {
                    boxes[b].0[a] = boxes[b].0[a].min(*v);
                    boxes[b].1[a] = boxes[b].1[a].max(*v);
                }
            }
        }
        let grow = |acc: &mut ([f32; 3], [f32; 3]), b: &([f32; 3], [f32; 3])| {
            for a in 0..3 {
                acc.0[a] = acc.0[a].min(b.0[a]);
                acc.1[a] = acc.1[a].max(b.1[a]);
            }
        };
        let mut left_area = [0.0f32; BINS];
        let mut left_count = [0usize; BINS];
        let mut acc = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        let mut n = 0;
        for b in 0..BINS - 1 {
            grow(&mut acc, &boxes[b]);
            n += counts[b];
            left_area[b] = area(acc.0, acc.1);
            left_count[b] = n;
        }
        let mut acc = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        let mut n = 0;
        for b in (1..BINS).rev() {
            grow(&mut acc, &boxes[b]);
            n += counts[b];
            let cost = left_area[b - 1] * left_count[b - 1] as f32 + area(acc.0, acc.1) * n as f32;
            if left_count[b - 1] > 0 && n > 0 && best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, axis, cmin[axis] + extent * b as f32 / BINS as f32));
            }
        }
    }
    let mid = match best {
        Some((_, axis, plane)) => partition(order, |k| centroids[k as usize][axis] < plane),
        None => order.len() / 2,
    };
    let mid = if mid == 0 || mid == order.len() { order.len() / 2 } else { mid };
    let (left, right) = order.split_at_mut(mid);
    let children = if left.len() + right.len() > 4096 {
        rayon::join(|| build_at(triangles, centroids, left, start), || build_at(triangles, centroids, right, start + mid))
    } else {
        (build_at(triangles, centroids, left, start), build_at(triangles, centroids, right, start + mid))
    };
    Tree::Split { children: Box::new(children), min, max }
}

/// Moves the indices `keep` accepts to the front; returns how many.
fn partition(order: &mut [u32], keep: impl Fn(u32) -> bool) -> usize {
    let mut at = 0;
    for i in 0..order.len() {
        if keep(order[i]) {
            order.swap(i, at);
            at += 1;
        }
    }
    at
}

/// Writes `tree` into `nodes[slot]`, its children into a pair of slots after it.
fn flatten(tree: &Tree, nodes: &mut Vec<Node>, slot: usize) {
    match tree {
        Tree::Leaf { start, count, min, max } => nodes[slot] = Node { min: *min, max: *max, first: *start as u32, count: *count as u32 },
        Tree::Split { children, min, max } => {
            let first = nodes.len();
            nodes.push(Node { min: [0.0; 3], max: [0.0; 3], first: 0, count: 0 });
            nodes.push(Node { min: [0.0; 3], max: [0.0; 3], first: 0, count: 0 });
            nodes[slot] = Node { min: *min, max: *max, first: first as u32, count: 0 };
            flatten(&children.0, nodes, first);
            flatten(&children.1, nodes, first + 1);
        }
    }
}

/// Runs `f` over `items` on every core, keeping their order.
pub fn par_map<T: Sync, U: Send>(items: &[T], f: impl Fn(&T) -> U + Sync + Send) -> Vec<U> {
    items.par_iter().map(f).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor at z = 0 from −1000 to 1000 and a wall at x = 100 standing on it.
    fn room() -> Collider {
        let quad = |a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]| [[a, b, c], [a, c, d]];
        let mut triangles = Vec::new();
        triangles.extend(quad([-1000.0, -1000.0, 0.0], [1000.0, -1000.0, 0.0], [1000.0, 1000.0, 0.0], [-1000.0, 1000.0, 0.0]));
        triangles.extend(quad([100.0, -50.0, 0.0], [100.0, 50.0, 0.0], [100.0, 50.0, 200.0], [100.0, -50.0, 200.0]));
        // clutter, so the tree has depth
        for k in 0..200 {
            let x = -900.0 + k as f32 * 4.0;
            triangles.push([[x, 800.0, 10.0], [x + 2.0, 800.0, 10.0], [x, 802.0, 12.0]]);
        }
        Collider::new(triangles)
    }

    #[test]
    fn rays_find_the_nearest_surface_from_either_side() {
        let c = room();
        assert_eq!(c.triangles(), 204);
        let d = c.hit_distance(DVec3::new(0.0, 0.0, 50.0), DVec3::X, 1e9).unwrap();
        assert!((d - 100.0).abs() < 1e-9, "{d}");
        // the wall's back face counts too
        let d = c.hit_distance(DVec3::new(300.0, 0.0, 50.0), DVec3::NEG_X, 1e9).unwrap();
        assert!((d - 200.0).abs() < 1e-9);
        assert_eq!(c.floor_below(DVec3::new(10.0, 10.0, 60.0), 500.0), Some(0.0));
        assert_eq!(c.floor_below(DVec3::new(10.0, 10.0, 600.0), 500.0), None);
        assert!(c.blocked(DVec3::new(0.0, 0.0, 50.0), DVec3::X, 150.0));
        assert!(!c.blocked(DVec3::new(0.0, 0.0, 50.0), DVec3::X, 90.0));
        assert!(!c.blocked(DVec3::new(0.0, 0.0, 50.0), DVec3::Z, 1e9));
    }

    #[test]
    fn clear_between_ignores_the_targets_own_surface() {
        let c = room();
        assert!(!c.clear_between(DVec3::new(0.0, 0.0, 50.0), DVec3::new(300.0, 0.0, 50.0)));
        // a target 10 units behind the wall's face is on it
        assert!(c.clear_between(DVec3::new(0.0, 0.0, 50.0), DVec3::new(110.0, 0.0, 50.0)));
        assert!(c.clear_between(DVec3::new(0.0, 0.0, 50.0), DVec3::new(0.0, 0.0, 51.0)));
        assert!(c.clear_between(DVec3::new(0.0, 0.0, 250.0), DVec3::new(300.0, 0.0, 250.0)));
    }

    #[test]
    fn the_tree_answers_as_testing_every_triangle_would() {
        let c = room();
        let mut seed = 1u64;
        let mut random = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..2000 {
            let origin = DVec3::new(random() * 2000.0 - 1000.0, random() * 2000.0 - 1000.0, random() * 300.0 - 20.0);
            let dir = DVec3::new(random() - 0.5, random() - 0.5, random() - 0.5).normalize();
            let brute = c.triangles.iter().filter_map(|t| intersect(origin, dir, t)).fold(None, |best: Option<f64>, d| Some(best.map_or(d, |b| b.min(d))));
            assert_eq!(c.hit_distance(origin, dir, f64::INFINITY), brute);
        }
    }

    #[test]
    fn an_awmh_file_reads_back() {
        let mut bytes = b"AWMH".to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        for v in [[0.0f32, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]] {
            for x in v {
                bytes.extend(x.to_le_bytes());
            }
        }
        for i in [0u32, 1, 2] {
            bytes.extend(i.to_le_bytes());
        }
        let c = Collider::from_awmh(&bytes).unwrap();
        assert_eq!(c.floor_below(DVec3::new(1.0, 1.0, 5.0), 10.0), Some(0.0));
        assert!(Collider::from_awmh(&bytes[..20]).is_none());
        assert!(Collider::from_awmh(b"NOPE").is_none());
    }
}
