//! The packs and what the app reads from them besides the map: the map's collider (its pack's Collider section, made
//! from the map's own physics) and the game's smoke noise and fire flipbooks (the shared effects pack,
//! `<packs>/shared/effects.pack`). Both are built by `nades-pack` from the game's files; without them the result shot
//! has no line of sight and the smoke and fire draw the web view's stand-ins.
//!
//! The packs folder (docs/standalone-api.md §8): `--packs <dir>`, else `NADES_SANDBOX_PACKS`, else `[assets] packs` in
//! the config file, else `packs/` in the platform's cache folder (nades-config). `NADES_PACK` names one map's pack.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context as _, Result};
use nades_scene::collider::Collider;

static PACKS: OnceLock<PathBuf> = OnceLock::new();

/// Settles the packs folder for this run from `--packs`; the first call wins.
pub fn set_packs(flag: Option<&Path>) -> Result<()> {
    let setting = nades_config::packs(flag)?;
    let _ = PACKS.set(setting.path);
    Ok(())
}

/// The packs folder.
pub fn packs() -> &'static Path {
    PACKS.get_or_init(|| match nades_config::packs(None) {
        Ok(setting) => setting.path,
        Err(e) => {
            eprintln!("packs: {e:#}");
            PathBuf::from("packs")
        }
    })
}

/// The pack to draw: `NADES_PACK`, else `<packs>/<map>.pack`.
pub fn pack_path(map: Option<&str>) -> PathBuf {
    if let Some(path) = std::env::var_os("NADES_PACK") {
        return PathBuf::from(path);
    }
    packs().join(format!("{}.pack", map.unwrap_or("de_cache")))
}

/// The maps with packs in the packs folder.
pub fn packed_maps() -> Vec<String> {
    let mut maps: Vec<String> = std::fs::read_dir(packs())
        .map(|dir| dir.filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".pack").map(str::to_string)).collect())
        .unwrap_or_default();
    maps.sort();
    maps
}

/// `map`'s two colliders, from its pack's Collider and GrenadeCollider sections, the trees built side by side.
pub fn colliders(map: &str) -> Result<crate::collider::Colliders> {
    let path = pack_path(Some(map));
    let pack = nades_pack::Pack::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let no = |what: &str| format!("{} has no {what}: rebuild it with nades-pack {map} (it needs Source 2 Viewer's CLI)", path.display());
    let (sight, flight) = (pack.collider().with_context(|| no("collider"))?, pack.grenade_collider().with_context(|| no("grenade collider"))?);
    let (sight, flight) = rayon::join(|| Collider::from_awmh(sight), || Collider::from_awmh(flight));
    let not_a_mesh = |section: &str| format!("{}'s {section} section is not a triangle mesh", path.display());
    Ok(crate::collider::Colliders { sight: sight.with_context(|| not_a_mesh("Collider"))?, flight: flight.with_context(|| not_a_mesh("GrenadeCollider"))? })
}

/// What the camera sees of `map`, as a collider: every opaque or cut-out triangle its pack draws (not the 3D skybox),
/// in Hammer units. The map's physics leaves out props, trims and the walls of places players never reach, so a camera
/// kept clear of it alone still passes through what is drawn (S36).
pub fn sight(map: &str) -> Result<Collider> {
    let path = pack_path(Some(map));
    let pack = nades_pack::Pack::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let scene = pack.scene().with_context(|| format!("reading {}'s scene", path.display()))?;
    Ok(Collider::new(drawn_triangles(&scene, pack.geometry())))
}

/// The scene's opaque and cut-out triangles in Hammer units (glTF metres, y up → Source, z up).
pub(crate) fn drawn_triangles(scene: &nades_pack::scene::Scene, geometry: &[u8]) -> Vec<[[f32; 3]; 3]> {
    use nades_pack::scene::{AlphaMode, IndexFormat};
    use rayon::prelude::*;
    let inch = 1.0 / 0.0254_f32;
    scene
        .draws
        .par_iter()
        .filter(|d| scene.materials.get(d.material as usize).is_none_or(|m| m.alpha != AlphaMode::Blend))
        .flat_map_iter(|d| {
            let positions: Vec<glam::Vec3> = d
                .positions
                .of(geometry)
                .chunks_exact(8)
                .map(|v| {
                    let c = |k: usize| f32::from(i16::from_le_bytes([v[k], v[k + 1]])) / 32767.0;
                    glam::Vec3::new(c(0), c(2), c(4))
                })
                .collect();
            let bytes = d.indices.of(geometry);
            let indices: Vec<usize> = match d.index_format {
                IndexFormat::U16 => bytes.chunks_exact(2).map(|b| usize::from(u16::from_le_bytes([b[0], b[1]]))).collect(),
                IndexFormat::U32 => bytes.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).collect(),
            };
            d.world_transforms(geometry)
                .into_iter()
                .flat_map(move |m| {
                    let m = glam::Mat4::from_cols_array(&m);
                    let world: Vec<[f32; 3]> = positions.iter().map(|p| {
                        let g = m.transform_point3(*p);
                        [g.z * inch, g.x * inch, g.y * inch]
                    }).collect();
                    indices.chunks_exact(3).filter_map(|t| Some([*world.get(t[0])?, *world.get(t[1])?, *world.get(t[2])?])).collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The game's smoke noise (or the stand-in, and false) and fire flipbooks (or none), from the effects pack.
pub fn effects() -> (Vec<u8>, bool, Option<nades_render::throw::Flipbooks>) {
    let path = nades_config::effects_pack(packs());
    let read = nades_pack::Pack::open(&path).map_err(anyhow::Error::from).and_then(|pack| Ok(pack.effects()?));
    match read {
        Ok(effects) => {
            let books = nades_render::throw::Flipbooks::from_effects(&effects);
            (effects.noise, true, Some(books))
        }
        Err(e) => {
            eprintln!("effects: {} ({e:#}; nades-pack --effects builds it): the stand-in noise, no flipbooks", path.display());
            (nades_scene::noise::stand_in(nades_scene::noise::SIZE, 7), false, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The local packs: every packed map's collider builds, and the effects pack reads.
    #[test]
    #[ignore = "needs the packs built (nades-pack <map>, nades-pack --effects)"]
    fn every_packed_maps_collider_and_the_effects_read() {
        let maps = packed_maps();
        assert!(!maps.is_empty(), "no packs in {}", packs().display());
        for map in maps {
            let started = std::time::Instant::now();
            let c = colliders(&map).unwrap();
            eprintln!("{map}: {} triangles, the grenade's {}, {:.1} ms", c.sight.triangles(), c.flight.triangles(), started.elapsed().as_secs_f64() * 1e3);
            assert!(c.sight.triangles() > 50_000 && c.flight.triangles() > c.sight.triangles());
        }
        let (noise, game, books) = effects();
        assert!(game && books.is_some() && noise.len() == nades_scene::noise::SIZE.pow(3));
    }
}
