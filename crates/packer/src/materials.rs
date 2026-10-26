//! A glTF material of the export as the pack's [`Material`]: CS2's shader family ([`Shading`]) with its parameters
//! resolved from `extras.vmat`, and the textures it wants, by the game path its `TextureParams` names (and the
//! export's image for the same slot, as a fallback).
//!
//! The parameter logic is show-me-the-nades' `web/app/src/scene/render/csgoMaterials.ts` and `csgoEffects.ts`,
//! themselves ports of Source 2 Viewer's renderer (MIT): `MatrixColorCorrect2` and `MatrixColorTint2` for the layers'
//! colour, `RotateVector2D` for their coordinates, and `csgo_effects`' fades. Texture conventions follow Source 2
//! Viewer's `csgo_environment.frag`: a layer's colour holds its occlusion in alpha, its height map the height, tint
//! mask and metalness, its normal map the hemi-octahedral normal and the roughness.

use glam::{Mat4, Vec3};
use nades_pack::scene::{AlphaMode, COLOR_IDENTITY, ColorMatrix, Effects, Environment, Facing, Layer, Layered, Material, Semantic, Shading, UvTransform};

use crate::geometry::Group;
use crate::gltf;

/// Scene units per Hammer unit: the export's metres.
pub const INCH: f32 = 0.0254;
/// The longest side a texture keeps, by what it holds: colour and normals as the export resized them, height maps
/// and masks (smooth) at half that.
pub const DETAIL_SIZE: u32 = 1024;
pub const DATA_SIZE: u32 = 512;

/// Which of a material's slots a texture fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    /// [`Material::base_color`]; an environment material's layer 1 colour.
    BaseColor,
    /// [`Material::normal`]; an environment material's layer 1 normal.
    Normal,
    Occlusion,
    /// An environment material's layer `n` (0-based) colour, normal and height. Layer 0's colour and normal are the
    /// material's own slots above.
    LayerColor(usize),
    LayerNormal(usize),
    LayerHeight(usize),
    /// A two-layer csgo_lightmappedgeneric's second colour and its modulation.
    Color2,
    Modulation,
}

/// A texture a material wants for one slot.
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    pub slot: Slot,
    /// The `TextureParams` entry, e.g. `g_tNormal1`.
    pub param: String,
    /// The game path it names (`materials/…/x.vtex`), when it names one.
    pub path: Option<String>,
    /// The export's image for the same slot, when it has one: used when the game's file cannot be read.
    pub image: Option<usize>,
    pub semantic: Semantic,
    pub srgb: bool,
    pub max_size: u32,
}

/// The material described, its texture slots still empty, with what fills them and where its draws go.
pub struct Described {
    pub material: Material,
    pub wants: Vec<Want>,
    pub group: Group,
}

/// Rec. 709 luminance weights normalised to unit length (Source 2 Viewer's VfxEvalFunctions).
fn luma() -> Vec3 {
    Vec3::new(0.2126, 0.7152, 0.0722).normalize()
}

/// The rotation taking the luminance axis onto z.
fn luma_rotation() -> Mat4 {
    let z = Vec3::Z;
    let cross = luma().cross(z);
    let angle = cross.length().atan2(luma().dot(z));
    Mat4::from_axis_angle(cross.normalize(), angle)
}

/// `steps` applied in order to a colour (a column vector): the last step's matrix leftmost.
fn chain(steps: &[Mat4]) -> Mat4 {
    steps.iter().fold(Mat4::IDENTITY, |m, step| *step * m)
}

/// MatrixColorCorrect2: contrast about the texture's average colour, then brightness, then saturation in a space where
/// luminance is one axis. That luminance weighs the channels by the unit Rec. 709 vector squared, since the colour is
/// scaled by it before being projected on it.
pub fn color_correct(contrast: f32, saturation: f32, brightness: f32, average: [f32; 3]) -> Mat4 {
    let average = Vec3::from_array(average);
    let l = luma();
    chain(&[
        Mat4::from_translation(-average),
        Mat4::from_scale(Vec3::splat(contrast)),
        Mat4::from_translation(average),
        Mat4::from_scale(Vec3::splat(brightness)),
        Mat4::from_scale(l),
        luma_rotation(),
        Mat4::from_scale(Vec3::new(saturation, saturation, 1.0)),
        luma_rotation().transpose(),
        Mat4::from_scale(l.recip()),
    ])
}

/// MatrixColorTint2 at full strength: moves a colour's chroma toward the tint's by the tint's saturation s and its
/// luminance toward the tint's by s², so a white tint changes nothing and a fully saturated one replaces the colour.
pub fn color_tint(rgb: [f32; 3]) -> Mat4 {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let saturation = if max == 0.0 { 0.0 } else { (max - r.min(g).min(b)) / max };
    let l = luma();
    let gray = luma_rotation().transform_point3(Vec3::new(r * l.x, g * l.y, b * l.z));
    let desaturate = 1.0 - saturation;
    let keep = 1.0 - saturation * saturation;
    chain(&[
        Mat4::from_scale(l),
        luma_rotation(),
        Mat4::from_translation(Vec3::new(-gray.x, -gray.y, 0.0)),
        Mat4::from_scale(Vec3::new(desaturate, desaturate, keep)),
        Mat4::from_translation(Vec3::new(gray.x, gray.y, (1.0 - keep) * gray.z)),
        luma_rotation().transpose(),
        Mat4::from_scale(l.recip()),
    ])
}

/// Layer `n`'s facing mask, when `F_BLEND_BY_FACING_DIRECTION_n` turns one on, as Source 2 Viewer's
/// csgo_environment.vert reads it. The game masks by the surface's normal ("Geometric", 1) or by the normal map's
/// ("Normal Map", 2); both are drawn here with the surface's normal, and Source 2 Viewer, which ignores mode 2 on layer
/// 3, would leave such a layer everywhere its paint is (de_ancient's top moss under its overhangs).
fn facing(vmat: &gltf::Vmat, n: usize) -> Option<Facing> {
    if vmat.int(&format!("F_BLEND_BY_FACING_DIRECTION_{n}")).unwrap_or(0) == 0 {
        return None;
    }
    let [x, y, z] = vmat.vector::<3>(&format!("g_vFacingDirection{n}"), 0.0).unwrap_or([0.0, 0.0, 1.0]);
    // Source's z up; a z of exactly 0 is nudged as the shader's g_vFacingDirectionNormalizedSafe does
    let source = Vec3::new(x, y, if z == 0.0 { 0.0001 } else { z }).normalize();
    let spread = vmat.float(&format!("g_flFacingDirectionMaskSpread{n}")).unwrap_or(0.5);
    let falloff = vmat.float(&format!("g_vFacingDirectionMaskFalloff{n}")).unwrap_or(0.1);
    let max = ((1.0 - spread + 0.001) + falloff).min(1.0);
    let min = ((1.0 - spread) - falloff).max(0.0).min(max - 0.001);
    Some(Facing { direction: [source.y, source.z, source.x], min, max })
}

/// A colour matrix's first three rows: what it does to (r, g, b, 1).
pub fn color_matrix(m: Mat4) -> ColorMatrix {
    let rows = m.transpose();
    [rows.x_axis.to_array(), rows.y_axis.to_array(), rows.z_axis.to_array()]
}

/// Source 2 Viewer's `RotateVector2D(uv, rotation, scale, offset, center)` as an affine transform:
/// `rotate(scale × (uv − center)) + center + offset`, the rotation in degrees.
pub fn uv_transform(scale: [f32; 2], offset: [f32; 2], center: [f32; 2], rotation_degrees: f32) -> UvTransform {
    let (s, c) = rotation_degrees.to_radians().sin_cos();
    let ([sx, sy], [ox, oy], [cx, cy]) = (scale, offset, center);
    [c * sx, -s * sy, -c * sx * cx + s * sy * cy + cx + ox, s * sx, c * sy, -s * sx * cx - c * sy * cy + cy + oy]
}

/// sRGB-encoded 0..1 to linear.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn alpha(m: &gltf::Material) -> AlphaMode {
    match m.alpha_mode.as_deref() {
        Some("MASK") => AlphaMode::Mask { cutoff: m.alpha_cutoff.unwrap_or(0.5) },
        Some("BLEND") => AlphaMode::Blend,
        _ => AlphaMode::Opaque,
    }
}

/// The image a glTF texture slot samples.
fn image(doc: &gltf::Document, info: Option<&gltf::TextureInfo>) -> Option<usize> {
    doc.textures.get(info?.index)?.image()
}

/// Whether a compiled mask is a constant the material set rather than a texture (Source 2 makes one for it).
fn constant_mask(vmat: &gltf::Vmat, slot: &str) -> bool {
    match vmat.texture(slot) {
        None => true,
        Some(name) => name.contains(&format!("_vmat_{}_", slot.to_lowercase())),
    }
}

/// Describes material `index` of `doc`. Its base colour is not among the wants: the packer finds it on its own, by
/// the image's name first, as M1 did.
pub fn describe(doc: &gltf::Document, index: usize) -> Described {
    let m = &doc.materials[index];
    let empty = gltf::Vmat::default();
    let vmat = m.vmat().unwrap_or(&empty);
    let shader = vmat.shader.clone().unwrap_or_default();
    let pbr = m.pbr_metallic_roughness.as_ref();
    let factor = pbr.and_then(|p| p.base_color_factor).unwrap_or([1.0; 4]);
    let int = |key: &str, default: i64| vmat.int(key).unwrap_or(default);
    let float = |key: &str, default: f32| vmat.float(key).unwrap_or(default);
    let mut wants: Vec<Want> = Vec::new();
    let mut want = |slot: Slot, param: &str, image: Option<usize>, semantic: Semantic, max_size: u32| {
        let path = vmat.texture(param).map(str::to_string);
        if path.is_some() || image.is_some() {
            wants.push(Want { slot, param: param.to_string(), path, image, semantic, srgb: semantic == Semantic::Color, max_size });
        }
    };
    let normal_image = image(doc, m.normal_texture.as_ref());
    let mut material = Material {
        name: m.name.clone().unwrap_or_else(|| format!("material {index}")),
        shader: shader.clone(),
        shading: Shading::Generic,
        base_color: None,
        base_color_factor: factor,
        normal: None,
        occlusion: None,
        alpha: alpha(m),
        double_sided: m.double_sided.unwrap_or(false),
    };
    let group = |material: &Material| match (&material.shading, material.alpha) {
        (Shading::Effects(_), _) => Group::Effects,
        (Shading::Unlit { additive: true }, _) | (_, AlphaMode::Blend) => Group::Blend,
        _ => Group::Opaque,
    };
    match shader.as_str() {
        "csgo_environment.vfx" | "csgo_environment_blend.vfx" => {
            let blend = shader == "csgo_environment_blend.vfx";
            let count = if !blend || vmat.texture("g_tColor2").is_none() {
                1
            } else if int("F_ENABLE_LAYER_3", 0) == 1 && vmat.texture("g_tColor3").is_some() {
                3
            } else {
                2
            };
            let tint = [factor[0], factor[1], factor[2]];
            let layers = (1..=count)
                .map(|n| {
                    let key = |name: &str| format!("{name}{n}");
                    let average = m.layer_average(n).unwrap_or([1.0; 3]);
                    let correct = color_correct(float(&key("g_fTextureColorContrast"), 1.0), float(&key("g_fTextureColorSaturation"), 1.0), float(&key("g_fTextureColorBrightness"), 1.0), average);
                    let tint_n = vmat.vector::<3>(&key("g_vTextureColorTint"), 1.0).unwrap_or([1.0; 3]);
                    let uv = uv_transform(
                        vmat.vector::<2>(&key("g_vTexCoordScale"), 1.0).unwrap_or([1.0; 2]),
                        vmat.vector::<2>(&key("g_vTexCoordOffset"), 0.0).unwrap_or([0.0; 2]),
                        vmat.vector::<2>(&key("g_vTexCoordCenter"), 0.5).unwrap_or([0.5; 2]),
                        float(&key("g_flTexCoordRotation"), 0.0),
                    );
                    Layer {
                        color: None,
                        normal: None,
                        height: None,
                        uv_set: if int(&key("g_nUVSet"), 1) == 2 { 2 } else { 1 },
                        uv,
                        adjust: if int(&key("g_nColorCorrectionMode"), 0) == 1 { color_matrix(correct) } else { COLOR_IDENTITY },
                        tint_adjust: color_matrix(correct * color_tint(tint_n)),
                        model_tint: int(&key("g_bModelTint"), 1) != 0,
                        tint_mask: [float(&key("g_fTintMaskContrast"), 1.0), float(&key("g_fTintMaskBrightness"), 1.0)],
                        height_scale: float(&key("g_flHeightMapScale"), 1.0),
                        height_zero: float(&key("g_flHeightMapZeroPoint"), 0.5),
                        softness: float(&key("g_flBlendSoftness"), 0.01),
                        underlying: float(&key("g_flUnderlyingHeightMapInfluence"), 1.0),
                        mask_with_height: float(&key("g_flMaskWithHeight"), 0.0),
                        normal_contrast: float(&key("g_fTextureNormalContrast"), 1.0),
                        roughness: [float(&key("g_fTextureRoughnessContrast"), 1.0), float(&key("g_fTextureRoughnessBrightness"), 1.0)],
                        metalness: int(&key("g_bMetalness"), 1) != 0,
                        vertex_color: int(&key("g_nVertexColorMode"), 0).clamp(0, 2) as u8,
                        facing: if n >= 2 { facing(vmat, n) } else { None },
                    }
                })
                .collect();
            material.shading = Shading::Environment(Environment {
                layers,
                new_blending: int("F_USE_NEW_BLENDING", 0) == 1,
                tint,
                tint_amount: float("g_flModelTintAmount", 1.0) * (1.0 - tint[0].min(tint[1]).min(tint[2])),
            });
            want(Slot::Normal, "g_tNormal1", normal_image, Semantic::NormalHemiOct, DETAIL_SIZE);
            want(Slot::LayerHeight(0), "g_tHeight1", None, Semantic::Data, DATA_SIZE);
            for n in 2..=count {
                want(Slot::LayerColor(n - 1), &format!("g_tColor{n}"), None, Semantic::Color, DETAIL_SIZE);
                want(Slot::LayerNormal(n - 1), &format!("g_tNormal{n}"), None, Semantic::NormalHemiOct, DETAIL_SIZE);
                want(Slot::LayerHeight(n - 1), &format!("g_tHeight{n}"), None, Semantic::Data, DATA_SIZE);
            }
        }
        "csgo_lightmappedgeneric.vfx" if int("F_LAYERS", 0) == 1 && vmat.texture("g_tLayer2Color").is_some() => {
            let tint = |key: &str| vmat.vector::<3>(key, 1.0).unwrap_or([1.0; 3]).map(srgb_to_linear);
            material.shading = Shading::Layered(Layered {
                color2: None,
                modulation: None,
                tint1: tint("g_vLayer1Tint"),
                tint2: tint("g_vLayer2Tint"),
                fancy: int("F_FANCY_BLENDING", 0).clamp(0, 3) as u8,
                softness: float("g_flBlendSoftness", 0.5),
                uv2_scale: vmat.vector::<2>("g_vTexCoordScale2", 1.0).unwrap_or([1.0; 2]),
            });
            want(Slot::Normal, "g_tNormal", normal_image, Semantic::NormalHemiOct, DETAIL_SIZE);
            want(Slot::Color2, "g_tLayer2Color", None, Semantic::Color, DETAIL_SIZE);
            want(Slot::Modulation, "g_tBlendModulation", None, Semantic::Data, DATA_SIZE);
        }
        "csgo_effects.vfx" => {
            let pan = |n: u32| -> Option<[f32; 4]> {
                let slot = format!("g_tMask{n}");
                if constant_mask(vmat, &slot) {
                    return None;
                }
                let [sx, sy] = vmat.vector::<2>(&format!("g_vMask{n}Scale"), 1.0).unwrap_or([1.0; 2]);
                let [px, py] = vmat.vector::<2>(&format!("g_vMask{n}PanSpeed"), 0.0).unwrap_or([0.0; 2]);
                Some([sx, sy, px, py])
            };
            material.shading = Shading::Effects(Effects {
                additive: int("F_ADDITIVE_BLEND", 0) == 1,
                masks: [pan(1), pan(2), pan(3)],
                color_boost: float("g_flColorBoost", 1.0),
                opacity: float("g_flOpacityScale", 1.0) * factor[3],
                fade: [float("g_flFadeDistance", 1.0).max(0.0001) * INCH, float("g_flFadeFalloff", 1.0), float("g_flFadeMin", 0.0), float("g_flFadeMax", 1.0)],
                fresnel: [float("g_flFresnelExponent", 0.001), float("g_flFresnelFalloff", 1.0), float("g_flFresnelMin", 0.0), float("g_flFresnelMax", 1.0)],
                feather: [if int("F_DEPTH_FEATHER", 0) == 1 { float("g_flFeatherDistance", 0.0).max(0.0) * INCH } else { 0.0 }, float("g_flFeatherFalloff", 1.0)],
            });
            material.alpha = AlphaMode::Blend;
            material.double_sided = int("F_RENDER_BACKFACES", 0) == 1;
        }
        "csgo_unlitgeneric.vfx" => {
            let additive = int("F_BLEND_MODE", 0) == 4 || int("F_ADDITIVE_BLEND", 0) == 1;
            if let Some(tint) = vmat.vector::<3>("g_vColorTint", 1.0) {
                let [r, g, b] = tint.map(srgb_to_linear);
                material.base_color_factor = [r, g, b, factor[3]];
            }
            material.shading = Shading::Unlit { additive };
        }
        "csgo_black_unlit.vfx" => material.shading = Shading::Black,
        _ => {
            want(Slot::Normal, "g_tNormal", normal_image, Semantic::NormalHemiOct, DETAIL_SIZE);
            want(Slot::Occlusion, "g_tAmbientOcclusion", image(doc, m.occlusion_texture.as_ref()), Semantic::Data, DATA_SIZE);
        }
    }
    let group = group(&material);
    Described { material, wants, group }
}

/// Puts texture `texture` in `material`'s `slot`.
pub fn fill(material: &mut Material, slot: Slot, texture: u32) {
    let t = Some(texture);
    match (slot, &mut material.shading) {
        (Slot::BaseColor, Shading::Environment(e)) => {
            material.base_color = t;
            e.layers[0].color = t;
        }
        (Slot::Normal, Shading::Environment(e)) => {
            material.normal = t;
            e.layers[0].normal = t;
        }
        (Slot::BaseColor, _) => material.base_color = t,
        (Slot::Normal, _) => material.normal = t,
        (Slot::Occlusion, _) => material.occlusion = t,
        (Slot::LayerColor(n), Shading::Environment(e)) => e.layers[n].color = t,
        (Slot::LayerNormal(n), Shading::Environment(e)) => e.layers[n].normal = t,
        (Slot::LayerHeight(n), Shading::Environment(e)) => e.layers[n].height = t,
        (Slot::Color2, Shading::Layered(l)) => l.color2 = t,
        (Slot::Modulation, Shading::Layered(l)) => l.modulation = t,
        _ => {}
    }
}

/// Counts of materials by family, in [`nades_pack::scene::Stats::shading`]'s order.
pub fn family(shading: &Shading) -> usize {
    match shading {
        Shading::Generic => 0,
        Shading::Environment(_) => 1,
        Shading::Layered(_) => 2,
        Shading::Effects(_) => 3,
        Shading::Unlit { .. } => 4,
        Shading::Black => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &ColorMatrix, c: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2] + m[i][3])
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn colour_correction_pivots_on_the_average_and_keeps_luminance_when_saturating() {
        let average = [0.4, 0.35, 0.3];
        // neutral parameters change nothing
        assert!(close(apply(&color_matrix(color_correct(1.0, 1.0, 1.0, average)), [0.2, 0.5, 0.7]), [0.2, 0.5, 0.7]));
        // contrast leaves the average where it is
        assert!(close(apply(&color_matrix(color_correct(3.0, 1.0, 1.0, average)), average), average));
        // brightness scales
        assert!(close(apply(&color_matrix(color_correct(1.0, 1.0, 0.5, average)), [0.2, 0.4, 0.6]), [0.1, 0.2, 0.3]));
        // no saturation: a grey of the colour's luminance, any saturation keeps it
        let luminance = |c: [f32; 3]| (0.2126f32.powi(2) * c[0] + 0.7152f32.powi(2) * c[1] + 0.0722f32.powi(2) * c[2]) / (0.2126f32.powi(2) + 0.7152f32.powi(2) + 0.0722f32.powi(2));
        let colour = [0.8, 0.2, 0.1];
        let y = luminance(colour);
        assert!(close(apply(&color_matrix(color_correct(1.0, 0.0, 1.0, average)), colour), [y, y, y]));
        assert!((luminance(apply(&color_matrix(color_correct(1.0, 1.7, 1.0, average)), colour)) - y).abs() < 1e-4);
    }

    #[test]
    fn a_white_tint_changes_nothing_a_pale_one_moves_the_hue_and_a_saturated_one_replaces_the_colour() {
        assert!(close(apply(&color_matrix(color_tint([1.0, 1.0, 1.0])), [0.3, 0.6, 0.2]), [0.3, 0.6, 0.2]));
        let grey = [0.5, 0.5, 0.5];
        let pale = apply(&color_matrix(color_tint([1.0, 0.5, 0.5])), grey);
        assert!(pale[0] > pale[1] && (pale[1] - pale[2]).abs() < 1e-4, "{pale:?}");
        assert!(close(apply(&color_matrix(color_tint([1.0, 0.0, 0.0])), grey), [1.0, 0.0, 0.0]));
    }

    #[test]
    fn a_facing_layer_keeps_to_surfaces_turned_toward_its_direction() {
        let vmat = |json: serde_json::Value| -> gltf::Vmat { serde_json::from_value(json).unwrap() };
        // de_ancient's top moss: up by default, spread 0.431 and falloff 0.331
        let moss = vmat(serde_json::json!({
            "IntParams": {"F_BLEND_BY_FACING_DIRECTION_3": 2},
            "FloatParams": {"g_flFacingDirectionMaskSpread3": 0.431, "g_vFacingDirectionMaskFalloff3": 0.331}
        }));
        let f = facing(&moss, 3).unwrap();
        assert!((Vec3::from_array(f.direction) - Vec3::Y).length() < 1e-3, "Source's up is the scene's y: {:?}", f.direction);
        assert!((f.min - 0.238).abs() < 1e-4 && (f.max - 0.901).abs() < 1e-4, "{f:?}");
        assert_eq!(facing(&moss, 2), None, "layer 2 has no mode");
        // a direction along Source's x is the scene's z; no spread or falloff still leaves a band to smoothstep over
        let sideways = vmat(serde_json::json!({
            "IntParams": {"F_BLEND_BY_FACING_DIRECTION_2": 1},
            "FloatParams": {"g_flFacingDirectionMaskSpread2": 0.0, "g_vFacingDirectionMaskFalloff2": 0.0},
            "VectorParams": {"g_vFacingDirection2": [1.0, 0.0, 0.0, 0.0]}
        }));
        let f = facing(&sideways, 2).unwrap();
        assert!((Vec3::from_array(f.direction) - Vec3::Z).length() < 1e-3 && f.min < f.max && f.max <= 1.0, "{f:?}");
    }

    #[test]
    fn coordinates_scale_about_the_centre_then_rotate_then_offset() {
        let apply = |t: UvTransform, (u, v): (f32, f32)| (t[0] * u + t[1] * v + t[2], t[3] * u + t[4] * v + t[5]);
        // identity
        let id = uv_transform([1.0; 2], [0.0; 2], [0.5; 2], 0.0);
        assert_eq!(apply(id, (0.25, 0.75)), (0.25, 0.75));
        // a scale of 2 about the centre: 0.75 → 1.0
        let scaled = uv_transform([2.0, 2.0], [0.0; 2], [0.5; 2], 0.0);
        assert_eq!(apply(scaled, (0.75, 0.5)), (1.0, 0.5));
        // 90° about the centre: (1, 0.5) → (0.5, 1), as Source 2 Viewer's RotateVector2D turns
        let (u, v) = apply(uv_transform([1.0; 2], [0.0; 2], [0.5; 2], 90.0), (1.0, 0.5));
        assert!((u - 0.5).abs() < 1e-6 && (v - 1.0).abs() < 1e-6, "({u}, {v})");
        // the offset comes last
        assert_eq!(apply(uv_transform([1.0; 2], [0.1, 0.2], [0.5; 2], 0.0), (0.0, 0.0)), (0.1, 0.2));
    }
}
