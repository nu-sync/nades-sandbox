//! The scene description, format 3 (laid out as format 2 was): what a pack's Scene section holds, and how it points
//! into the Geometry and Textures sections. It is the interface every renderer of a pack reads, so each field says its
//! units, its layout and what its indices refer to. `nades-pack <map>` (crates/packer) writes it;
//! [`crate::Pack::scene`] reads and validates it.
//!
//! # Frame and units
//!
//! Everything is in **glTF space as Source 2 Viewer exports a map**: metres, y up, right-handed, with
//! glTF (x, y, z) = Source (y, z, x) × 0.0254. `bench/camera.json` is in this frame, and so are the fog's distances
//! and heights, the sun's direction and the 3D skybox's placement ([`Skybox`]). Matrices are column-major
//! `[f32; 16]` (glam's `Mat4::from_cols_array`, WGSL's `mat4x4<f32>` in a buffer). Every number in the Geometry
//! and Textures sections is little-endian.
//!
//! # Drawing a draw
//!
//! One [`Draw`] is one glTF primitive of one node: an indexed triangle list. For vertex `v` of instance `i`:
//!
//! ```text
//! world = draw.model × instance[i] × vec4(position.xyz, 1)      (instance[i] = identity when not instanced)
//! ```
//!
//! - `position` is [`Draw::positions`]: snorm16 × 4 per vertex ([`POSITION_STRIDE`] = 8 bytes; wgpu `Snorm16x4`),
//!   the export's quantized positions. x, y and z lie in [-1, 1]; `model` holds the dequantization (the export's
//!   scale and offset). The fourth component is stored as 32767, so it reads as 1.0: the attribute can be used as a
//!   homogeneous position as it is.
//! - `uv0` is [`Draw::uv0`]: f32 × 2 per vertex ([`UV_STRIDE`] = 8 bytes; `Float32x2`), TEXCOORD_0, the base colour
//!   texture's coordinates. They tile: sample with repeat. [`Draw::uv1`] is TEXCOORD_1 in the same layout, when the
//!   primitive has it: the second UV set a layered material's layer may read ([`Layer::uv_set`]).
//! - the lightmap coordinates are [`LightmapUv::uv`]: f32 × 2 per vertex, the same layout, in the atlas's [0, 1].
//!   v = 0 is the lightmap texture's first row, as glTF puts it. They come from TEXCOORD_2 (taken as exported; on
//!   de_cache all of it lies in [0, 1]), or from TEXCOORD_1 when every one of its coordinates lies in [0, 1] within
//!   0.001 ([`UvSet`]); draws with neither have `lightmap: None`. An instanced draw's instances share its lightmap
//!   coordinates, as the web view draws them.
//! - the tangent frame is [`Draw::frames`]: per vertex the normal as snorm8 × 4 (x, y, z, 0) and the tangent as
//!   snorm8 × 4 (x, y, z, w = ±1, the bitangent's sign: bitangent = cross(normal, tangent) × w), 8 bytes
//!   ([`FRAME_STRIDE`]), the export's own quantized values. Both are in the primitive's own space: transform them
//!   with `model × instance` like positions (as directions) and normalize.
//! - [`Draw::colors`] is COLOR_0, unorm8 × 4 per vertex ([`COLOR_STRIDE`]), linear as glTF stores it; none means white.
//! - [`Draw::paint`] is the export's `_TEXCOORD_4`, f32 × 4 per vertex ([`PAINT_STRIDE`]): the vertex paint layered
//!   materials blend by (x: layer 2, y: layer 3, z: wetness, w: painted softness); none means zero.
//! - indices are [`Draw::indices`]: `index_count` u16 or u32 ([`Draw::index_format`]), three per triangle, each less
//!   than `vertex_count`.
//! - instances are [`Instances::transforms`]: `count` column-major 4×4 f32 matrices ([`INSTANCE_STRIDE`] = 64
//!   bytes), EXT_mesh_gpu_instancing's translation × rotation × scale. [`Draw::world_transforms`] gives
//!   `model × instance[i]` for a renderer that wants one matrix per instance.
//! - **front faces are counter-clockwise** in world space for every draw: where `model × instance` mirrors
//!   (a negative determinant), the packer reversed the triangles, and a draw whose instances mirror differently is
//!   split in two. Cull back faces unless the material is double-sided.
//! - [`Draw::bounds`] is the box around every instance, in world space, for culling.
//!
//! Every span starts on a [`SPAN_ALIGN`] (256-byte) boundary of the Geometry section, so a renderer can bind the
//! whole section as one buffer and offset into it: as vertex or index buffers, or as storage buffers.
//!
//! # Order
//!
//! [`Scene::draws`] is in a fixed order: every opaque and alpha-tested draw, then every blended one, then every
//! csgo_effects one ([`Shading::Effects`]), each group in the export's node order. [`Skybox::draws`] is ordered the
//! same way. Drawing in this order gives every renderer the same picture where blended surfaces overlap.
//!
//! # Materials
//!
//! A [`Material`] says how CS2 shades its surfaces, from the parameters the export keeps of the game's material
//! (`extras.vmat`) and the game's own textures. Its [`Shading`] names the shader family, with that family's
//! parameters already resolved into what a shader reads (colour matrices, coordinate transforms, blend constants):
//! the packer ports show-me-the-nades' `csgoMaterials.ts` and `csgoEffects.ts`, themselves ports of Source 2
//! Viewer's renderer. Every texture index is into [`Scene::textures`]; none means a neutral default (white colour,
//! a flat normal, heights at 0.5 with a full tint mask, no occlusion).
//!
//! The game's textures pack several things per texel, and the pack keeps them as the game stores them
//! (Source 2 Viewer's `csgo_environment` reads them so):
//! - a layer's colour ([`Layer::color`], [`Material::base_color`]): sRGB colour, and in alpha its ambient occlusion
//!   for environment materials (the export's images dropped it: a texture from the export has none, read as 1);
//! - a layer's height map ([`Layer::height`]): red the height, green the tint mask, alpha the metalness;
//! - a normal map ([`Texture::semantic`] [`Semantic::NormalHemiOct`]): red and green the normal, hemi-octahedral,
//!   blue the roughness. A normal map from the export's images ([`Semantic::NormalRgb`]) is a plain tangent-space
//!   normal in RGB instead (×2 − 1).
//!
//! The base colour is multiplied by [`Material::base_color_factor`] for every family but [`Shading::Environment`],
//! where the factor is the object's tint, which the game colourizes by instead ([`Environment::tint`]).
//!
//! # Textures
//!
//! A [`Texture`] is a 2D texture or a cube map with its full stored mip chain, ready for `copy_buffer_to_texture`:
//! - [`Texture::format`] and [`Texture::srgb`] name the GPU format; the table at [`TextureFormat`] gives wgpu's.
//! - [`Texture::layers`] is 1, or 6 for a cube map (the sky), whose faces are layers 0–5 in wgpu's order
//!   (+X, −X, +Y, −Y, +Z, −Z).
//! - its bytes are in the Textures section, inside chunk [`Texture::chunk`] ([`Scene::chunks`]): mip `m` is
//!   `bytes_per_row × rows × layers` bytes at `chunks[chunk].offset + mips[m].offset` of the section, its layers one
//!   after the other. In a buffer holding the chunk, `mips[m].offset` is the copy's `offset`, `bytes_per_row` its
//!   `bytes_per_row` (a multiple of 256) and `rows` its `rows_per_image`. Rows are rows of 4×4 blocks for the BC
//!   formats and rows of texels for RGBA8; each is padded with zeros to `bytes_per_row`.
//! - the copy's extent is [`Mip::copy_size`]: the mip's size rounded up to whole blocks (wgpu's physical size), with
//!   `layers` as its depth.
//! - a BC texture's width and height are multiples of 4, as WebGPU requires.
//! - chunks start on 16 KiB boundaries of the section and their lengths are multiples of 16 KiB, so a chunk is whole
//!   pages of the mapping. A texture lies in one chunk, and textures are in chunk order, so each chunk's textures are
//!   a run of indices ([`Scene::chunk_textures`]). Chunks are upload units of ≈32 MiB, the largest first.
//! - sample material textures with repeat wrapping and linear filtering between texels and mips.
//!
//! # Light
//!
//! [`Scene::lightmaps`] names the map's baked light, the game's own blocks from the map's VPK
//! (`maps/<map>/lightmaps/`), sampled at a draw's [`LightmapUv`] with clamp-to-edge and linear filtering:
//! - **irradiance** (BC6H, unsigned, linear; 8192² on de_cache with the 4096² level beside it): the indirect light
//!   arriving at the surface, the sun's bounce included and the sun itself left out, used as stored (no scale).
//!   Checked on de_cache (crates/packer `tests/export.rs`): decoded as unsigned BC6H, every texel of level 0 equals
//!   the export's `irradiance.exr` at the same row and column.
//! - **directional irradiance** (BC1, linear): per texel the dominant direction the indirect light arrives from, in
//!   the surface's tangent space, in red and green (× 2 − 1), and in blue the share of it that arrives from all
//!   around instead. Source 2 Viewer's `ComputeLightmapShading` (lightmap version 2, CS2) shades a normal map with it;
//!   for an unbumped surface it gives the irradiance back unchanged.
//! - **shadows** (BC4, BC5 or BC7: one channel per baked light, up to four): each light's baked shadow per texel,
//!   1 in full shadow. The sun's is channel [`Lightmaps::sun_channel`] (its light_environment's `bakedshadowindex`),
//!   and the sun lights a surface by [`Sun::color`] × max(N·L, 0) × (1 − shadow).
//!
//! Diffuse light is then albedo × (sun + indirect) × ambient occlusion, as CS2 adds them (no 1/π).
//! [`Scene::sun`] is the map's light_environment; [`Scene::sky`] its env_sky, a BC6H cube map sampled by direction.
//!
//! A draw without lightmap coordinates is lit as the game lights it, by the light probe volumes ([`Probes`]): each
//! probe an ambient cube (the irradiance arriving along ±x, ±y, ±z of Source's frame) and a baked shadow mask, in two
//! 3D textures shared by every volume (Source 2 Viewer's `lighting.lpv.slang`). Whether a draw has lightmap
//! coordinates is the game's own answer where the packer could read it (the mesh's draw calls'
//! `m_bHasBakedLightingFromLightMap`): a mesh whose second UV set lies in [0, 1] is not lightmapped for that alone.
//!
//! # Validation
//!
//! [`crate::Pack::scene`] checks every invariant above against the sections' lengths ([`Scene::validate`]), so a
//! renderer can slice spans and mips without bounds checks of its own. (That indices name the draw's own vertices is
//! checked when the pack is built.)
//!
//! # Changing it
//!
//! The Scene section is postcard, which is not self-describing: any change to a type here changes the layout, so it
//! bumps [`crate::VERSION`] (and `the_scene_layout_is_format_3` in the tests says so).

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// Bytes per vertex in [`Draw::positions`]: snorm16 × 4.
pub const POSITION_STRIDE: u64 = 8;
/// Bytes per vertex in [`Draw::uv0`], [`Draw::uv1`] and [`LightmapUv::uv`]: f32 × 2.
pub const UV_STRIDE: u64 = 8;
/// Bytes per vertex in [`Draw::frames`]: the normal and the tangent, snorm8 × 4 each.
pub const FRAME_STRIDE: u64 = 8;
/// Bytes per vertex in [`Draw::colors`]: unorm8 × 4.
pub const COLOR_STRIDE: u64 = 4;
/// Bytes per vertex in [`Draw::paint`]: f32 × 4.
pub const PAINT_STRIDE: u64 = 16;
/// Bytes per instance in [`Instances::transforms`]: a column-major 4×4 f32 matrix.
pub const INSTANCE_STRIDE: u64 = 64;
/// Every span in the Geometry section starts on this boundary.
pub const SPAN_ALIGN: u64 = 256;
/// `bytes_per_row` is a multiple of this, as `copy_buffer_to_texture` requires; mip offsets are too.
pub const ROW_ALIGN: u32 = 256;
/// Texture chunks start on this boundary of the Textures section and their lengths are multiples of it.
pub const CHUNK_ALIGN: u64 = crate::ALIGN;
/// Layers per environment material at most.
pub const MAX_LAYERS: usize = 3;

/// A byte range inside a section: [`Draw`]'s are in the Geometry section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub offset: u64,
    pub len: u64,
}

impl Span {
    pub fn end(&self) -> u64 {
        self.offset + self.len
    }

    pub fn range(&self) -> Range<usize> {
        self.offset as usize..self.end() as usize
    }

    /// The span's bytes in `section` (the Geometry section for a draw's spans).
    pub fn of<'a>(&self, section: &'a [u8]) -> &'a [u8] {
        &section[self.range()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IndexFormat {
    U16,
    U32,
}

impl IndexFormat {
    pub fn bytes(self) -> u64 {
        match self {
            Self::U16 => 2,
            Self::U32 => 4,
        }
    }
}

/// Which glTF attribute a draw's lightmap coordinates came from, by the web view's rule (show-me-the-nades
/// `web/app/src/scene/render/mapModel.ts`, `lightmapCoordinates`): TEXCOORD_2 when the primitive has it, otherwise
/// TEXCOORD_1 when every one of its coordinates lies in [0, 1] (within 0.001), otherwise none. (The web view checks
/// about 256 of TEXCOORD_1's coordinates; [`Stats::lightmap_rule_differs_from_web`] counts where that would differ.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UvSet {
    Texcoord1,
    Texcoord2,
}

/// A draw's lightmap coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightmapUv {
    /// f32 × 2 per vertex, `vertex_count` of them, in [0, 1]; v = 0 is the lightmap's first row. Shared by every
    /// instance of an instanced draw.
    pub uv: Span,
    pub from: UvSet,
}

/// EXT_mesh_gpu_instancing: the draw is drawn once per matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instances {
    pub count: u32,
    /// `count` column-major 4×4 f32 matrices, applied before [`Draw::model`].
    pub transforms: Span,
}

/// An axis-aligned box in world space (glTF space, metres).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// One indexed triangle list: a glTF primitive of one node, with its material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draw {
    /// The glTF node's name, for debugging.
    pub name: String,
    /// The node's world transform (column-major), including the export's position dequantization.
    pub model: [f32; 16],
    pub vertex_count: u32,
    /// snorm16 × 4 per vertex: x, y, z in [-1, 1] before `model`, w stored as 32767 (1.0).
    pub positions: Span,
    /// f32 × 2 per vertex: TEXCOORD_0, the base colour's coordinates.
    pub uv0: Span,
    /// f32 × 2 per vertex: TEXCOORD_1, when the primitive has it.
    pub uv1: Option<Span>,
    /// The lightmap coordinates, or none: such a draw is lit without the lightmaps.
    pub lightmap: Option<LightmapUv>,
    /// snorm8 × 4 normal, then snorm8 × 4 tangent with the bitangent's sign in w, per vertex.
    pub frames: Span,
    /// unorm8 × 4 per vertex: COLOR_0, or none (white).
    pub colors: Option<Span>,
    /// f32 × 4 per vertex: `_TEXCOORD_4`, the vertex paint, or none (zero).
    pub paint: Option<Span>,
    /// `index_count` indices of `index_format`, three per counter-clockwise triangle.
    pub indices: Span,
    pub index_format: IndexFormat,
    pub index_count: u32,
    pub instances: Option<Instances>,
    /// An index into [`Scene::materials`].
    pub material: u32,
    /// Around every instance, in world space.
    pub bounds: Bounds,
}

impl Draw {
    /// How many times the draw is drawn: its instances, or one.
    pub fn instance_count(&self) -> u32 {
        self.instances.map_or(1, |i| i.count)
    }

    /// Triangles drawn, counting every instance.
    pub fn triangles(&self) -> u64 {
        u64::from(self.index_count / 3) * u64::from(self.instance_count())
    }

    /// `model × instance[i]` for each instance, or `[model]`: one world matrix per instance. `geometry` is the
    /// Geometry section.
    pub fn world_transforms(&self, geometry: &[u8]) -> Vec<[f32; 16]> {
        match self.instances {
            None => vec![self.model],
            Some(instances) => instances.transforms.of(geometry).chunks_exact(INSTANCE_STRIDE as usize).map(|m| mul(&self.model, &matrix(m))).collect(),
        }
    }
}

/// A column-major matrix from 64 little-endian bytes.
fn matrix(bytes: &[u8]) -> [f32; 16] {
    std::array::from_fn(|k| f32::from_le_bytes(bytes[k * 4..k * 4 + 4].try_into().expect("4 bytes")))
}

/// `a × b`, both column-major.
fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    std::array::from_fn(|k| {
        let (column, row) = (k / 4, k % 4);
        (0..4).map(|i| a[i * 4 + row] * b[column * 4 + i]).sum()
    })
}

/// glTF's alpha modes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AlphaMode {
    /// Alpha is ignored: the texture's alpha channel may hold something else.
    Opaque,
    /// Fragments whose alpha is below `cutoff` are discarded; the rest are opaque.
    Mask { cutoff: f32 },
    /// Alpha-blended over what is behind, after every opaque draw.
    Blend,
}

/// A 2D affine transform of texture coordinates: `u' = a u + b v + c`, `v' = d u + e v + f`, as `[a, b, c, d, e, f]`
/// (the material's scale, rotation about its centre and offset: Source 2 Viewer's `RotateVector2D`).
pub type UvTransform = [f32; 6];
/// The identity [`UvTransform`].
pub const UV_IDENTITY: UvTransform = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
/// A colour matrix, three rows of four: `rgb'[i] = m[i][0] r + m[i][1] g + m[i][2] b + m[i][3]` (linear colour).
pub type ColorMatrix = [[f32; 4]; 3];
/// The identity [`ColorMatrix`].
pub const COLOR_IDENTITY: ColorMatrix = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

/// One layer of an environment material (csgo_environment's layer 1, csgo_environment_blend's 2 and 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    /// Its colour texture (sRGB colour, ambient occlusion in alpha), `g_tColorN`.
    pub color: Option<u32>,
    /// Its normal map, `g_tNormalN`.
    pub normal: Option<u32>,
    /// Its height map (red height, green tint mask, alpha metalness), `g_tHeightN`.
    pub height: Option<u32>,
    /// Which coordinates it reads: 1 for TEXCOORD_0, 2 for TEXCOORD_1 (`g_nUVSetN`; the game's biplanar 0 reads
    /// TEXCOORD_0 here).
    pub uv_set: u8,
    /// Its coordinates' transform (`g_vTexCoordScaleN`, `…OffsetN`, `…CenterN`, `g_flTexCoordRotationN`).
    pub uv: UvTransform,
    /// The colour correction applied where the tint mask is off (`g_mTextureAdjustN`): identity unless
    /// `g_nColorCorrectionModeN` is 1.
    pub adjust: ColorMatrix,
    /// The colour correction and tint applied where the tint mask is on (`g_mTextureColorAdjustN`).
    pub tint_adjust: ColorMatrix,
    /// Whether the object's tint colourizes it (`g_bModelTintN`).
    pub model_tint: bool,
    /// The tint mask's contrast and brightness (`g_fTintMaskContrastN`, `g_fTintMaskBrightnessN`).
    pub tint_mask: [f32; 2],
    /// `g_flHeightMapScaleN` and `g_flHeightMapZeroPointN`.
    pub height_scale: f32,
    pub height_zero: f32,
    /// Layers 2 and 3: `g_flBlendSoftnessN`, `g_flUnderlyingHeightMapInfluenceN`, `g_flMaskWithHeightN`.
    pub softness: f32,
    pub underlying: f32,
    pub mask_with_height: f32,
    /// `g_fTextureNormalContrastN`: 1 keeps the normal map as it is.
    pub normal_contrast: f32,
    /// The roughness's contrast and brightness (`g_fTextureRoughnessContrastN`, `…BrightnessN`).
    pub roughness: [f32; 2],
    /// Whether its height map's alpha is metalness (`g_bMetalnessN`).
    pub metalness: bool,
    /// `g_nVertexColorModeN`: 0 the vertex colour masked by the tint mask, 1 unmasked, 2 none.
    pub vertex_color: u8,
    /// Layers 2 and 3: kept to surfaces that face a direction (`F_BLEND_BY_FACING_DIRECTION_N`); None when the paint
    /// alone blends the layer in.
    pub facing: Option<Facing>,
}

/// A layer kept to surfaces that face a direction: its paint is scaled by
/// `smoothstep(min, max, dot(direction, normal) × 0.5 + 0.5)` (`g_vFacingDirectionN`, with `min` and `max` from
/// `g_flFacingDirectionMaskSpreadN` and `g_vFacingDirectionMaskFalloffN`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Facing {
    /// A unit vector in the scene's frame.
    pub direction: [f32; 3],
    pub min: f32,
    pub max: f32,
}

/// csgo_environment and csgo_environment_blend: one to three [`Layer`]s, the upper ones blended in by the vertex
/// paint ([`Draw::paint`]) against their height maps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub layers: Vec<Layer>,
    /// `F_USE_NEW_BLENDING`: the height-band blend, else the old two-height one.
    pub new_blending: bool,
    /// The object's tint (linear; the export's base colour factor), which each layer is colourized by through its
    /// tint mask, by `tint_amount` (`g_flModelTintAmount`).
    pub tint: [f32; 3],
    pub tint_amount: f32,
}

/// csgo_lightmappedgeneric with two layers: the second layer's colour blended in by the vertex paint, sharpened by a
/// modulation texture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layered {
    /// `g_tLayer2Color`, sampled at TEXCOORD_0 × `uv2_scale`.
    pub color2: Option<u32>,
    /// `g_tBlendModulation`: red softness, green the blend mask (sampled with the second layer's coordinates).
    pub modulation: Option<u32>,
    /// `g_vLayer1Tint` and `g_vLayer2Tint`, linear.
    pub tint1: [f32; 3],
    pub tint2: [f32; 3],
    /// `F_FANCY_BLENDING`: 0 the paint alone, 1 modulated by the mask with its red as softness, 2 with
    /// [`Layered::softness`], 3 by the modulation's alpha with that softness.
    pub fancy: u8,
    pub softness: f32,
    pub uv2_scale: [f32; 2],
}

/// csgo_effects: an unlit card (light shafts, dust, haze, steam, cloud cards, sun glows), faded out edge-on, up close
/// and where it meets the geometry behind it (Source 2 Viewer's `csgo_effects.frag`, through the web view's port).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effects {
    /// Added to what is behind rather than blended over it.
    pub additive: bool,
    /// The panning masks the export leaves out: per mask, its scale and pan speed (`g_vMaskNScale`, `…PanSpeed`),
    /// or none where the material's mask is a constant. A renderer stands a tiling noise in for each.
    pub masks: [Option<[f32; 4]>; 3],
    /// `g_flColorBoost` and `g_flOpacityScale` (× the export's alpha).
    pub color_boost: f32,
    pub opacity: f32,
    /// Fade distance (scene units), falloff, minimum and maximum (`g_flFadeDistance`, …).
    pub fade: [f32; 4],
    /// Fresnel exponent, falloff, minimum and maximum (`g_flFresnelExponent`, …).
    pub fresnel: [f32; 4],
    /// Depth feather distance (scene units; 0 for none) and falloff (`F_DEPTH_FEATHER`, `g_flFeatherDistance`, …).
    pub feather: [f32; 2],
}

/// How a material is shaded: CS2's shader families as the pack draws them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Shading {
    /// Lit, one layer: base colour, its normal map and ambient occlusion. csgo_complex, csgo_vertexlitgeneric,
    /// csgo_lightmappedgeneric (one layer), csgo_static_overlay, csgo_foliage, csgo_glass,
    /// csgo_simple_3layer_parallax, generic and anything not recognised.
    Generic,
    /// csgo_environment and csgo_environment_blend.
    Environment(Environment),
    /// Two-layer csgo_lightmappedgeneric.
    Layered(Layered),
    /// csgo_effects, drawn after everything else.
    Effects(Effects),
    /// csgo_unlitgeneric: colour × factor, unlit; added rather than blended when `additive`.
    Unlit { additive: bool },
    /// csgo_black_unlit: black.
    Black,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    /// The export's material name, for debugging.
    pub name: String,
    /// The CS2 shader it names (`extras.vmat.ShaderName`, e.g. `csgo_environment_blend.vfx`).
    pub shader: String,
    pub shading: Shading,
    /// An index into [`Scene::textures`], or none (sample white). For [`Shading::Environment`] it is layer 1's colour.
    pub base_color: Option<u32>,
    /// Linear RGBA, multiplying the base colour texture's sample (but see [`Environment::tint`]).
    pub base_color_factor: [f32; 4],
    /// The normal map: layer 1's for an environment material. None is flat.
    pub normal: Option<u32>,
    /// Ambient occlusion in red (`g_tAmbientOcclusion`, or the export's occlusion image), for the families whose
    /// colour texture does not carry it. None is unoccluded.
    pub occlusion: Option<u32>,
    pub alpha: AlphaMode,
    /// Draw both faces; otherwise cull back faces.
    pub double_sided: bool,
}

impl Material {
    /// Every texture the material samples.
    pub fn textures(&self) -> Vec<u32> {
        let mut all: Vec<Option<u32>> = vec![self.base_color, self.normal, self.occlusion];
        match &self.shading {
            Shading::Environment(e) => all.extend(e.layers.iter().flat_map(|l| [l.color, l.normal, l.height])),
            Shading::Layered(l) => all.extend([l.color2, l.modulation]),
            Shading::Generic | Shading::Effects(_) | Shading::Unlit { .. } | Shading::Black => {}
        }
        all.into_iter().flatten().collect()
    }

    /// Whether it draws in the effects pass, after every other draw.
    pub fn is_effect(&self) -> bool {
        matches!(self.shading, Shading::Effects(_))
    }
}

/// Texture formats, with their block sizes. wgpu's names, by `srgb`:
///
/// | format | `srgb: false` | `srgb: true` |
/// |---|---|---|
/// | `Bc1` | `Bc1RgbaUnorm` | `Bc1RgbaUnormSrgb` |
/// | `Bc3` | `Bc3RgbaUnorm` | `Bc3RgbaUnormSrgb` |
/// | `Bc4` | `Bc4RUnorm` | — |
/// | `Bc5` | `Bc5RgUnorm` | — |
/// | `Bc6h` | `Bc6hRgbUfloat` | — |
/// | `Bc7` | `Bc7RgbaUnorm` | `Bc7RgbaUnormSrgb` |
/// | `Rgba8` | `Rgba8Unorm` | `Rgba8UnormSrgb` |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextureFormat {
    /// BC1 (DXT1): RGB with 1-bit alpha, 8 bytes a block.
    Bc1,
    /// BC3 (DXT5): RGBA, 16 bytes a block.
    Bc3,
    /// BC4 (ATI1N): one channel, 8 bytes a block.
    Bc4,
    /// BC5 (ATI2N): two channels, 16 bytes a block.
    Bc5,
    /// BC6H, unsigned half floats (the lightmap's irradiance, the sky), 16 bytes a block.
    Bc6h,
    /// BC7: RGBA, 16 bytes a block.
    Bc7,
    /// Uncompressed RGBA, 4 bytes a texel (images with no game file, decoded from the export).
    Rgba8,
}

impl TextureFormat {
    /// Texels along a block's side: 4 for the BC formats, 1 for RGBA8.
    pub fn block_dim(self) -> u32 {
        if self == Self::Rgba8 { 1 } else { 4 }
    }

    /// Bytes per block (per texel for RGBA8).
    pub fn block_bytes(self) -> u32 {
        match self {
            Self::Bc1 | Self::Bc4 => 8,
            Self::Rgba8 => 4,
            Self::Bc3 | Self::Bc5 | Self::Bc6h | Self::Bc7 => 16,
        }
    }

    /// Whether the format has an sRGB form: the ones a colour can be stored in.
    pub fn has_srgb(self) -> bool {
        matches!(self, Self::Bc1 | Self::Bc3 | Self::Bc7 | Self::Rgba8)
    }
}

/// One mip level's place and layout. Its bytes are `bytes_per_row × rows × layers`, at `offset` in its texture's
/// chunk, the layers one after the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mip {
    /// Texels: `max(1, texture.width >> level)`, and the same for the height.
    pub width: u32,
    pub height: u32,
    /// From the start of the texture's chunk; a multiple of 256.
    pub offset: u64,
    /// A multiple of 256: a row of blocks (or texels, for RGBA8) and its zero padding.
    pub bytes_per_row: u32,
    /// Rows of blocks (texels, for RGBA8) in one layer: `height` rounded up to whole blocks.
    pub rows: u32,
    /// Layers: 1, or 6 for a cube map.
    pub layers: u32,
}

impl Mip {
    pub fn len(&self) -> u64 {
        u64::from(self.bytes_per_row) * u64::from(self.rows) * u64::from(self.layers)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The copy's extent: the size rounded up to whole blocks (wgpu's physical size of the mip).
    pub fn copy_size(&self, format: TextureFormat) -> (u32, u32) {
        let block = format.block_dim();
        (self.width.div_ceil(block) * block, self.height.div_ceil(block) * block)
    }
}

/// Where a texture's blocks came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureSource {
    /// The game's `.vtex_c` named like the export's image (the name is the game file's, unique under the game
    /// folder). `file` is relative to `game/csgo`; mip 0 here is the file's mip `first_mip`, the first that fits
    /// 1024 as the export does.
    ByName { file: String, first_mip: u32 },
    /// The game's `.vtex_c` that a material names in its `TextureParams` (`param`, e.g. `g_tColor1`), from the
    /// unpacked game folder or the game's `pak01` archives. `file` is the game path.
    ByTextureParams { param: String, file: String, first_mip: u32 },
    /// No game file: the export's own image (glTF image `image`, WebP), decoded at pack time to RGBA8, with mips
    /// box-filtered (in linear light for colour).
    ExportImage { image: u32 },
    /// A file in a VPK of the map's own: `vpk` relative to `game/csgo` (e.g. `maps/de_cache.vpk`, or a 3D skybox's
    /// `maps/prefabs/…vpk`), `entry` inside it (e.g. `maps/de_cache/lightmaps/irradiance.vtex_c`).
    MapVpk { vpk: String, entry: String },
}

/// What a texture holds, which says how a shader reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Semantic {
    /// A colour (sRGB), alpha as its material uses it.
    Color,
    /// Numbers in each channel: height maps, masks, occlusion.
    Data,
    /// A game normal map: hemi-octahedral normal in red and green, roughness in blue.
    NormalHemiOct,
    /// An export image normal map: tangent-space normal in RGB, × 2 − 1.
    NormalRgb,
    /// Lightmaps (see "Light" in the module docs).
    Irradiance,
    IrradianceDirection,
    Shadows,
    /// The sky's cube map.
    Sky,
    /// The light probes' ambient cubes (a BC6H volume) and their baked shadows (a volume of shadow masks).
    ProbeIrradiance,
    ProbeShadows,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Texture {
    /// The export's image name, or the game path or VPK entry; for debugging.
    pub name: String,
    pub format: TextureFormat,
    /// Sample through the format's sRGB form (colour), or linear.
    pub srgb: bool,
    pub semantic: Semantic,
    /// Mip 0's size in texels.
    pub width: u32,
    pub height: u32,
    /// 2D slices in each mip: 1, 6 for a cube map, or the depth of a volume texture.
    pub layers: u32,
    /// A 3D texture of `layers` slices (one mip): the light probes' atlases.
    pub volume: bool,
    /// An index into [`Scene::chunks`]: every mip is inside it.
    pub chunk: u32,
    /// Largest first; every level down to the smallest the source stores.
    pub mips: Vec<Mip>,
    pub source: TextureSource,
}

impl Texture {
    /// Bytes of every mip, padding included.
    pub fn bytes(&self) -> u64 {
        self.mips.iter().map(Mip::len).sum()
    }

    pub fn is_cube(&self) -> bool {
        self.layers == 6 && !self.volume
    }
}

/// A range of the Textures section that holds whole textures: an upload unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// From the start of the Textures section; a multiple of 16 KiB.
    pub offset: u64,
    /// A multiple of 16 KiB.
    pub len: u64,
}

impl Chunk {
    pub fn range(&self) -> Range<usize> {
        self.offset as usize..(self.offset + self.len) as usize
    }
}

/// A map's (or a 3D skybox's) baked light: indices into [`Scene::textures`] (see "Light" in the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lightmaps {
    /// Linear BC6H.
    pub irradiance: u32,
    /// Linear BC1.
    pub directional: Option<u32>,
    /// BC4, BC5 or BC7, linear: one channel per baked light.
    pub shadows: Option<u32>,
    /// The channel of `shadows` that holds the sun's (0–3), or none when the sun has no baked shadow.
    pub sun_channel: Option<u8>,
}

/// The light probe volumes: where each sits in the scene and in the atlases every volume shares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probes {
    /// A linear BC6H volume of six slabs, `layers / 6` slices each: the probes' irradiance arriving along +x, +y, +z,
    /// −x, −y and −z of Source's frame (scene z, x, y), in that order.
    pub irradiance: u32,
    /// A volume of shadow masks (BC7, one channel per baked light), `layers / 6` slices: each probe's baked shadow.
    pub shadows: Option<u32>,
    /// By priority: the ones to sample first (a higher indoor_outdoor_level, then a smaller volume) first.
    pub volumes: Vec<ProbeVolume>,
}

/// One env_combined_light_probe_volume (Source 2 Viewer's `SceneLightProbe.CalculateGpuProbeData`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeVolume {
    /// Scene position to the volume's box normalized to [0, 1]³ (column-major).
    pub to_local: [f32; 16],
    /// The volume's place in the atlases, in texture coordinates of one slab: local × scale + offset.
    pub atlas_scale: [f32; 3],
    pub atlas_offset: [f32; 3],
    /// Half a probe inside the box, in local coordinates with z in sixths: where the irradiance stops filtering.
    pub border_min: [f32; 3],
    pub border_max: [f32; 3],
    /// The fade at its edges: 1 / (fade distance / size) per axis, and how far past its box it reaches (local).
    pub inverse_edge: [f32; 3],
    pub bounds_extend: [f32; 3],
    /// indoor_outdoor_level.
    pub priority: i32,
    /// Its box in the scene's frame.
    pub bounds: Bounds,
}

/// The texture formats a baked shadow mask comes in, and how many lights' channels each holds.
pub fn shadow_channels(format: TextureFormat) -> Option<u8> {
    match format {
        TextureFormat::Bc4 => Some(1),
        TextureFormat::Bc5 => Some(2),
        TextureFormat::Bc1 => Some(3),
        TextureFormat::Bc3 | TextureFormat::Bc7 => Some(4),
        TextureFormat::Bc6h | TextureFormat::Rgba8 => None,
    }
}

/// The map's sun (its light_environment, as the export's glTF light carries it).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sun {
    /// Towards the sun, a unit vector in the scene's frame.
    pub direction: [f32; 3],
    /// Linear colour × brightness: the light on a surface facing it, before its baked shadow.
    pub color: [f32; 3],
}

/// The sky the game draws behind everything: an env_sky's cube map.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sky {
    /// An index into [`Scene::textures`]: a BC6H cube map ([`Texture::layers`] 6) with its mips.
    pub texture: u32,
    /// The env_sky's yaw in degrees: the sky is turned by it about the vertical. A view direction `d` (Source's frame,
    /// z up) reads the cube at `Rz(−yaw) d` (Source 2 Viewer's `SceneSkybox2D`).
    pub yaw: f32,
    /// Linear: the env_sky's tint × its brightness × 2^(the sky material's exposure biases).
    pub tint: [f32; 3],
}

/// env_gradient_fog: fog toward one colour, by distance and by height. Distances and heights are in the scene's
/// metres (a height is a scene y).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientFog {
    pub start: f32,
    pub end: f32,
    pub falloff: f32,
    /// Whether it also thins out with height, from `height_start` (none) to `height_end`.
    pub height: bool,
    pub height_start: f32,
    pub height_end: f32,
    pub vertical_exponent: f32,
    /// Linear colour × strength.
    pub color: [f32; 3],
    pub max_opacity: f32,
}

/// env_cubemap_fog: fog toward the sky behind the surface, blurred the more as there is less fog. Distances and
/// heights in the scene's metres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CubemapFog {
    pub start: f32,
    pub end: f32,
    pub falloff: f32,
    pub height: bool,
    pub height_start: f32,
    pub height_end: f32,
    pub height_exponent: f32,
    pub max_opacity: f32,
    /// How much less blurred the sky behind gets as the fog thickens.
    pub lod_bias: f32,
    /// 2^(the fog sky's exposure bias), the sky's brightness included.
    pub sky_scale: f32,
}

/// The map's tone curve (Hable's filmic curve, as the post-processing volume sets it).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tonemap {
    /// In stops.
    pub exposure_bias: f32,
    pub shoulder_strength: f32,
    pub linear_strength: f32,
    pub linear_angle: f32,
    pub toe_strength: f32,
    pub toe_num: f32,
    pub toe_denom: f32,
    /// log2 of the white point.
    pub white_point: f32,
}

impl Tonemap {
    /// Source 2 Viewer's curve for a map without its own.
    pub const DEFAULT: Self = Self { exposure_bias: 0.0, shoulder_strength: 0.15, linear_strength: 0.5, linear_angle: 0.1, toe_strength: 0.2, toe_num: 0.02, toe_denom: 0.3, white_point: 4.0 };
}

/// The exposure the game adapts to, bounded.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Exposure {
    pub enabled: bool,
    pub min: f32,
    pub max: f32,
    /// In stops.
    pub compensation: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BloomMode {
    Add,
    Screen,
    Blur,
}

/// The bloom layer of the map's post-processing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bloom {
    pub mode: BloomMode,
    /// Each mode's strength.
    pub strength: f32,
    pub screen: f32,
    pub blur: f32,
    pub threshold: f32,
    pub threshold_width: f32,
    pub opacity: f32,
}

/// The colour lookup table the game grades its frames with, in gamma space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lut {
    /// The table is `dim`³ texels.
    pub dim: u32,
    pub weight: f32,
    /// RGBA8, red fastest, then green, then blue (x, y, z of a 3D texture).
    pub rgba: Vec<u8>,
}

/// The map's look after lighting: its post-processing volume, colour correction and fog. Any part the map lacks is
/// none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub tonemap: Option<Tonemap>,
    pub exposure: Option<Exposure>,
    pub bloom: Option<Bloom>,
    pub lut: Option<Lut>,
    pub fog: Option<CubemapFog>,
    pub gradient_fog: Option<GradientFog>,
}

/// The map's 3D skybox: its distant scenery, a prefab built at 1/scale, drawn first through a camera at
/// `(eye − reference) / scale + origin` (Source 2 Viewer's SkyTransform), then the map over it with the depth cleared.
/// Its draws, like the map's, index [`Scene::materials`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skybox {
    pub draws: Vec<Draw>,
    /// The prefab's own baked light (its lightmap coordinates are into these).
    pub lightmaps: Option<Lightmaps>,
    pub scale: f32,
    /// The prefab's sky_camera, in the skybox's own frame, and the map's skybox_reference, in the map's (metres).
    pub origin: [f32; 3],
    pub reference: [f32; 3],
    /// The prefab's own gradient fog, which its pass uses instead of the map's, in the map's units (a renderer
    /// moves it into the skybox's frame).
    pub gradient_fog: Option<GradientFog>,
}

/// What the packer could not represent, and why: `count` items with this `reason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub reason: String,
    pub count: u32,
}

/// Counts from the pack's build, for reports and the shell.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// Images in the export, and how many of them some material uses as its base colour.
    pub images: u32,
    pub base_color_images: u32,
    /// Base colour textures by where they came from: [`TextureSource::ByName`], [`TextureSource::ByTextureParams`],
    /// and [`TextureSource::ExportImage`] (the fallback).
    pub by_name: u32,
    pub by_texture_params: u32,
    pub by_fallback: u32,
    /// Textures besides base colours (normal maps, layers, heights, occlusion), from the game's files and from the
    /// export's images.
    pub material_textures_game: u32,
    pub material_textures_export: u32,
    /// Material textures the material names but neither the game's files nor the export hold: drawn with defaults.
    pub material_textures_missing: u32,
    pub draws: u32,
    pub skybox_draws: u32,
    pub instanced_draws: u32,
    /// Instances across the instanced draws.
    pub instances: u32,
    /// Draws with lightmap coordinates, by the attribute they came from, and draws without: the map's and the 3D
    /// skybox's together, as are the counts after them.
    pub lightmapped_texcoord2: u32,
    pub lightmapped_texcoord1: u32,
    pub unlit: u32,
    /// Draws where the web view, which checks about 256 of a TEXCOORD_1's coordinates, would decide otherwise than
    /// the every-coordinate check here.
    pub lightmap_rule_differs_from_web: u32,
    /// Draws the game says are not lightmapped though their UV sets would pass for lightmap coordinates, and draws
    /// whose mesh the packer found no game answer for (decided by the UV sets alone).
    pub probe_lit_by_game: u32,
    pub lightmapped_unknown: u32,
    /// Draws whose triangles the packer reversed because their transform mirrors.
    pub mirrored: u32,
    /// Triangles drawn, counting instances, and vertices stored.
    pub triangles: u64,
    pub vertices: u64,
    /// Materials by shading family: generic, environment, layered, effects, unlit, black.
    pub shading: [u32; 6],
    /// What was left out, by reason: primitives that are not triangle lists, images that could not be read, ….
    pub skipped: Vec<Skipped>,
}

/// The Scene section: everything a renderer needs besides the Geometry and Textures sections' bytes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// Opaque and alpha-tested draws first, then blended ones, then effects, each in the export's node order.
    pub draws: Vec<Draw>,
    pub materials: Vec<Material>,
    /// In chunk order: [`Texture::chunk`] never decreases.
    pub textures: Vec<Texture>,
    /// The Textures section's upload units, in section order.
    pub chunks: Vec<Chunk>,
    pub lightmaps: Option<Lightmaps>,
    pub probes: Option<Probes>,
    pub sun: Option<Sun>,
    pub sky: Option<Sky>,
    pub skybox: Option<Skybox>,
    pub post: Post,
    pub stats: Stats,
}

/// A broken invariant: what [`Scene::validate`] found.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Invalid(pub String);

impl Scene {
    /// postcard, as the Scene section stores it.
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("a scene always serializes")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }

    /// Mip `mip` of texture `texture` (every layer), from the Textures section.
    pub fn mip_bytes<'a>(&self, textures: &'a [u8], texture: u32, mip: usize) -> &'a [u8] {
        let t = &self.textures[texture as usize];
        let m = &t.mips[mip];
        let at = (self.chunks[t.chunk as usize].offset + m.offset) as usize;
        &textures[at..at + m.len() as usize]
    }

    /// Chunk `chunk`'s bytes, from the Textures section.
    pub fn chunk_bytes<'a>(&self, textures: &'a [u8], chunk: u32) -> &'a [u8] {
        &textures[self.chunks[chunk as usize].range()]
    }

    /// The indices of the textures in chunk `chunk`: a run, since textures are in chunk order.
    pub fn chunk_textures(&self, chunk: u32) -> Range<u32> {
        let start = self.textures.partition_point(|t| t.chunk < chunk);
        let end = self.textures.partition_point(|t| t.chunk <= chunk);
        start as u32..end as u32
    }

    /// Every draw: the map's, then the 3D skybox's.
    pub fn all_draws(&self) -> impl Iterator<Item = &Draw> {
        self.draws.iter().chain(self.skybox.iter().flat_map(|s| s.draws.iter()))
    }

    /// Checks every invariant the module documents, against the Geometry and Textures sections' lengths.
    pub fn validate(&self, geometry_len: u64, textures_len: u64) -> Result<(), Invalid> {
        let fail = |what: String| Err(Invalid(what));
        let span = |what: &str, s: &Span| -> Result<(), Invalid> {
            if !s.offset.is_multiple_of(SPAN_ALIGN) {
                return fail(format!("{what} starts at {}, not on a {SPAN_ALIGN}-byte boundary", s.offset));
            }
            if s.offset.checked_add(s.len).is_none_or(|end| end > geometry_len) {
                return fail(format!("{what} ends past the Geometry section ({geometry_len} bytes)"));
            }
            Ok(())
        };
        let texture = |what: &str, t: Option<u32>| -> Result<(), Invalid> {
            match t {
                Some(t) if t as usize >= self.textures.len() => fail(format!("{what} names texture {t} of {}", self.textures.len())),
                _ => Ok(()),
            }
        };
        for (i, d) in self.all_draws().enumerate() {
            let what = |field: &str| format!("draw {i} ({}) {field}", d.name);
            if d.material as usize >= self.materials.len() {
                return fail(what(&format!("names material {} of {}", d.material, self.materials.len())));
            }
            let vertices = u64::from(d.vertex_count);
            let stream = |field: &str, s: &Span, stride: u64| -> Result<(), Invalid> {
                span(&what(field), s)?;
                if s.len != vertices * stride {
                    return fail(what(&format!("{field} does not hold {vertices} vertices")));
                }
                Ok(())
            };
            stream("positions", &d.positions, POSITION_STRIDE)?;
            stream("uv0", &d.uv0, UV_STRIDE)?;
            stream("frames", &d.frames, FRAME_STRIDE)?;
            if let Some(s) = &d.uv1 {
                stream("uv1", s, UV_STRIDE)?;
            }
            if let Some(lightmap) = &d.lightmap {
                stream("lightmap uv", &lightmap.uv, UV_STRIDE)?;
            }
            if let Some(s) = &d.colors {
                stream("colours", s, COLOR_STRIDE)?;
            }
            if let Some(s) = &d.paint {
                stream("paint", s, PAINT_STRIDE)?;
            }
            span(&what("indices"), &d.indices)?;
            if d.indices.len != u64::from(d.index_count) * d.index_format.bytes() || !d.index_count.is_multiple_of(3) || d.index_count == 0 {
                return fail(what(&format!("indices: {} bytes for {} {:?} indices", d.indices.len, d.index_count, d.index_format)));
            }
            if let Some(instances) = &d.instances {
                span(&what("instance transforms"), &instances.transforms)?;
                if instances.count == 0 || instances.transforms.len != u64::from(instances.count) * INSTANCE_STRIDE {
                    return fail(what(&format!("instances: {} bytes for {} matrices", instances.transforms.len, instances.count)));
                }
            }
        }
        for (i, m) in self.materials.iter().enumerate() {
            let what = format!("material {i} ({})", m.name);
            for t in m.textures() {
                texture(&what, Some(t))?;
            }
            if let AlphaMode::Mask { cutoff } = m.alpha
                && !cutoff.is_finite()
            {
                return fail(format!("{what} has alpha cutoff {cutoff}"));
            }
            if let Shading::Environment(e) = &m.shading {
                if e.layers.is_empty() || e.layers.len() > MAX_LAYERS || e.layers.iter().any(|l| !(1..=2).contains(&l.uv_set)) {
                    return fail(format!("{what} has {} layers or a UV set other than 1 or 2", e.layers.len()));
                }
                for (k, f) in e.layers.iter().enumerate().filter_map(|(k, l)| Some((k, l.facing?))) {
                    let length = f.direction.iter().map(|c| c * c).sum::<f32>().sqrt();
                    if k == 0 || !(length - 1.0).abs().lt(&1e-3) || !(f.min >= 0.0 && f.min < f.max && f.max <= 1.0) {
                        return fail(format!("{what}'s layer {} faces {:?} from {} to {}", k + 1, f.direction, f.min, f.max));
                    }
                }
            }
        }
        let mut end = 0;
        for (i, c) in self.chunks.iter().enumerate() {
            if !c.offset.is_multiple_of(CHUNK_ALIGN) || !c.len.is_multiple_of(CHUNK_ALIGN) || c.offset < end {
                return fail(format!("chunk {i} at {}+{} is not on {CHUNK_ALIGN}-byte pages after the one before", c.offset, c.len));
            }
            end = c.offset + c.len;
            if end > textures_len {
                return fail(format!("chunk {i} ends past the Textures section ({textures_len} bytes)"));
            }
        }
        for (i, t) in self.textures.iter().enumerate() {
            let what = |field: String| format!("texture {i} ({}) {field}", t.name);
            let Some(chunk) = self.chunks.get(t.chunk as usize) else {
                return fail(what(format!("names chunk {} of {}", t.chunk, self.chunks.len())));
            };
            if i > 0 && t.chunk < self.textures[i - 1].chunk {
                return fail(what(format!("is in chunk {}, before the texture ahead of it", t.chunk)));
            }
            if t.srgb && !t.format.has_srgb() {
                return fail(what(format!("is sRGB in {:?}, which has no sRGB form", t.format)));
            }
            if t.volume {
                if t.mips.len() != 1 || t.layers == 0 {
                    return fail(what(format!("is a volume of {} slices with {} mips", t.layers, t.mips.len())));
                }
            } else if t.layers != 1 && (t.layers != 6 || t.width != t.height) {
                return fail(what(format!("has {} layers of {}x{}: neither a 2D texture nor a square cube map", t.layers, t.width, t.height)));
            }
            let block = t.format.block_dim();
            if t.width == 0 || t.height == 0 || !t.width.is_multiple_of(block) || !t.height.is_multiple_of(block) {
                return fail(what(format!("is {}x{}: not whole {block}x{block} blocks", t.width, t.height)));
            }
            if t.mips.is_empty() || t.mips.len() as u32 > 32 - t.width.max(t.height).leading_zeros() {
                return fail(what(format!("has {} mips", t.mips.len())));
            }
            for (level, m) in t.mips.iter().enumerate() {
                let (w, h) = ((t.width >> level).max(1), (t.height >> level).max(1));
                let row = w.div_ceil(block) * t.format.block_bytes();
                if (m.width, m.height, m.layers) != (w, h, t.layers) || m.rows != h.div_ceil(block) {
                    return fail(what(format!("mip {level} is {}x{}x{} in {} rows, not {w}x{h}x{}", m.width, m.height, m.layers, m.rows, t.layers)));
                }
                if !m.bytes_per_row.is_multiple_of(ROW_ALIGN) || m.bytes_per_row < row || !m.offset.is_multiple_of(u64::from(ROW_ALIGN)) {
                    return fail(what(format!("mip {level} rows are {} bytes at {} ({row} needed, {ROW_ALIGN}-aligned)", m.bytes_per_row, m.offset)));
                }
                if m.offset + m.len() > chunk.len {
                    return fail(what(format!("mip {level} ends past chunk {}", t.chunk)));
                }
            }
        }
        let expect = |what: &str, index: u32, format: TextureFormat, cube: bool| -> Result<(), Invalid> {
            match self.textures.get(index as usize) {
                Some(t) if t.format == format && !t.srgb && t.is_cube() == cube => Ok(()),
                _ => fail(format!("{what} names texture {index}, which is not a linear {format:?}{}", if cube { " cube map" } else { " texture" })),
            }
        };
        let lightmaps = |what: &str, l: &Lightmaps| -> Result<(), Invalid> {
            expect(&format!("{what} irradiance"), l.irradiance, TextureFormat::Bc6h, false)?;
            if let Some(d) = l.directional {
                expect(&format!("{what} directional irradiance"), d, TextureFormat::Bc1, false)?;
            }
            if let Some(s) = l.shadows {
                let t = &self.textures[s as usize];
                let channels = shadow_channels(t.format);
                if t.srgb || t.is_cube() || channels.is_none() {
                    return fail(format!("{what} shadows name texture {s}, which is not a linear shadow mask"));
                }
                if l.sun_channel.is_some_and(|c| c >= channels.unwrap_or(0)) {
                    return fail(format!("{what} sun's shadow is channel {:?} of a {:?} mask", l.sun_channel, t.format));
                }
            }
            Ok(())
        };
        if let Some(l) = &self.lightmaps {
            lightmaps("the lightmap's", l)?;
        }
        if let Some(l) = self.skybox.as_ref().and_then(|s| s.lightmaps.as_ref()) {
            lightmaps("the skybox's lightmap's", l)?;
        }
        if let Some(sky) = &self.sky {
            expect("the sky", sky.texture, TextureFormat::Bc6h, true)?;
        }
        if let Some(p) = &self.probes {
            match self.textures.get(p.irradiance as usize) {
                Some(t) if t.volume && t.format == TextureFormat::Bc6h && t.layers.is_multiple_of(6) => {}
                _ => return fail(format!("the probes' irradiance names texture {}, which is not a BC6H volume of six slabs", p.irradiance)),
            }
            if let Some(s) = p.shadows {
                match self.textures.get(s as usize) {
                    Some(t) if t.volume && shadow_channels(t.format).is_some() && t.layers * 6 == self.textures[p.irradiance as usize].layers => {}
                    _ => return fail(format!("the probes' shadows name texture {s}, which is not a shadow mask volume a sixth of the irradiance's depth")),
                }
            }
        }
        if let Some(lut) = &self.post.lut
            && (lut.dim < 2 || lut.rgba.len() as u64 != u64::from(lut.dim).pow(3) * 4)
        {
            return fail(format!("the lookup table holds {} bytes for {}³ texels", lut.rgba.len(), lut.dim));
        }
        if let Some(skybox) = &self.skybox
            && !(skybox.scale.is_finite() && skybox.scale > 0.0)
        {
            return fail(format!("the skybox's scale is {}", skybox.scale));
        }
        Ok(())
    }
}

/// `x` rounded up to a multiple of `to`.
pub fn align_up(x: u64, to: u64) -> u64 {
    x.div_ceil(to) * to
}

/// The layout of `levels` mips of a `width`×`height` texture of `layers` layers in `format`, largest first, packed
/// from offset 0: each row padded to [`ROW_ALIGN`], each mip starting on a [`ROW_ALIGN`] boundary. Returns the mips
/// and their total size.
pub fn mip_chain(format: TextureFormat, width: u32, height: u32, levels: u32, layers: u32) -> (Vec<Mip>, u64) {
    let block = format.block_dim();
    let mut offset = 0;
    let mips = (0..levels)
        .map(|level| {
            let (w, h) = ((width >> level).max(1), (height >> level).max(1));
            let bytes_per_row = align_up(u64::from(w.div_ceil(block) * format.block_bytes()), u64::from(ROW_ALIGN)) as u32;
            let mip = Mip { width: w, height: h, offset, bytes_per_row, rows: h.div_ceil(block), layers };
            offset = align_up(offset + mip.len(), u64::from(ROW_ALIGN));
            mip
        })
        .collect();
    (mips, offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pack, PackError, SectionKind, Source, write};

    fn identity() -> [f32; 16] {
        std::array::from_fn(|k| if k % 5 == 0 { 1.0 } else { 0.0 })
    }

    fn translation(x: f32, y: f32, z: f32) -> [f32; 16] {
        let mut m = identity();
        m[12..15].copy_from_slice(&[x, y, z]);
        m
    }

    fn put(geometry: &mut Vec<u8>, bytes: &[u8]) -> Span {
        geometry.resize(align_up(geometry.len() as u64, SPAN_ALIGN) as usize, 0);
        let span = Span { offset: geometry.len() as u64, len: bytes.len() as u64 };
        geometry.extend_from_slice(bytes);
        span
    }

    fn layer(color: Option<u32>, uv_set: u8) -> Layer {
        Layer {
            color,
            normal: None,
            height: Some(2),
            uv_set,
            uv: UV_IDENTITY,
            adjust: COLOR_IDENTITY,
            tint_adjust: COLOR_IDENTITY,
            model_tint: true,
            tint_mask: [1.0, 1.0],
            height_scale: 1.0,
            height_zero: 0.5,
            softness: 0.1,
            underlying: 1.0,
            mask_with_height: 0.0,
            normal_contrast: 1.0,
            roughness: [1.0, 1.0],
            metalness: false,
            vertex_color: 0,
            facing: None,
        }
    }

    /// A small scene as the packer would write it: a lightmapped, painted triangle with an environment material of
    /// two layers, the same triangle instanced twice with a generic one, a BC7 colour texture with three mips, a BC6H
    /// lightmap, a BC4 height map and a BC6H sky cube, in two chunks; a skybox of one draw and a lookup table.
    fn sample() -> (Scene, Vec<u8>, Vec<u8>) {
        let mut geometry = Vec::new();
        let positions: Vec<u8> = [[0i16, 0, 0, 32767], [32767, 0, 0, 32767], [0, 32767, 0, 32767]].iter().flatten().flat_map(|c| c.to_le_bytes()).collect();
        let uv: Vec<u8> = [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0].iter().flat_map(|c| c.to_le_bytes()).collect();
        let frames: Vec<u8> = (0..3).flat_map(|_| [0u8, 127, 0, 0, 127, 0, 0, 127]).collect();
        let colors = vec![255u8; 12];
        let paint: Vec<u8> = (0..12).flat_map(|k| (k as f32 / 12.0).to_le_bytes()).collect();
        let indices: Vec<u8> = [0u16, 1, 2].iter().flat_map(|c| c.to_le_bytes()).collect();
        let transforms: Vec<u8> = [translation(10.0, 0.0, 0.0), translation(0.0, 0.0, -5.0)].iter().flatten().flat_map(|c| c.to_le_bytes()).collect();
        let p = put(&mut geometry, &positions);
        let u = put(&mut geometry, &uv);
        let l = put(&mut geometry, &uv);
        let f = put(&mut geometry, &frames);
        let c = put(&mut geometry, &colors);
        let pa = put(&mut geometry, &paint);
        let i = put(&mut geometry, &indices);
        let t = put(&mut geometry, &transforms);
        let bounds = Bounds { min: [0.0; 3], max: [1.0, 1.0, 0.0] };
        let draw = Draw {
            name: "triangle".into(),
            model: translation(1.0, 2.0, 3.0),
            vertex_count: 3,
            positions: p,
            uv0: u,
            uv1: Some(l),
            lightmap: Some(LightmapUv { uv: l, from: UvSet::Texcoord2 }),
            frames: f,
            colors: Some(c),
            paint: Some(pa),
            indices: i,
            index_format: IndexFormat::U16,
            index_count: 3,
            instances: None,
            material: 0,
            bounds,
        };
        let instanced = Draw { name: "instanced".into(), uv1: None, lightmap: None, colors: None, paint: None, instances: Some(Instances { count: 2, transforms: t }), material: 1, ..draw.clone() };
        let (base_mips, base_len) = mip_chain(TextureFormat::Bc7, 16, 8, 3, 1);
        let (light_mips, light_len) = mip_chain(TextureFormat::Bc6h, 8, 8, 2, 1);
        let (height_mips, height_len) = mip_chain(TextureFormat::Bc4, 8, 8, 1, 1);
        let (sky_mips, sky_len) = mip_chain(TextureFormat::Bc6h, 4, 4, 1, 6);
        let second = align_up(light_len, u64::from(ROW_ALIGN));
        let third = second + align_up(height_len, u64::from(ROW_ALIGN));
        let chunk1 = third + sky_len;
        let chunks = vec![Chunk { offset: 0, len: align_up(base_len, CHUNK_ALIGN) }, Chunk { offset: CHUNK_ALIGN, len: align_up(chunk1, CHUNK_ALIGN) }];
        let shift = |mips: Vec<Mip>, by: u64| mips.into_iter().map(|m| Mip { offset: m.offset + by, ..m }).collect::<Vec<_>>();
        let mut textures_bytes = vec![0u8; (chunks[1].offset + chunks[1].len) as usize];
        for (k, b) in textures_bytes.iter_mut().enumerate() {
            *b = (k % 253) as u8;
        }
        let texture = |name: &str, format, srgb, semantic, (width, height, layers), chunk, mips, source| Texture { name: name.into(), format, srgb, semantic, width, height, layers, volume: false, chunk, mips, source };
        let vpk = |entry: &str| TextureSource::MapVpk { vpk: "maps/de_test.vpk".into(), entry: entry.into() };
        let environment = Environment { layers: vec![layer(Some(0), 1), layer(Some(0), 2)], new_blending: true, tint: [1.0, 0.5, 0.25], tint_amount: 1.0 };
        let effects = Effects { additive: true, masks: [Some([1.0, 1.0, 0.1, 0.0]), None, None], color_boost: 2.0, opacity: 1.0, fade: [10.0, 1.0, 0.0, 1.0], fresnel: [0.001, 1.0, 0.0, 1.0], feather: [0.5, 1.0] };
        let skybox_draw = Draw { name: "sky island".into(), material: 2, ..draw.clone() };
        let scene = Scene {
            draws: vec![draw, instanced],
            materials: vec![
                Material {
                    name: "wall".into(),
                    shader: "csgo_environment_blend.vfx".into(),
                    shading: Shading::Environment(environment),
                    base_color: Some(0),
                    base_color_factor: [1.0; 4],
                    normal: None,
                    occlusion: None,
                    alpha: AlphaMode::Opaque,
                    double_sided: false,
                },
                Material { name: "leaves".into(), shader: "csgo_foliage.vfx".into(), shading: Shading::Generic, base_color: None, base_color_factor: [0.5, 0.5, 0.5, 1.0], normal: None, occlusion: Some(2), alpha: AlphaMode::Mask { cutoff: 0.5 }, double_sided: true },
                Material { name: "shaft".into(), shader: "csgo_effects.vfx".into(), shading: Shading::Effects(effects), base_color: Some(0), base_color_factor: [1.0; 4], normal: None, occlusion: None, alpha: AlphaMode::Blend, double_sided: true },
            ],
            textures: vec![
                texture("wall_color.vtex", TextureFormat::Bc7, true, Semantic::Color, (16, 8, 1), 0, base_mips, TextureSource::ByName { file: "materials/wall_color.vtex_c".into(), first_mip: 1 }),
                texture("irradiance", TextureFormat::Bc6h, false, Semantic::Irradiance, (8, 8, 1), 1, light_mips, vpk("maps/de_test/lightmaps/irradiance.vtex_c")),
                texture("wall_height.vtex", TextureFormat::Bc4, false, Semantic::Data, (8, 8, 1), 1, shift(height_mips, second), TextureSource::ByTextureParams { param: "g_tHeight1".into(), file: "materials/wall_height.vtex".into(), first_mip: 0 }),
                texture("sky", TextureFormat::Bc6h, false, Semantic::Sky, (4, 4, 6), 1, shift(sky_mips, third), TextureSource::ByTextureParams { param: "g_tSkyTexture".into(), file: "materials/sky.vtex".into(), first_mip: 0 }),
            ],
            chunks,
            lightmaps: Some(Lightmaps { irradiance: 1, directional: None, shadows: None, sun_channel: None }),
            probes: None,
            sun: Some(Sun { direction: [0.0, 1.0, 0.0], color: [1.9, 1.7, 1.4] }),
            sky: Some(Sky { texture: 3, yaw: 116.0, tint: [4.0, 4.0, 4.0] }),
            skybox: Some(Skybox { draws: vec![skybox_draw], lightmaps: None, scale: 16.0, origin: [0.0; 3], reference: [1.0, 0.0, 0.0], gradient_fog: None }),
            post: Post {
                tonemap: Some(Tonemap::DEFAULT),
                exposure: Some(Exposure { enabled: true, min: 1.0, max: 1.2, compensation: 0.0 }),
                bloom: Some(Bloom { mode: BloomMode::Screen, strength: 0.0, screen: 0.03, blur: 0.0, threshold: 4.0, threshold_width: 10.0, opacity: 1.0 }),
                lut: Some(Lut { dim: 2, weight: 1.0, rgba: vec![128; 32] }),
                fog: None,
                gradient_fog: Some(GradientFog { start: 25.4, end: 635.0, falloff: 1.5, height: false, height_start: 40.0, height_end: 1000.0, vertical_exponent: 1.5, color: [0.4, 0.5, 0.6], max_opacity: 1.0 }),
            },
            stats: Stats { draws: 2, by_name: 1, ..Stats::default() },
        };
        (scene, geometry, textures_bytes)
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nades-scene-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("map.pack")
    }

    #[test]
    fn a_scene_comes_back_from_a_pack_with_its_spans_and_mips() {
        let (scene, geometry, textures) = sample();
        let path = scratch("round-trip");
        let source = Source { map: "de_test".into(), game_build: "1".into(), export_hash: "abc".into() };
        write(&path, source, &[(SectionKind::Scene, &scene.to_bytes()), (SectionKind::Geometry, &geometry), (SectionKind::Textures, &textures)]).unwrap();
        let pack = Pack::open(&path).unwrap();
        let back = pack.scene().unwrap();
        assert_eq!(back, scene);
        let draw = &back.draws[0];
        assert_eq!(draw.indices.of(pack.geometry()), &[0, 0, 1, 0, 2, 0]);
        assert_eq!(draw.positions.of(pack.geometry()).len(), 24);
        assert_eq!(draw.frames.of(pack.geometry())[..8], [0, 127, 0, 0, 127, 0, 0, 127]);
        assert_eq!(back.mip_bytes(pack.textures(), 1, 1), &textures[CHUNK_ALIGN as usize + 512..CHUNK_ALIGN as usize + 768]);
        // a cube map's mip holds its six faces: six blocks, each row padded to 256 bytes
        assert_eq!(back.mip_bytes(pack.textures(), 3, 0).len(), 6 * 256);
        assert_eq!(back.chunk_bytes(pack.textures(), 0).len() as u64, CHUNK_ALIGN);
        // world = model × instance: the instances' translations add to the model's
        assert_eq!(draw.world_transforms(pack.geometry()), vec![translation(1.0, 2.0, 3.0)]);
        assert_eq!(back.draws[1].world_transforms(pack.geometry()), vec![translation(11.0, 2.0, 3.0), translation(1.0, 2.0, -2.0)]);
        assert_eq!((back.draws[1].instance_count(), back.draws[1].triangles()), (2, 2));
        assert_eq!((back.chunk_textures(0), back.chunk_textures(1), back.chunk_textures(2)), (0..1, 1..4, 4..4));
        assert_eq!(back.all_draws().count(), 3);
        assert_eq!(back.materials[0].textures(), vec![0, 0, 2, 0, 2]);
    }

    /// The Scene section is postcard: a change to any of its types changes the bytes a pack holds. This hash of the
    /// sample scene's bytes moves with them, and when it does, `VERSION` must move too, or old packs would be misread.
    #[test]
    fn the_scene_layout_is_format_3() {
        let bytes = sample().0.to_bytes();
        let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        assert_eq!(
            (crate::VERSION, bytes.len(), hash),
            (3, 1543, 0x58bd_b9d2_4189_8707),
            "the Scene section's layout changed: bump nades_pack::VERSION, then update the length and hash here"
        );
    }

    #[test]
    fn a_scene_that_breaks_an_invariant_is_refused() {
        let (good, geometry, textures) = sample();
        let (g, t) = (geometry.len() as u64, textures.len() as u64);
        assert_eq!(good.validate(g, t), Ok(()));
        type Breaks = Box<dyn Fn(&mut Scene)>;
        let broken: Vec<(&str, Breaks)> = vec![
            ("a missing material", Box::new(|s| s.draws[0].material = 7)),
            ("a span past the section", Box::new(|s| s.draws[0].indices.offset = 1 << 20)),
            ("an unaligned span", Box::new(|s| s.draws[0].uv0.offset += 8)),
            ("streams of different lengths", Box::new(|s| s.draws[0].vertex_count = 4)),
            ("a short paint stream", Box::new(|s| s.draws[0].paint.as_mut().unwrap().len = 12)),
            ("a short frame stream", Box::new(|s| s.draws[0].frames.len = 16)),
            ("a partial triangle", Box::new(|s| s.draws[0].index_count = 2)),
            ("a short instance table", Box::new(|s| s.draws[1].instances.as_mut().unwrap().count = 3)),
            ("a skybox draw with a missing material", Box::new(|s| s.skybox.as_mut().unwrap().draws[0].material = 9)),
            ("a missing texture", Box::new(|s| s.materials[0].base_color = Some(9))),
            ("a missing layer texture", Box::new(|s| {
                if let Shading::Environment(e) = &mut s.materials[0].shading {
                    e.layers[1].height = Some(9);
                }
            })),
            ("four layers", Box::new(|s| {
                if let Shading::Environment(e) = &mut s.materials[0].shading {
                    let extra = e.layers[0].clone();
                    e.layers.extend([extra.clone(), extra]);
                }
            })),
            ("an sRGB lightmap", Box::new(|s| s.textures[1].srgb = true)),
            ("a lightmap that is not BC6H", Box::new(|s| s.lightmaps = Some(Lightmaps { irradiance: 0, directional: None, shadows: None, sun_channel: None }))),
            ("shadows that are not a mask", Box::new(|s| s.lightmaps.as_mut().unwrap().shadows = Some(1))),
            ("the sun's shadow past the mask's channels", Box::new(|s| {
                let l = s.lightmaps.as_mut().unwrap();
                l.shadows = Some(2);
                l.sun_channel = Some(1);
            })),
            ("a sky that is not a cube", Box::new(|s| s.sky.as_mut().unwrap().texture = 1)),
            ("a cube that is not square", Box::new(|s| s.textures[3].width = 8)),
            ("rows that are not 256-aligned", Box::new(|s| s.textures[0].mips[0].bytes_per_row = 300)),
            ("a mip of the wrong size", Box::new(|s| s.textures[0].mips[1].width = 16)),
            ("a mip of the wrong layer count", Box::new(|s| s.textures[3].mips[0].layers = 1)),
            ("a BC texture of partial blocks", Box::new(|s| s.textures[0].width = 18)),
            ("a mip past its chunk", Box::new(|s| s.textures[0].mips[2].offset = CHUNK_ALIGN)),
            ("an unaligned chunk", Box::new(|s| s.chunks[1].offset = 100)),
            ("textures out of chunk order", Box::new(|s| s.textures.swap(0, 1))),
            ("a lookup table of the wrong size", Box::new(|s| s.post.lut.as_mut().unwrap().dim = 3)),
            ("a skybox of no scale", Box::new(|s| s.skybox.as_mut().unwrap().scale = 0.0)),
        ];
        for (what, breaks) in broken {
            let mut scene = good.clone();
            breaks(&mut scene);
            assert!(scene.validate(g, t).is_err(), "{what} passed validation");
        }
        // and through the pack, as an error rather than a panic
        let path = scratch("refused");
        let mut scene = good.clone();
        scene.draws[0].material = 7;
        let source = Source { map: "de_test".into(), game_build: "1".into(), export_hash: "abc".into() };
        write(&path, source, &[(SectionKind::Scene, &scene.to_bytes()), (SectionKind::Geometry, &geometry), (SectionKind::Textures, &textures)]).unwrap();
        assert!(matches!(Pack::open(&path).unwrap().scene(), Err(PackError::Scene(_))));
    }

    #[test]
    fn mip_chains_pad_rows_to_256_bytes_and_round_small_mips_up_to_a_block() {
        // 1024x512 BC7: 256 blocks of 16 bytes a row (4096, aligned), down to one block padded to 256 bytes
        let (mips, total) = mip_chain(TextureFormat::Bc7, 1024, 512, 11, 1);
        assert_eq!((mips[0].width, mips[0].height, mips[0].bytes_per_row, mips[0].rows), (1024, 512, 4096, 128));
        assert_eq!((mips[10].width, mips[10].height, mips[10].bytes_per_row, mips[10].rows), (1, 1, 256, 1));
        assert_eq!(mips[10].copy_size(TextureFormat::Bc7), (4, 4));
        assert_eq!(mips[1].offset, 4096 * 128);
        assert!(mips.iter().all(|m| m.offset % 256 == 0));
        assert_eq!(total, mips[10].offset + 256);
        // RGBA8 rows are texels: 5 texels are 20 bytes, padded to 256
        let (rgba, _) = mip_chain(TextureFormat::Rgba8, 5, 3, 3, 1);
        assert_eq!((rgba[0].bytes_per_row, rgba[0].rows, rgba[2].width, rgba[2].height), (256, 3, 1, 1));
        assert_eq!(rgba[0].copy_size(TextureFormat::Rgba8), (5, 3));
        // a cube's mip is six faces: 16x16 BC6H is 4 rows of 64 bytes (256 padded) a face
        let (cube, total) = mip_chain(TextureFormat::Bc6h, 16, 16, 2, 6);
        assert_eq!((cube[0].len(), cube[1].offset, total), (6 * 4 * 256, 6 * 4 * 256, 6 * 4 * 256 + 6 * 2 * 256));
    }
}
