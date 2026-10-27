//! The map's look after lighting and its sky, from the records [`crate::lump`] reads from the map's own files (as
//! show-me-the-nades' `export_map.py` wrote them beside the export): the post-processing (the post-processing volume,
//! its `.vpost`, the fog and sun entities), the colour lookup table (dim³ RGB bytes), the visible env_sky and where the
//! 3D skybox sits. Hammer units become the scene's metres here, and Source's z-up
//! frame glTF's y-up one: glTF (x, y, z) = Source (y, z, x) × 0.0254.

use anyhow::{Result, ensure};
use nades_pack::scene::{Bloom, BloomMode, CubemapFog, Exposure, GradientFog, Lut, Post, Tonemap};
use serde::Deserialize;

use crate::materials::INCH;

/// A Source position (Hammer units, z up) in the scene's frame.
pub fn scene_point(source: [f32; 3]) -> [f32; 3] {
    [source[1] * INCH, source[2] * INCH, source[0] * INCH]
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PostFile {
    pub tonemap: Option<TonemapFile>,
    pub bloom: Option<BloomFile>,
    pub lut: Option<LutFile>,
    pub exposure: Option<ExposureFile>,
    pub fog: Option<FogFile>,
    pub gradient_fog: Option<GradientFogFile>,
    pub sky: Option<SkyMaterialFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TonemapFile {
    pub exposure_bias: f32,
    pub shoulder_strength: f32,
    pub linear_strength: f32,
    pub linear_angle: f32,
    pub toe_strength: f32,
    pub toe_num: f32,
    pub toe_denom: f32,
    pub white_point: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BloomFile {
    pub mode: Option<String>,
    pub strength: f32,
    pub screen: f32,
    pub blur: f32,
    pub threshold: f32,
    pub threshold_width: f32,
    pub opacity: f32,
}

#[derive(Deserialize)]
pub struct LutFile {
    pub dim: u32,
    pub weight: f32,
}

#[derive(Deserialize)]
pub struct ExposureFile {
    pub enabled: bool,
    pub min: f32,
    pub max: f32,
    pub compensation: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FogFile {
    pub start: f32,
    pub end: f32,
    pub falloff: f32,
    pub height: bool,
    pub height_start: f32,
    pub height_end: f32,
    pub height_exponent: f32,
    pub max_opacity: f32,
    pub lod_bias: Option<f32>,
    pub sky: Option<FogSkyFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FogSkyFile {
    pub exposure_bias: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradientFogFile {
    pub start: f32,
    pub end: f32,
    pub falloff: f32,
    pub height: bool,
    pub height_start: f32,
    pub height_end: f32,
    pub vertical_exponent: f32,
    pub color: Vec<f32>,
    pub strength: f32,
    pub max_opacity: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkyMaterialFile {
    pub exposure_bias: f32,
    pub render_only_exposure_bias: f32,
    pub tint: Option<Vec<f32>>,
}

/// `<map>.sky.json`: the env_sky the game draws.
#[derive(Deserialize)]
pub struct SkyFile {
    pub yaw: f32,
    pub brightness: f32,
    pub material: String,
}

/// `<map>.skybox.json`: the prefab and where it sits.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkyboxFile {
    pub prefab: String,
    pub scale: f32,
    pub origin: [f32; 3],
    pub reference: [f32; 3],
    pub gradient_fog: Option<GradientFogFile>,
}

/// env_gradient_fog in the scene's units: distances and heights in metres, its colour linear (Source 2 Viewer reads
/// entity colours as linear 0..1) times its strength.
pub fn gradient_fog(f: &GradientFogFile) -> GradientFog {
    let channel = |k: usize| f.color.get(k).copied().unwrap_or(255.0) / 255.0 * f.strength;
    GradientFog {
        start: f.start * INCH,
        end: f.end * INCH,
        falloff: f.falloff,
        height: f.height,
        height_start: f.height_start * INCH,
        height_end: f.height_end * INCH,
        vertical_exponent: f.vertical_exponent,
        color: [channel(0), channel(1), channel(2)],
        max_opacity: f.max_opacity,
    }
}

/// The post-processing records, and the lookup table from `lut` (dim³ RGB bytes, red fastest) when the file has one.
pub fn post(file: &PostFile, lut: Option<&[u8]>) -> Result<Post> {
    let tonemap = file.tonemap.as_ref().map(|t| Tonemap {
        exposure_bias: t.exposure_bias,
        shoulder_strength: t.shoulder_strength,
        linear_strength: t.linear_strength,
        linear_angle: t.linear_angle,
        toe_strength: t.toe_strength,
        toe_num: t.toe_num,
        toe_denom: t.toe_denom,
        white_point: t.white_point,
    });
    let bloom = file.bloom.as_ref().map(|b| Bloom {
        mode: match b.mode.as_deref() {
            Some("BLOOM_BLEND_SCREEN") => BloomMode::Screen,
            Some("BLOOM_BLEND_BLUR") => BloomMode::Blur,
            _ => BloomMode::Add,
        },
        strength: b.strength,
        screen: b.screen,
        blur: b.blur,
        threshold: b.threshold,
        threshold_width: b.threshold_width,
        opacity: b.opacity,
    });
    let lut = match (&file.lut, lut) {
        (Some(info), Some(bytes)) => {
            let texels = (info.dim as usize).pow(3);
            ensure!(bytes.len() == texels * 3, "the lookup table holds {} bytes, {} expected for {}³ RGB", bytes.len(), texels * 3, info.dim);
            Some(Lut { dim: info.dim, weight: info.weight, rgba: bytes.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect() })
        }
        _ => None,
    };
    let fog = file.fog.as_ref().map(|f| CubemapFog {
        start: f.start * INCH,
        end: f.end * INCH,
        falloff: f.falloff,
        height: f.height,
        height_start: f.height_start * INCH,
        height_end: f.height_end * INCH,
        height_exponent: f.height_exponent,
        max_opacity: f.max_opacity,
        lod_bias: f.lod_bias.unwrap_or(0.0),
        sky_scale: 2f32.powf(f.sky.as_ref().map_or(0.0, |s| s.exposure_bias)),
    });
    Ok(Post {
        tonemap,
        exposure: file.exposure.as_ref().map(|e| Exposure { enabled: e.enabled, min: e.min, max: e.max, compensation: e.compensation }),
        bloom,
        lut,
        fog,
        gradient_fog: file.gradient_fog.as_ref().map(gradient_fog),
    })
}

/// The sky's tint: env_sky's tint × its brightness × 2^(the sky material's exposure biases), linear.
pub fn sky_tint(sky: &SkyFile, material: Option<&SkyMaterialFile>) -> [f32; 3] {
    let stops = material.map_or(0.0, |m| m.exposure_bias + m.render_only_exposure_bias);
    let scale = sky.brightness * 2f32.powf(stops);
    let tint = material.and_then(|m| m.tint.clone()).unwrap_or_else(|| vec![255.0; 3]);
    std::array::from_fn(|k| tint.get(k).copied().unwrap_or(255.0) / 255.0 * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_points_turn_into_the_scenes_frame_and_metres() {
        assert_eq!(scene_point([100.0, 0.0, 0.0]), [0.0, 0.0, 2.54]);
        assert_eq!(scene_point([0.0, 100.0, 0.0]), [2.54, 0.0, 0.0]);
        assert_eq!(scene_point([0.0, 0.0, 100.0]), [0.0, 2.54, 0.0]);
    }

    #[test]
    fn de_caches_post_processing_reads_into_scene_units() {
        let json = r#"{"tonemap":{"exposureBias":-0.1,"shoulderStrength":0.15,"linearStrength":0.5,"linearAngle":0.1,"toeStrength":0.2,"toeNum":0.02,"toeDenom":0.3,"whitePoint":4.0,"opacity":1.0},
            "bloom":{"mode":"BLOOM_BLEND_SCREEN","strength":0.0,"screen":0.03,"blur":0.0,"threshold":4.03,"thresholdWidth":9.999,"opacity":1.0},
            "lut":{"dim":2,"weight":1.0},"exposure":{"enabled":true,"min":1.0,"max":1.2,"compensation":0.0},
            "fog":{"start":256.0,"end":17636.2,"falloff":1.4,"height":true,"heightStart":1900.0,"heightEnd":6285.7,"heightExponent":1.0,"maxOpacity":1.0,"lodBias":0.5,"sky":{"material":"m","brightness":4.0,"exposureBias":2.0}},
            "gradientFog":{"start":1000.0,"end":25000.0,"falloff":1.5,"height":false,"heightStart":1600.0,"heightEnd":40000.0,"verticalExponent":1.5,"color":[110.0,136.0,160.0],"strength":1.0,"maxOpacity":1.0},
            "sun":null,"sky":{"material":"m","exposureBias":1.0,"renderOnlyExposureBias":0.0,"tint":[255.0,255.0,255.0]}}"#;
        let file: PostFile = serde_json::from_str(json).unwrap();
        let lut: Vec<u8> = (0..24).collect();
        let post = post(&file, Some(&lut)).unwrap();
        let fog = post.fog.unwrap();
        assert!((fog.start - 6.5024).abs() < 1e-4 && (fog.height_start - 48.26).abs() < 1e-3 && fog.sky_scale == 4.0);
        let gradient = post.gradient_fog.unwrap();
        assert!((gradient.color[0] - 110.0 / 255.0).abs() < 1e-6 && (gradient.end - 635.0).abs() < 1e-3);
        assert_eq!(post.bloom.unwrap().mode, BloomMode::Screen);
        assert_eq!(post.lut.unwrap().rgba[..8], [0, 1, 2, 255, 3, 4, 5, 255]);
        assert!(super::post(&file, Some(&lut[..21])).is_err(), "a table of the wrong size");
        let sky = SkyFile { yaw: 116.0, brightness: 4.0, material: "m".into() };
        assert_eq!(sky_tint(&sky, file.sky.as_ref()), [8.0, 8.0, 8.0]);
    }
}
