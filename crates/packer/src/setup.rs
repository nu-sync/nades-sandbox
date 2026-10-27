//! `nades-pack setup <map>`: fetches what a map's pack needs from Steam's CS2 depot into the game folder, with
//! DepotDownloader (anonymous; app 730, depot 2347770 "cs2 content"), the way show-me-the-nades' `export_map.py`
//! fetched it:
//!
//! 1. the base files: both `gameinfo.gi`, `pak01_dir.vpk`, the core archives and the map's VPK, then the 3D skybox
//!    prefab's VPK its entity lump names;
//! 2. rounds of the geometry export from a staged folder without `pak01` mounted (a `pak01` archive the folder lacks
//!    is passed over in silence when it is mounted, but named, `Failed to load`, when it is not): each round fetches
//!    the archives that hold what the CLI could not load and unpacks those files into the staged folder, until it
//!    loads everything (at most 8 rounds, as export_map.py);
//! 3. the archives of the layer textures, the sky's material and texture, the post-processing resource and the effects.
//!
//! The depot's manifest is pinned to the one the game folder already holds, so that what is fetched comes from the same
//! game build as what is there; a fresh folder takes the current one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, ensure};
use nades_source2::vpk::Vpk;

use crate::export::{self, Exporter};
use crate::{Inputs, lump};

pub const APP: &str = "730";
pub const DEPOT: &str = "2347770";
const ROUNDS: usize = 8;

/// The manifest the game folder holds (`.DepotDownloader/<depot>_<manifest>.manifest`), if one.
pub fn manifest(game: &Path) -> Option<String> {
    let mut found: Vec<String> = std::fs::read_dir(game.join(".DepotDownloader"))
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_prefix(&format!("{DEPOT}_"))?.strip_suffix(".manifest").map(str::to_string))
        .collect();
    found.sort();
    found.pop()
}

/// DepotDownloader on a file list.
pub struct Fetcher<'a> {
    pub tool: &'a Path,
    pub game: &'a Path,
    pub manifest: Option<String>,
    pub scratch: PathBuf,
    /// Lines fetched, for the report.
    pub fetched: Vec<String>,
}

impl Fetcher<'_> {
    pub fn fetch(&mut self, lines: &[String]) -> Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.scratch)?;
        let list = self.scratch.join("filelist.txt");
        std::fs::write(&list, lines.join("\n") + "\n")?;
        eprintln!("setup: fetching {} file(s) from the CS2 depot{}", lines.len(), self.manifest.as_ref().map(|m| format!(" (manifest {m})")).unwrap_or_default());
        let mut command = Command::new(self.tool);
        command.args(["-app", APP, "-depot", DEPOT]);
        if let Some(m) = &self.manifest {
            command.args(["-manifest", m]);
        }
        let output = command
            .arg("-filelist")
            .arg(&list)
            .arg("-dir")
            .arg(self.game)
            .args(["-max-downloads", "24"])
            .current_dir(&self.scratch)
            .output()
            .with_context(|| format!("running {}", self.tool.display()))?;
        ensure!(output.status.success(), "{} exited {}: {}", self.tool.display(), output.status, String::from_utf8_lossy(&output.stdout).lines().rev().take(5).collect::<Vec<_>>().join(" / "));
        self.fetched.extend(lines.iter().cloned());
        Ok(())
    }

    /// The `pak01` archives holding `entries` that the game folder lacks.
    pub fn archives(&mut self, pak: &Vpk, entries: &[String]) -> Result<usize> {
        let parts: BTreeSet<u16> = entries.iter().filter_map(|e| pak.entry(e)).filter(|e| e.length > 0 && e.archive != nades_source2::vpk::IN_DIRECTORY).map(|e| e.archive).filter(|&a| !pak.archive_path(a).is_file()).collect();
        let lines: Vec<String> = parts.iter().map(|p| format!("game/csgo/pak01_{p:03}.vpk")).collect();
        self.fetch(&lines)?;
        Ok(lines.len())
    }
}

/// What setup did.
#[derive(Debug, Default)]
pub struct Setup {
    pub fetched: Vec<String>,
    /// Per export round: files the CLI could not load, and of those how many `pak01` holds.
    pub rounds: Vec<(String, usize, usize)>,
}

fn compiled(path: &str) -> String {
    if path.ends_with("_c") { path.to_string() } else { format!("{path}_c") }
}

/// Fetches what `inputs.map` needs.
pub fn run(inputs: &Inputs, tool: &Path) -> Result<Setup> {
    let cli = inputs.cli().with_context(|| crate::no_cli(inputs))?;
    let bunx = inputs.bunx.as_deref().filter(|b| b.is_file()).context("setup runs the geometry export, which needs Bun's bunx (--bunx, NADES_SANDBOX_BUNX or [tools] bunx)")?;
    let scratch = inputs.work.join(format!(".setup-{}", inputs.map));
    let mut fetcher = Fetcher { tool, game: &inputs.game, manifest: manifest(&inputs.game), scratch: scratch.clone(), fetched: Vec::new() };
    let map = &inputs.map;
    let base: Vec<String> = ["game/csgo/gameinfo.gi", "game/core/gameinfo.gi", "game/csgo/pak01_dir.vpk"].iter().map(|s| s.to_string()).chain([format!("game/csgo/maps/{map}.vpk"), r"regex:^game/core/pak01_.*\.vpk$".to_string()]).collect();
    let missing_base: Vec<String> = base.iter().filter(|l| l.starts_with("regex:") || !inputs.game.join(l.as_str()).is_file()).cloned().collect();
    // the core archives' regex makes DepotDownloader check them; skip the call when everything else is there
    if missing_base.iter().any(|l| !l.starts_with("regex:")) {
        fetcher.fetch(&missing_base)?;
    }
    ensure!(inputs.vpk().is_file(), "{} is not in the depot's manifest", inputs.vpk().display());
    let pak = Vpk::open(&inputs.pak()).with_context(|| inputs.pak().display().to_string())?;
    // the 3D skybox's prefab, from the entity lump
    let read_lump = |pak: &Vpk| -> Result<lump::Lump> {
        let mut d = lump::Decompiler::new(cli, &std::env::temp_dir(), &format!("setup-{map}"))?;
        lump::read(&mut d, &lump::Game { maps: inputs.csgo().join("maps"), pak: Some(pak), pak_path: inputs.pak() }, map)
    };
    let first = read_lump(&pak)?;
    let prefab = match &first.entities {
        text if !text.is_empty() => {
            let blocks = lump::blocks(text);
            lump::classes(&blocks, "skybox_reference").into_iter().next().and_then(|r| lump::keyvalue(r, "targetmapname")).and_then(|v| v.trim_matches('"').strip_prefix("maps/")?.strip_suffix(".vmap").map(str::to_string))
        }
        _ => None,
    };
    if let Some(p) = &prefab
        && !inputs.map_vpk(p).is_file()
    {
        fetcher.fetch(&[format!("game/csgo/maps/{p}.vpk")])?;
    }
    let mut maps = vec![map.clone()];
    maps.extend(prefab.filter(|p| inputs.map_vpk(p).is_file()));
    // the export's rounds, pak01 unmounted
    let stage_dir = scratch.join("stage");
    let names: Vec<&str> = maps.iter().map(String::as_str).collect();
    let gameinfo = export::stage_with(&inputs.game, &stage_dir, &names, false)?;
    let staged_csgo = gameinfo.parent().expect("csgo").to_path_buf();
    let exporter = Exporter { cli, bunx, game: &inputs.game, work: scratch.join("work") };
    let mut done = Setup::default();
    let mut unpacked: BTreeSet<String> = BTreeSet::new();
    let mut layer_wanted = Vec::new();
    for (k, name) in maps.iter().enumerate() {
        let label = if k == 0 { map.clone() } else { format!("{map}.skybox") };
        for _ in 0..ROUNDS {
            let log = exporter.export(&gameinfo, name, &label)?;
            let missing = export::failed_to_load(&log);
            let wanted: Vec<String> = missing.iter().map(|p| compiled(p)).filter(|p| pak.entry(p).is_some() && !unpacked.contains(p)).collect();
            done.rounds.push((label.clone(), missing.len(), wanted.len()));
            if wanted.is_empty() {
                break;
            }
            fetcher.archives(&pak, &wanted)?;
            for entry in &wanted {
                let bytes = pak.read_checked(entry).with_context(|| format!("{entry} in pak01"))?;
                let target = staged_csgo.join(entry);
                std::fs::create_dir_all(target.parent().expect("a folder"))?;
                std::fs::write(&target, &bytes)?;
                unpacked.insert(entry.clone());
            }
        }
        layer_wanted.extend(export::layer_entries(&exporter.work.join(&label).join(format!("{label}.gltf")), &pak)?);
    }
    fetcher.archives(&pak, &layer_wanted)?;
    // the sky, the post-processing, the fog's sky and the effects
    let mut more: Vec<String> = crate::effects::FLIPBOOKS.iter().map(|n| format!("{}/{n}.vtex_c", crate::effects::FLIPBOOK_DIR)).chain([crate::effects::NOISE.to_string()]).collect();
    // the materials of the sky the game draws (the prefab's or the map's) and of the fog's sky, by name
    for material in [first.post.get("sky").and_then(|s| s.get("material")), first.post.get("fog").and_then(|f| f.get("sky")).and_then(|s| s.get("material"))].into_iter().flatten() {
        more.extend(material.as_str().map(compiled));
    }
    let blocks = lump::blocks(&first.entities);
    for b in lump::classes(&blocks, "post_processing_volume") {
        more.extend(lump::keyvalue(b, "postprocessing").and_then(|v| lump::resource(v, ".vpost")).map(|r| compiled(&r)));
    }
    fetcher.archives(&pak, &more)?;
    // the sky's texture, which its material names
    let second = read_lump(&pak)?;
    if let Some(material) = second.post.get("sky").and_then(|s| s.get("material")).and_then(|m| m.as_str())
        && let Some(texture) = lump::sky_texture(second.sky_vmat.as_deref(), material, &pak)
    {
        fetcher.archives(&pak, &[compiled(&texture)])?;
    }
    done.fetched = fetcher.fetched;
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(done)
}
