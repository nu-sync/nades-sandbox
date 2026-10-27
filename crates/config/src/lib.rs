//! Where the sandbox's settings come from (docs/standalone-api.md §8), for the app and its MCP server alike. The throws:
//! `--data <file or folder>` (repeatable), else `NADES_SANDBOX_DATA` (paths separated as the platform separates PATH),
//! else `[data] paths` in `config.toml` in the platform's config folder, else the demo adapter's cache folder. A flag
//! beats the environment, which beats the file, which beats the default. No default names another project's folder.
//! A data path may be a CS2 demo, or a folder with demos in it: each is imported into the cache first (§6) and read
//! from there.
//!
//! The assets' settings take one path each, with the same precedence ([`Asset`]): the packs (`--packs`,
//! `NADES_SANDBOX_PACKS`, `[assets] packs`, else `packs/` in the platform's cache folder), CS2's files (`--game`,
//! `NADES_SANDBOX_GAME`, `[assets] game`, no default), Source 2 Viewer's CLI (`--s2v`, `NADES_SANDBOX_S2V`,
//! `[tools] s2v`, no default), Bun's `bunx` for glTF-Transform (`--bunx`, `NADES_SANDBOX_BUNX`, `[tools] bunx`, else
//! the PATH's) and DepotDownloader (`--depotdownloader`, `NADES_SANDBOX_DEPOTDOWNLOADER`, `[tools] depotdownloader`, no
//! default).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use nades_model::ThrowSource;
use nades_source_file::FileSource;
use serde::Deserialize;

/// Where a setting came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Flag,
    Environment,
    ConfigFile(PathBuf),
    Default,
}

/// The data paths and where they were set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Data {
    pub paths: Vec<PathBuf>,
    pub origin: Origin,
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    data: DataSection,
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

#[derive(Debug, Default, Deserialize)]
struct DataSection {
    #[serde(default)]
    paths: Vec<PathBuf>,
}

fn dirs() -> Option<directories::ProjectDirs> {
    // the app bundle's identifier (crates/sandbox/macos/Info.plist): ~/Library/Application Support/io.github.nu-sync.nades-sandbox on macOS
    directories::ProjectDirs::from("io.github", "nu-sync", "nades-sandbox")
}

/// `config.toml` in the platform's config folder.
pub fn config_file() -> Option<PathBuf> {
    Some(dirs()?.config_dir().join("config.toml"))
}

/// The demo adapter's cache: one cs2-throws file per demo it has read.
pub fn default_data() -> PathBuf {
    dirs().map_or_else(|| PathBuf::from("throws"), |d| d.cache_dir().join("throws"))
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

/// The precedence itself: `flags`, then the environment's value, then the config file (its path and text), then the
/// default.
pub fn resolve(flags: &[PathBuf], environment: Option<OsString>, file: Option<(PathBuf, String)>, default: PathBuf) -> Result<Data> {
    if !flags.is_empty() {
        return Ok(Data { paths: flags.iter().map(|p| resolved(p, None)).collect(), origin: Origin::Flag });
    }
    if let Some(value) = environment.filter(|v| !v.is_empty()) {
        return Ok(Data { paths: std::env::split_paths(&value).filter(|p| !p.as_os_str().is_empty()).map(|p| resolved(&p, None)).collect(), origin: Origin::Environment });
    }
    if let Some((path, text)) = file {
        let config: ConfigFile = toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
        if !config.data.paths.is_empty() {
            let base = path.parent().map(Path::to_path_buf);
            return Ok(Data { paths: config.data.paths.iter().map(|p| resolved(p, base.as_deref())).collect(), origin: Origin::ConfigFile(path) });
        }
    }
    Ok(Data { paths: vec![default], origin: Origin::Default })
}

/// The data settings for this run, given the `--data` flags.
pub fn data(flags: &[PathBuf]) -> Result<Data> {
    resolve(flags, std::env::var_os("NADES_SANDBOX_DATA"), read_config(), default_data())
}

/// `paths` with their demos brought into `cache`: a demo stands for its cache file, and a folder's demos follow the
/// folder. `report` hears each import as it ends.
pub fn import_demos(paths: &[PathBuf], cache: &Path, report: &mut dyn FnMut(&nades_source_demo::Imported, Duration)) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for path in paths {
        let demos = nades_source_demo::demos_in(path)?;
        if path.is_dir() || demos.is_empty() {
            out.push(path.clone());
        }
        for demo in demos {
            let started = Instant::now();
            let done = nades_source_demo::import(&demo, cache, &mut |_| {}).with_context(|| format!("reading the demo {}", demo.display()))?;
            report(&done, started.elapsed());
            if !out.contains(&done.file) {
                out.push(done.file);
            }
        }
    }
    Ok(out)
}

/// The source the data settings name, and what to tell the person when it holds nothing: a missing default folder is
/// an empty source with a hint, any other failure an empty source with the error. Demos among the paths are imported
/// into the cache first, each reported on stderr.
pub fn open(data: &Data) -> (Arc<FileSource>, Option<String>) {
    let empty = || Arc::new(FileSource::open(&[]).expect("no files to open"));
    if data.origin == Origin::Default && data.paths.iter().all(|p| !p.exists()) {
        return (empty(), Some("No throws to show: pass --data <demo, cs2-throws file or folder> (see docs/setup.md).".into()));
    }
    let report = &mut |done: &nades_source_demo::Imported, took: Duration| {
        let how = if done.parsed { format!("read in {:.1} s", took.as_secs_f64()) } else { "already in the cache".into() };
        eprintln!("data: {}: {} throws, {how}", done.demo.display(), done.throws);
    };
    let files = match import_demos(&data.paths, &default_data(), report) {
        Ok(paths) => paths,
        Err(e) => return (empty(), Some(format!("{e:#} ({})", from(&data.origin)))),
    };
    match FileSource::open(&files) {
        Ok(source) if source.files().is_empty() => (Arc::new(source), Some(format!("No cs2-throws files in {}.", paths(&data.paths)))),
        Ok(source) => (Arc::new(source), None),
        Err(e) => (empty(), Some(format!("{e} ({})", from(&data.origin)))),
    }
}

/// The source for a run without a window (`--cameras`, `--render --throw`, a video for the MCP server): the files the
/// settings name, given the `--data` flags; one that does not open, or no data at all, is an error.
pub fn source(flags: &[PathBuf]) -> Result<Arc<dyn ThrowSource>> {
    match open(&data(flags)?) {
        (_, Some(notice)) => anyhow::bail!("{notice}"),
        (source, None) => Ok(source),
    }
}

fn paths(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
}

/// Where a setting came from, in words.
pub fn from(origin: &Origin) -> String {
    match origin {
        Origin::Flag => "from --data".into(),
        Origin::Environment => "from NADES_SANDBOX_DATA".into(),
        Origin::ConfigFile(path) => format!("from {}", path.display()),
        Origin::Default => "the default".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demos_among_the_paths_are_read_into_the_cache_and_the_rest_pass_through() {
        let dir = std::env::temp_dir().join(format!("nades-config-demos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        let file = dir.join("throws.sqlite");
        let cache = dir.join("cache");
        let mut heard = 0;
        let paths = import_demos(&[file.clone(), dir.join("folder")], &cache, &mut |_, _| heard += 1).unwrap();
        assert_eq!((paths, heard), (vec![file, dir.join("folder")], 0));
        // a demo that does not parse names itself
        std::fs::write(dir.join("folder/broken.dem"), b"PBDEMS2\0 and nothing else").unwrap();
        let err = import_demos(&[dir.join("folder")], &cache, &mut |_, _| {}).unwrap_err();
        assert!(format!("{err:#}").contains("reading the demo") && format!("{err:#}").contains("broken.dem"), "{err:#}");
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn a_flag_beats_the_environment_which_beats_the_file_which_beats_the_default() {
        let file = || Some((p("/conf/nades-sandbox/config.toml"), "[data]\npaths = [\"pro.sqlite\", \"/abs/throws\"]\n".to_string()));
        let joined = std::env::join_paths([p("/env/a.sqlite"), p("/env/b")]).unwrap();
        let all = resolve(&[p("/flag.sqlite")], Some(joined.clone()), file(), p("/cache/throws")).unwrap();
        assert_eq!(all, Data { paths: vec![p("/flag.sqlite")], origin: Origin::Flag });
        let env = resolve(&[], Some(joined), file(), p("/cache/throws")).unwrap();
        assert_eq!(env, Data { paths: vec![p("/env/a.sqlite"), p("/env/b")], origin: Origin::Environment });
        // a relative path in the file is the file's folder's
        let config = resolve(&[], None, file(), p("/cache/throws")).unwrap();
        assert_eq!(config, Data { paths: vec![p("/conf/nades-sandbox/pro.sqlite"), p("/abs/throws")], origin: Origin::ConfigFile(p("/conf/nades-sandbox/config.toml")) });
        // an empty variable, or a file without paths, is not set
        let none = resolve(&[], Some(OsString::new()), Some((p("/c.toml"), "[assets]\npacks = \"x\"\n".into())), p("/cache/throws")).unwrap();
        assert_eq!(none, Data { paths: vec![p("/cache/throws")], origin: Origin::Default });
        assert!(resolve(&[], None, Some((p("/c.toml"), "[data\n".into())), p("/d")).is_err(), "a broken file is said so");
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
        assert_eq!(resolve_asset(Asset::Game, None, None, Some((p("/c.toml"), "[data]\npaths = []\n".into())), None).unwrap(), None, "no default: unset");
        assert!(Asset::Packs.default().unwrap().ends_with("packs"));
        assert!(!Asset::Packs.default().unwrap().to_string_lossy().contains(".cache/nades"), "no default names another project's folder");
    }

    #[test]
    fn the_default_folder_missing_is_an_empty_source_with_a_hint() {
        let data = Data { paths: vec![std::env::temp_dir().join("nades-sandbox-no-such-folder")], origin: Origin::Default };
        let (source, notice) = open(&data);
        assert!(source.maps().unwrap().is_empty());
        assert!(notice.unwrap().contains("--data"));
        let data = Data { origin: Origin::Flag, ..data };
        let (_, notice) = open(&data);
        assert!(notice.unwrap().contains("from --data"), "a path someone named is reported as theirs");
    }
}
