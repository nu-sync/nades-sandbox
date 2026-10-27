//! The noise CS2's smoke shader breaks its cloud up with: the game's own (the shared effects pack's, which `nades-pack
//! --effects` reads from the game's `pak01`), or the stand-in the web view builds to its measurements
//! (`web/app/src/scene/smokeNoise.ts`), ported here.
//!
//! The game's smoke material samples a 128³ RGBA8 volume, `materials/dev/noise/worley_perlin`, and uses only R + 0.95 G
//! of it: R is clustered blobs, G a smooth, low-contrast field. The stand-in has the same make-up: a smooth field, plus
//! blobs splatted where a cluster field allows. It tiles.

/// Texels a side.
pub const SIZE: usize = 128;

/// A seeded Park–Miller generator in (0, 1).
struct Generator(f64);

impl Generator {
    fn new(seed: u64) -> Self {
        let s = (seed % 2_147_483_647) as f64;
        Self(if s == 0.0 { 1.0 } else { s })
    }

    fn next(&mut self) -> f64 {
        self.0 = (self.0 * 16807.0) % 2_147_483_647.0;
        self.0 / 2_147_483_647.0
    }
}

fn index(size: usize, x: i64, y: i64, z: i64) -> usize {
    let w = |c: i64| c.rem_euclid(size as i64) as usize;
    (w(z) * size + w(y)) * size + w(x)
}

/// The stand-in: one channel of `size`³ texels, 0..255.
pub fn stand_in(size: usize, seed: u64) -> Vec<u8> {
    let n = size * size * size;
    let mut random = Generator::new(seed);
    let texel = size as f64 / 128.0;
    // G: a smooth field, 32 texels a lattice cell, with a quarter of an octave twice as fine
    let smooth = value_noise(size, size / 32, &mut random);
    let finer = value_noise(size, size / 16, &mut random);
    let mut g: Vec<f32> = (0..n).map(|i| (f64::from(smooth[i]) + 0.25 * f64::from(finer[i])) as f32).collect();
    standardize(&mut g, 0.147, 0.031);
    // R: overlapping blobs, one per cell of a jittered grid, stronger where a slower field is high
    let cluster = value_noise(size, size / 16, &mut random);
    let mut r = vec![0f32; n];
    let cells = ((size as f64 / (10.0 * texel) + 0.5).floor() as usize).max(1);
    let step = size as f64 / cells as f64;
    for cz in 0..cells {
        for cy in 0..cells {
            for cx in 0..cells {
                let x = (cx as f64 + random.next()) * step;
                let y = (cy as f64 + random.next()) * step;
                let z = (cz as f64 + random.next()) * step;
                let gather = f64::from(cluster[index(size, x.floor() as i64, y.floor() as i64, z.floor() as i64)]);
                let amplitude = random.next().powf(0.8) * (0.3 + 1.4 * gather);
                let radius = 7.5 * texel * (0.8 + 0.4 * random.next());
                splat(&mut r, size, [x, y, z], radius, amplitude);
            }
        }
    }
    let mean = r.iter().map(|v| f64::from(*v)).sum::<f64>() / n as f64;
    let scale = 0.074 / if mean == 0.0 { 1.0 } else { mean };
    (0..n).map(|i| (255.0 * (f64::from(r[i]) * scale + 0.95 * f64::from(g[i])).clamp(0.0, 1.0) + 0.5).floor() as u8).collect()
}

/// Adds a soft blob, (1 − (d/r)²)².
fn splat(field: &mut [f32], size: usize, [x, y, z]: [f64; 3], radius: f64, amplitude: f64) {
    let r2 = radius * radius;
    let (lo, hi) = (|c: f64| (c - radius).floor() as i64, |c: f64| (c + radius).ceil() as i64);
    for k in lo(z)..=hi(z) {
        let dz = k as f64 + 0.5 - z;
        for j in lo(y)..=hi(y) {
            let dy = j as f64 + 0.5 - y;
            for i in lo(x)..=hi(x) {
                let dx = i as f64 + 0.5 - x;
                let d2 = dx * dx + dy * dy + dz * dz;
                if d2 >= r2 {
                    continue;
                }
                let w = 1.0 - d2 / r2;
                let at = index(size, i, j, k);
                field[at] = (f64::from(field[at]) + amplitude * w * w) as f32;
            }
        }
    }
}

/// Tiling value noise in 0..1 with `cells` lattice cells across, smoothly interpolated; separable, three passes.
fn value_noise(size: usize, cells: usize, random: &mut Generator) -> Vec<f32> {
    let lattice: Vec<f32> = (0..cells * cells * cells).map(|_| random.next() as f32).collect();
    let weights: Vec<(usize, usize, f64)> = (0..size)
        .map(|t| {
            let f = t as f64 / size as f64 * cells as f64;
            let i = f.floor();
            let u = f - i;
            (i as usize % cells, (i as usize + 1) % cells, u * u * (3.0 - 2.0 * u))
        })
        .collect();
    let lerp = |a: f32, b: f32, w: f64| (f64::from(a) + (f64::from(b) - f64::from(a)) * w) as f32;
    let mut a = vec![0f32; size * cells * cells];
    for k in 0..cells {
        for j in 0..cells {
            for (x, &(i0, i1, w)) in weights.iter().enumerate() {
                a[(k * cells + j) * size + x] = lerp(lattice[(k * cells + j) * cells + i0], lattice[(k * cells + j) * cells + i1], w);
            }
        }
    }
    let mut b = vec![0f32; size * size * cells];
    for k in 0..cells {
        for (y, &(j0, j1, w)) in weights.iter().enumerate() {
            for x in 0..size {
                b[(k * size + y) * size + x] = lerp(a[(k * cells + j0) * size + x], a[(k * cells + j1) * size + x], w);
            }
        }
    }
    let mut c = vec![0f32; size * size * size];
    for (z, &(k0, k1, w)) in weights.iter().enumerate() {
        for y in 0..size {
            for x in 0..size {
                c[(z * size + y) * size + x] = lerp(b[(k0 * size + y) * size + x], b[(k1 * size + y) * size + x], w);
            }
        }
    }
    c
}

/// Rescales in place to the given mean and standard deviation.
fn standardize(v: &mut [f32], mean: f64, sd: f64) {
    let n = v.len() as f64;
    let m = v.iter().map(|x| f64::from(*x)).sum::<f64>() / n;
    let s = (v.iter().map(|x| (f64::from(*x) - m).powi(2)).sum::<f64>() / n).sqrt();
    let s = if s == 0.0 { 1.0 } else { s };
    for x in v.iter_mut() {
        *x = (mean + (f64::from(*x) - m) / s * sd) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stand-in's make-up, as web/app/test/smokeNoise.test.ts checks it against the game's measurements: R + 0.95 G
    /// with median about 0.19 and 90th percentile about 0.33, tiling.
    #[test]
    fn the_stand_in_has_the_games_percentiles() {
        let noise = stand_in(64, 7);
        let mut sorted: Vec<f64> = noise.iter().map(|&v| f64::from(v) / 255.0).collect();
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
        assert!((at(0.5) - 0.194).abs() < 0.04, "median {}", at(0.5));
        assert!((at(0.9) - 0.325).abs() < 0.07, "90th {}", at(0.9));
        assert!(at(0.1) > 0.08);
        assert_eq!(stand_in(64, 7), noise, "seeded");
    }
}
