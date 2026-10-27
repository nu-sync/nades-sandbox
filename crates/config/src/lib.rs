//! Where the sandbox's settings come from (docs/standalone-api.md §8), for the app, its MCP server and the packer alike.
//! Each takes one path ([`Asset`]), from a flag, else the environment, else `config.toml` in the platform's config
//! folder, else a default; no default names another project's folder: the packs (`--packs`,
//! `NADES_SANDBOX_PACKS`, `[assets] packs`, else `packs/` in the platform's cache folder), CS2's files (`--game`,
//! `NADES_SANDBOX_GAME`, `[assets] game`, no default), Source 2 Viewer's CLI (`--s2v`, `NADES_SANDBOX_S2V`,
//! `[tools] s2v`, no default), Bun's `bunx` for glTF-Transform (`--bunx`, `NADES_SANDBOX_BUNX`, `[tools] bunx`, else
//! the PATH's) and DepotDownloader (`--depotdownloader`, `NADES_SANDBOX_DEPOTDOWNLOADER`, `[tools] depotdownloader`, no
//! default).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Deserialize;

/// Where a setting came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Flag,
    Environment,
    ConfigFile(PathBuf),
    Default,
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    assets: AssetsSection,
    #[serde(default)]
    tools: ToolsSection,
}

#[derive(Debug, Default, Deserialize)]
struct AssetsSection {
    packs: Option<PathBuf>,
    game: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct ToolsSection {
    s2v: Option<PathBuf>,
    bunx: Option<PathBuf>,
    depotdownloader: Option<PathBuf>,
}

/// A setting of one path (docs/standalone-api.md §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asset {
    /// The folder of map packs (`<map>.pack`, and `shared/effects.pack`).
    Packs,
    /// CS2's files: an install's or a depot download's folder, the one holding `game/csgo`.
    Game,
    /// Source 2 Viewer's CLI.
    S2v,
    /// Bun's `bunx`, which runs glTF-Transform for the geometry export.
    Bunx,
    /// DepotDownloader, which `nades-pack setup` fetches a map's files from Steam with.
    DepotDownloader,
}

impl Asset {
    pub fn flag(self) -> &'static str {
        match self {
            Asset::Packs => "--packs",
            Asset::Game => "--game",
            Asset::S2v => "--s2v",
            Asset::Bunx => "--bunx",
            Asset::DepotDownloader => "--depotdownloader",
        }
    }

    pub fn environment(self) -> &'static str {
        match self {
            Asset::Packs => "NADES_SANDBOX_PACKS",
            Asset::Game => "NADES_SANDBOX_GAME",
            Asset::S2v => "NADES_SANDBOX_S2V",
            Asset::Bunx => "NADES_SANDBOX_BUNX",
            Asset::DepotDownloader => "NADES_SANDBOX_DEPOTDOWNLOADER",
        }
    }

    /// Its key in the config file, `[section] key`.
    pub fn key(self) -> &'static str {
        match self {
            Asset::Packs => "[assets] packs",
            Asset::Game => "[assets] game",
            Asset::S2v => "[tools] s2v",
            Asset::Bunx => "[tools] bunx",
            Asset::DepotDownloader => "[tools] depotdownloader",
        }
    }

    fn in_file(self, file: &ConfigFile) -> Option<&PathBuf> {
        match self {
            Asset::Packs => file.assets.packs.as_ref(),
            Asset::Game => file.assets.game.as_ref(),
            Asset::S2v => file.tools.s2v.as_ref(),
            Asset::Bunx => file.tools.bunx.as_ref(),
            Asset::DepotDownloader => file.tools.depotdownloader.as_ref(),
        }
    }

    fn default(self) -> Option<PathBuf> {
        match self {
            Asset::Packs => Some(dirs().map_or_else(|| PathBuf::from("packs"), |d| d.cache_dir().join("packs"))),
            // bunx where the PATH has it
            Asset::Bunx => std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("bunx")).find(|b| b.is_file())),
            Asset::Game | Asset::S2v | Asset::DepotDownloader => None,
        }
    }
}

/// One path and where it was set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub path: PathBuf,
    pub origin: Origin,
}

impl Setting {
    /// Where it came from, in words.
    pub fn from(&self, asset: Asset) -> String {
        match &self.origin {
            Origin::Flag => format!("from {}", asset.flag()),
            Origin::Environment => format!("from {}", asset.environment()),
            Origin::ConfigFile(path) => format!("from {} in {}", asset.key(), path.display()),
            Origin::Default => "the default".into(),
        }
    }
}

/// An asset setting's precedence: the flag, then the environment's value, then the config file (its path and text),
/// then the default; None when none of them sets it.
pub fn resolve_asset(asset: Asset, flag: Option<&Path>, environment: Option<OsString>, file: Option<(PathBuf, String)>, default: Option<PathBuf>) -> Result<Option<Setting>> {
    if let Some(flag) = flag {
        return Ok(Some(Setting { path: resolved(flag, None), origin: Origin::Flag }));
    }
    if let Some(value) = environment.filter(|v| !v.is_empty()) {
        return Ok(Some(Setting { path: resolved(Path::new(&value), None), origin: Origin::Environment }));
    }
    if let Some((path, text)) = file {
        let config: ConfigFile = toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
        if let Some(value) = asset.in_file(&config) {
            let base = path.parent().map(Path::to_path_buf);
            return Ok(Some(Setting { path: resolved(value, base.as_deref()), origin: Origin::ConfigFile(path) }));
        }
    }
    Ok(default.map(|path| Setting { path, origin: Origin::Default }))
}

fn read_config() -> Option<(PathBuf, String)> {
    config_file().and_then(|path| Some((path.clone(), std::fs::read_to_string(&path).ok()?)))
}

/// An asset setting for this run, given its flag.
pub fn asset(asset: Asset, flag: Option<&Path>) -> Result<Option<Setting>> {
    resolve_asset(asset, flag, std::env::var_os(asset.environment()), read_config(), asset.default())
}

/// The packs folder for this run, given `--packs`.
pub fn packs(flag: Option<&Path>) -> Result<Setting> {
    Ok(asset(Asset::Packs, flag)?.expect("the packs have a default"))
}

/// The platform's cache folder for the sandbox (`~/Library/Caches/io.github.nu-sync.nades-sandbox` on macOS).
pub fn cache_dir() -> PathBuf {
    dirs().map_or_else(|| PathBuf::from("cache"), |d| d.cache_dir().to_path_buf())
}

/// Where the packer makes the maps' geometry exports: `exports/` in the cache folder.
pub fn exports_dir() -> PathBuf {
    cache_dir().join("exports")
}

/// The shared effects pack in a packs folder: the game's smoke noise and fire flipbooks, for every map.
pub fn effects_pack(packs: &Path) -> PathBuf {
    packs.join("shared").join("effects.pack")
}

fn dirs() -> Option<directories::ProjectDirs> {
    // the app bundle's identifier (crates/sandbox/macos/Info.plist): ~/Library/Application Support/io.github.nu-sync.nades-sandbox on macOS
    directories::ProjectDirs::from("io.github", "nu-sync", "nades-sandbox")
}

/// `config.toml` in the platform's config folder.
pub fn config_file() -> Option<PathBuf> {
    Some(dirs()?.config_dir().join("config.toml"))
}

/// `~/…` from the home folder; a relative path from `base`.
fn resolved(path: &Path, base: Option<&Path>) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    match base {
        Some(base) if path.is_relative() => base.join(path),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn each_asset_setting_has_the_same_precedence() {
        let file = || Some((p("/conf/c.toml"), "[assets]\npacks = \"~/packs\"\ngame = \"cs2\"\n[tools]\ns2v = \"/t/Source2Viewer-CLI\"\n".to_string()));
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let flag = resolve_asset(Asset::Packs, Some(&p("/flag")), Some("/env".into()), file(), Some(p("/default"))).unwrap().unwrap();
        assert_eq!(flag, Setting { path: p("/flag"), origin: Origin::Flag });
        let env = resolve_asset(Asset::Packs, None, Some("/env".into()), file(), Some(p("/default"))).unwrap().unwrap();
        assert_eq!((env.path.clone(), env.from(Asset::Packs)), (p("/env"), "from NADES_SANDBOX_PACKS".to_string()));
        let packs = resolve_asset(Asset::Packs, None, None, file(), Some(p("/default"))).unwrap().unwrap();
        assert_eq!(packs.path, home.join("packs"), "~ is the home folder");
        assert_eq!(packs.from(Asset::Packs), "from [assets] packs in /conf/c.toml");
        assert_eq!(resolve_asset(Asset::Game, None, None, file(), None).unwrap().unwrap().path, p("/conf/cs2"), "relative to the file");
        assert_eq!(resolve_asset(Asset::S2v, None, None, file(), None).unwrap().unwrap().path, p("/t/Source2Viewer-CLI"));
        assert_eq!(resolve_asset(Asset::Packs, None, Some(OsString::new()), None, Some(p("/default"))).unwrap().unwrap().origin, Origin::Default);
        assert_eq!(resolve_asset(Asset::Game, None, None, Some((p("/c.toml"), "[tools]\nbunx = \"b\"\n".into())), None).unwrap(), None, "no default: unset");
        assert!(Asset::Packs.default().unwrap().ends_with("packs"));
        assert!(!Asset::Packs.default().unwrap().to_string_lossy().contains(".cache/nades"), "no default names another project's folder");
    }

}
