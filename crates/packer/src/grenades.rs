//! The grenades' world models (S34), which the game draws a thrown grenade with, for the shared effects pack: each
//! exported by Source 2 Viewer's CLI from `pak01` as a `.glb` with its materials, as show-me-the-nades'
//! `scripts/cs2_effects.py` exports them for its web view (its D30), then read here: every mesh in its bind pose (the
//! models are rigged for the hand; drawn rigged, the pin and ring's vertices stretch), its node's transform applied,
//! turned from glTF's metres (x, y, z the game's y, z, x) into Hammer units, and its base colour cut to
//! [`TEXELS`] texels.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail, ensure};
use glam::{Mat4, Quat, Vec3};
use nades_pack::effects::{GrenadeModel, GrenadePart};
use serde_json::Value;

use crate::resample;

/// Each grenade's world model in `pak01`.
pub const MODELS: [(&str, &str); 6] = [
    ("hegrenade", "weapons/models/grenade/hegrenade/weapon_hegrenade.vmdl_c"),
    ("flashbang", "weapons/models/grenade/flashbang/weapon_flashbang.vmdl_c"),
    ("smokegrenade", "weapons/models/grenade/smokegrenade/weapon_smokegrenade.vmdl_c"),
    ("molotov", "weapons/models/grenade/molotov/weapon_molotov.vmdl_c"),
    ("incendiary", "weapons/models/grenade/incendiary/weapon_incendiarygrenade.vmdl_c"),
    ("decoy", "weapons/models/grenade/decoy/weapon_decoy.vmdl_c"),
];

/// Texels a model's texture keeps a side: it is a few dozen pixels across on screen.
pub const TEXELS: usize = 256;
const INCH: f32 = 0.0254;

/// Every grenade's model `cli` can export from `pak`, in a scratch folder under `dir`; one it cannot (its files not in
/// this depot) is left out, said on stderr.
pub fn build(cli: &Path, pak: &Path, dir: &Path) -> Vec<GrenadeModel> {
    MODELS
        .iter()
        .filter_map(|(name, entry)| match model(cli, pak, dir, name, entry) {
            Ok(model) => Some(model),
            Err(e) => {
                eprintln!("grenades: {name}: {e:#} (the grenade flies as a stand-in)");
                None
            }
        })
        .collect()
}

fn model(cli: &Path, pak: &Path, dir: &Path, name: &str, entry: &str) -> Result<GrenadeModel> {
    let work = dir.join(format!("grenade-{name}"));
    std::fs::create_dir_all(&work)?;
    let glb = work.join(format!("{name}.glb"));
    let output = Command::new(cli)
        .arg("-i")
        .arg(pak)
        .args(["-f", entry, "-d", "--gltf_export_format", "glb", "--gltf_export_materials", "--gltf_textures_adapt", "-o"])
        .arg(&glb)
        // the CLI writes exceptions.txt where it runs
        .current_dir(&work)
        .output()
        .with_context(|| format!("running {}", cli.display()))?;
    ensure!(output.status.success() && glb.exists(), "{} exited {} exporting {entry}: {}", cli.display(), output.status, String::from_utf8_lossy(&output.stderr).trim());
    let model = read(name, &std::fs::read(&glb)?, &work)?;
    ensure!(!model.parts.is_empty(), "{entry} exported no mesh");
    Ok(model)
}

/// A model from a `.glb` whose images are files in `images`.
pub fn read(name: &str, glb: &[u8], images: &Path) -> Result<GrenadeModel> {
    ensure!(glb.len() > 20 && &glb[0..4] == b"glTF", "not a glb");
    let json_length = u32::from_le_bytes(glb[12..16].try_into()?) as usize;
    let doc: Value = serde_json::from_slice(&glb[20..20 + json_length])?;
    let binary = glb.get(20 + json_length + 8..).unwrap_or_default();
    let nodes = doc["nodes"].as_array().cloned().unwrap_or_default();
    // each node's world transform, walking down from the scene's roots
    let mut world = vec![None; nodes.len()];
    let roots: Vec<usize> = doc["scenes"][doc["scene"].as_u64().unwrap_or(0) as usize]["nodes"].as_array().map(|r| r.iter().filter_map(Value::as_u64).map(|n| n as usize).collect()).unwrap_or_default();
    let mut stack: Vec<(usize, Mat4)> = roots.into_iter().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((n, parent)) = stack.pop() {
        let node = &nodes[n];
        let local = local_matrix(node);
        let m = parent * local;
        world[n] = Some(m);
        for child in node["children"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
            stack.push((child as usize, m));
        }
    }
    // glTF's metres and axes into Hammer units: x = z, y = x, z = y
    let to_source = Mat4::from_cols_array(&[0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]).transpose() * Mat4::from_scale(Vec3::splat(1.0 / INCH));
    let mut parts = Vec::new();
    for (n, node) in nodes.iter().enumerate() {
        let (Some(mesh), Some(m)) = (node["mesh"].as_u64(), world[n]) else { continue };
        let m = to_source * m;
        let normal_m = Mat4::from_mat3(glam::Mat3::from_mat4(m).inverse().transpose());
        for primitive in doc["meshes"][mesh as usize]["primitives"].as_array().into_iter().flatten() {
            if primitive["mode"].as_u64().is_some_and(|mode| mode != 4) {
                continue;
            }
            let attributes = &primitive["attributes"];
            let positions: Vec<[f32; 3]> = floats(&doc, binary, attributes["POSITION"].as_u64().context("a primitive without positions")? as usize, 3)?.chunks_exact(3).map(|p| m.transform_point3(Vec3::new(p[0], p[1], p[2])).to_array()).collect();
            let normals: Vec<[f32; 3]> = match attributes["NORMAL"].as_u64() {
                Some(a) => floats(&doc, binary, a as usize, 3)?.chunks_exact(3).map(|v| normal_m.transform_vector3(Vec3::new(v[0], v[1], v[2])).normalize_or_zero().to_array()).collect(),
                None => vec![[0.0, 0.0, 1.0]; positions.len()],
            };
            let uvs: Vec<[f32; 2]> = match attributes["TEXCOORD_0"].as_u64() {
                Some(a) => floats(&doc, binary, a as usize, 2)?.chunks_exact(2).map(|v| [v[0], v[1]]).collect(),
                None => vec![[0.0, 0.0]; positions.len()],
            };
            let indices: Vec<u32> = match primitive["indices"].as_u64() {
                Some(a) => integers(&doc, binary, a as usize)?,
                None => (0..positions.len() as u32).collect(),
            };
            let (width, height, rgba) = base_color(&doc, primitive["material"].as_u64(), binary, images)?;
            parts.push(GrenadePart { positions, normals, uvs, indices, width, height, rgba });
        }
    }
    Ok(GrenadeModel { name: name.into(), parts })
}

fn local_matrix(node: &Value) -> Mat4 {
    if let Some(m) = node["matrix"].as_array() {
        let v: Vec<f32> = m.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
        if v.len() == 16 {
            return Mat4::from_cols_slice(&v);
        }
    }
    let vec = |key: &str, n: usize, default: &[f32]| -> Vec<f32> { node[key].as_array().map(|a| a.iter().filter_map(Value::as_f64).map(|x| x as f32).collect()).filter(|v: &Vec<f32>| v.len() == n).unwrap_or_else(|| default.to_vec()) };
    let t = vec("translation", 3, &[0.0, 0.0, 0.0]);
    let r = vec("rotation", 4, &[0.0, 0.0, 0.0, 1.0]);
    let s = vec("scale", 3, &[1.0, 1.0, 1.0]);
    Mat4::from_scale_rotation_translation(Vec3::new(s[0], s[1], s[2]), Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize(), Vec3::new(t[0], t[1], t[2]))
}

/// An accessor's bytes: where they start, how far apart its elements are, its component type and count.
fn accessor<'a>(doc: &Value, binary: &'a [u8], index: usize, components: usize) -> Result<(&'a [u8], usize, u64, usize)> {
    let a = &doc["accessors"][index];
    let view = &doc["bufferViews"][a["bufferView"].as_u64().context("a sparse or empty accessor")? as usize];
    ensure!(view["buffer"].as_u64().unwrap_or(0) == 0, "an accessor outside the glb's own buffer");
    let kind = a["componentType"].as_u64().context("an accessor without a component type")?;
    let size = match kind {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        other => bail!("component type {other}"),
    };
    let start = view["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
    let stride = view["byteStride"].as_u64().map_or(size * components, |s| s as usize);
    let count = a["count"].as_u64().unwrap_or(0) as usize;
    ensure!(count == 0 || start + stride * (count - 1) + size * components <= binary.len(), "an accessor past the buffer's end");
    Ok((&binary[start.min(binary.len())..], stride, kind, count))
}

fn floats(doc: &Value, binary: &[u8], index: usize, components: usize) -> Result<Vec<f32>> {
    let (bytes, stride, kind, count) = accessor(doc, binary, index, components)?;
    let normalized = doc["accessors"][index]["normalized"].as_bool().unwrap_or(false);
    let mut out = Vec::with_capacity(count * components);
    for i in 0..count {
        for c in 0..components {
            let at = i * stride;
            out.push(match kind {
                5126 => f32::from_le_bytes(bytes[at + c * 4..at + c * 4 + 4].try_into()?),
                5123 => f32::from(u16::from_le_bytes(bytes[at + c * 2..at + c * 2 + 2].try_into()?)) / if normalized { 65535.0 } else { 1.0 },
                5121 => f32::from(bytes[at + c]) / if normalized { 255.0 } else { 1.0 },
                5122 => (f32::from(i16::from_le_bytes(bytes[at + c * 2..at + c * 2 + 2].try_into()?)) / if normalized { 32767.0 } else { 1.0 }).max(-1.0),
                5120 => (f32::from(bytes[at + c] as i8) / if normalized { 127.0 } else { 1.0 }).max(-1.0),
                other => bail!("floats of component type {other}"),
            });
        }
    }
    Ok(out)
}

fn integers(doc: &Value, binary: &[u8], index: usize) -> Result<Vec<u32>> {
    let (bytes, stride, kind, count) = accessor(doc, binary, index, 1)?;
    (0..count)
        .map(|i| {
            let at = i * stride;
            Ok(match kind {
                5125 => u32::from_le_bytes(bytes[at..at + 4].try_into()?),
                5123 => u32::from(u16::from_le_bytes(bytes[at..at + 2].try_into()?)),
                5121 => u32::from(bytes[at]),
                other => bail!("indices of component type {other}"),
            })
        })
        .collect()
}

/// A material's base colour image (its factor applied), cut to TEXELS a side; a plain colour without one.
fn base_color(doc: &Value, material: Option<u64>, binary: &[u8], images: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let pbr = material.map(|m| &doc["materials"][m as usize]["pbrMetallicRoughness"]);
    let factor: Vec<f32> = pbr.and_then(|p| p["baseColorFactor"].as_array()).map(|f| f.iter().filter_map(Value::as_f64).map(|x| x as f32).collect()).filter(|f: &Vec<f32>| f.len() == 4).unwrap_or_else(|| vec![1.0; 4]);
    let image = pbr.and_then(|p| p["baseColorTexture"]["index"].as_u64()).and_then(|t| doc["textures"][t as usize]["source"].as_u64()).map(|i| &doc["images"][i as usize]);
    let Some(image) = image else {
        let texel = factor.iter().map(|f| (f.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
        return Ok((1, 1, texel));
    };
    let png = match (image["uri"].as_str(), image["bufferView"].as_u64()) {
        (Some(uri), _) => std::fs::read(images.join(uri)).with_context(|| format!("the texture {uri}"))?,
        (None, Some(view)) => {
            let v = &doc["bufferViews"][view as usize];
            let start = v["byteOffset"].as_u64().unwrap_or(0) as usize;
            binary.get(start..start + v["byteLength"].as_u64().unwrap_or(0) as usize).context("an image past the buffer's end")?.to_vec()
        }
        _ => bail!("an image with neither a file nor bytes"),
    };
    let (w, h, rgba) = decode_png(&png)?;
    let (tw, th) = if w.max(h) > TEXELS { ((w * TEXELS / w.max(h)).max(1), (h * TEXELS / w.max(h)).max(1)) } else { (w, h) };
    let mut rgba = if (tw, th) == (w, h) { rgba } else { resample::resize_rgba(&rgba, (w, h), (tw, th), resample::Filter::Lanczos) };
    if factor.iter().any(|f| (f - 1.0).abs() > 1e-6) {
        for p in rgba.chunks_exact_mut(4) {
            for c in 0..4 {
                p[c] = (f32::from(p[c]) * factor[c]).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok((tw as u32, th as u32, rgba))
}

fn decode_png(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info()?;
    let mut buffer = vec![0u8; reader.output_buffer_size().context("a PNG too large")?];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());
    let rgba = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buffer,
        (png::ColorType::Rgba, png::BitDepth::Sixteen) => buffer.chunks_exact(2).map(|s| s[0]).collect(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => buffer.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        (png::ColorType::Rgb, png::BitDepth::Sixteen) => buffer.chunks_exact(6).flat_map(|p| [p[0], p[2], p[4], 255]).collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => buffer.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        other => bail!("a {other:?} PNG"),
    };
    Ok((info.width as usize, info.height as usize, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A glb of one triangle 1 m along glTF's y (the game's z), under a node turned a quarter round glTF's z.
    fn triangle() -> Vec<u8> {
        let positions: [f32; 9] = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0];
        let indices: [u16; 3] = [0, 1, 2];
        let mut bin: Vec<u8> = positions.iter().flat_map(|f| f.to_le_bytes()).collect();
        bin.extend(indices.iter().flat_map(|i| i.to_le_bytes()));
        bin.extend([0, 0]);
        let doc = serde_json::json!({
            "asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0]}],
            "nodes": [{"children": [1], "rotation": [0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2]}, {"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "material": 0}]}],
            "materials": [{"pbrMetallicRoughness": {"baseColorFactor": [0.5, 0.25, 1.0, 1.0]}}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}, {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36}, {"buffer": 0, "byteOffset": 36, "byteLength": 6}],
            "buffers": [{"byteLength": bin.len()}]
        });
        let mut json = serde_json::to_vec(&doc).unwrap();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut glb = b"glTF".to_vec();
        glb.extend(2u32.to_le_bytes());
        glb.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        glb.extend((json.len() as u32).to_le_bytes());
        glb.extend(b"JSON");
        glb.extend(json);
        glb.extend((bin.len() as u32).to_le_bytes());
        glb.extend(b"BIN\0");
        glb.extend(bin);
        glb
    }

    #[test]
    fn a_models_nodes_metres_and_axes_turn_into_hammer_units() {
        let model = read("test", &triangle(), Path::new(".")).unwrap();
        let part = &model.parts[0];
        // glTF's y is the game's z; a quarter turn round glTF's z takes glTF's x to its y: 1 m up in the game
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
        assert!(close(part.positions[1], [0.0, -1.0 / INCH, 0.0]), "{:?}", part.positions[1]);
        assert!(close(part.positions[2], [0.0, 0.0, 1.0 / INCH]), "{:?}", part.positions[2]);
        assert_eq!(part.indices, [0, 1, 2]);
        assert_eq!((part.width, part.height, part.rgba.as_slice()), (1, 1, &[128, 64, 255, 255][..]));
    }

    #[test]
    #[ignore = "needs the game's files and Source 2 Viewer's CLI (nades-config)"]
    fn every_grenade_exports_from_the_game() {
        let game = nades_config::asset(nades_config::Asset::Game, None).unwrap().unwrap().path;
        let cli = nades_config::asset(nades_config::Asset::S2v, None).unwrap().unwrap().path;
        let dir = std::env::temp_dir().join(format!("nades-grenades-{}", std::process::id()));
        let models = build(&cli, &game.join("game/csgo/pak01_dir.vpk"), &dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(models.len(), 6);
        for m in &models {
            let z: Vec<f32> = m.parts.iter().flat_map(|p| p.positions.iter().map(|v| v[2])).collect();
            let length = z.iter().cloned().fold(f32::MIN, f32::max) - z.iter().cloned().fold(f32::MAX, f32::min);
            eprintln!("{}: {} parts, {:.1} units long", m.name, m.parts.len(), length);
            assert!((3.0..16.0).contains(&length), "{}: {length}", m.name);
        }
    }
}
