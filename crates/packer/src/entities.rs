//! The map's entities, from the entity lump Source 2 Viewer's CLI decompiles from the map's VPK ([`crate::lump`],
//! `maps/<map>/entities/default_ents.vents`): blocks of `key value` lines between `====N====` lines. The packer reads the sun's baked shadow channel and the light probe volumes from them.

use std::collections::HashMap;

use glam::{Mat4, Vec3};
use nades_pack::scene::{Bounds, ProbeVolume};

use crate::materials::INCH;

/// One entity: its keys and values (quotes and `resource_name:` prefixes taken off).
pub type Entity = HashMap<String, String>;

/// Every entity of a lump's text.
pub fn parse(text: &str) -> Vec<Entity> {
    text.split("\n====")
        .map(|block| {
            block
                .lines()
                .filter_map(|line| {
                    let mut words = line.trim().splitn(2, char::is_whitespace);
                    let key = words.next()?.trim();
                    if key.is_empty() || key.starts_with("====") {
                        return None;
                    }
                    let value = words.next().unwrap_or("").trim();
                    let value = value.strip_prefix("resource_name:").unwrap_or(value).trim_matches('"');
                    Some((key.to_string(), value.to_string()))
                })
                .collect::<Entity>()
        })
        .filter(|e: &Entity| e.contains_key("classname"))
        .collect()
}

fn numbers(value: Option<&String>) -> Vec<f32> {
    value.map_or_else(Vec::new, |v| v.trim_matches(|c| c == '[' || c == ']').split([' ', ',']).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect())
}

fn vector(e: &Entity, key: &str) -> Option<Vec3> {
    let n = numbers(e.get(key));
    (n.len() >= 3).then(|| Vec3::new(n[0], n[1], n[2]))
}

fn number(e: &Entity, key: &str) -> Option<f32> {
    e.get(key)?.trim().parse().ok()
}

/// The sun's channel in the baked shadow mask: its light_environment's `bakedshadowindex` (0 when unset). None when
/// there is no light_environment.
pub fn sun_shadow_index(entities: &[Entity]) -> Option<i32> {
    let sun = entities.iter().find(|e| e.get("classname").map(String::as_str) == Some("light_environment"))?;
    Some(sun.get("bakedshadowindex").and_then(|v| v.trim().parse().ok()).unwrap_or(0))
}

/// Source's angles (pitch, yaw, roll, degrees) as a rotation: yaw about z, then pitch about y, then roll about x.
fn rotation(angles: Vec3) -> Mat4 {
    let [pitch, yaw, roll] = angles.to_array().map(f32::to_radians);
    Mat4::from_rotation_z(yaw) * Mat4::from_rotation_y(pitch) * Mat4::from_rotation_x(roll)
}

/// Source (Hammer units, z up) from the scene's frame (metres, y up): source = (z, x, y) / 0.0254.
fn source_from_scene() -> Mat4 {
    Mat4::from_cols_array(&[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]) * Mat4::from_scale(Vec3::splat(1.0 / INCH))
}

/// The light probe volumes (env_combined_light_probe_volume, env_light_probe_volume) that sample the atlas whose
/// probe grid is `atlas` probes (width, height, depth of one slab), by priority. Each is placed as Source 2 Viewer's
/// `SceneLightProbe.CalculateGpuProbeData` places it.
pub fn probe_volumes(entities: &[Entity], atlas: Vec3) -> Vec<ProbeVolume> {
    let mut volumes: Vec<(i32, f32, ProbeVolume)> = Vec::new();
    for e in entities {
        let class = e.get("classname").map(String::as_str);
        if !matches!(class, Some("env_combined_light_probe_volume" | "env_light_probe_volume")) || e.get("lightprobetexture").is_none_or(String::is_empty) {
            continue;
        }
        let (Some(min), Some(max)) = (vector(e, "box_mins"), vector(e, "box_maxs")) else { continue };
        let size = (max - min).max(Vec3::splat(1e-3));
        let origin = vector(e, "origin").unwrap_or(Vec3::ZERO);
        let angles = vector(e, "angles").unwrap_or(Vec3::ZERO);
        let probes = Vec3::new(number(e, "light_probe_size_x").unwrap_or(1.0), number(e, "light_probe_size_y").unwrap_or(1.0), number(e, "light_probe_size_z").unwrap_or(1.0)).max(Vec3::ONE);
        let offset = Vec3::new(number(e, "light_probe_atlas_x").unwrap_or(0.0), number(e, "light_probe_atlas_y").unwrap_or(0.0), number(e, "light_probe_atlas_z").unwrap_or(0.0));
        let fade = vector(e, "edge_fade_dists").unwrap_or(Vec3::ZERO);
        let priority = number(e, "indoor_outdoor_level").map_or(0, |p| p as i32);
        // world (Source) → the entity's frame → the box normalized to [0, 1]³
        let to_entity = (Mat4::from_translation(origin) * rotation(angles)).inverse();
        let to_box = Mat4::from_scale(size.recip()) * Mat4::from_translation(-min);
        let to_local = to_box * to_entity * source_from_scene();
        let depth = Vec3::new(1.0, 1.0, 1.0 / 6.0);
        let inverse = |d: f32| if d > 0.0 { 1.0 / d } else { f32::MAX };
        let edge = fade / size;
        // the box's corners in the scene's frame, for its bounds
        let to_scene = (to_entity * source_from_scene()).inverse();
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for k in 0..8 {
            let c = Vec3::new(if k & 1 == 0 { min.x } else { max.x }, if k & 2 == 0 { min.y } else { max.y }, if k & 4 == 0 { min.z } else { max.z });
            let s = to_scene.transform_point3(c);
            lo = lo.min(s);
            hi = hi.max(s);
        }
        let volume = ProbeVolume {
            to_local: to_local.to_cols_array(),
            atlas_scale: (probes / atlas).to_array(),
            atlas_offset: (offset / atlas).to_array(),
            border_min: (Vec3::splat(0.5) * depth / probes).to_array(),
            border_max: ((probes - 0.5) * depth / probes).to_array(),
            inverse_edge: [inverse(edge.x), inverse(edge.y), inverse(edge.z)],
            bounds_extend: (Vec3::splat(0.1) / size).to_array(),
            priority,
            bounds: Bounds { min: lo.to_array(), max: hi.to_array() },
        };
        volumes.push((priority, size.x * size.y * size.z, volume));
    }
    // a higher indoor_outdoor_level first, then a smaller volume
    volumes.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.total_cmp(&b.1)));
    volumes.into_iter().map(|(_, _, v)| v).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LUMP: &str = "====1====\nclassname                      \"env_wind\"\nangles [ 0.0, 0.0, 0.0 ]\n====2====\nbakedshadowindex               \"2\"\nbrightness 1.9\nclassname                      \"light_environment\"\n====3====\nlightprobetexture              resource_name:\"maps/x/lightmaps/env_light_probe_volume_atlas.vtex\"\nedge_fade_dists                [ 32.0, 32.0, 0.0 ]\nindoor_outdoor_level           -1\nbox_maxs                       [ 100.0, 50.0, 40.0 ]\nbox_mins                       [ -100.0, -50.0, 0.0 ]\nclassname                      \"env_combined_light_probe_volume\"\norigin                         [ 1000.0, 0.0, 100.0 ]\nangles                         [ 0.0, 0.0, 0.0 ]\nlight_probe_size_x             20\nlight_probe_size_y             10\nlight_probe_size_z             4\nlight_probe_atlas_x            5\nlight_probe_atlas_y            0\nlight_probe_atlas_z            2\n";

    #[test]
    fn the_suns_shadow_channel_is_its_light_environments_baked_shadow_index() {
        assert_eq!(sun_shadow_index(&parse(LUMP)), Some(2));
        assert_eq!(sun_shadow_index(&parse("====1====\nclassname \"light_environment\"\n")), Some(0), "unset: the first baked light");
        assert_eq!(sun_shadow_index(&parse("====1====\nclassname \"env_sky\"\n")), None);
    }

    #[test]
    fn a_probe_volume_maps_its_box_onto_the_unit_cube_and_its_probes_into_the_atlas() {
        let entities = parse(LUMP);
        let volumes = probe_volumes(&entities, Vec3::new(100.0, 50.0, 10.0));
        assert_eq!(volumes.len(), 1);
        let v = &volumes[0];
        let to_local = Mat4::from_cols_array(&v.to_local);
        // the box's minimum corner, Source (900, -50, 100), is the scene's (-50, 100, 900) × 0.0254
        let corner = Vec3::new(-50.0, 100.0, 900.0) * INCH;
        assert!(to_local.transform_point3(corner).length() < 1e-4, "{}", to_local.transform_point3(corner));
        let far = Vec3::new(50.0, 140.0, 1100.0) * INCH;
        assert!((to_local.transform_point3(far) - Vec3::ONE).length() < 1e-4);
        assert_eq!(v.atlas_scale, [0.2, 0.2, 0.4]);
        assert_eq!(v.atlas_offset, [0.05, 0.0, 0.2]);
        assert!((v.border_min[2] - 0.5 / 6.0 / 4.0).abs() < 1e-6);
        assert_eq!(v.inverse_edge[2], f32::MAX, "no fade along z");
        assert_eq!(v.priority, -1);
        assert!((v.bounds.min[1] - 100.0 * INCH).abs() < 1e-4 && (v.bounds.max[2] - 1100.0 * INCH).abs() < 1e-3);
    }
}
