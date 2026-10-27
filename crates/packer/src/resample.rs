//! Pillow's `Image.resize` for 8-bit images, ported exactly (its `Resample.c`): the filter's weights per output texel,
//! normalized and fixed to 22 bits, horizontal then vertical (vertical first when the height shrinks by more than
//! twice what the width does, as Pillow 13 orders them), each pass rounded and clipped to a byte. RGBA images are
//! resized premultiplied (`RGBa`) and straightened again, as `Image.resize` does for them. The packer's effects
//! (`cs2_effects.py`'s Lanczos frames) and the layer data `export_map.py` makes (bilinear bands, box averages) go
//! through it, so they come out as those scripts' Pillow made them.

/// Pillow's filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Box,
    Bilinear,
    Lanczos,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Box => 0.5,
            Filter::Bilinear => 1.0,
            Filter::Lanczos => 3.0,
        }
    }

    fn weight(self, x: f64) -> f64 {
        match self {
            Filter::Box => {
                if x > -0.5 && x <= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Filter::Bilinear => {
                let x = x.abs();
                if x < 1.0 { 1.0 - x } else { 0.0 }
            }
            Filter::Lanczos => {
                let sinc = |x: f64| if x == 0.0 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                if (-3.0..3.0).contains(&x) { sinc(x) * sinc(x / 3.0) } else { 0.0 }
            }
        }
    }
}

/// Fixed point: 32 − 8 − 2 bits.
const PRECISION: u32 = 22;

/// `precompute_coeffs` and `normalize_coeffs_8bpc`: each output texel's first input texel and its weights.
fn coefficients(filter: Filter, input: usize, output: usize) -> Vec<(usize, Vec<i32>)> {
    let scale = input as f64 / output as f64;
    let filterscale = scale.max(1.0);
    let support = filter.support() * filterscale;
    (0..output)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let ss = 1.0 / filterscale;
            let xmin = ((center - support + 0.5) as i64).max(0) as usize;
            let xmax = ((center + support + 0.5) as i64).min(input as i64) as usize - xmin;
            let mut k: Vec<f64> = (0..xmax).map(|x| filter.weight((x as f64 + xmin as f64 - center + 0.5) * ss)).collect();
            let ww: f64 = k.iter().sum();
            if ww != 0.0 {
                for w in &mut k {
                    *w /= ww;
                }
            }
            let one = f64::from(1u32 << PRECISION);
            (xmin, k.iter().map(|&w| if w < 0.0 { (-0.5 + w * one) as i32 } else { (0.5 + w * one) as i32 }).collect())
        })
        .collect()
}

fn clip8(ss: i64) -> u8 {
    (ss >> PRECISION).clamp(0, 255) as u8
}

/// Pillow's `MULDIV255`.
fn muldiv255(a: u32, b: u32) -> u8 {
    let t = a * b + 128;
    (((t >> 8) + t) >> 8) as u8
}

/// Resizes `bands`-channel 8-bit texels (`width` × `height`, rows top first) as Pillow resizes them, without the
/// premultiplication (`L`, `RGB` and `RGBa` images).
pub fn resize_bands(texels: &[u8], bands: usize, (width, height): (usize, usize), (out_w, out_h): (usize, usize), filter: Filter) -> Vec<u8> {
    // Pillow 13: "if height is being scaled down more than twice the amount the width is, run the vertical pass first"
    let vertical_first = height > out_h && (height - out_h) as i64 > (width as i64 - out_w as i64) * 2;
    let mut current = texels.to_vec();
    let mut size = (width, height);
    let passes: [bool; 2] = if vertical_first { [false, true] } else { [true, false] };
    for horizontal in passes {
        if horizontal && out_w != size.0 {
            current = horizontal_pass(&current, bands, size, out_w, filter);
            size.0 = out_w;
        } else if !horizontal && out_h != size.1 {
            current = vertical_pass(&current, bands, size, out_h, filter);
            size.1 = out_h;
        }
    }
    current
}

fn horizontal_pass(current: &[u8], bands: usize, (w, h): (usize, usize), out_w: usize, filter: Filter) -> Vec<u8> {
    let half = 1i64 << (PRECISION - 1);
    let k = coefficients(filter, w, out_w);
    let mut out = vec![0u8; out_w * h * bands];
    for y in 0..h {
        for (xx, (xmin, weights)) in k.iter().enumerate() {
            for c in 0..bands {
                let mut ss = half;
                for (x, &wt) in weights.iter().enumerate() {
                    ss += i64::from(current[(y * w + xmin + x) * bands + c]) * i64::from(wt);
                }
                out[(y * out_w + xx) * bands + c] = clip8(ss);
            }
        }
    }
    out
}

fn vertical_pass(current: &[u8], bands: usize, (w, h): (usize, usize), out_h: usize, filter: Filter) -> Vec<u8> {
    let half = 1i64 << (PRECISION - 1);
    let k = coefficients(filter, h, out_h);
    let mut out = vec![0u8; w * out_h * bands];
    for (yy, (ymin, weights)) in k.iter().enumerate() {
        for x in 0..w {
            for c in 0..bands {
                let mut ss = half;
                for (y, &wt) in weights.iter().enumerate() {
                    ss += i64::from(current[((ymin + y) * w + x) * bands + c]) * i64::from(wt);
                }
                out[(yy * w + x) * bands + c] = clip8(ss);
            }
        }
    }
    out
}

/// `Image.resize` of an RGBA image: premultiplied, resized, straightened.
pub fn resize_rgba(rgba: &[u8], size: (usize, usize), out: (usize, usize), filter: Filter) -> Vec<u8> {
    let premultiplied: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| {
            let a = u32::from(p[3]);
            [muldiv255(u32::from(p[0]), a), muldiv255(u32::from(p[1]), a), muldiv255(u32::from(p[2]), a), p[3]]
        })
        .collect();
    let mut current = resize_bands(&premultiplied, 4, size, out, filter);
    for p in current.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((255 * u32::from(*c)) / a).min(255) as u8;
            }
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_image_stays_flat_and_the_weights_sum_to_one() {
        for filter in [Filter::Box, Filter::Bilinear, Filter::Lanczos] {
            let flat = [200u8, 100, 50, 255].repeat(300 * 90);
            assert!(resize_rgba(&flat, (300, 90), (128, 128), filter).chunks_exact(4).all(|p| p == [200, 100, 50, 255]), "{filter:?}");
            for (_, k) in coefficients(filter, 300, 128).iter().chain(&coefficients(filter, 90, 128)).chain(&coefficients(filter, 1, 64)) {
                let sum: i64 = k.iter().map(|&w| i64::from(w)).sum();
                assert!((sum - (1 << PRECISION)).abs() <= k.len() as i64, "{filter:?}");
            }
        }
        // transparent texels do not bleed their colour: premultiplied
        let mut half = [255u8, 0, 0, 255].repeat(8 * 8);
        for p in half.chunks_exact_mut(4).skip(32) {
            p.copy_from_slice(&[0, 255, 0, 0]);
        }
        assert!(resize_rgba(&half, (8, 8), (4, 4), Filter::Lanczos).chunks_exact(4).all(|p| p[3] == 0 || p[1] == 0));
    }

    #[test]
    fn a_tall_image_is_resized_vertical_pass_first_as_pillow_does() {
        // 2 × 8 → 1 × 2: the height shrinks by 6, more than twice the width's 1
        let texels: Vec<u8> = (0..16u8).map(|i| i.wrapping_mul(37)).collect();
        let vertical_then_horizontal = horizontal_pass(&vertical_pass(&texels, 1, (2, 8), 2, Filter::Bilinear), 1, (2, 2), 1, Filter::Bilinear);
        assert_eq!(resize_bands(&texels, 1, (2, 8), (1, 2), Filter::Bilinear), vertical_then_horizontal);
        // 8 × 2 → 2 × 1: horizontal first
        let horizontal_then_vertical = vertical_pass(&horizontal_pass(&texels, 1, (8, 2), 2, Filter::Bilinear), 1, (2, 2), 1, Filter::Bilinear);
        assert_eq!(resize_bands(&texels, 1, (8, 2), (2, 1), Filter::Bilinear), horizontal_then_vertical);
    }

    #[test]
    fn a_box_halving_averages_pairs_and_a_one_texel_image_fills_the_output() {
        let row = [10u8, 20, 30, 50];
        assert_eq!(resize_bands(&row, 1, (4, 1), (2, 1), Filter::Box), vec![15, 40]);
        assert_eq!(resize_bands(&[77], 1, (1, 1), (64, 64), Filter::Box), vec![77; 64 * 64]);
    }
}
