//! Where the sandbox's settings come from (docs/standalone-api.md §8), for the app and its MCP server alike. The throws:
//! `--data <file or folder>` (repeatable), else `NADES_SANDBOX_DATA` (paths separated as the platform separates PATH),
//! else `[data] paths` in `config.toml` in the platform's config folder, else the demo adapter's cache folder. A flag
//! beats the environment, which beats the file, which beats the default. No default names another project's folder.
//! (The packs, the collider and the effects keep their `~/.cache/nades` paths until maps come from the game's files.)

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// The demo adapter's cache: one cs2-throws file per demo it has read (that adapter comes next; until then, empty).
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
    let file = config_file().and_then(|path| Some((path.clone(), std::fs::read_to_string(&path).ok()?)));
    resolve(flags, std::env::var_os("NADES_SANDBOX_DATA"), file, default_data())
}

/// The source the data settings name, and what to tell the person when it holds nothing: a missing default folder is
/// an empty source with a hint, any other failure an empty source with the error.
pub fn open(data: &Data) -> (Arc<dyn ThrowSource>, Option<String>) {
    let empty = || -> Arc<dyn ThrowSource> { Arc::new(FileSource::open(&[]).expect("no files to open")) };
    if data.origin == Origin::Default && data.paths.iter().all(|p| !p.exists()) {
        return (empty(), Some("No throws to show: pass --data <cs2-throws file or folder> (see docs/setup.md).".into()));
    }
    match FileSource::open(&data.paths) {
        Ok(source) if source.files().is_empty() => (Arc::new(source), Some(format!("No cs2-throws files in {}.", paths(&data.paths)))),
        Ok(source) => (Arc::new(source), None),
        Err(e) => (empty(), Some(format!("{e} ({})", from(&data.origin)))),
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
