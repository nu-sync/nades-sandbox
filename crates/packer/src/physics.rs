//! The map's collider from its own physics: `maps/<map>/world_physics.vmdl_c` in the map's VPK, exported by Source 2
//! Viewer's CLI as glTF and reduced exactly as awpy-data's `process_geometry.py` reduces it into the published
//! `<map>.mesh` that the web view's line of sight reads (its D26), so the sandbox's cameras keep the web view's
//! answers on any map the game has, not only the ones awpy publishes:
//!
//! 1. the CLI's export (`-d --gltf_export_format glb`): the physics as an unwelded triangle soup, one mesh per
//!    collision attribute and surface property, named `physics_<interacts as>_<surface>` (awpy-data exports with
//!    `--gltf_export_materials`, which gives each primitive a material of the same name and needs the game's shaders;
//!    the names are what count, and the triangles are the same);
//! 2. primitives whose material (else mesh) name names a non-occluder are dropped: [`DROP`], case-insensitive
//!    (player, NPC and grenade clips, the sky, glass, windows, `passbullets`); a primitive with no name is kept;
//! 3. positions only, welded by their exact bits, in the export's mesh and index order;
//! 4. vertex clustering on a [`GRID`]-unit lattice: each cell's first vertex stands for it, triangles that collapse or
//!    repeat (either winding) are dropped.
//!
//! Positions pass through unchanged: the CLI writes the physics in Hammer units, z up, the demos' frame.
//!
//! The grenade's collider ([`grenade_from_glb`], the GrenadeCollider section) is the same export reduced for the
//! flight instead (S40): grenade clips are kept (they stop grenades), the rest of [`DROP`] is left out (players' clips,
//! the sky, glass, windows and `passbullets`, which grenades pass), and the vertices are welded but not clustered, since
//! 8-unit clustering moves edges, steps and slopes by several units, which a bounce multiplies.
//!
//! The Collider section is the triangles as awpy's AWMH file holds them: "AWMH", version 1, the vertex and triangle
//! counts, the vertices (3 × f32) and the triangles (3 × u32), so the app reads both with one reader.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, ensure};

use crate::gltf;

/// A material is a non-occluder when its name holds one of these (awpy-data's `DROP_SUBSTRINGS`).
pub const DROP: [&str; 6] = ["playerclip", "grenadeclip", "sky", "passbullets", "glass", "window"];
/// What a grenade passes through: [`DROP`] without the grenade clips.
pub const GRENADE_DROP: [&str; 5] = ["playerclip", "sky", "passbullets", "glass", "window"];
/// awpy-data's vertex-clustering cell, Hammer units.
pub const GRID: f32 = 8.0;

/// The world's occluding triangles, Hammer units.
#[derive(Debug, Default)]
pub struct Physics {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// The export's triangles, those dropped as non-occluders, and those left once welded (before clustering).
    pub exported: usize,
    pub dropped: usize,
    pub welded: usize,
}

/// The export's file: the CLI writes `world_physics.glb` (the model, empty) and `world_physics_physics.glb` (its
/// physics) beside the materials' textures; the physics is the largest glb, as awpy-data takes it.
fn physics_glb(dir: &Path) -> Result<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).with_context(|| format!("reading {}", d.display()))? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("glb")) {
                let size = entry.metadata()?.len();
                if best.as_ref().is_none_or(|(s, _)| size > *s) {
                    best = Some((size, path));
                }
            }
        }
    }
    best.map(|(_, p)| p).with_context(|| format!("the CLI wrote no glb into {}", dir.display()))
}

/// Exports the map's world physics with `cli` (into a scratch folder under `scratch`, removed afterwards) and reduces
/// it.
pub fn of_map(cli: &Path, vpk: &Path, map: &str, scratch: &Path) -> Result<Physics> {
    from_glb(&glb_of_map(cli, vpk, map, scratch)?)
}

/// The map's two colliders from one export: line of sight ([`from_glb`]) and the grenade's ([`grenade_from_glb`]).
pub fn both_of_map(cli: &Path, vpk: &Path, map: &str, scratch: &Path) -> Result<(Physics, Physics)> {
    let glb = glb_of_map(cli, vpk, map, scratch)?;
    Ok((from_glb(&glb)?, grenade_from_glb(&glb)?))
}

/// The CLI's export of the map's world physics (step 1), as the glb's bytes.
pub fn glb_of_map(cli: &Path, vpk: &Path, map: &str, scratch: &Path) -> Result<Vec<u8>> {
    let entry = format!("maps/{map}/world_physics.vmdl_c");
    let dir = scratch.join(format!(".nades-pack-physics-{map}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let result = (|| {
        let output = Command::new(cli)
            .arg("-i")
            .arg(vpk)
            .args(["-f", &entry, "-o"])
            .arg(&dir)
            .args(["-d", "--gltf_export_format", "glb"])
            // the CLI writes exceptions.txt where it runs
            .current_dir(&dir)
            .output()
            .with_context(|| format!("running {}", cli.display()))?;
        ensure!(output.status.success(), "{} exited {} exporting {entry}: {}", cli.display(), output.status, String::from_utf8_lossy(&output.stderr).trim());
        let glb = physics_glb(&dir).with_context(|| format!("{entry} in {}", vpk.display()))?;
        std::fs::read(&glb).with_context(|| format!("reading {}", glb.display()))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

pub fn is_occluder(material: Option<&str>) -> bool {
    match material {
        None | Some("") => true,
        Some(name) => {
            let name = name.to_ascii_lowercase();
            !DROP.iter().any(|d| name.contains(d))
        }
    }
}

/// Steps 2–4 on the CLI's glb.
pub fn from_glb(bytes: &[u8]) -> Result<Physics> {
    reduce(bytes, &|name| is_occluder(name), Some(GRID))
}

/// A grenade bounces off a primitive unless its material (else mesh) name names something it passes, [`GRENADE_DROP`];
/// a grenade clip always stops it.
pub fn stops_grenades(material: Option<&str>) -> bool {
    match material {
        None | Some("") => true,
        Some(name) => {
            let name = name.to_ascii_lowercase();
            name.contains("grenadeclip") || !GRENADE_DROP.iter().any(|d| name.contains(d))
        }
    }
}

/// The grenade's collider from the CLI's glb: [`stops_grenades`]'s primitives, welded, not clustered.
pub fn grenade_from_glb(bytes: &[u8]) -> Result<Physics> {
    reduce(bytes, &|name| stops_grenades(name), None)
}

/// Steps 2–4 under other rules: `keep` says which primitives stay (by material, else mesh, name), `grid` is the
/// clustering cell (None welds only).
pub fn reduce(bytes: &[u8], keep: &dyn Fn(Option<&str>) -> bool, grid: Option<f32>) -> Result<Physics> {
    let parts = gltf::split(bytes)?;
    let doc = gltf::parse(parts.json)?;
    let buffers = gltf::buffers(&doc, parts.bin)?;
    let mut out = Physics::default();
    let mut welded: HashMap<[u32; 3], u32> = HashMap::new();
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut corners: Vec<u32> = Vec::new();
    for (m, mesh) in doc.meshes.iter().enumerate() {
        for (p, primitive) in mesh.primitives.iter().enumerate() {
            ensure!(primitive.mode.unwrap_or(4) == 4, "mesh {m} primitive {p} is not triangles");
            let position = *primitive.attributes.get("POSITION").with_context(|| format!("mesh {m} primitive {p} has no POSITION"))?;
            let pos = gltf::elements(&doc, &buffers, position)?;
            ensure!(pos.components == 3 && pos.component == gltf::Component::F32, "mesh {m} primitive {p}'s positions are not f32 triples");
            let indices = match primitive.indices {
                Some(i) => gltf::elements(&doc, &buffers, i)?.integers()?,
                None => (0..pos.count as u32).collect(),
            };
            out.exported += indices.len() / 3;
            let name = primitive.material.and_then(|i| doc.materials.get(i)).and_then(|mat| mat.name.as_deref()).or(mesh.name.as_deref());
            if !keep(name) {
                out.dropped += indices.len() / 3;
                continue;
            }
            let floats = pos.floats();
            for &j in &indices {
                let j = j as usize;
                ensure!(j < pos.count, "mesh {m} primitive {p} names vertex {j} of {}", pos.count);
                let v = [floats[3 * j], floats[3 * j + 1], floats[3 * j + 2]];
                let next = positions.len() as u32;
                let k = *welded.entry(v.map(f32::to_bits)).or_insert_with(|| {
                    positions.push(v);
                    next
                });
                corners.push(k);
            }
        }
    }
    out.welded = corners.len() / 3;
    match grid {
        Some(grid) => decimate(&mut out, &positions, &corners, grid),
        None => {
            out.vertices = positions;
            out.triangles = corners.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]).collect();
        }
    }
    Ok(out)
}

/// Vertex clustering as awpy-data's `decimate`: a cell's first vertex stands for it; collapsed and repeated
/// triangles go.
fn decimate(out: &mut Physics, positions: &[[f32; 3]], corners: &[u32], grid: f32) {
    let inverse = 1.0 / f64::from(grid);
    let mut cells: HashMap<[i64; 3], u32> = HashMap::new();
    let remap: Vec<u32> = positions
        .iter()
        .map(|v| {
            let cell = v.map(|c| (f64::from(c) * inverse).floor() as i64);
            let next = out.vertices.len() as u32;
            *cells.entry(cell).or_insert_with(|| {
                out.vertices.push(*v);
                next
            })
        })
        .collect();
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    for t in corners.chunks_exact(3) {
        let (a, b, c) = (remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]);
        if a == b || b == c || a == c {
            continue;
        }
        let mut key = [a, b, c];
        key.sort_unstable();
        if seen.insert(key) {
            out.triangles.push([a, b, c]);
        }
    }
}

impl Physics {
    /// The Collider section: awpy's AWMH layout.
    pub fn to_awmh(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16 + self.vertices.len() * 12 + self.triangles.len() * 12);
        bytes.extend_from_slice(b"AWMH");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(self.vertices.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.triangles.len() as u32).to_le_bytes());
        for v in &self.vertices {
            for c in v {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
        }
        for t in &self.triangles {
            for i in t {
                bytes.extend_from_slice(&i.to_le_bytes());
            }
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A glb of two primitives: a wall (two triangles sharing an edge, one of them sliver-thin so clustering folds it)
    /// and a player clip.
    fn glb() -> Vec<u8> {
        let wall: [[f32; 3]; 6] = [[0.0, 0.0, 0.0], [64.0, 0.0, 0.0], [0.0, 0.0, 64.0], [64.0, 0.0, 0.0], [64.0, 0.0, 64.0], [0.0, 0.0, 64.0]];
        let sliver: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 64.0]];
        let clip: [[f32; 3]; 3] = [[0.0, 10.0, 0.0], [64.0, 10.0, 0.0], [0.0, 10.0, 64.0]];
        let mut bin = Vec::new();
        for v in wall.iter().chain(&sliver).chain(&clip) {
            for c in v {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let json = serde_json::json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 108}, {"buffer": 0, "byteOffset": 108, "byteLength": 36}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 9, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}
            ],
            "materials": [{"name": "physics_group_concrete_material"}, {"name": "physics_npcclip_playerclip_material"}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "material": 0}, {"attributes": {"POSITION": 1}, "material": 1}]}]
        })
        .to_string();
        let mut json = json.into_bytes();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    #[test]
    fn the_occluders_are_welded_and_clustered_as_awpy_does() {
        let p = from_glb(&glb()).unwrap();
        assert_eq!((p.exported, p.dropped, p.welded), (4, 1, 3));
        // the sliver's (1, 0, 0) shares (0, 0, 0)'s cell: it collapses and goes
        assert_eq!(p.vertices, vec![[0.0, 0.0, 0.0], [64.0, 0.0, 0.0], [0.0, 0.0, 64.0], [64.0, 0.0, 64.0]]);
        assert_eq!(p.triangles, vec![[0, 1, 2], [1, 3, 2]]);
        let awmh = p.to_awmh();
        assert_eq!(&awmh[0..4], b"AWMH");
        assert_eq!(awmh.len(), 16 + 4 * 12 + 2 * 12);
    }

    #[test]
    fn non_occluders_are_named_by_their_material() {
        assert!(!is_occluder(Some("physics_csgo_grenadeclip_metal_material")));
        assert!(!is_occluder(Some("physics_group_glass_material")));
        assert!(!is_occluder(Some("physics_Sky_material")));
        assert!(is_occluder(Some("physics_group_metal_material")));
        assert!(is_occluder(None));
    }
}
