//! The map's geometry export, made by the packer from the game's files as show-me-the-nades' `export_map.py` made it
//! (`<map>.glb`, and `<map>.skybox.glb` for the 3D skybox's prefab), step for step, so the packs come out the same:
//!
//! 1. **Stage** a game folder Source 2 Viewer resolves files in: `game/csgo/gameinfo.gi` (a copy), `game/core` and
//!    the maps' VPKs (links into the game folder), and `pak01_dir.vpk` with the numbered archives the game folder
//!    holds (links). CS2's shader archives stay out (`shaders_vulkan_*`, in `core` and `csgo`), as they were absent
//!    when `export_map.py` made the exports: with their directory and not its archives the CLI fails on materials that
//!    ask a shader, and with all of them it would read other material attributes. A `pak01` archive missing from the game folder is
//!    passed over in silence by the CLI, so `nades-pack setup <map>` fetches what a map needs first ([`setup`]).
//! 2. **Export** `maps/<map>.vmap_c` with export_map.py's arguments: `-d --gltf_export_format gltf
//!    --gltf_export_materials --gltf_textures_adapt --gltf_export_extras --game <staged gameinfo.gi> --threads 8`.
//! 3. **Layers** ([`layers`]): `export_map.py`'s `layer_textures`, ported line for line: per layered material
//!    (csgo_environment(_blend), two-layer csgo_lightmappedgeneric) a packed data texture of its heights and tint mask
//!    (or its blend modulation), its second and third layers' colours in the sheen and specular slots, and each layer
//!    colour's linear average in `extras.layers`, from the layer textures the CLI decompiles. Pillow's resizes are
//!    [`crate::resample`]'s; the glTF is edited with its key order kept ([`crate::ojson`]).
//! 4. **Compress** with glTF-Transform 4.5.1 (`bunx --bun @gltf-transform/cli@4.5.1`), export_map.py's two commands:
//!    `optimize` (no simplify, palette, flatten or attribute pruning; WebP textures of at most 1024) and `meshopt`
//!    (texture coordinates in 14 bits, the rest in 16).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail, ensure};
use nades_source2::vpk::Vpk;

use crate::ojson::{self, Json};
use crate::resample::{self, Filter};

/// The glTF-Transform the exports were compressed with.
pub const GLTF_TRANSFORM: &str = "@gltf-transform/cli@4.5.1";

/// The tools and places one export uses.
pub struct Exporter<'a> {
    /// Source 2 Viewer's CLI.
    pub cli: &'a Path,
    /// `bunx`, which runs glTF-Transform.
    pub bunx: &'a Path,
    /// The game folder (holding `game/csgo`).
    pub game: &'a Path,
    /// Where the exports are made: `<work>/<label>/` while working, `<work>/<label>.glb` when done.
    pub work: PathBuf,
}

/// What one export did.
#[derive(Debug, Default)]
pub struct Exported {
    pub glb: PathBuf,
    /// Files the CLI said it could not load.
    pub failed_to_load: Vec<String>,
    /// Layered materials: environment, lightmapped, with a second layer, with a third.
    pub layered: [u32; 4],
    pub seconds: [f64; 3],
}

fn link(target: &Path, at: &Path) -> Result<()> {
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, at).with_context(|| format!("linking {} to {}", at.display(), target.display()))?;
    #[cfg(windows)]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, at)
        } else {
            std::fs::hard_link(target, at).or_else(|_| std::os::windows::fs::symlink_file(target, at))
        }
        .with_context(|| format!("linking {} to {}", at.display(), target.display()))?;
    }
    Ok(())
}

/// The staged game folder for `maps` (names under `maps/`, e.g. `de_cache`, `prefabs/de_cache/de_cache_sky`), made
/// afresh at `dir`; its `game/csgo/gameinfo.gi`.
pub fn stage(game: &Path, dir: &Path, maps: &[&str]) -> Result<PathBuf> {
    stage_with(game, dir, maps, true)
}

/// [`stage`], with `pak01` linked in or not: without it the CLI names each file it cannot find (`Failed to load`),
/// which is how `nades-pack setup` learns what a map needs; files unpacked into the staged `game/csgo` stand in.
pub fn stage_with(game: &Path, dir: &Path, maps: &[&str], with_pak: bool) -> Result<PathBuf> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    let csgo = game.join("game/csgo");
    let staged = dir.join("game/csgo");
    std::fs::create_dir_all(&staged)?;
    std::fs::copy(csgo.join("gameinfo.gi"), staged.join("gameinfo.gi")).with_context(|| format!("{} has no game/csgo/gameinfo.gi", game.display()))?;
    // core's files, its shader archives left out as csgo's are
    for entry in std::fs::read_dir(game.join("game/core")).with_context(|| format!("{} has no game/core", game.display()))? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with("shaders_") {
            link(&entry.path(), &dir.join("game/core").join(entry.file_name()))?;
        }
    }
    for map in maps {
        let vpk = csgo.join("maps").join(format!("{map}.vpk"));
        ensure!(vpk.is_file(), "{} is not in the game folder (nades-pack setup fetches it)", vpk.display());
        link(&vpk, &staged.join("maps").join(format!("{map}.vpk")))?;
    }
    for entry in std::fs::read_dir(&csgo)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if with_pak && name.starts_with("pak01_") && name.ends_with(".vpk") {
            link(&entry.path(), &staged.join(name.as_ref()))?;
        }
    }
    Ok(staged.join("gameinfo.gi"))
}

/// `Failed to load "…"` lines.
pub fn failed_to_load(log: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for part in log.split("Failed to load \"").skip(1) {
        if let Some(end) = part.find('"') {
            out.insert(part[..end].to_string());
        }
    }
    out.into_iter().collect()
}

impl Exporter<'_> {
    /// Step 2: `maps/<map>.vmap_c` exported to `<work>/<label>/<label>.gltf` from the staged `gameinfo`; the CLI's
    /// output.
    pub fn export(&self, gameinfo: &Path, map: &str, label: &str) -> Result<String> {
        let out = self.work.join(label);
        if out.exists() {
            std::fs::remove_dir_all(&out).with_context(|| format!("clearing {}", out.display()))?;
        }
        std::fs::create_dir_all(&out)?;
        let staged_maps = gameinfo.parent().expect("gameinfo's folder").join("maps");
        let output = Command::new(self.cli)
            .arg("-i")
            .arg(staged_maps.join(format!("{map}.vpk")))
            .args(["-f", &format!("maps/{map}.vmap_c"), "-o"])
            .arg(out.join(format!("{label}.gltf")))
            .args(["-d", "--gltf_export_format", "gltf", "--gltf_export_materials", "--gltf_textures_adapt", "--gltf_export_extras", "--game"])
            .arg(gameinfo)
            .args(["--threads", "8"])
            // the CLI writes exceptions.txt where it runs
            .current_dir(&out)
            .output()
            .with_context(|| format!("running {}", self.cli.display()))?;
        let log = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        std::fs::write(out.join("export.log"), &log)?;
        // the CLI exits 2 when a side export fails (the physics mesh's materials want the game's shaders); the map's
        // glTF is what counts
        let gltf = out.join(format!("{label}.gltf"));
        ensure!(gltf.is_file() && out.join(format!("{label}.bin")).is_file(), "{} exported no {} ({}; see {})", self.cli.display(), gltf.display(), output.status, out.join("export.log").display());
        Ok(log)
    }

    /// Steps 2–4 for one map (or prefab) under `label`.
    pub fn make(&self, gameinfo: &Path, pak: &Vpk, pak_path: &Path, map: &str, label: &str) -> Result<Exported> {
        let mut done = Exported::default();
        let clock = std::time::Instant::now();
        let log = self.export(gameinfo, map, label)?;
        done.failed_to_load = failed_to_load(&log);
        done.seconds[0] = clock.elapsed().as_secs_f64();
        let clock = std::time::Instant::now();
        done.layered = layers(self.cli, pak, pak_path, &self.work.join(label), label)?;
        done.seconds[1] = clock.elapsed().as_secs_f64();
        let clock = std::time::Instant::now();
        done.glb = self.compress(label)?;
        done.seconds[2] = clock.elapsed().as_secs_f64();
        Ok(done)
    }

    /// Step 4: `<work>/<label>/<label>.gltf` into `<work>/<label>.glb`.
    pub fn compress(&self, label: &str) -> Result<PathBuf> {
        let src = self.work.join(label).join(format!("{label}.gltf"));
        let tmp = self.work.join(label).join(format!("{label}.tmp.glb"));
        let target = self.work.join(format!("{label}.glb"));
        let run = |args: &[&std::ffi::OsStr]| -> Result<()> {
            let output = Command::new(self.bunx).args(["--bun", GLTF_TRANSFORM]).args(args).current_dir(self.work.join(label)).output().with_context(|| format!("running {}", self.bunx.display()))?;
            ensure!(output.status.success(), "glTF-Transform {:?} exited {}: {}", args.first(), output.status, String::from_utf8_lossy(&output.stderr).trim());
            Ok(())
        };
        let os = |s: &'static str| std::ffi::OsStr::new(s);
        run(&[
            os("optimize"),
            src.as_os_str(),
            tmp.as_os_str(),
            os("--compress"),
            os("false"),
            os("--simplify"),
            os("false"),
            os("--palette"),
            os("false"),
            os("--prune-attributes"),
            os("false"),
            os("--flatten"),
            os("false"),
            os("--texture-compress"),
            os("webp"),
            os("--texture-size"),
            os("1024"),
        ])?;
        let partial = self.work.join(format!("{label}.partial.glb"));
        run(&[os("meshopt"), tmp.as_os_str(), partial.as_os_str(), os("--quantize-texcoord"), os("14"), os("--quantize-generic"), os("16")])?;
        let _ = std::fs::remove_file(&tmp);
        std::fs::rename(&partial, &target)?;
        Ok(target)
    }
}

/// SHA-1, for the packed textures' names (`hashlib.sha1(…).hexdigest()[:16]`).
pub fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().expect("4 bytes"));
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// An image as Pillow's `convert("RGBA")` gives it.
struct Rgba {
    width: usize,
    height: usize,
    texels: Vec<u8>,
}

fn read_png(path: &Path) -> Result<Rgba> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).with_context(|| path.display().to_string())?));
    let mut reader = decoder.read_info().with_context(|| path.display().to_string())?;
    let mut buffer = vec![0u8; reader.output_buffer_size().context("a PNG too large")?];
    let info = reader.next_frame(&mut buffer).with_context(|| path.display().to_string())?;
    buffer.truncate(info.buffer_size());
    let texels = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buffer,
        (png::ColorType::Rgb, png::BitDepth::Eight) => buffer.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => buffer.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        other => bail!("{}: a {other:?} PNG, which the layer step does not read", path.display()),
    };
    Ok(Rgba { width: info.width as usize, height: info.height as usize, texels })
}

fn write_png(path: &Path, size: usize, rgba: &[u8]) -> Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| path.display().to_string())?);
    let mut encoder = png::Encoder::new(file, size as u32, size as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// One band of an RGBA image resized to `size`² by Pillow's bilinear filter, or `default` everywhere without one.
fn channel(image: Option<&Rgba>, band: usize, size: usize, default: u8) -> Vec<u8> {
    let Some(image) = image else { return vec![default; size * size] };
    let plane: Vec<u8> = image.texels.chunks_exact(4).map(|p| p[band]).collect();
    if (image.width, image.height) == (size, size) {
        return plane;
    }
    resample::resize_bands(&plane, 1, (image.width, image.height), (size, size), Filter::Bilinear)
}

/// A colour texture's mean in linear light: its RGB box-filtered to 64², each channel linearized and averaged, rounded
/// to 5 places.
fn average(image: &Rgba) -> [String; 3] {
    let rgb: Vec<u8> = image.texels.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let small = if (image.width, image.height) == (64, 64) { rgb } else { resample::resize_bands(&rgb, 3, (image.width, image.height), (64, 64), Filter::Box) };
    let mut sums = [0f64; 3];
    for p in small.chunks_exact(3) {
        for (s, &v) in sums.iter_mut().zip(p) {
            let srgb = f64::from(v) / 255.0;
            *s += if srgb <= 0.04045 { srgb / 12.92 } else { ((srgb + 0.055) / 1.055).powf(2.4) };
        }
    }
    sums.map(|s| format!("{:.5}", s / 4096.0))
}

/// What one layered material asks for.
struct Plan {
    material: usize,
    kind: &'static str,
    data: Vec<Option<String>>,
    colors: [Option<String>; 3],
}

fn param<'j>(map: Option<&'j Json>, key: &str) -> Option<&'j Json> {
    map.and_then(|m| m.get(key))
}

/// The layered materials of a glTF and the layer textures they name.
fn plans(gltf: &Json) -> Vec<Plan> {
    let mut plans = Vec::new();
    for (i, material) in gltf.get("materials").and_then(Json::as_array).unwrap_or_default().iter().enumerate() {
        let vmat = material.get("extras").and_then(|e| e.get("vmat"));
        let shader = vmat.and_then(|v| v.get("ShaderName")).and_then(Json::as_str);
        let textures = vmat.and_then(|v| v.get("TextureParams"));
        let ints = vmat.and_then(|v| v.get("IntParams"));
        let texture = |key: &str| param(textures, key).and_then(Json::as_str).map(str::to_string);
        let has = |key: &str| param(textures, key).is_some();
        let int_is_one = |key: &str| param(ints, key).and_then(Json::as_f64) == Some(1.0);
        match shader {
            Some(s @ ("csgo_environment.vfx" | "csgo_environment_blend.vfx")) => {
                let layered = s == "csgo_environment_blend.vfx" && has("g_tColor2");
                let third = layered && int_is_one("F_ENABLE_LAYER_3") && has("g_tColor3");
                let pick = |key: &str, on: bool| if on { texture(key) } else { None };
                plans.push(Plan {
                    material: i,
                    kind: "environment",
                    data: vec![texture("g_tHeight1"), pick("g_tHeight2", layered), pick("g_tHeight3", third)],
                    colors: [texture("g_tColor1"), pick("g_tColor2", layered), pick("g_tColor3", third)],
                });
            }
            Some("csgo_lightmappedgeneric.vfx") if int_is_one("F_LAYERS") && has("g_tLayer2Color") => {
                plans.push(Plan { material: i, kind: "lightmapped", data: vec![texture("g_tBlendModulation")], colors: [texture("g_tColor"), texture("g_tLayer2Color"), None] });
            }
            _ => {}
        }
    }
    plans
}

/// The `pak01` entries the layer step decompiles for an exported glTF.
pub fn layer_entries(gltf: &Path, pak: &Vpk) -> Result<Vec<String>> {
    let gltf = ojson::parse(&std::fs::read_to_string(gltf).with_context(|| gltf.display().to_string())?)?;
    let wanted: BTreeSet<String> = plans(&gltf).iter().flat_map(|p| p.data.iter().chain(&p.colors)).flatten().map(|p| format!("{p}_c")).filter(|p| pak.entry(p).is_some()).collect();
    Ok(wanted.into_iter().collect())
}

/// Step 3 on `<folder>/<label>.gltf`: the counts of layered materials (environment, lightmapped, with a second layer,
/// with a third); the glTF is left as the CLI wrote it when no material is layered.
pub fn layers(cli: &Path, pak: &Vpk, pak_path: &Path, folder: &Path, label: &str) -> Result<[u32; 4]> {
    let target = folder.join(format!("{label}.gltf"));
    let mut gltf = ojson::parse(&std::fs::read_to_string(&target).with_context(|| target.display().to_string())?)?;
    let plans = plans(&gltf);
    if plans.is_empty() {
        return Ok([0; 4]);
    }
    // the layer textures, decompiled by the CLI from pak01
    let wanted: BTreeSet<String> = plans.iter().flat_map(|p| p.data.iter().chain(&p.colors)).flatten().map(|p| format!("{p}_c")).filter(|p| pak.entry(p).is_some()).collect();
    let wanted: Vec<String> = wanted.into_iter().collect();
    let raw = folder.join("layers").join("raw");
    for batch in wanted.chunks(200) {
        let _ = Command::new(cli).arg("-i").arg(pak_path).args(["-f", &batch.join(","), "-d", "-o"]).arg(&raw).current_dir(folder).output().with_context(|| format!("running {}", cli.display()))?;
    }
    let decompiled = |path: Option<&String>| -> Option<PathBuf> {
        let file = raw.join(format!("{}.png", path?.strip_suffix(".vtex").unwrap_or(path?)));
        file.is_file().then_some(file)
    };
    let mut images: HashMap<PathBuf, Rgba> = HashMap::new();
    fn load(images: &mut HashMap<PathBuf, Rgba>, file: &Path) -> Result<()> {
        if !images.contains_key(file) {
            images.insert(file.to_path_buf(), read_png(file)?);
        }
        Ok(())
    }
    let mut averages: HashMap<PathBuf, [String; 3]> = HashMap::new();
    let mut added: HashMap<PathBuf, usize> = HashMap::new();
    let packed_dir = folder.join("layers").join("data");
    std::fs::create_dir_all(&packed_dir)?;
    let mut counts = [0u32; 4];
    for plan in &plans {
        let material = &gltf.get("materials").and_then(Json::as_array).expect("materials")[plan.material];
        let base = material.get("pbrMetallicRoughness").and_then(|p| p.get("baseColorTexture"));
        let sampler = base.and_then(|b| b.get("index")).and_then(Json::as_f64).and_then(|t| gltf.get("textures")?.as_array()?.get(t as usize)?.get("sampler").cloned());
        let files: Vec<Option<PathBuf>> = plan.data.iter().map(|p| decompiled(p.as_ref())).collect();
        for f in files.iter().flatten() {
            load(&mut images, f)?;
        }
        let image = |k: usize| files.get(k).and_then(Option::as_ref).and_then(|f| images.get(f));
        let size = (0..files.len()).filter_map(image).map(|im| im.width).max().unwrap_or(4).min(512);
        let bands: [Vec<u8>; 4] = if plan.kind == "environment" {
            // R height 1, G tint mask 1 (height 1's green), B height 2, A height 3
            [channel(image(0), 0, size, 128), channel(image(0), 1, size, 255), channel(image(1), 0, size, 128), channel(image(2), 0, size, 128)]
        } else {
            // the modulation as it is: R softness, G blend mask
            [channel(image(0), 0, size, 0), channel(image(0), 1, size, 128), vec![0; size * size], vec![255; size * size]]
        };
        let merged: Vec<u8> = (0..size * size).flat_map(|k| [bands[0][k], bands[1][k], bands[2][k], bands[3][k]]).collect();
        let key = sha1_hex(plan.data.iter().map(|p| p.as_deref().unwrap_or("-")).collect::<Vec<_>>().join("|").as_bytes());
        let packed = packed_dir.join(format!("{}-{}.png", plan.kind, &key[..16]));
        write_png(&packed, size, &merged)?;
        let mut texture = |gltf: &mut Json, file: &Path| -> usize {
            if let Some(&t) = added.get(file) {
                return t;
            }
            let uri = file.strip_prefix(folder).unwrap_or(file).to_string_lossy().replace('\\', "/");
            let images = gltf.entry("images", Json::Array(vec![]));
            let Json::Array(list) = images else { unreachable!("images is a list") };
            list.push(Json::object(vec![("uri", Json::String(uri))]));
            let source = list.len() - 1;
            let mut entry = vec![("source", Json::number(source))];
            if let Some(s) = &sampler {
                entry.push(("sampler", s.clone()));
            }
            let Json::Array(textures) = gltf.entry("textures", Json::Array(vec![])) else { unreachable!("textures is a list") };
            textures.push(Json::object(entry));
            let t = textures.len() - 1;
            added.insert(file.to_path_buf(), t);
            t
        };
        let packed_index = texture(&mut gltf, &packed);
        let mut sheen = vec![
            ("sheenColorFactor", Json::Array(vec![Json::number(1); 3])),
            ("sheenRoughnessFactor", Json::number(1)),
            ("sheenRoughnessTexture", Json::object(vec![("index", Json::number(packed_index))])),
        ];
        let second = decompiled(plan.colors[1].as_ref());
        let third = decompiled(plan.colors[2].as_ref());
        if let Some(second) = &second {
            sheen.push(("sheenColorTexture", Json::object(vec![("index", Json::number(texture(&mut gltf, second)))])));
            counts[2] += 1;
        }
        let specular = third.as_ref().map(|third| {
            Json::object(vec![
                ("specularFactor", Json::number(1)),
                ("specularColorFactor", Json::Array(vec![Json::number(1); 3])),
                ("specularColorTexture", Json::object(vec![("index", Json::number(texture(&mut gltf, third)))])),
            ])
        });
        let mut average_of = |path: Option<&String>| -> Result<Json> {
            let Some(file) = decompiled(path) else { return Ok(Json::Null) };
            if !averages.contains_key(&file) {
                load(&mut images, &file)?;
                averages.insert(file.clone(), average(&images[&file]));
            }
            Ok(Json::Array(averages[&file].iter().map(|v| Json::Number(v.clone())).collect()))
        };
        let average_list = Json::Array(plan.colors.iter().map(|c| average_of(c.as_ref())).collect::<Result<Vec<_>>>()?);
        let Json::Array(materials) = gltf.get_mut("materials").expect("materials") else { unreachable!("materials is a list") };
        let material = &mut materials[plan.material];
        let extensions = material.entry("extensions", Json::Object(vec![]));
        extensions.set("KHR_materials_sheen", Json::object(sheen));
        if let Some(specular) = specular {
            extensions.set("KHR_materials_specular", specular);
            counts[3] += 1;
        }
        material.entry("extras", Json::Object(vec![])).set("layers", Json::object(vec![("kind", Json::String(plan.kind.into())), ("average", average_list)]));
        counts[usize::from(plan.kind != "environment")] += 1;
    }
    let mut used: BTreeSet<String> = gltf.get("extensionsUsed").and_then(Json::as_array).unwrap_or_default().iter().filter_map(|e| e.as_str().map(str::to_string)).collect();
    used.insert("KHR_materials_sheen".into());
    used.insert("KHR_materials_specular".into());
    gltf.set("extensionsUsed", Json::Array(used.into_iter().map(Json::String).collect()));
    std::fs::write(&target, gltf.to_text())?;
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_is_sha1() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha1_hex(&[b'a'; 1000]), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
    }

    #[test]
    fn the_cli_s_missing_files_are_listed_once() {
        let log = "--- x\nFailed to load \"materials/a.vmat\"\nFailed to load \"materials/b.vtex\"\nFailed to load \"materials/a.vmat\"\n";
        assert_eq!(failed_to_load(log), vec!["materials/a.vmat".to_string(), "materials/b.vtex".into()]);
    }

    #[test]
    fn a_flat_texture_averages_to_its_linear_colour() {
        let image = Rgba { width: 2, height: 2, texels: [255u8, 0, 188, 255].repeat(4) };
        assert_eq!(average(&image), ["1.00000".to_string(), "0.00000".into(), "0.50289".into()]);
    }
}

#[cfg(test)]
mod stage_tests {
    use super::*;

    #[test]
    fn the_stage_links_the_game_but_not_its_shaders() {
        let root = std::env::temp_dir().join(format!("nades-stage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("depot");
        for f in ["game/csgo/gameinfo.gi", "game/csgo/pak01_dir.vpk", "game/csgo/pak01_007.vpk", "game/csgo/shaders_vulkan_dir.vpk", "game/csgo/maps/de_x.vpk", "game/csgo/maps/de_y.vpk", "game/core/pak01_dir.vpk", "game/core/shaders_vulkan_dir.vpk"] {
            std::fs::create_dir_all(game.join(f).parent().unwrap()).unwrap();
            std::fs::write(game.join(f), f).unwrap();
        }
        let gameinfo = stage(&game, &root.join("stage"), &["de_x"]).unwrap();
        let csgo = gameinfo.parent().unwrap();
        let core = root.join("stage/game/core");
        assert!(csgo.join("maps/de_x.vpk").exists() && !csgo.join("maps/de_y.vpk").exists(), "only the maps asked for");
        assert!(csgo.join("pak01_dir.vpk").exists() && csgo.join("pak01_007.vpk").exists() && core.join("pak01_dir.vpk").exists());
        assert!(!csgo.join("shaders_vulkan_dir.vpk").exists() && !core.join("shaders_vulkan_dir.vpk").exists(), "no shaders");
        let without = stage_with(&game, &root.join("bare"), &["de_x"], false).unwrap();
        assert!(!without.parent().unwrap().join("pak01_dir.vpk").exists(), "setup's stage leaves pak01 out");
        assert!(stage(&game, &root.join("stage"), &["de_z"]).is_err(), "a map the game folder lacks");
        let _ = std::fs::remove_dir_all(&root);
    }
}
