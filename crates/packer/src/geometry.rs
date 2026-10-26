//! The export's nodes and primitives as the pack's draws, and the Geometry section their spans point into.
//!
//! Each glTF primitive's streams are converted once into the layouts `nades_pack::scene` documents (snorm16 × 4
//! positions with w = 1, f32 × 2 coordinates, snorm8 normals and tangents, unorm8 colours, f32 × 4 vertex paint, the
//! export's own u16 or u32 indices), and every node that draws it points at the same spans. A node's world matrix is
//! its parents' times its own. Mirroring transforms get the primitive's triangles reversed, so every draw's front
//! faces are counter-clockwise.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use anyhow::{Context as _, Result, bail, ensure};
use glam::{Mat4, Vec3};
use nades_pack::scene::{Bounds, Draw, IndexFormat, Instances, LightmapUv, SPAN_ALIGN, Span, UvSet, align_up};

use crate::gltf::{Component, Document, Elements, elements};

/// Where a material's draws go in the order: opaque and alpha-tested, blended, then csgo_effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Opaque,
    Blend,
    Effects,
}

/// The Geometry section as it is built: each span starts on a `SPAN_ALIGN` boundary.
#[derive(Default)]
pub struct Geometry {
    pub bytes: Vec<u8>,
}

impl Geometry {
    pub fn put(&mut self, data: &[u8]) -> Span {
        self.bytes.resize(align_up(self.bytes.len() as u64, SPAN_ALIGN) as usize, 0);
        let span = Span { offset: self.bytes.len() as u64, len: data.len() as u64 };
        self.bytes.extend_from_slice(data);
        span
    }
}

/// Lightmap coordinates lie in the atlas: [0, 1], within the web view's tolerance. A NaN is outside it.
fn outside_atlas(x: f32) -> bool {
    !(-0.001..=1.001).contains(&x)
}

/// The web view's own test, as `mapModel.ts` writes it (a NaN passes there).
#[allow(clippy::manual_range_contains)]
fn web_outside_atlas(x: f32) -> bool {
    x < -0.001 || x > 1.001
}

/// Which attribute holds a primitive's lightmap coordinates, by the web view's rule (show-me-the-nades
/// `mapModel.ts`, `lightmapCoordinates`): TEXCOORD_2 when present, otherwise TEXCOORD_1 when every coordinate lies in
/// the atlas. `texcoord1` is its coordinates, u and v interleaved.
pub fn lightmap_set(has_texcoord2: bool, texcoord1: Option<&[f32]>) -> Option<UvSet> {
    if has_texcoord2 {
        return Some(UvSet::Texcoord2);
    }
    texcoord1.filter(|uv| !uv.iter().copied().any(outside_atlas)).map(|_| UvSet::Texcoord1)
}

/// The web view's own decision, which looks at one vertex in `max(1, floor(count / 256))`: the pack checks every
/// coordinate instead and reports where the two differ.
pub fn web_lightmap_set(has_texcoord2: bool, texcoord1: Option<&[f32]>) -> Option<UvSet> {
    if has_texcoord2 {
        return Some(UvSet::Texcoord2);
    }
    let uv = texcoord1?;
    let count = uv.len() / 2;
    let step = (count / 256).max(1);
    (0..count).step_by(step).all(|i| !web_outside_atlas(uv[2 * i]) && !web_outside_atlas(uv[2 * i + 1])).then_some(UvSet::Texcoord1)
}

/// A primitive's streams in the Geometry section, shared by every node that draws it.
#[derive(Clone)]
struct Streams {
    vertex_count: u32,
    positions: Span,
    uv0: Span,
    uv1: Option<Span>,
    lightmap: Option<LightmapUv>,
    frames: Span,
    colors: Option<Span>,
    paint: Option<Span>,
    indices: Span,
    /// The same triangles wound the other way, made when a mirroring transform first needs them.
    reversed: Option<Span>,
    index_format: IndexFormat,
    index_count: u32,
    /// The positions' box before the node's transform.
    min: Vec3,
    max: Vec3,
    /// The raw indices, kept to reverse them on demand.
    index_data: Vec<u32>,
    /// A skinned primitive's own placement, its vertices posed by its skin and quantized afresh.
    model: Option<Mat4>,
}

/// The draws, the Geometry section and what was counted on the way.
pub struct Draws {
    pub draws: Vec<Draw>,
    pub skipped: BTreeMap<String, u32>,
    pub texcoord2: u32,
    pub texcoord1: u32,
    pub unlit: u32,
    pub web_differs: u32,
    pub mirrored: u32,
    pub missing_uv0: u32,
    /// Primitives without NORMAL or TANGENT, given an up normal or a tangent across it.
    pub missing_frames: u32,
    /// Primitives the game says are not lightmapped though a UV set would pass for lightmap coordinates, and
    /// primitives of a mesh the game said nothing about.
    pub probe_lit_by_game: u32,
    pub lightmapped_unknown: u32,
}

/// What the game says of a mesh (by the export's mesh name): lightmapped, lit by probes, or nothing known.
pub type Lightmapped<'a> = &'a dyn Fn(&str) -> Option<bool>;

fn floats_le(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn indices_le(indices: &[u32], format: IndexFormat) -> Vec<u8> {
    match format {
        IndexFormat::U16 => indices.iter().flat_map(|&i| (i as u16).to_le_bytes()).collect(),
        IndexFormat::U32 => indices.iter().flat_map(|&i| i.to_le_bytes()).collect(),
    }
}

/// A float in [-1, 1] as snorm8, the way the GPU reads it back (×127).
fn snorm8(x: f32) -> u8 {
    (x.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8
}

/// A primitive's normals and tangents as snorm8 × 4 each, interleaved: the export's own bytes where it stores them as
/// normalized bytes (it does), converted otherwise; an up normal and a tangent across it where a stream is missing.
fn frames(normals: Option<&Elements>, tangents: Option<&Elements>, count: usize) -> Vec<u8> {
    let bytes_of = |e: Option<&Elements>, components: usize, missing: [f32; 4]| -> Vec<[u8; 4]> {
        match e {
            Some(e) if e.component == Component::I8 && e.normalized => e.bytes.chunks_exact(e.components).map(|c| std::array::from_fn(|k| if k < components { c[k] } else { 0 })).collect(),
            Some(e) => e.floats().chunks_exact(e.components).map(|c| std::array::from_fn(|k| if k < components { snorm8(c[k]) } else { 0 })).collect(),
            None => vec![missing.map(snorm8); count],
        }
    };
    let n = bytes_of(normals, 3, [0.0, 1.0, 0.0, 0.0]);
    let mut t = bytes_of(tangents, 4, [1.0, 0.0, 0.0, 1.0]);
    // the handedness is ±1 exactly: a stored 0 (no tangent) reads as right-handed
    for tangent in &mut t {
        tangent[3] = if (tangent[3] as i8) < 0 { snorm8(-1.0) } else { snorm8(1.0) };
    }
    n.iter().zip(&t).flat_map(|(n, t)| n.iter().chain(t.iter()).copied()).collect()
}

/// What [`streams`] noticed about a primitive besides its streams.
#[derive(Default)]
struct Noticed {
    web_differs: bool,
    missing_uv0: bool,
    missing_frames: bool,
    probe_lit_by_game: bool,
    lightmapped_unknown: bool,
}

/// Reads one primitive into the Geometry section, or says why it cannot be represented. Its lightmap coordinates are
/// the web view's choice, unless the game says the mesh is not lightmapped.
fn streams(doc: &Document, buffers: &[Cow<'_, [u8]>], mesh: usize, primitive: usize, geometry: &mut Geometry, lightmapped: Lightmapped, skin: Option<&[Mat4]>) -> Result<Result<(Streams, Noticed), String>> {
    let p = &doc.meshes[mesh].primitives[primitive];
    let mode = p.mode.unwrap_or(4);
    if mode != 4 {
        return Ok(Err(format!("primitive mode {mode} (only triangle lists are drawn)")));
    }
    let position = *p.attributes.get("POSITION").context("a primitive without POSITION")?;
    let positions = elements(doc, buffers, position).with_context(|| format!("mesh {mesh} primitive {primitive} POSITION"))?;
    if positions.component != Component::I16 || !positions.normalized || positions.components != 3 {
        return Ok(Err(format!("POSITION as {:?}×{} (normalized {}): only the export's quantized snorm16 positions are read", positions.component, positions.components, positions.normalized)));
    }
    let count = positions.count;
    let mut shorts = positions.shorts();
    let mut model = None;
    let mut normals = attribute_of(doc, buffers, p, "NORMAL", 3, count).with_context(|| format!("mesh {mesh} primitive {primitive}"))?;
    let mut tangents = attribute_of(doc, buffers, p, "TANGENT", 4, count).with_context(|| format!("mesh {mesh} primitive {primitive}"))?;
    if let Some(joints) = skin {
        let posed = pose(doc, buffers, p, joints, &shorts, normals.as_ref(), tangents.as_ref(), count).with_context(|| format!("mesh {mesh} primitive {primitive} skin"))?;
        shorts = posed.shorts;
        model = Some(posed.model);
        normals = Some(posed.normals);
        tangents = Some(posed.tangents);
    }
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for v in shorts.chunks_exact(3) {
        let at = Vec3::new(f32::from(v[0]), f32::from(v[1]), f32::from(v[2])) / 32767.0;
        let at = at.max(Vec3::splat(-1.0));
        min = min.min(at);
        max = max.max(at);
    }
    let attribute = |name: &str, components: usize| -> Result<Option<Elements>> {
        let Some(&index) = p.attributes.get(name) else { return Ok(None) };
        let e = elements(doc, buffers, index).with_context(|| format!("mesh {mesh} primitive {primitive} {name}"))?;
        ensure!(e.components == components && e.count == count, "mesh {mesh} primitive {primitive}: {name} is not {count} of {components} components");
        Ok(Some(e))
    };
    let coordinates = |name: &str| -> Result<Option<Vec<f32>>> { Ok(attribute(name, 2)?.map(|e| e.floats())) };
    let uv0 = coordinates("TEXCOORD_0")?;
    let missing_uv0 = uv0.is_none();
    let has_texcoord2 = p.attributes.contains_key("TEXCOORD_2");
    let texcoord1 = coordinates("TEXCOORD_1")?;
    let uv0 = uv0.unwrap_or_else(|| vec![0.0; count * 2]);
    let rule = lightmap_set(has_texcoord2, texcoord1.as_deref());
    let game = doc.meshes[mesh].name.as_deref().and_then(lightmapped);
    let set = if game == Some(false) { None } else { rule };
    let noticed = Noticed {
        web_differs: rule != web_lightmap_set(has_texcoord2, texcoord1.as_deref()),
        probe_lit_by_game: game == Some(false) && rule.is_some(),
        lightmapped_unknown: game.is_none(),
        ..Noticed::default()
    };
    let missing_frames = normals.is_none() || tangents.is_none();
    let colors = attribute("COLOR_0", 4)?;
    let paint = attribute("_TEXCOORD_4", 4)?;
    let indices = match p.indices {
        Some(index) => {
            let e = elements(doc, buffers, index).with_context(|| format!("mesh {mesh} primitive {primitive} indices"))?;
            let format = if e.component == Component::U32 { IndexFormat::U32 } else { IndexFormat::U16 };
            (e.integers()?, format)
        }
        None => ((0..count as u32).collect(), if count <= 1 << 16 { IndexFormat::U16 } else { IndexFormat::U32 }),
    };
    let (index_data, index_format) = indices;
    if index_data.is_empty() || index_data.len() % 3 != 0 {
        return Ok(Err(format!("{} indices: not whole triangles", index_data.len())));
    }
    if let Some(&bad) = index_data.iter().find(|&&i| i as usize >= count) {
        return Ok(Err(format!("index {bad} past the primitive's {count} vertices")));
    }

    // snorm16 × 4 with w = 32767, so the attribute reads as (x, y, z, 1)
    let positions = geometry.put(&shorts.chunks_exact(3).flat_map(|v| [v[0], v[1], v[2], i16::MAX]).flat_map(i16::to_le_bytes).collect::<Vec<u8>>());
    let uv0 = geometry.put(&floats_le(&uv0));
    let uv1 = texcoord1.as_deref().map(|uv| geometry.put(&floats_le(uv)));
    let lightmap = match set {
        Some(UvSet::Texcoord2) => Some(LightmapUv { uv: geometry.put(&floats_le(&coordinates("TEXCOORD_2")?.expect("present"))), from: UvSet::Texcoord2 }),
        Some(UvSet::Texcoord1) => Some(LightmapUv { uv: uv1.expect("TEXCOORD_1 is present"), from: UvSet::Texcoord1 }),
        None => None,
    };
    let frames = geometry.put(&frames(normals.as_ref(), tangents.as_ref(), count));
    let colors = colors.map(|e| {
        let bytes: Vec<u8> = match (e.component, e.normalized) {
            (Component::U8, true) => e.bytes.clone(),
            _ => e.floats().iter().map(|&c| (c.clamp(0.0, 1.0) * 255.0).round() as u8).collect(),
        };
        geometry.put(&bytes)
    });
    let paint = paint.map(|e| geometry.put(&floats_le(&e.floats())));
    let indices = geometry.put(&indices_le(&index_data, index_format));
    let streams = Streams {
        vertex_count: count as u32,
        positions,
        uv0,
        uv1,
        lightmap,
        frames,
        colors,
        paint,
        indices,
        reversed: None,
        index_format,
        index_count: index_data.len() as u32,
        min,
        max,
        index_data,
        model,
    };
    Ok(Ok((streams, Noticed { missing_uv0, missing_frames, ..noticed })))
}

/// A vertex attribute of `components` components for `count` vertices, or none.
fn attribute_of(doc: &Document, buffers: &[Cow<'_, [u8]>], p: &crate::gltf::Primitive, name: &str, components: usize, count: usize) -> Result<Option<Elements>> {
    let Some(&index) = p.attributes.get(name) else { return Ok(None) };
    let e = elements(doc, buffers, index).with_context(|| name.to_string())?;
    ensure!(e.components == components && e.count == count, "{name} is not {count} of {components} components");
    Ok(Some(e))
}

/// A skinned primitive in its bind pose, quantized afresh.
struct Posed {
    shorts: Vec<i16>,
    model: Mat4,
    normals: Elements,
    tangents: Elements,
}

/// Places a skinned primitive's vertices by its skin (`joints`: each joint's world matrix × its inverse bind matrix),
/// weighted by JOINTS_0 and WEIGHTS_0, as glTF skins them; then quantizes the posed positions into snorm16 again with
/// a model matrix of their own, and turns the normals and tangents with the blended joints.
#[allow(clippy::too_many_arguments)]
fn pose(doc: &Document, buffers: &[Cow<'_, [u8]>], p: &crate::gltf::Primitive, joints: &[Mat4], shorts: &[i16], normals: Option<&Elements>, tangents: Option<&Elements>, count: usize) -> Result<Posed> {
    let indices = attribute_of(doc, buffers, p, "JOINTS_0", 4, count)?.context("a skinned primitive without JOINTS_0")?;
    let weights = attribute_of(doc, buffers, p, "WEIGHTS_0", 4, count)?.context("a skinned primitive without WEIGHTS_0")?.floats();
    let indices = indices.floats();
    let normal_floats = normals.map(Elements::floats);
    let tangent_floats = tangents.map(Elements::floats);
    let mut positions = Vec::with_capacity(count);
    let mut posed_normals = Vec::with_capacity(count * 3);
    let mut posed_tangents = Vec::with_capacity(count * 4);
    for v in 0..count {
        let mut m = Mat4::ZERO;
        for k in 0..4 {
            let w = weights[v * 4 + k];
            if w > 0.0 {
                let j = indices[v * 4 + k] as usize;
                m += *joints.get(j).with_context(|| format!("joint {j} of {}", joints.len()))? * w;
            }
        }
        let at = Vec3::new(f32::from(shorts[v * 3]), f32::from(shorts[v * 3 + 1]), f32::from(shorts[v * 3 + 2])) / 32767.0;
        positions.push(m.transform_point3(at));
        let n = normal_floats.as_ref().map_or(Vec3::Y, |n| Vec3::from_slice(&n[v * 3..v * 3 + 3]));
        posed_normals.extend(m.transform_vector3(n).normalize_or_zero().to_array());
        let t = tangent_floats.as_ref().map_or([1.0, 0.0, 0.0, 1.0], |t| [t[v * 4], t[v * 4 + 1], t[v * 4 + 2], t[v * 4 + 3]]);
        let turned = m.transform_vector3(Vec3::new(t[0], t[1], t[2])).normalize_or_zero();
        posed_tangents.extend([turned.x, turned.y, turned.z, t[3]]);
    }
    let (lo, hi) = positions.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let centre = (lo + hi) * 0.5;
    let half = ((hi - lo) * 0.5).max(Vec3::splat(1e-6));
    let shorts = positions.iter().flat_map(|p| (((*p - centre) / half) * 32767.0).round().clamp(Vec3::splat(-32767.0), Vec3::splat(32767.0)).to_array().map(|c| c as i16)).collect();
    let floats = |values: Vec<f32>, components: usize| Elements { bytes: values.iter().flat_map(|f| f.to_le_bytes()).collect(), component: Component::F32, components, normalized: false, count };
    Ok(Posed { shorts, model: Mat4::from_translation(centre) * Mat4::from_scale(half), normals: floats(posed_normals, 3), tangents: floats(posed_tangents, 4) })
}

/// Each joint's world matrix × its inverse bind matrix, for skin `skin`; `worlds` is every node's world matrix.
fn joint_matrices(doc: &Document, buffers: &[Cow<'_, [u8]>], skin: usize, worlds: &HashMap<usize, Mat4>) -> Result<Vec<Mat4>> {
    let s = doc.skins.get(skin).with_context(|| format!("no skin {skin}"))?;
    let inverse: Vec<Mat4> = match s.inverse_bind_matrices {
        Some(a) => elements(doc, buffers, a)?.floats().chunks_exact(16).map(Mat4::from_cols_slice).collect(),
        None => vec![Mat4::IDENTITY; s.joints.len()],
    };
    ensure!(inverse.len() == s.joints.len(), "skin {skin}: {} inverse bind matrices for {} joints", inverse.len(), s.joints.len());
    Ok(s.joints.iter().zip(inverse).map(|(j, ib)| worlds.get(j).copied().unwrap_or(Mat4::IDENTITY) * ib).collect())
}

/// An instanced node's per-instance transforms (translation × rotation × scale), or none.
fn instance_transforms(doc: &Document, buffers: &[Cow<'_, [u8]>], node: usize) -> Result<Option<Vec<Mat4>>> {
    let Some(instancing) = doc.nodes[node].instancing() else { return Ok(None) };
    let read = |name: &str, components: usize| -> Result<Option<Vec<f32>>> {
        let Some(&index) = instancing.attributes.get(name) else { return Ok(None) };
        let e = elements(doc, buffers, index).with_context(|| format!("node {node} instance {name}"))?;
        ensure!(e.components == components, "node {node}: instance {name} has {} components", e.components);
        Ok(Some(e.floats()))
    };
    let (t, r, s) = (read("TRANSLATION", 3)?, read("ROTATION", 4)?, read("SCALE", 3)?);
    let count = [t.as_ref().map(|v| v.len() / 3), r.as_ref().map(|v| v.len() / 4), s.as_ref().map(|v| v.len() / 3)].into_iter().flatten().max().unwrap_or(0);
    for (name, len) in [("TRANSLATION", t.as_ref().map(|v| v.len() / 3)), ("ROTATION", r.as_ref().map(|v| v.len() / 4)), ("SCALE", s.as_ref().map(|v| v.len() / 3))] {
        ensure!(len.is_none_or(|n| n == count), "node {node}: instance {name} has {len:?} entries of {count}");
    }
    Ok(Some(
        (0..count)
            .map(|i| {
                let translation = t.as_ref().map_or(Vec3::ZERO, |t| Vec3::from_slice(&t[i * 3..i * 3 + 3]));
                let rotation = r.as_ref().map_or(glam::Quat::IDENTITY, |r| glam::Quat::from_slice(&r[i * 4..i * 4 + 4]));
                let scale = s.as_ref().map_or(Vec3::ONE, |s| Vec3::from_slice(&s[i * 3..i * 3 + 3]));
                Mat4::from_scale_rotation_translation(scale, rotation, translation)
            })
            .collect(),
    ))
}

/// The box around `min..max` after each of `transforms`.
fn world_bounds(min: Vec3, max: Vec3, transforms: impl Iterator<Item = Mat4>) -> Bounds {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for m in transforms {
        for corner in 0..8 {
            let c = Vec3::new(if corner & 1 == 0 { min.x } else { max.x }, if corner & 2 == 0 { min.y } else { max.y }, if corner & 4 == 0 { min.z } else { max.z });
            let w = m.transform_point3(c);
            lo = lo.min(w);
            hi = hi.max(w);
        }
    }
    Bounds { min: lo.to_array(), max: hi.to_array() }
}

/// Every node in the default scene with its world matrix, parents before children, in the export's order.
pub fn walk(doc: &Document) -> Result<Vec<(usize, Mat4)>> {
    let scene = doc.scenes.get(doc.scene.unwrap_or(0)).context("the glb has no scene")?;
    let mut out = Vec::new();
    let mut stack: Vec<(usize, Mat4, usize)> = scene.nodes.iter().rev().map(|&n| (n, Mat4::IDENTITY, 0)).collect();
    while let Some((node, parent, depth)) = stack.pop() {
        ensure!(depth < 256, "the node hierarchy is deeper than 256 (a cycle?)");
        let n = doc.nodes.get(node).with_context(|| format!("no node {node}"))?;
        let world = parent * n.local();
        out.push((node, world));
        stack.extend(n.children.iter().rev().map(|&c| (c, world, depth + 1)));
    }
    Ok(out)
}

/// Every drawn primitive of the default scene as draws, their streams added to `geometry`. `materials` maps the glTF
/// material index to the pack's, `groups` gives each pack material's place in the order; primitives without a
/// material use `default_material`.
pub fn build(doc: &Document, buffers: &[Cow<'_, [u8]>], geometry: &mut Geometry, materials: &[u32], groups: &[Group], default_material: u32, lightmapped: Lightmapped) -> Result<Draws> {
    let mut cache: HashMap<(usize, usize, Option<usize>), Option<Streams>> = HashMap::new();
    let mut out = Draws { draws: Vec::new(), skipped: BTreeMap::new(), texcoord2: 0, texcoord1: 0, unlit: 0, web_differs: 0, mirrored: 0, missing_uv0: 0, missing_frames: 0, probe_lit_by_game: 0, lightmapped_unknown: 0 };
    let mut grouped: Vec<(Group, Draw)> = Vec::new();
    let nodes = walk(doc)?;
    let worlds: HashMap<usize, Mat4> = nodes.iter().copied().collect();
    for (node, world) in nodes {
        let n = &doc.nodes[node];
        let Some(mesh) = n.mesh else { continue };
        ensure!(mesh < doc.meshes.len(), "node {node} names mesh {mesh}");
        let instances = instance_transforms(doc, buffers, node)?;
        // a skinned mesh is placed by its joints alone: posed once per node, its own model matrix in place of the node's
        let skin = n.skin.map(|s| joint_matrices(doc, buffers, s, &worlds)).transpose()?;
        for (primitive, p) in doc.meshes[mesh].primitives.iter().enumerate() {
            let slot = match cache.entry((mesh, primitive, skin.as_ref().map(|_| node))) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => v.insert(match streams(doc, buffers, mesh, primitive, geometry, lightmapped, skin.as_deref())? {
                    Ok((s, noticed)) => {
                        out.web_differs += u32::from(noticed.web_differs);
                        out.missing_uv0 += u32::from(noticed.missing_uv0);
                        out.missing_frames += u32::from(noticed.missing_frames);
                        out.probe_lit_by_game += u32::from(noticed.probe_lit_by_game);
                        out.lightmapped_unknown += u32::from(noticed.lightmapped_unknown);
                        Some(s)
                    }
                    Err(reason) => {
                        *out.skipped.entry(reason).or_default() += 1;
                        None
                    }
                }),
            };
            let Some(s) = slot.as_mut() else { continue };
            let world = s.model.unwrap_or(world);
            let material = match p.material {
                Some(m) if m < materials.len() => materials[m],
                Some(m) => bail!("mesh {mesh} primitive {primitive} names material {m} of {}", materials.len()),
                None => default_material,
            };
            // the instances that keep the winding and the ones that mirror it: a draw each
            let parts: Vec<(bool, Option<Vec<Mat4>>)> = match &instances {
                None => vec![(world.determinant() < 0.0, None)],
                Some(all) => {
                    let (mirrored, kept): (Vec<Mat4>, Vec<Mat4>) = all.iter().partition(|m| (world * **m).determinant() < 0.0);
                    [(false, kept), (true, mirrored)].into_iter().filter(|(_, g)| !g.is_empty()).map(|(m, g)| (m, Some(g))).collect()
                }
            };
            for (mirrored, part) in parts {
                let indices = if mirrored {
                    if s.reversed.is_none() {
                        let flipped: Vec<u32> = s.index_data.chunks_exact(3).flat_map(|t| [t[0], t[2], t[1]]).collect();
                        s.reversed = Some(geometry.put(&indices_le(&flipped, s.index_format)));
                    }
                    out.mirrored += 1;
                    s.reversed.expect("made above")
                } else {
                    s.indices
                };
                let bounds = match &part {
                    None => world_bounds(s.min, s.max, std::iter::once(world)),
                    Some(g) => world_bounds(s.min, s.max, g.iter().map(|m| world * *m)),
                };
                let instances = part.map(|g| Instances { count: g.len() as u32, transforms: geometry.put(&floats_le(&g.iter().flat_map(|m| m.to_cols_array()).collect::<Vec<_>>())) });
                match s.lightmap.map(|l| l.from) {
                    Some(UvSet::Texcoord2) => out.texcoord2 += 1,
                    Some(UvSet::Texcoord1) => out.texcoord1 += 1,
                    None => out.unlit += 1,
                }
                let draw = Draw {
                    name: n.name.clone().unwrap_or_else(|| format!("node {node}")),
                    model: world.to_cols_array(),
                    vertex_count: s.vertex_count,
                    positions: s.positions,
                    uv0: s.uv0,
                    uv1: s.uv1,
                    lightmap: s.lightmap,
                    frames: s.frames,
                    colors: s.colors,
                    paint: s.paint,
                    indices,
                    index_format: s.index_format,
                    index_count: s.index_count,
                    instances,
                    material,
                    bounds,
                };
                grouped.push((groups.get(material as usize).copied().unwrap_or(Group::Opaque), draw));
            }
        }
    }
    // a stable sort keeps the export's node order within each group
    grouped.sort_by_key(|(group, _)| *group);
    out.draws = grouped.into_iter().map(|(_, d)| d).collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lightmap_coordinates_follow_the_web_views_rule() {
        let inside = [0.0, 0.5, 1.0, 1.0005, -0.0005, 0.25];
        let tiling = [0.0, 0.5, 3.0, 1.0];
        // TEXCOORD_2 wins whatever TEXCOORD_1 holds
        assert_eq!(lightmap_set(true, Some(&tiling)), Some(UvSet::Texcoord2));
        assert_eq!(lightmap_set(true, None), Some(UvSet::Texcoord2));
        // TEXCOORD_1 when every coordinate is in [0, 1] within 0.001
        assert_eq!(lightmap_set(false, Some(&inside)), Some(UvSet::Texcoord1));
        assert_eq!(lightmap_set(false, Some(&tiling)), None);
        assert_eq!(lightmap_set(false, Some(&[0.0, 1.002])), None);
        assert_eq!(lightmap_set(false, Some(&[-0.002, 0.0])), None);
        // a mesh with one UV set is not lightmapped
        assert_eq!(lightmap_set(false, None), None);
    }

    #[test]
    fn the_web_view_samples_where_the_pack_checks_every_coordinate() {
        // 1000 vertices: the web view looks at every third (floor(1000 / 256) = 3), so vertex 1 tiling escapes it
        let mut uv = vec![0.5f32; 2000];
        uv[2] = 4.0;
        assert_eq!(web_lightmap_set(false, Some(&uv)), Some(UvSet::Texcoord1));
        assert_eq!(lightmap_set(false, Some(&uv)), None);
        // ...but not when it is one the web view looks at
        uv[6] = 4.0;
        assert_eq!(web_lightmap_set(false, Some(&uv)), None);
        // with 256 or fewer vertices it looks at all of them
        let few = [0.5, 0.5, 2.0, 0.5];
        assert_eq!(web_lightmap_set(false, Some(&few)), lightmap_set(false, Some(&few)));
    }

    #[test]
    fn spans_start_on_256_byte_boundaries() {
        let mut g = Geometry::default();
        let a = g.put(&[1; 10]);
        let b = g.put(&[2; 300]);
        let c = g.put(&[3; 4]);
        assert_eq!((a.offset, b.offset, c.offset), (0, 256, 768));
        assert_eq!(&g.bytes[256..260], &[2; 4]);
    }

    #[test]
    fn bounds_cover_every_instance() {
        let one = world_bounds(Vec3::splat(-1.0), Vec3::splat(1.0), [Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)), Mat4::from_scale(Vec3::splat(2.0))].into_iter());
        assert_eq!((one.min, one.max), ([-2.0, -2.0, -2.0], [11.0, 2.0, 2.0]));
    }

    fn elements_of(component: Component, normalized: bool, components: usize, bytes: Vec<u8>) -> Elements {
        let count = bytes.len() / (components * component.size());
        Elements { bytes, component, components, normalized, count }
    }

    #[test]
    fn frames_keep_the_exports_bytes_and_make_the_handedness_whole() {
        // two vertices: the export's snorm8 normals and tangents, one tangent left-handed, one with w = 0
        let normals = elements_of(Component::I8, true, 3, vec![0, 127, 0, 129, 0, 0]);
        let tangents = elements_of(Component::I8, true, 4, vec![127, 0, 0, 129, 0, 0, 127, 0]);
        let f = frames(Some(&normals), Some(&tangents), 2);
        assert_eq!(f, vec![0, 127, 0, 0, 127, 0, 0, 129, 129, 0, 0, 0, 0, 0, 127, 127]);
        // float streams are quantized; missing ones are up and across
        let floats = elements_of(Component::F32, false, 3, [0.0f32, 0.0, -1.0].iter().flat_map(|f| f.to_le_bytes()).collect());
        assert_eq!(frames(Some(&floats), None, 1), vec![0, 0, 129, 0, 127, 0, 0, 127]);
    }
}
