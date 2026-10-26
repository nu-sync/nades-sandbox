//! Just enough glTF 2.0 to read a Source 2 Viewer export after glTF-Transform: the glb container, the JSON parts the
//! pack uses (unknown keys, like the nodes' entity key-values, are skipped), EXT_meshopt_compression buffers decoded
//! into their fallback buffers, and accessors (KHR_mesh_quantization's integer types, sparse accessors) read into
//! plain arrays.

use std::borrow::Cow;
use std::collections::BTreeMap;

use anyhow::{Context as _, Result, bail, ensure};
use rayon::prelude::*;
use serde::Deserialize;

/// The two chunks of a `.glb`: the JSON and the binary buffer.
pub struct Glb<'a> {
    pub json: &'a [u8],
    pub bin: Option<&'a [u8]>,
}

/// Splits a `.glb` into its JSON and BIN chunks.
pub fn split(data: &[u8]) -> Result<Glb<'_>> {
    let u32_at = |at: usize| data.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")));
    ensure!(data.get(0..4) == Some(b"glTF"), "not a glb (no glTF magic)");
    ensure!(u32_at(4) == Some(2), "glb version {:?}, not 2", u32_at(4));
    let mut at = 12;
    let (mut json, mut bin) = (None, None);
    while let (Some(len), Some(kind)) = (u32_at(at), u32_at(at + 4)) {
        let chunk = data.get(at + 8..at + 8 + len as usize).context("a glb chunk runs past the file")?;
        match kind {
            0x4E4F_534A => json = Some(chunk),
            0x004E_4942 => bin = Some(chunk),
            _ => {}
        }
        at += 8 + len as usize;
    }
    Ok(Glb { json: json.context("the glb has no JSON chunk")?, bin })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    #[serde(default)]
    pub accessors: Vec<Accessor>,
    #[serde(default)]
    pub buffer_views: Vec<BufferView>,
    #[serde(default)]
    pub buffers: Vec<Buffer>,
    #[serde(default)]
    pub images: Vec<Image>,
    #[serde(default)]
    pub textures: Vec<Texture>,
    #[serde(default)]
    pub materials: Vec<Material>,
    #[serde(default)]
    pub meshes: Vec<Mesh>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub scenes: Vec<Scene>,
    pub scene: Option<usize>,
    #[serde(default)]
    pub skins: Vec<Skin>,
    pub extensions: Option<DocumentExtensions>,
}

/// A skin: its joints (nodes) and their inverse bind matrices (an accessor of MAT4s).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skin {
    pub joints: Vec<usize>,
    pub inverse_bind_matrices: Option<usize>,
}

#[derive(Deserialize)]
pub struct DocumentExtensions {
    #[serde(rename = "KHR_lights_punctual")]
    pub lights: Option<Lights>,
}

#[derive(Deserialize)]
pub struct Lights {
    #[serde(default)]
    pub lights: Vec<Light>,
}

/// A KHR_lights_punctual light: Source 2 Viewer writes a map's light_environment as a directional light whose
/// intensity is its brightness × 683.
#[derive(Deserialize)]
pub struct Light {
    #[serde(rename = "type")]
    pub kind: String,
    pub color: Option<[f32; 3]>,
    pub intensity: Option<f32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Accessor {
    pub buffer_view: Option<usize>,
    #[serde(default)]
    pub byte_offset: usize,
    pub component_type: u32,
    #[serde(default)]
    pub normalized: bool,
    pub count: usize,
    #[serde(rename = "type")]
    pub kind: String,
    pub sparse: Option<Sparse>,
}

#[derive(Deserialize)]
pub struct Sparse {
    pub count: usize,
    pub indices: SparseIndices,
    pub values: SparseValues,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SparseIndices {
    pub buffer_view: usize,
    #[serde(default)]
    pub byte_offset: usize,
    pub component_type: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SparseValues {
    pub buffer_view: usize,
    #[serde(default)]
    pub byte_offset: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferView {
    pub buffer: usize,
    #[serde(default)]
    pub byte_offset: usize,
    pub byte_length: usize,
    pub byte_stride: Option<usize>,
    pub extensions: Option<BufferViewExtensions>,
}

#[derive(Deserialize)]
pub struct BufferViewExtensions {
    #[serde(rename = "EXT_meshopt_compression")]
    pub meshopt: Option<Meshopt>,
}

/// EXT_meshopt_compression on a buffer view: its compressed bytes, and how to decode them into the view.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meshopt {
    pub buffer: usize,
    #[serde(default)]
    pub byte_offset: usize,
    pub byte_length: usize,
    pub byte_stride: usize,
    pub count: usize,
    pub mode: String,
    pub filter: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Buffer {
    pub byte_length: usize,
    pub uri: Option<String>,
    pub extensions: Option<BufferExtensions>,
}

#[derive(Deserialize)]
pub struct BufferExtensions {
    #[serde(rename = "EXT_meshopt_compression")]
    pub meshopt: Option<MeshoptBuffer>,
}

#[derive(Deserialize)]
pub struct MeshoptBuffer {
    #[serde(default)]
    pub fallback: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub buffer_view: Option<usize>,
    pub mime_type: Option<String>,
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct Texture {
    pub source: Option<usize>,
    pub extensions: Option<TextureExtensions>,
}

#[derive(Deserialize)]
pub struct TextureExtensions {
    #[serde(rename = "EXT_texture_webp")]
    pub webp: Option<TextureSource>,
}

#[derive(Deserialize)]
pub struct TextureSource {
    pub source: usize,
}

impl Texture {
    /// The image it samples: EXT_texture_webp's source when it has one.
    pub fn image(&self) -> Option<usize> {
        self.extensions.as_ref().and_then(|e| e.webp.as_ref()).map(|w| w.source).or(self.source)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureInfo {
    pub index: usize,
    #[serde(default)]
    pub tex_coord: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pbr {
    pub base_color_factor: Option<[f32; 4]>,
    pub base_color_texture: Option<TextureInfo>,
    pub metallic_roughness_texture: Option<TextureInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Material {
    pub name: Option<String>,
    pub pbr_metallic_roughness: Option<Pbr>,
    pub normal_texture: Option<TextureInfo>,
    pub occlusion_texture: Option<TextureInfo>,
    pub alpha_mode: Option<String>,
    pub alpha_cutoff: Option<f32>,
    pub double_sided: Option<bool>,
    pub extras: Option<MaterialExtras>,
}

#[derive(Deserialize)]
pub struct MaterialExtras {
    pub vmat: Option<Vmat>,
    /// export_map.py's layer step: each layer colour's linear average, the pivot of its contrast.
    pub layers: Option<LayerAverages>,
}

#[derive(Deserialize)]
pub struct LayerAverages {
    #[serde(default)]
    pub average: Vec<Option<[f32; 3]>>,
}

/// The CS2 material Source 2 Viewer exported the glTF material from: its shader and parameters as the game's
/// compiled material holds them.
#[derive(Deserialize, Default)]
pub struct Vmat {
    #[serde(rename = "ShaderName")]
    pub shader: Option<String>,
    #[serde(rename = "IntParams", default)]
    pub ints: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "FloatParams", default)]
    pub floats: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "VectorParams", default)]
    pub vectors: BTreeMap<String, serde_json::Value>,
    /// Texture parameters to game texture paths, e.g. `g_tColor1` → `materials/…/x_color_psd_1234.vtex`.
    #[serde(rename = "TextureParams")]
    pub textures: Option<BTreeMap<String, serde_json::Value>>,
}

impl Vmat {
    pub fn int(&self, key: &str) -> Option<i64> {
        self.ints.get(key).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
    }

    pub fn float(&self, key: &str) -> Option<f32> {
        self.floats.get(key).and_then(serde_json::Value::as_f64).map(|f| f as f32)
    }

    /// A vector parameter's first `N` components (missing ones are `fill`).
    pub fn vector<const N: usize>(&self, key: &str, fill: f32) -> Option<[f32; N]> {
        let list = self.vectors.get(key)?.as_array()?;
        Some(std::array::from_fn(|k| list.get(k).and_then(serde_json::Value::as_f64).map_or(fill, |f| f as f32)))
    }

    pub fn texture(&self, param: &str) -> Option<&str> {
        self.textures.as_ref()?.get(param)?.as_str()
    }
}

impl Material {
    pub fn vmat(&self) -> Option<&Vmat> {
        self.extras.as_ref().and_then(|e| e.vmat.as_ref())
    }

    /// The game texture path a `TextureParams` entry names.
    pub fn texture_param(&self, param: &str) -> Option<&str> {
        self.vmat()?.texture(param)
    }

    /// Layer `n`'s (1-based) colour average, when the export recorded one.
    pub fn layer_average(&self, n: usize) -> Option<[f32; 3]> {
        self.extras.as_ref()?.layers.as_ref()?.average.get(n - 1).copied().flatten()
    }
}

#[derive(Deserialize)]
pub struct Mesh {
    pub name: Option<String>,
    pub primitives: Vec<Primitive>,
}

#[derive(Deserialize)]
pub struct Primitive {
    pub attributes: BTreeMap<String, usize>,
    pub indices: Option<usize>,
    pub material: Option<usize>,
    pub mode: Option<u32>,
}

#[derive(Deserialize)]
pub struct Node {
    pub name: Option<String>,
    pub mesh: Option<usize>,
    #[serde(default)]
    pub children: Vec<usize>,
    pub translation: Option<[f32; 3]>,
    pub rotation: Option<[f32; 4]>,
    pub scale: Option<[f32; 3]>,
    pub matrix: Option<[f32; 16]>,
    /// A skinned mesh's skin: its vertices are placed by the skin's joints, and the node's own transform is ignored.
    pub skin: Option<usize>,
    pub extensions: Option<NodeExtensions>,
}

#[derive(Deserialize)]
pub struct NodeExtensions {
    #[serde(rename = "EXT_mesh_gpu_instancing")]
    pub instancing: Option<Instancing>,
    #[serde(rename = "KHR_lights_punctual")]
    pub light: Option<NodeLight>,
}

#[derive(Deserialize)]
pub struct NodeLight {
    pub light: usize,
}

#[derive(Deserialize)]
pub struct Instancing {
    pub attributes: BTreeMap<String, usize>,
}

impl Node {
    /// The node's transform relative to its parent (column-major).
    pub fn local(&self) -> glam::Mat4 {
        match self.matrix {
            Some(m) => glam::Mat4::from_cols_array(&m),
            None => glam::Mat4::from_scale_rotation_translation(
                glam::Vec3::from_array(self.scale.unwrap_or([1.0; 3])),
                glam::Quat::from_array(self.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0])),
                glam::Vec3::from_array(self.translation.unwrap_or([0.0; 3])),
            ),
        }
    }

    pub fn instancing(&self) -> Option<&Instancing> {
        self.extensions.as_ref().and_then(|e| e.instancing.as_ref())
    }

    /// The KHR_lights_punctual light it carries.
    pub fn light(&self) -> Option<usize> {
        self.extensions.as_ref()?.light.as_ref().map(|l| l.light)
    }
}

#[derive(Deserialize)]
pub struct Scene {
    #[serde(default)]
    pub nodes: Vec<usize>,
}

pub fn parse(json: &[u8]) -> Result<Document> {
    serde_json::from_slice(json).context("the glb's JSON")
}

/// Every buffer's bytes: the glb's BIN chunk for the buffer without a URI, and for each EXT_meshopt_compression
/// fallback buffer, its views decoded (in parallel) from their compressed bytes.
pub fn buffers<'a>(doc: &Document, bin: Option<&'a [u8]>) -> Result<Vec<Cow<'a, [u8]>>> {
    let mut out: Vec<Cow<'a, [u8]>> = Vec::with_capacity(doc.buffers.len());
    for (i, b) in doc.buffers.iter().enumerate() {
        let fallback = b.extensions.as_ref().and_then(|e| e.meshopt.as_ref()).is_some_and(|m| m.fallback);
        if fallback {
            out.push(Cow::Owned(vec![0; b.byte_length]));
        } else if b.uri.is_none() && i == 0 {
            let bin = bin.context("buffer 0 has no URI and the glb no BIN chunk")?;
            ensure!(bin.len() >= b.byte_length, "the BIN chunk is shorter than buffer 0");
            out.push(Cow::Borrowed(&bin[..b.byte_length]));
        } else {
            bail!("buffer {i} lives outside the glb ({:?}): not read", b.uri);
        }
    }
    // decode each compressed view into its own range of its fallback buffer: the ranges are disjoint, so the buffer is
    // split into one mutable slice per view and every view decodes on its own thread
    let mut by_target: BTreeMap<usize, Vec<(usize, &BufferView, &Meshopt)>> = BTreeMap::new();
    for (i, view) in doc.buffer_views.iter().enumerate() {
        if let Some(m) = view.extensions.as_ref().and_then(|e| e.meshopt.as_ref()) {
            by_target.entry(view.buffer).or_default().push((i, view, m));
        }
    }
    for (target, mut views) in by_target {
        ensure!(matches!(out.get(target), Some(Cow::Owned(_))), "compressed views decode into buffer {target}, which is not a meshopt fallback buffer");
        views.sort_by_key(|(_, view, _)| view.byte_offset);
        // the compressed bytes are in the glb's own buffer, borrowed for 'a, so they outlive the borrow of `out`
        let mut sources: Vec<&'a [u8]> = Vec::with_capacity(views.len());
        for &(i, view, m) in &views {
            let Some(Cow::Borrowed(buffer)) = out.get(m.buffer) else { bail!("view {i}'s compressed bytes are not in the glb's own buffer") };
            let source = buffer.get(m.byte_offset..m.byte_offset + m.byte_length).with_context(|| format!("view {i}'s compressed bytes lie outside buffer {}", m.buffer))?;
            ensure!(view.byte_length == m.count * m.byte_stride, "view {i}: {} bytes, but {} × {}", view.byte_length, m.count, m.byte_stride);
            sources.push(source);
        }
        let Cow::Owned(target_bytes) = &mut out[target] else { unreachable!("checked above") };
        let mut rest: &mut [u8] = target_bytes;
        let mut consumed = 0;
        let mut jobs = Vec::with_capacity(views.len());
        for ((i, view, m), source) in views.into_iter().zip(sources) {
            ensure!(view.byte_offset >= consumed, "compressed views {i} and the one before overlap");
            let (_, tail) = std::mem::take(&mut rest).split_at_mut(view.byte_offset - consumed);
            ensure!(tail.len() >= view.byte_length, "view {i} runs past buffer {target}");
            let (slot, tail) = tail.split_at_mut(view.byte_length);
            rest = tail;
            consumed = view.byte_offset + view.byte_length;
            jobs.push((i, slot, source, m));
        }
        jobs.into_par_iter().try_for_each(|(i, slot, source, m)| decode_meshopt(slot, source, m).with_context(|| format!("decoding view {i}")))?;
    }
    Ok(out)
}

/// One EXT_meshopt_compression view: `out` is exactly `count × byte_stride` bytes.
pub fn decode_meshopt(out: &mut [u8], source: &[u8], m: &Meshopt) -> Result<()> {
    assert_eq!(out.len(), m.count * m.byte_stride);
    let ptr = out.as_mut_ptr().cast();
    // SAFETY: meshopt writes exactly count × stride bytes into `out`, which is that long, and reads only `source`;
    // the filters rewrite those same bytes in place
    let rc = unsafe {
        match m.mode.as_str() {
            "ATTRIBUTES" => meshopt::ffi::meshopt_decodeVertexBuffer(ptr, m.count, m.byte_stride, source.as_ptr(), source.len()),
            "TRIANGLES" => meshopt::ffi::meshopt_decodeIndexBuffer(ptr, m.count, m.byte_stride, source.as_ptr(), source.len()),
            "INDICES" => meshopt::ffi::meshopt_decodeIndexSequence(ptr, m.count, m.byte_stride, source.as_ptr(), source.len()),
            other => bail!("meshopt mode {other}"),
        }
    };
    ensure!(rc == 0, "meshopt returned {rc} ({} × {} bytes, mode {})", m.count, m.byte_stride, m.mode);
    // SAFETY: as above, in place over the decoded bytes
    unsafe {
        match m.filter.as_deref() {
            None | Some("NONE") => {}
            Some("OCTAHEDRAL") => meshopt::ffi::meshopt_decodeFilterOct(ptr, m.count, m.byte_stride),
            Some("QUATERNION") => meshopt::ffi::meshopt_decodeFilterQuat(ptr, m.count, m.byte_stride),
            Some("EXPONENTIAL") => meshopt::ffi::meshopt_decodeFilterExp(ptr, m.count, m.byte_stride),
            Some("COLOR") => meshopt::ffi::meshopt_decodeFilterColor(ptr, m.count, m.byte_stride),
            Some(other) => bail!("meshopt filter {other}"),
        }
    }
    Ok(())
}

/// glTF component types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    I8,
    U8,
    I16,
    U16,
    U32,
    F32,
}

impl Component {
    pub fn from_code(code: u32) -> Result<Self> {
        Ok(match code {
            5120 => Self::I8,
            5121 => Self::U8,
            5122 => Self::I16,
            5123 => Self::U16,
            5125 => Self::U32,
            5126 => Self::F32,
            other => bail!("component type {other}"),
        })
    }

    pub fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::U32 | Self::F32 => 4,
        }
    }

    /// Component `k` of `bytes` as glTF reads it: normalized integers map to [-1, 1] or [0, 1].
    fn read(self, bytes: &[u8], k: usize, normalized: bool) -> f32 {
        let at = k * self.size();
        let b = &bytes[at..at + self.size()];
        match (self, normalized) {
            (Self::F32, _) => f32::from_le_bytes(b.try_into().expect("4 bytes")),
            (Self::I8, true) => (f32::from(b[0] as i8) / 127.0).max(-1.0),
            (Self::U8, true) => f32::from(b[0]) / 255.0,
            (Self::I16, true) => (f32::from(i16::from_le_bytes([b[0], b[1]])) / 32767.0).max(-1.0),
            (Self::U16, true) => f32::from(u16::from_le_bytes([b[0], b[1]])) / 65535.0,
            (Self::I8, false) => f32::from(b[0] as i8),
            (Self::U8, false) => f32::from(b[0]),
            (Self::I16, false) => f32::from(i16::from_le_bytes([b[0], b[1]])),
            (Self::U16, false) => f32::from(u16::from_le_bytes([b[0], b[1]])),
            (Self::U32, _) => u32::from_le_bytes(b.try_into().expect("4 bytes")) as f32,
        }
    }
}

/// An accessor's elements, tightly packed: `count` elements of `components` components each.
pub struct Elements {
    pub bytes: Vec<u8>,
    pub component: Component,
    pub components: usize,
    pub normalized: bool,
    pub count: usize,
}

impl Elements {
    /// Every component as f32, normalized ones mapped as glTF says.
    pub fn floats(&self) -> Vec<f32> {
        (0..self.count * self.components).map(|k| self.component.read(&self.bytes, k, self.normalized)).collect()
    }

    /// Every component as u32 (index data).
    pub fn integers(&self) -> Result<Vec<u32>> {
        ensure!(self.components == 1 && !self.normalized, "index data must be plain scalars");
        let b = &self.bytes;
        Ok(match self.component {
            Component::U8 => b.iter().map(|&x| u32::from(x)).collect(),
            Component::U16 => b.chunks_exact(2).map(|c| u32::from(u16::from_le_bytes([c[0], c[1]]))).collect(),
            Component::U32 => b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().expect("4 bytes"))).collect(),
            other => bail!("index data of {other:?}"),
        })
    }

    /// The i16 components, for normalized SHORT data (quantized positions).
    pub fn shorts(&self) -> Vec<i16> {
        assert_eq!(self.component, Component::I16);
        self.bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
    }
}

fn components(kind: &str) -> Result<usize> {
    Ok(match kind {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        "MAT2" => 4,
        "MAT3" => 9,
        "MAT4" => 16,
        other => bail!("accessor type {other}"),
    })
}

/// A buffer view's bytes.
fn view<'d, 'b>(doc: &'d Document, buffers: &'b [Cow<'_, [u8]>], index: usize) -> Result<(&'b [u8], &'d BufferView)> {
    let v = doc.buffer_views.get(index).with_context(|| format!("no buffer view {index}"))?;
    let buffer = buffers.get(v.buffer).with_context(|| format!("view {index} names buffer {}", v.buffer))?;
    let bytes = buffer.get(v.byte_offset..v.byte_offset + v.byte_length).with_context(|| format!("view {index} lies outside buffer {}", v.buffer))?;
    Ok((bytes, v))
}

/// Reads accessor `index` into tightly packed elements: its view's strided bytes (or zeros, without a view), then
/// its sparse values in place.
pub fn elements(doc: &Document, buffers: &[Cow<'_, [u8]>], index: usize) -> Result<Elements> {
    let a = doc.accessors.get(index).with_context(|| format!("no accessor {index}"))?;
    let component = Component::from_code(a.component_type)?;
    let components = components(&a.kind)?;
    let size = component.size() * components;
    let mut bytes = vec![0u8; a.count * size];
    if let Some(v) = a.buffer_view {
        let (data, view_) = view(doc, buffers, v)?;
        let stride = view_.byte_stride.unwrap_or(size);
        ensure!(stride >= size, "accessor {index}: stride {stride} under its element size {size}");
        for (i, element) in bytes.chunks_exact_mut(size).enumerate() {
            let at = a.byte_offset + i * stride;
            element.copy_from_slice(data.get(at..at + size).with_context(|| format!("accessor {index} runs past its view"))?);
        }
    }
    if let Some(sparse) = &a.sparse {
        let (index_data, _) = view(doc, buffers, sparse.indices.buffer_view)?;
        let index_component = Component::from_code(sparse.indices.component_type)?;
        let (value_data, _) = view(doc, buffers, sparse.values.buffer_view)?;
        for k in 0..sparse.count {
            let at = sparse.indices.byte_offset + k * index_component.size();
            let raw = index_data.get(at..at + index_component.size()).with_context(|| format!("accessor {index}: sparse index {k} lies outside its view"))?;
            let target = match index_component {
                Component::U8 => usize::from(raw[0]),
                Component::U16 => usize::from(u16::from_le_bytes([raw[0], raw[1]])),
                Component::U32 => u32::from_le_bytes(raw.try_into().expect("4 bytes")) as usize,
                other => bail!("accessor {index}: sparse indices of {other:?}"),
            };
            ensure!(target < a.count, "accessor {index}: sparse index {target} of {}", a.count);
            let from = sparse.values.byte_offset + k * size;
            let value = value_data.get(from..from + size).with_context(|| format!("accessor {index}: sparse value {k} lies outside its view"))?;
            bytes[target * size..(target + 1) * size].copy_from_slice(value);
        }
    }
    Ok(Elements { bytes, component, components, normalized: a.normalized, count: a.count })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(json: &str) -> Document {
        parse(json.as_bytes()).unwrap()
    }

    #[test]
    fn reads_strided_normalized_and_sparse_accessors() {
        // view 0: three VEC2 u16 normalized coordinates with stride 8; views 1 and 2: a sparse override of element 1
        let mut bin = Vec::new();
        for (u, v) in [(0u16, 65535u16), (32768, 0), (65535, 65535)] {
            bin.extend_from_slice(&u.to_le_bytes());
            bin.extend_from_slice(&v.to_le_bytes());
            bin.extend_from_slice(&[0xAA; 4]);
        }
        bin.extend_from_slice(&1u16.to_le_bytes());
        bin.extend_from_slice(&[0, 0]);
        for c in [0.25f32, 0.75] {
            bin.extend_from_slice(&c.to_le_bytes());
        }
        let d = doc(&format!(
            r#"{{"buffers":[{{"byteLength":{}}}],
                "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":24,"byteStride":8}},
                               {{"buffer":0,"byteOffset":24,"byteLength":4}},
                               {{"buffer":0,"byteOffset":28,"byteLength":8}}],
                "accessors":[{{"bufferView":0,"componentType":5123,"normalized":true,"count":3,"type":"VEC2"}},
                             {{"componentType":5126,"count":3,"type":"VEC2",
                               "sparse":{{"count":1,"indices":{{"bufferView":1,"componentType":5123}},"values":{{"bufferView":2}}}}}}]}}"#,
            bin.len()
        ));
        let buffers = buffers(&d, Some(&bin)).unwrap();
        let uv = elements(&d, &buffers, 0).unwrap().floats();
        assert_eq!(uv, vec![0.0, 1.0, 32768.0 / 65535.0, 0.0, 1.0, 1.0]);
        // no view: zeros, then the sparse value at element 1
        assert_eq!(elements(&d, &buffers, 1).unwrap().floats(), vec![0.0, 0.0, 0.25, 0.75, 0.0, 0.0]);
    }

    #[test]
    fn decodes_meshopt_views_into_their_fallback_buffer() {
        // two vertex streams and an index buffer, compressed as glTF-Transform writes them
        let positions: Vec<[i16; 4]> = (0..40).map(|i| [i * 100, -i * 50, 7, 0]).collect();
        let uv: Vec<[u16; 2]> = (0..40).map(|i| [i as u16 * 1000, 65535 - i as u16]).collect();
        let indices: Vec<u32> = (0..38).flat_map(|i| [i, i + 1, i + 2]).collect();
        let p = meshopt::encode_vertex_buffer(&positions).unwrap();
        let u = meshopt::encode_vertex_buffer(&uv).unwrap();
        let ix = meshopt::encode_index_buffer(&indices, 40).unwrap();
        let mut bin = Vec::new();
        let mut put = |bytes: &[u8]| {
            let at = bin.len();
            bin.extend_from_slice(bytes);
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            (at, bytes.len())
        };
        let (pa, pl) = put(&p);
        let (ua, ul) = put(&u);
        let (ia, il) = put(&ix);
        let json = format!(
            r#"{{"buffers":[{{"byteLength":{}}},{{"byteLength":{},"extensions":{{"EXT_meshopt_compression":{{"fallback":true}}}}}}],
                "bufferViews":[
                  {{"buffer":1,"byteOffset":0,"byteLength":320,"byteStride":8,"extensions":{{"EXT_meshopt_compression":{{"buffer":0,"byteOffset":{pa},"byteLength":{pl},"byteStride":8,"count":40,"mode":"ATTRIBUTES"}}}}}},
                  {{"buffer":1,"byteOffset":320,"byteLength":160,"byteStride":4,"extensions":{{"EXT_meshopt_compression":{{"buffer":0,"byteOffset":{ua},"byteLength":{ul},"byteStride":4,"count":40,"mode":"ATTRIBUTES"}}}}}},
                  {{"buffer":1,"byteOffset":480,"byteLength":456,"extensions":{{"EXT_meshopt_compression":{{"buffer":0,"byteOffset":{ia},"byteLength":{il},"byteStride":4,"count":114,"mode":"TRIANGLES"}}}}}}],
                "accessors":[
                  {{"bufferView":0,"componentType":5122,"normalized":true,"count":40,"type":"VEC3"}},
                  {{"bufferView":1,"componentType":5123,"normalized":true,"count":40,"type":"VEC2"}},
                  {{"bufferView":2,"componentType":5125,"count":114,"type":"SCALAR"}}]}}"#,
            bin.len(),
            480 + 456
        );
        let d = doc(&json);
        let buffers = buffers(&d, Some(&bin)).unwrap();
        let shorts = elements(&d, &buffers, 0).unwrap().shorts();
        assert_eq!(shorts, positions.iter().flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<_>>());
        let floats = elements(&d, &buffers, 1).unwrap().floats();
        assert_eq!(floats[2..4], [1000.0 / 65535.0, 65534.0 / 65535.0]);
        // the index codec may rotate a triangle's corners, never its winding: compare each from its smallest corner
        let rotated = |ix: &[u32]| -> Vec<[u32; 3]> {
            ix.chunks_exact(3)
                .map(|t| {
                    let k = (0..3).min_by_key(|&k| t[k]).unwrap();
                    [t[k], t[(k + 1) % 3], t[(k + 2) % 3]]
                })
                .collect()
        };
        assert_eq!(rotated(&elements(&d, &buffers, 2).unwrap().integers().unwrap()), rotated(&indices));
    }

    #[test]
    fn refuses_what_is_not_a_glb() {
        assert!(split(b"nope").is_err());
        let mut glb = b"glTF".to_vec();
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&0u32.to_le_bytes());
        assert!(split(&glb).is_err(), "a glb without a JSON chunk");
        let json = br#"{"asset":{"version":"2.0"}}"#;
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        glb.extend_from_slice(json);
        let split = split(&glb).unwrap();
        assert_eq!((split.json, split.bin), (&json[..], None));
    }
}
