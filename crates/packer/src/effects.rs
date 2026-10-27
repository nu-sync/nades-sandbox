//! The shared effects pack (`nades-pack --effects`): the game's smoke noise and fire flipbooks from its `pak01`, as
//! show-me-the-nades' `scripts/cs2_effects.py web` made them for the web view (its D24, D25, D27), so the two draw the
//! same textures.
//!
//! - **The noise**: `materials/dev/noise/worley_perlin_z0000_tga_37c9ebc1.vtex_c` (the smoke material's
//!   `g_tSmokeNoiseTex`; the folder holds another texture of the same source name), a 128³ RGBA8 volume read
//!   straight from `pak01`, kept as R + 0.95 G in f32 as the script computes it.
//! - **The flipbooks**: each of [`FLIPBOOKS`] decompiled by Source 2 Viewer's CLI into its sheet (`.mks`: sequences
//!   of frames) and one PNG a frame; each frame resized to a [`CELL`]-texel square by Pillow's Lanczos filter (on
//!   premultiplied alpha, 22-bit fixed point, as Pillow resizes RGBA) and laid out in sequence order, √n columns. A
//!   flipbook whose frames have no alpha of their own (a_v02) takes its colour's brightness as alpha, as the game's
//!   SPRITECARD_TEXTURE_CHANNEL_MIX_RGBA_RGBALPHA draws it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail, ensure};
use nades_pack::effects::{Effects, Flipbook, NOISE_SIZE};
use nades_source2::vpk::Vpk;

/// The fire systems' flipbooks the view draws (show-me-the-nades D24).
pub const FLIPBOOKS: [&str; 4] = ["fire_small_sim_a", "fire_small_sim_b", "fire_small_sim_b_desat", "fire_small_sim_a_v02"];
pub const FLIPBOOK_DIR: &str = "materials/particle/fire_small_sim";
pub const NOISE: &str = "materials/dev/noise/worley_perlin_z0000_tga_37c9ebc1.vtex_c";
/// Texels a frame keeps: the view's flames are a few dozen pixels across.
pub const CELL: u32 = 128;

/// Builds the effects from `pak` (`game/csgo/pak01_dir.vpk`), decompiling the flipbooks with `cli` in a scratch
/// folder under `scratch`.
pub fn build(cli: &Path, pak: &Path, scratch: &Path) -> Result<Effects> {
    let vpk = Vpk::open(pak).with_context(|| format!("opening {}", pak.display()))?;
    let noise = noise(&vpk.read_checked(NOISE).with_context(|| format!("{NOISE} in {}", pak.display()))?)?;
    let dir = scratch.join(format!(".nades-pack-effects-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let result = (|| {
        let entries: Vec<String> = FLIPBOOKS.iter().map(|n| format!("{FLIPBOOK_DIR}/{n}.vtex_c")).collect();
        let output = Command::new(cli)
            .arg("-i")
            .arg(pak)
            .args(["-f", &entries.join(","), "-d", "-o"])
            .arg(&dir)
            // the CLI writes exceptions.txt where it runs
            .current_dir(&dir)
            .output()
            .with_context(|| format!("running {}", cli.display()))?;
        ensure!(output.status.success(), "{} exited {} decompiling the flipbooks: {}", cli.display(), output.status, String::from_utf8_lossy(&output.stderr).trim());
        let src = dir.join(FLIPBOOK_DIR);
        FLIPBOOKS.iter().map(|name| flipbook(&src, name)).collect::<Result<Vec<_>>>()
    })();
    let _ = std::fs::remove_dir_all(&dir);
    let effects = Effects { noise, flipbooks: result? };
    effects.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(effects)
}

/// R + 0.95 G of the 128³ RGBA8 volume, in f32 and rounded half to even, as numpy computes it.
pub fn noise(file: &[u8]) -> Result<Vec<u8>> {
    let vtex = nades_source2::parse(file).context(NOISE)?;
    ensure!(
        (vtex.width, vtex.height, vtex.depth, vtex.format) == (NOISE_SIZE as u32, NOISE_SIZE as u32, NOISE_SIZE as u32, nades_source2::Format::Rgba8888),
        "{NOISE} is {}×{}×{} {:?}, not 128³ RGBA8",
        vtex.width,
        vtex.height,
        vtex.depth,
        vtex.format
    );
    let mut texels = vec![0u8; vtex.mip_bytes(0)];
    vtex.read_mip(file, 0, &mut texels).context(NOISE)?;
    ensure!(texels.len() == NOISE_SIZE.pow(3) * 4, "{NOISE}'s first mip holds {} bytes", texels.len());
    Ok(texels
        .chunks_exact(4)
        .map(|t| {
            let (r, g) = (f32::from(t[0]) / 255.0, f32::from(t[1]) / 255.0);
            ((r + 0.95 * g).clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
        })
        .collect())
}

/// One frame, RGBA8, as Pillow opens the CLI's PNG: 16-bit samples keep their high byte.
struct Image {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

fn read_png(path: &Path) -> Result<Image> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).with_context(|| path.display().to_string())?));
    let mut reader = decoder.read_info().with_context(|| path.display().to_string())?;
    let mut buffer = vec![0u8; reader.output_buffer_size().context("a PNG too large")?];
    let info = reader.next_frame(&mut buffer).with_context(|| path.display().to_string())?;
    buffer.truncate(info.buffer_size());
    let (width, height) = (info.width as usize, info.height as usize);
    let rgba = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buffer,
        // big-endian samples: the high byte first
        (png::ColorType::Rgba, png::BitDepth::Sixteen) => buffer.chunks_exact(2).map(|s| s[0]).collect(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => buffer.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        (png::ColorType::Rgb, png::BitDepth::Sixteen) => buffer.chunks_exact(6).flat_map(|p| [p[0], p[2], p[4], 255]).collect(),
        other => bail!("{}: a {other:?} PNG", path.display()),
    };
    Ok(Image { width, height, rgba })
}

/// The `.mks` sheet: each sequence's frame files.
fn sheet(text: &str) -> Vec<Vec<String>> {
    let mut sequences: Vec<Vec<String>> = Vec::new();
    for line in text.lines() {
        if line.strip_prefix("sequence ").is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit())) {
            sequences.push(Vec::new());
        } else if let Some(rest) = line.strip_prefix("frame ")
            && let Some(file) = rest.split_whitespace().next()
            && let Some(last) = sequences.last_mut()
        {
            last.push(file.to_string());
        }
    }
    sequences
}

fn flipbook(src: &Path, name: &str) -> Result<Flipbook> {
    let mks: PathBuf = src.join(format!("{name}.mks"));
    let sequences = sheet(&std::fs::read_to_string(&mks).with_context(|| format!("{}: the CLI wrote no sheet", mks.display()))?);
    let frames: Vec<&String> = sequences.iter().flatten().collect();
    ensure!(!frames.is_empty(), "{name}'s sheet has no frames");
    let columns = (frames.len() as f64).sqrt().ceil() as usize;
    let rows = frames.len().div_ceil(columns);
    let cell = CELL as usize;
    let stride = columns * cell * 4;
    let mut rgba = vec![0u8; rows * cell * stride];
    let mut opaque = true;
    for (i, file) in frames.iter().enumerate() {
        let frame = read_png(&src.join(file))?;
        opaque &= frame.rgba.chunks_exact(4).all(|p| f32::from(p[3]) / 255.0 > 0.99);
        let resized = lanczos(&frame, cell, cell);
        let (y, x) = (i / columns, i % columns);
        for row in 0..cell {
            let at = (y * cell + row) * stride + x * cell * 4;
            rgba[at..at + cell * 4].copy_from_slice(&resized[row * cell * 4..(row + 1) * cell * 4]);
        }
    }
    if opaque {
        for p in rgba.chunks_exact_mut(4) {
            p[3] = p[0].max(p[1]).max(p[2]);
        }
    }
    let mut first = 0u32;
    let sequences = sequences
        .iter()
        .map(|s| {
            let at = first;
            first += s.len() as u32;
            (at, s.len() as u32)
        })
        .collect();
    Ok(Flipbook { name: name.to_string(), cell: CELL, columns: columns as u32, rows: rows as u32, sequences, rgba })
}

/// Pillow's fixed point: 32 − 8 − 2 bits.
const PRECISION: u32 = 22;

fn lanczos_kernel(x: f64) -> f64 {
    let sinc = |x: f64| if x == 0.0 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
    if -3.0 < x && x < 3.0 { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

/// Pillow's `precompute_coeffs` and `normalize_coeffs_8bpc`: each output texel's first input texel and its weights.
fn coefficients(input: usize, output: usize) -> Vec<(usize, Vec<i32>)> {
    let scale = input as f64 / output as f64;
    let filterscale = scale.max(1.0);
    let support = 3.0 * filterscale;
    (0..output)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let ss = 1.0 / filterscale;
            let xmin = ((center - support + 0.5) as i64).max(0) as usize;
            let xmax = ((center + support + 0.5) as i64).min(input as i64) as usize - xmin;
            let mut k: Vec<f64> = (0..xmax).map(|x| lanczos_kernel((x as f64 + xmin as f64 - center + 0.5) * ss)).collect();
            let ww: f64 = k.iter().sum();
            if ww != 0.0 {
                for w in &mut k {
                    *w /= ww;
                }
            }
            let fixed = k.iter().map(|&w| if w < 0.0 { (-0.5 + w * f64::from(1u32 << PRECISION)) as i32 } else { (0.5 + w * f64::from(1u32 << PRECISION)) as i32 }).collect();
            (xmin, fixed)
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

/// `Image.resize((w, h), Image.LANCZOS)` of an RGBA image: premultiplied (RGBa), horizontal then vertical, back to
/// straight alpha.
fn lanczos(image: &Image, width: usize, height: usize) -> Vec<u8> {
    let premultiplied: Vec<u8> = image.rgba.chunks_exact(4).flat_map(|p| [muldiv255(u32::from(p[0]), u32::from(p[3])), muldiv255(u32::from(p[1]), u32::from(p[3])), muldiv255(u32::from(p[2]), u32::from(p[3])), p[3]]).collect();
    let half = 1i64 << (PRECISION - 1);
    let mut current = premultiplied;
    let (mut w, h0) = (image.width, image.height);
    if width != w {
        let k = coefficients(w, width);
        let mut out = vec![0u8; width * h0 * 4];
        for y in 0..h0 {
            for (xx, (xmin, weights)) in k.iter().enumerate() {
                for c in 0..4 {
                    let mut ss = half;
                    for (x, &wt) in weights.iter().enumerate() {
                        ss += i64::from(current[(y * w + xmin + x) * 4 + c]) * i64::from(wt);
                    }
                    out[(y * width + xx) * 4 + c] = clip8(ss);
                }
            }
        }
        current = out;
        w = width;
    }
    if height != h0 {
        let k = coefficients(h0, height);
        let mut out = vec![0u8; w * height * 4];
        for (yy, (ymin, weights)) in k.iter().enumerate() {
            for x in 0..w {
                for c in 0..4 {
                    let mut ss = half;
                    for (y, &wt) in weights.iter().enumerate() {
                        ss += i64::from(current[((ymin + y) * w + x) * 4 + c]) * i64::from(wt);
                    }
                    out[(yy * w + x) * 4 + c] = clip8(ss);
                }
            }
        }
        current = out;
    }
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
    fn a_sheet_lists_its_sequences_frames() {
        let mks = "// Reconstructed with Source 2 Viewer\n\n\nsequence 0\nframe a_seq0_0.png 1\nframe a_seq0_1.png 1\n\nsequence 1\nloop\nframe a_seq1_0.png 1\n";
        assert_eq!(sheet(mks), vec![vec!["a_seq0_0.png".to_string(), "a_seq0_1.png".into()], vec!["a_seq1_0.png".into()]]);
    }

    #[test]
    fn the_resize_keeps_a_flat_image_flat_and_weights_sum_to_one() {
        let flat = Image { width: 300, height: 90, rgba: [200u8, 100, 50, 255].repeat(300 * 90) };
        let out = lanczos(&flat, 128, 128);
        assert!(out.chunks_exact(4).all(|p| p == [200, 100, 50, 255]));
        for (_, k) in coefficients(300, 128).iter().chain(&coefficients(90, 128)) {
            let sum: i64 = k.iter().map(|&w| i64::from(w)).sum();
            assert!((sum - (1 << PRECISION)).abs() <= k.len() as i64);
        }
        // transparent texels do not bleed their colour: premultiplied
        let mut half = [255u8, 0, 0, 255].repeat(8 * 8);
        for p in half.chunks_exact_mut(4).skip(32) {
            p.copy_from_slice(&[0, 255, 0, 0]);
        }
        let out = lanczos(&Image { width: 8, height: 8, rgba: half }, 4, 4);
        assert!(out.chunks_exact(4).all(|p| p[3] == 0 || p[1] == 0), "{out:?}");
    }

    #[test]
    fn the_noise_is_r_plus_most_of_g() {
        assert_eq!(((100.0f32 / 255.0 + 0.95 * (40.0f32 / 255.0)).clamp(0.0, 1.0) * 255.0).round_ties_even() as u8, 138);
    }
}
