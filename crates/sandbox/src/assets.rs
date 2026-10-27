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

/// `map`'s collider, from its pack's Collider section.
pub fn collider(map: &str) -> Result<Collider> {
    let path = pack_path(Some(map));
    let pack = nades_pack::Pack::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let bytes = pack.collider().with_context(|| format!("{} has no collider: rebuild it with nades-pack {map} (it needs Source 2 Viewer's CLI)", path.display()))?;
    Collider::from_awmh(bytes).with_context(|| format!("{}'s Collider section is not a triangle mesh", path.display()))
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
            let c = collider(&map).unwrap();
            eprintln!("{map}: {} triangles, {:.1} ms", c.triangles(), started.elapsed().as_secs_f64() * 1e3);
            assert!(c.triangles() > 50_000);
        }
        let (noise, game, books) = effects();
        assert!(game && books.is_some() && noise.len() == nades_scene::noise::SIZE.pow(3));
    }
}
