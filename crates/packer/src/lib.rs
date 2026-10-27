//! `nades-pack <map>`: builds `~/.cache/nades/packs/<map>.pack` (format 2) from the map's Source 2 Viewer export
//! (`~/.cache/nades/maps/<map>.glb` and the files beside it) and the game's own files (`~/.cache/nades/cs2`). The pack
//! is derived from Valve's assets, so it stays on this machine.
//!
//! - **Geometry** ([`geometry`]): the export's meshopt-compressed, quantized streams decoded once into the layouts
//!   `nades_pack::scene` documents; one draw per node and primitive; EXT_mesh_gpu_instancing's instances as matrices;
//!   lightmap coordinates by the web view's rule. The 3D skybox's export (`<map>.skybox.glb`) the same way.
//! - **Materials** ([`materials`]): CS2's shader families with their parameters resolved from `extras.vmat`.
//! - **Textures** ([`textures`]): the game's own blocks where a `.vtex_c` stands for a texture (base colours by name,
//!   everything else by the material's `TextureParams`, from the unpacked game folder or the game's `pak01`);
//!   otherwise the export's WebP decoded to RGBA8 with mips. Nothing is made up: a texture with neither is left out
//!   and reported.
//! - **The baked light:** irradiance (BC6H), directional irradiance (BC1) and sun shadows (BC4, BC5 or BC7) from the
//!   map's VPK, and the 3D skybox's from its prefab's VPK; the light probe volumes' atlases from the map's VPK, placed
//!   by its entities ([`entities`]). Which meshes are lightmapped is the game's answer ([`world`]) where Source 2
//!   Viewer's CLI can tell it, else the web view's rule.
//! - **The sky:** the visible env_sky's cube map (BC6H) from `pak01`; **the sun** from the export's light;
//!   **the post-processing** from `<map>.post.json` and `<map>.lut.bin` ([`post`]).
//! - **The collider** ([`physics`]): the map's own physics from its VPK, through Source 2 Viewer's CLI, reduced as
//!   awpy reduces it, in the Collider section.

pub mod effects;
pub mod entities;
pub mod export;
pub mod geometry;
pub mod gltf;
pub mod lump;
pub mod ojson;
pub mod materials;
pub mod physics;
pub mod post;
pub mod resample;
pub mod setup;
pub mod textures;
pub mod world;

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use memmap2::Mmap;
use nades_pack::scene::{AlphaMode, Draw, Lightmaps, Material, Scene, Semantic, Shading, Skipped, Sky, Skybox, Stats, Sun, TextureFormat, TextureSource};
use nades_pack::{SectionKind, Source};
use nades_source2::vpk::Vpk;

use crate::geometry::{Geometry, Group};
use crate::materials::{Slot, Want};
use crate::textures::{ByName, CHUNK_TARGET, Fill, GameFile, GameIndex, Planned};

/// Where a pack's inputs are and where it goes.
#[derive(Debug, Clone)]
pub struct Inputs {
    /// e.g. `de_cache`.
    pub map: String,
    /// A geometry export made elsewhere (`--export <map>.glb`, as show-me-the-nades' `export_map.py` made them), its
    /// 3D skybox's `<map>.skybox.glb` beside it. None: the packer makes both itself ([`export`]) in `work`.
    pub export: Option<PathBuf>,
    /// Where the packer makes the exports: `exports/` in the platform's cache folder.
    pub work: PathBuf,
    /// Bun's `bunx`, which runs glTF-Transform for the export (`--bunx`, `NADES_SANDBOX_BUNX`, `[tools] bunx`, else the
    /// PATH's).
    pub bunx: Option<PathBuf>,
    /// CS2's files: the folder holding `game/csgo` (an install, or DepotDownloader's folder, whose manifests name the
    /// game build), with the maps' VPKs in `game/csgo/maps` and `pak01` in `game/csgo` (`--game`, `NADES_SANDBOX_GAME`,
    /// `[assets] game`).
    pub game: PathBuf,
    /// Game files unpacked from `pak01` (a `game/csgo` folder), which stand for the archive's entries at their paths
    /// (`--unpacked`): for a depot download whose archives were dropped once unpacked. None reads `pak01` alone.
    pub unpacked: Option<PathBuf>,
    /// Source 2 Viewer's CLI (`--s2v`, `NADES_SANDBOX_S2V`, `[tools] s2v`): it exports the map's physics (the
    /// collider), decompiles its entity lump, prints its meshes' draw calls and decompiles the fire's flipbooks.
    pub cli: Option<PathBuf>,
    /// The pack: `<packs>/<map>.pack` (`--packs`, `NADES_SANDBOX_PACKS`, `[assets] packs`, or `--out <file>`).
    pub out: PathBuf,
}

/// The packer's settings (docs/standalone-api.md §8): flags first, then the environment, the config file, the default.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    pub packs: Option<PathBuf>,
    pub game: Option<PathBuf>,
    pub s2v: Option<PathBuf>,
    pub bunx: Option<PathBuf>,
    pub depotdownloader: Option<PathBuf>,
}

impl Inputs {
    /// The settings' places for `map`.
    pub fn for_map(map: &str) -> Result<Self> {
        Self::with(map, &Flags::default())
    }

    pub fn with(map: &str, flags: &Flags) -> Result<Self> {
        let packs = nades_config::packs(flags.packs.as_deref())?;
        let game = nades_config::asset(nades_config::Asset::Game, flags.game.as_deref())?
            .with_context(|| "CS2's files are not set: pass --game <folder holding game/csgo>, or set NADES_SANDBOX_GAME or [assets] game in the config file".to_string())?;
        let cli = nades_config::asset(nades_config::Asset::S2v, flags.s2v.as_deref())?;
        let bunx = nades_config::asset(nades_config::Asset::Bunx, flags.bunx.as_deref())?;
        Ok(Self {
            map: map.to_string(),
            export: None,
            work: nades_config::exports_dir(),
            bunx: bunx.map(|s| s.path),
            game: game.path,
            unpacked: None,
            cli: cli.map(|s| s.path),
            out: packs.path.join(format!("{map}.pack")),
        })
    }

    /// `game/csgo` under the game folder.
    pub fn csgo(&self) -> PathBuf {
        self.game.join("game/csgo")
    }

    /// A map's (or a prefab's) VPK: `game/csgo/maps/<name>.vpk`.
    pub fn map_vpk(&self, name: &str) -> PathBuf {
        self.csgo().join("maps").join(format!("{name}.vpk"))
    }

    pub fn vpk(&self) -> PathBuf {
        self.map_vpk(&self.map)
    }

    pub fn lightmap_entry(&self) -> String {
        format!("maps/{}/lightmaps/irradiance.vtex_c", self.map)
    }

    /// The map's export: the one given, else the packer's own.
    pub fn glb(&self) -> PathBuf {
        self.export.clone().unwrap_or_else(|| self.work.join(format!("{}.glb", self.map)))
    }

    /// A file beside the export: `<map><suffix>`.
    pub fn beside(&self, suffix: &str) -> PathBuf {
        self.glb().with_file_name(format!("{}{suffix}", self.map))
    }

    pub fn pak(&self) -> PathBuf {
        self.csgo().join("pak01_dir.vpk")
    }

    /// The CLI, when it is set and there.
    pub fn cli(&self) -> Option<&Path> {
        self.cli.as_deref().filter(|c| c.is_file())
    }
}

/// What a build made, for the report.
pub struct Report {
    pub scene: Scene,
    pub source: Source,
    /// Section bytes: Scene, Geometry, Textures, and the Collider when there is one.
    pub sections: Vec<(SectionKind, u64)>,
    /// The collider's triangles and the export's, and why there is none.
    pub collider: Result<physics::Physics, String>,
    pub file_size: u64,
    /// Stage name and milliseconds, in order.
    pub stages: Vec<(&'static str, f64)>,
    /// Draws whose TEXCOORD_2 has coordinates outside [0, 1]: taken as they are.
    pub texcoord2_outside: u32,
    /// Draws drawn with uv0 = 0 for want of TEXCOORD_0.
    pub missing_uv0: u32,
    /// Primitives given a default normal or tangent for want of one.
    pub missing_frames: u32,
    /// World-node meshes the game said are lightmapped, of those it said anything about; none without the CLI.
    pub game_meshes: Option<(usize, usize)>,
    pub game_files: usize,
    /// The CLI's decompiles for the entity lumps, the skies' materials and the post-processing.
    pub decompiled: u32,
    /// The exports the packer made (label and what each did); none when one was given.
    pub exported: Vec<(String, export::Exported)>,
    pub pak_entries: usize,
    /// Each base colour image decoded from the export, and why no game file stood for it.
    pub fallbacks: Vec<String>,
    /// Each material texture that neither the game nor the export holds, and why.
    pub missing: Vec<String>,
}

/// FNV-1a, 64-bit: a stable hash of the export, so a changed export rebuilds its pack.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The map's and its 3D skybox's exports, made by the packer ([`export`]).
fn make_exports(inputs: &Inputs, pak: Option<&Vpk>, prefab: Option<&str>) -> Result<Vec<(String, export::Exported)>> {
    let cli = inputs.cli().with_context(|| format!("the geometry export needs Source 2 Viewer's CLI: {} (or pass --export <map>.glb)", no_cli(inputs)))?;
    let bunx = inputs.bunx.as_deref().filter(|b| b.is_file()).context("the geometry export needs Bun's bunx for glTF-Transform: install Bun, or set --bunx, NADES_SANDBOX_BUNX or [tools] bunx (or pass --export <map>.glb)")?;
    let pak = pak.with_context(|| format!("the geometry export needs {}", inputs.pak().display()))?;
    std::fs::create_dir_all(&inputs.work).with_context(|| format!("creating {}", inputs.work.display()))?;
    let mut maps = vec![inputs.map.as_str()];
    maps.extend(prefab.filter(|p| inputs.map_vpk(p).is_file()));
    let gameinfo = export::stage(&inputs.game, &inputs.work.join(format!(".stage-{}", inputs.map)), &maps)?;
    let exporter = export::Exporter { cli, bunx, game: &inputs.game, work: inputs.work.clone() };
    let mut done = Vec::new();
    for (k, name) in maps.iter().enumerate() {
        let label = if k == 0 { inputs.map.clone() } else { format!("{}.skybox", inputs.map) };
        let made = exporter.make(&gameinfo, pak, &inputs.pak(), name, &label)?;
        done.push((label, made));
    }
    // a skybox export left from another time would be read as this one's
    if prefab.is_none() {
        let _ = std::fs::remove_file(inputs.beside(".skybox.glb"));
    }
    let _ = std::fs::remove_dir_all(inputs.work.join(format!(".stage-{}", inputs.map)));
    Ok(done)
}

/// Why there is no CLI to run.
pub fn no_cli(inputs: &Inputs) -> String {
    match &inputs.cli {
        Some(cli) => format!("no Source 2 Viewer CLI at {}", cli.display()),
        None => "Source 2 Viewer's CLI is not set (--s2v, NADES_SANDBOX_S2V or [tools] s2v)".into(),
    }
}

/// The game build: DepotDownloader's manifests (`<depot>_<manifest id>.manifest`) in the game folder, else an
/// install's `game/csgo/steam.inf` `ClientVersion`.
pub fn game_build(game: &Path) -> String {
    let mut manifests: Vec<String> = std::fs::read_dir(game.join(".DepotDownloader"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".manifest").map(str::to_string))
        .collect();
    manifests.sort();
    if !manifests.is_empty() {
        return manifests.join("+");
    }
    std::fs::read_to_string(game.join("game/csgo/steam.inf"))
        .ok()
        .and_then(|inf| inf.lines().find_map(|l| l.strip_prefix("ClientVersion=").map(|v| format!("client {}", v.trim()))))
        .unwrap_or_else(|| "unknown".to_string())
}

/// One export (the map's, or its 3D skybox's), mapped and parsed.
struct Export {
    doc: gltf::Document,
    buffers: Vec<Cow<'static, [u8]>>,
}

impl Export {
    fn open(path: &Path) -> Result<(Self, u64)> {
        let file = std::fs::File::open(path).with_context(|| format!("opening the export {}", path.display()))?;
        // SAFETY: the export is written once by export_map.py and not while a pack is built from it; the mapping is
        // leaked so the decoded buffers can borrow it for the whole build
        let glb: &'static Mmap = Box::leak(Box::new(unsafe { Mmap::map(&file) }.with_context(|| format!("mapping {}", path.display()))?));
        let hash = fnv1a(glb);
        let parts = gltf::split(glb)?;
        let doc = gltf::parse(parts.json)?;
        let buffers = gltf::buffers(&doc, parts.bin)?;
        Ok((Self { doc, buffers }, hash))
    }

    /// The export's sun: its first directional light, shining down its node's −z.
    fn sun(&self) -> Result<Option<Sun>> {
        let Some(lights) = self.doc.extensions.as_ref().and_then(|e| e.lights.as_ref()) else { return Ok(None) };
        for (node, world) in geometry::walk(&self.doc)? {
            let Some(light) = self.doc.nodes[node].light().and_then(|l| lights.lights.get(l)) else { continue };
            if light.kind != "directional" {
                continue;
            }
            let towards = world.transform_vector3(glam::Vec3::Z).normalize();
            let brightness = light.intensity.unwrap_or(683.0) / 683.0;
            let color = light.color.unwrap_or([1.0; 3]).map(|c| c * brightness);
            return Ok(Some(Sun { direction: towards.to_array(), color }));
        }
        Ok(None)
    }
}

/// The texture table as it is planned: each texture once, by where its bytes come from.
#[derive(Default)]
struct Plan {
    planned: Vec<Planned>,
    /// (export, image) → texture, for base colours.
    by_image: HashMap<(usize, usize), u32>,
    /// (game file, semantic, size) → texture, for every other game texture.
    by_file: HashMap<(String, Semantic, u32), u32>,
    /// (export, image, semantic) → texture, for export images standing in for game files.
    by_export: HashMap<(usize, usize, Semantic), u32>,
    game_textures: u32,
    export_textures: u32,
}

impl Plan {
    fn push(&mut self, p: Planned) -> u32 {
        self.planned.push(p);
        self.planned.len() as u32 - 1
    }
}

/// A material texture from the game: its file, read for its header, planned from the mip that fits `want`.
fn plan_game(game: &GameIndex, want: &Want, path: &str) -> Result<Option<Planned>, String> {
    let Some(file) = game.find(path) else { return Ok(None) };
    let bytes = game.read(&file).map_err(|e| format!("{e:#}"))?;
    let vtex = nades_source2::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let (format, first) = textures::usable(&vtex, want.semantic, want.max_size).map_err(|e| format!("{path}: {e}"))?;
    let source = TextureSource::ByTextureParams { param: want.param.clone(), file: path.to_string(), first_mip: first };
    let fill = match file {
        GameFile::Disk(path) => Fill::GameFile { path, first },
        GameFile::Pak(entry) => Fill::Pak { entry, first },
    };
    Ok(Some(textures::from_vtex(path.to_string(), &vtex, format, want.srgb, want.semantic, source, fill)))
}

/// A texture from one of the map's own VPKs, all of its mips.
fn plan_vpk(vpk: &Vpk, vpk_name: &str, entry: &str, semantic: Semantic) -> Result<Option<Planned>> {
    if vpk.entry(entry).is_none() {
        return Ok(None);
    }
    let bytes = vpk.read_checked(entry).with_context(|| format!("{entry} in {vpk_name}"))?.into_owned();
    let vtex = nades_source2::parse(&bytes).with_context(|| entry.to_string())?;
    let (format, _) = textures::usable(&vtex, semantic, u32::MAX).map_err(|e| anyhow::anyhow!("{entry}: {e}"))?;
    let source = TextureSource::MapVpk { vpk: vpk_name.to_string(), entry: entry.to_string() };
    Ok(Some(textures::from_vtex(entry.to_string(), &vtex, format, false, semantic, source, Fill::Vtex { bytes, first: 0 })))
}

/// A map's (or skybox's) three lightmaps from its VPK, when it has them. `sun_index` is the sun's
/// `bakedshadowindex`: its channel in the shadow mask (−1 for none).
fn plan_lightmaps(plan: &mut Plan, vpk: &Vpk, vpk_name: &str, prefix: &str, sun_index: i32) -> Result<Option<Lightmaps>> {
    let Some(irradiance) = plan_vpk(vpk, vpk_name, &format!("{prefix}/lightmaps/irradiance.vtex_c"), Semantic::Irradiance)? else { return Ok(None) };
    let irradiance = plan.push(irradiance);
    let directional = plan_vpk(vpk, vpk_name, &format!("{prefix}/lightmaps/directional_irradiance.vtex_c"), Semantic::IrradianceDirection)?.map(|p| plan.push(p));
    let shadows = plan_vpk(vpk, vpk_name, &format!("{prefix}/lightmaps/direct_light_shadows.vtex_c"), Semantic::Shadows)?;
    let channels = shadows.as_ref().and_then(|p| nades_pack::scene::shadow_channels(p.texture.format)).unwrap_or(0);
    let sun_channel = u8::try_from(sun_index).ok().filter(|&c| c < channels);
    let shadows = shadows.map(|p| plan.push(p));
    Ok(Some(Lightmaps { irradiance, directional, shadows, sun_channel }))
}

/// Builds the pack and writes it to `inputs.out` (through a temporary name).
pub fn build(inputs: &Inputs) -> Result<Report> {
    let mut stages = Vec::new();
    let mut clock = Instant::now();
    let mut stage = |name: &'static str| {
        stages.push((name, clock.elapsed().as_secs_f64() * 1e3));
        clock = Instant::now();
    };

    // the entity lumps, the skies' materials and the post-processing, decompiled by the CLI
    let pak = if inputs.pak().is_file() { Some(Vpk::open(&inputs.pak()).with_context(|| format!("the game's archive {}", inputs.pak().display()))?) } else { None };
    let lump = match inputs.cli() {
        Some(cli) => {
            let mut decompiler = lump::Decompiler::new(cli, &std::env::temp_dir(), &format!("lump-{}", inputs.map))?;
            lump::read(&mut decompiler, &lump::Game { maps: inputs.csgo().join("maps"), pak: pak.as_ref(), pak_path: inputs.pak() }, &inputs.map)?
        }
        None => lump::Lump::default(),
    };
    let record = |value: &serde_json::Value| (!value.is_null()).then(|| value.clone());
    let skybox_file: Option<post::SkyboxFile> = record(&lump.skybox).map(serde_json::from_value).transpose().context("the 3D skybox's record")?;
    stage("the entity lumps, skies and post-processing");
    // the geometry: the export given, else the packer's own from the game's files
    let exported = match &inputs.export {
        Some(_) => None,
        None => Some(make_exports(inputs, pak.as_ref(), skybox_file.as_ref().map(|f| f.prefab.as_str()))?),
    };
    stage("the geometry export");
    let (map, export_hash) = Export::open(&inputs.glb())?;
    let skybox_glb = inputs.beside(".skybox.glb");
    let sky_export = match (&skybox_file, skybox_glb.is_file()) {
        (Some(_), true) => Some(Export::open(&skybox_glb)?.0),
        _ => None,
    };
    let exports: Vec<&Export> = std::iter::once(&map).chain(sky_export.as_ref()).collect();
    stage("read, parse and decode the exports");

    // materials of every export, in one table: the map's, then the skybox's, then glTF's default for primitives
    // without one (only when some primitive needs it)
    let mut materials: Vec<Material> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut wants: Vec<(u32, usize, Want)> = Vec::new();
    let mut base_image: Vec<Option<(usize, usize)>> = Vec::new();
    let mut first_material = Vec::new();
    let mut skipped: BTreeMap<String, u32> = BTreeMap::new();
    for (e, export) in exports.iter().enumerate() {
        first_material.push(materials.len() as u32);
        for (i, m) in export.doc.materials.iter().enumerate() {
            let described = materials::describe(&export.doc, i);
            let info = m.pbr_metallic_roughness.as_ref().and_then(|p| p.base_color_texture.as_ref());
            if info.is_some_and(|t| t.tex_coord != 0) {
                *skipped.entry("base colour on a UV set other than TEXCOORD_0 (drawn with TEXCOORD_0)".into()).or_default() += 1;
            }
            let image = info.map(|t| export.doc.textures.get(t.index).and_then(gltf::Texture::image).with_context(|| format!("material {i}'s base colour texture {} has no image", t.index))).transpose()?;
            let index = materials.len() as u32;
            wants.extend(described.wants.into_iter().map(|w| (index, e, w)));
            base_image.push(image.map(|image| (e, image)));
            groups.push(described.group);
            materials.push(described.material);
        }
    }
    let default_material = materials.len() as u32;
    materials.push(Material {
        name: "glTF default".into(),
        shader: String::new(),
        shading: Shading::Generic,
        base_color: None,
        base_color_factor: [1.0; 4],
        normal: None,
        occlusion: None,
        alpha: AlphaMode::Opaque,
        double_sided: false,
    });
    groups.push(Group::Opaque);
    base_image.push(None);

    // which world-node meshes the game lights by its lightmap, the map's and the skybox prefab's
    let mut meshes: HashMap<String, bool> = HashMap::new();
    let mut asked_game = false;
    if let Some(cli) = inputs.cli() {
        let mut ask = |vpk: &Path, prefix: &str| -> Result<()> {
            if vpk.is_file() {
                meshes.extend(world::lightmapped_meshes(cli, vpk, prefix)?);
                asked_game = true;
            }
            Ok(())
        };
        ask(&inputs.vpk(), &format!("maps/{}", inputs.map))?;
        if let Some(file) = &skybox_file {
            ask(&inputs.map_vpk(&file.prefab), &format!("maps/{}", file.prefab))?;
        }
    }
    let lightmapped = |mesh: &str| meshes.get(world::mesh_of(mesh)).copied();
    let mut geometry = Geometry::default();
    let mut counts = Vec::new();
    for (e, export) in exports.iter().enumerate() {
        let table: Vec<u32> = (0..export.doc.materials.len() as u32).map(|i| first_material[e] + i).collect();
        counts.push(geometry::build(&export.doc, &export.buffers, &mut geometry, &table, &groups, default_material, &lightmapped)?);
    }
    geometry.bytes.resize(nades_pack::scene::align_up(geometry.bytes.len() as u64, nades_pack::ALIGN) as usize, 0);
    for c in &counts {
        for (reason, count) in &c.skipped {
            *skipped.entry(format!("primitive: {reason}")).or_default() += count;
        }
    }
    if !counts.iter().any(|c| c.draws.iter().any(|d| d.material == default_material)) {
        materials.pop();
        groups.pop();
        base_image.pop();
    }
    stage("draws and the Geometry section");

    let game = GameIndex::scan(inputs.unpacked.as_deref(), Some(&inputs.pak()))?;
    stage("index the game's files");

    // base colours: one per image some material uses as its base colour, as M1 found them
    let mut plan = Plan::default();
    let mut users: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for (m, image) in base_image.iter().enumerate() {
        if let Some(key) = image {
            users.entry(*key).or_default().push(m);
        }
    }
    let mut fallbacks = Vec::new();
    let mut by_name = [0u32; 3];
    for (&(e, image), users) in &users {
        let export = exports[e];
        let local: Vec<usize> = users.iter().map(|&m| m - first_material[e] as usize).collect();
        match plan_base_color(&export.doc, &export.buffers, &game, image, &local) {
            Ok((p, fallback)) => {
                match &p.texture.source {
                    TextureSource::ByName { .. } => by_name[0] += 1,
                    TextureSource::ByTextureParams { .. } => by_name[1] += 1,
                    _ => by_name[2] += 1,
                }
                fallbacks.extend(fallback.map(|why| format!("{}: {why}", p.texture.name)));
                let t = plan.push(p);
                plan.by_image.insert((e, image), t);
            }
            Err(reason) => *skipped.entry(format!("base colour image: {reason}")).or_default() += 1,
        }
    }
    for (m, image) in base_image.iter().enumerate() {
        if let Some(&t) = image.as_ref().and_then(|key| plan.by_image.get(key)) {
            materials::fill(&mut materials[m], Slot::BaseColor, t);
        }
    }
    stage("base colour sources");

    // every other material texture: the game's file its TextureParams names, else the export's image for the slot
    let mut missing = Vec::new();
    let mut missing_count = 0u32;
    for (m, e, want) in &wants {
        let mut why = String::new();
        let mut texture = None;
        if let Some(path) = &want.path {
            let key = (path.clone(), want.semantic, want.max_size);
            if let Some(&t) = plan.by_file.get(&key) {
                texture = Some(t);
            } else {
                match plan_game(&game, want, path) {
                    Ok(Some(p)) => {
                        let t = plan.push(p);
                        plan.by_file.insert(key, t);
                        plan.game_textures += 1;
                        texture = Some(t);
                    }
                    Ok(None) => why = format!("{path} is neither unpacked nor in a pak01 archive the depot holds"),
                    Err(reason) => why = reason,
                }
            }
        }
        if texture.is_none()
            && let Some(image) = want.image
        {
            let semantic = if want.semantic == Semantic::NormalHemiOct { Semantic::NormalRgb } else { want.semantic };
            let key = (*e, image, semantic);
            if let Some(&t) = plan.by_export.get(&key) {
                texture = Some(t);
            } else {
                let export = exports[*e];
                match image_bytes(&export.doc, &export.buffers, image).and_then(|webp| textures::from_webp(export.doc.images[image].name.clone().unwrap_or_else(|| format!("image {image}")), image as u32, webp.to_vec(), want.srgb, semantic)) {
                    Ok(p) => {
                        let t = plan.push(p);
                        plan.by_export.insert(key, t);
                        plan.export_textures += 1;
                        texture = Some(t);
                    }
                    Err(e) => why = format!("{why}; the export's image {image}: {e:#}"),
                }
            }
        }
        match texture {
            Some(t) => materials::fill(&mut materials[*m as usize], want.slot, t),
            // a one-texel default (default_ao, default_normal) is as good as none
            None if want.path.as_deref().is_some_and(|p| p.contains("/default/")) => {}
            None => {
                missing_count += 1;
                if !why.is_empty() {
                    missing.push(format!("{} {}: {why}", materials[*m as usize].name, want.param));
                }
            }
        }
    }
    stage("material texture sources");

    let vpk = Vpk::open(&inputs.vpk()).with_context(|| format!("the map's VPK {}", inputs.vpk().display()))?;
    let vpk_name = format!("maps/{}.vpk", inputs.map);
    let entities = entities::parse(&lump.entities);
    let sun_index = entities::sun_shadow_index(&entities).unwrap_or(0);
    let lightmaps = plan_lightmaps(&mut plan, &vpk, &vpk_name, &format!("maps/{}", inputs.map), sun_index)?;
    let probe_atlas = plan_vpk(&vpk, &vpk_name, &format!("maps/{}/lightmaps/env_light_probe_volume_atlas.vtex_c", inputs.map), Semantic::ProbeIrradiance)?;
    let probes = match probe_atlas {
        Some(atlas) => {
            let grid = glam::Vec3::new(atlas.texture.width as f32, atlas.texture.height as f32, (atlas.texture.layers / 6) as f32);
            let volumes = entities::probe_volumes(&entities, grid);
            let irradiance = plan.push(atlas);
            let shadows = plan_vpk(&vpk, &vpk_name, &format!("maps/{}/lightmaps/env_light_probe_volume_atlas_dlshd.vtex_c", inputs.map), Semantic::ProbeShadows)?.map(|p| plan.push(p));
            (!volumes.is_empty()).then_some(nades_pack::scene::Probes { irradiance, shadows, volumes })
        }
        None => None,
    };
    if lightmaps.is_none() {
        bail!("{} holds no irradiance lightmap", inputs.vpk().display());
    }
    let skybox_lightmaps = match &skybox_file {
        Some(file) if sky_export.is_some() => {
            let name = format!("maps/{}.vpk", file.prefab);
            let path = inputs.map_vpk(&file.prefab);
            // the prefab's own sun, when it bakes one, is its first baked light
            if path.is_file() { plan_lightmaps(&mut plan, &Vpk::open(&path).with_context(|| format!("the skybox's VPK {}", path.display()))?, &name, &format!("maps/{}", file.prefab), 0)? } else { None }
        }
        _ => None,
    };
    stage("lightmaps from the VPKs");

    let post_file: post::PostFile = record(&lump.post).map(serde_json::from_value).transpose().context("the post-processing's record")?.unwrap_or_default();
    let post = post::post(&post_file, lump.lut.as_deref())?;
    let sky_file: Option<post::SkyFile> = record(&lump.sky).map(serde_json::from_value).transpose().context("the sky's record")?;
    let mut sky = None;
    if let Some(file) = &sky_file
        && let Some(pak) = &pak
        && let Some(path) = lump::sky_texture(lump.sky_vmat.as_deref(), &file.material, pak)
    {
        let want = Want { slot: Slot::BaseColor, param: "g_tSkyTexture".into(), path: Some(path.clone()), image: None, semantic: Semantic::Sky, srgb: false, max_size: u32::MAX };
        match plan_game(&game, &want, &path) {
            Ok(Some(p)) => sky = Some(Sky { texture: plan.push(p), yaw: file.yaw, tint: post::sky_tint(file, post_file.sky.as_ref()) }),
            Ok(None) => missing.push(format!("the sky {path}: not in the game files")),
            Err(why) => missing.push(format!("the sky: {why}")),
        }
    }
    let sun = map.sun()?;
    stage("sky, sun and post-processing");

    // the upload order: largest first
    let mut order: Vec<usize> = (0..plan.planned.len()).collect();
    order.sort_by(|&a, &b| plan.planned[b].texture.bytes().cmp(&plan.planned[a].texture.bytes()).then_with(|| plan.planned[a].texture.name.cmp(&plan.planned[b].texture.name)));
    let mut new_index = vec![0u32; order.len()];
    for (to, &from) in order.iter().enumerate() {
        new_index[from] = to as u32;
    }
    let mut slots: Vec<Option<Planned>> = plan.planned.into_iter().map(Some).collect();
    let mut planned: Vec<Planned> = order.iter().map(|&i| slots[i].take().expect("each texture once")).collect();
    let remap = |t: &mut Option<u32>| {
        if let Some(i) = t {
            *i = new_index[*i as usize];
        }
    };
    for m in &mut materials {
        remap(&mut m.base_color);
        remap(&mut m.normal);
        remap(&mut m.occlusion);
        match &mut m.shading {
            Shading::Environment(e) => {
                for l in &mut e.layers {
                    remap(&mut l.color);
                    remap(&mut l.normal);
                    remap(&mut l.height);
                }
            }
            Shading::Layered(l) => {
                remap(&mut l.color2);
                remap(&mut l.modulation);
            }
            Shading::Generic | Shading::Effects(_) | Shading::Unlit { .. } | Shading::Black => {}
        }
    }
    let remap_lightmaps = |l: Lightmaps| Lightmaps { irradiance: new_index[l.irradiance as usize], directional: l.directional.map(|t| new_index[t as usize]), shadows: l.shadows.map(|t| new_index[t as usize]), ..l };
    let lightmaps = lightmaps.map(remap_lightmaps);
    let probes = probes.map(|p| nades_pack::scene::Probes { irradiance: new_index[p.irradiance as usize], shadows: p.shadows.map(|t| new_index[t as usize]), ..p });
    let skybox_lightmaps = skybox_lightmaps.map(remap_lightmaps);
    let sky = sky.map(|s| Sky { texture: new_index[s.texture as usize], ..s });
    let mut records: Vec<_> = planned.iter().map(|p| p.texture.clone()).collect();
    let chunks = textures::assign_chunks(&mut records, CHUNK_TARGET);
    for (p, t) in planned.iter_mut().zip(records) {
        p.texture = t;
    }
    let textures_section = textures::fill(&planned, &chunks, &game)?;
    stage("fill the Textures section");

    let textures: Vec<_> = planned.into_iter().map(|p| p.texture).collect();
    let mut counts = counts.into_iter();
    let map_draws = counts.next().expect("the map's draws");
    let sky_draws = counts.next();
    let all_draws: Vec<&Draw> = map_draws.draws.iter().chain(sky_draws.iter().flat_map(|s| s.draws.iter())).collect();
    let sum = |f: fn(&geometry::Draws) -> u32| f(&map_draws) + sky_draws.as_ref().map_or(0, f);
    let mut shading = [0u32; 6];
    for m in &materials {
        shading[materials::family(&m.shading)] += 1;
    }
    let stats = Stats {
        images: exports.iter().map(|e| e.doc.images.len() as u32).sum(),
        base_color_images: users.len() as u32,
        by_name: by_name[0],
        by_texture_params: by_name[1],
        by_fallback: by_name[2],
        material_textures_game: plan.game_textures,
        material_textures_export: plan.export_textures,
        material_textures_missing: missing_count,
        draws: map_draws.draws.len() as u32,
        skybox_draws: sky_draws.as_ref().map_or(0, |s| s.draws.len() as u32),
        instanced_draws: all_draws.iter().filter(|d| d.instances.is_some()).count() as u32,
        instances: all_draws.iter().filter_map(|d| d.instances.map(|i| i.count)).sum(),
        lightmapped_texcoord2: sum(|d| d.texcoord2),
        lightmapped_texcoord1: sum(|d| d.texcoord1),
        unlit: sum(|d| d.unlit),
        lightmap_rule_differs_from_web: sum(|d| d.web_differs),
        probe_lit_by_game: sum(|d| d.probe_lit_by_game),
        lightmapped_unknown: sum(|d| d.lightmapped_unknown),
        mirrored: sum(|d| d.mirrored),
        triangles: all_draws.iter().map(|d| d.triangles()).sum(),
        vertices: all_draws.iter().map(|d| (d.positions, d.vertex_count)).collect::<HashMap<_, _>>().values().map(|&v| u64::from(v)).sum(),
        shading,
        skipped: skipped.into_iter().map(|(reason, count)| Skipped { reason, count }).collect(),
    };
    let texcoord2_outside = all_draws
        .iter()
        .filter(|d| d.lightmap.is_some_and(|l| l.from == nades_pack::scene::UvSet::Texcoord2))
        .filter(|d| {
            let uv = d.lightmap.expect("filtered").uv.of(&geometry.bytes);
            uv.chunks_exact(4).any(|c| {
                let x = f32::from_le_bytes(c.try_into().expect("4 bytes"));
                !(-0.001..=1.001).contains(&x)
            })
        })
        .count() as u32;
    let missing_uv0 = sum(|d| d.missing_uv0);
    let missing_frames = sum(|d| d.missing_frames);
    let skybox = match (skybox_file, sky_draws) {
        (Some(file), Some(draws)) => Some(Skybox {
            draws: draws.draws,
            lightmaps: skybox_lightmaps,
            scale: if file.scale > 0.0 { file.scale } else { 1.0 },
            origin: post::scene_point(file.origin),
            reference: post::scene_point(file.reference),
            gradient_fog: file.gradient_fog.as_ref().map(post::gradient_fog),
        }),
        _ => None,
    };
    let scene = Scene { draws: map_draws.draws, materials, textures, chunks, lightmaps, probes, sun, sky, skybox, post, stats };
    scene.validate(geometry.bytes.len() as u64, textures_section.len() as u64).map_err(|e| anyhow::anyhow!("the scene the packer made is invalid: {e}"))?;
    stage("scene and its checks");

    let collider = match inputs.cli() {
        Some(cli) => physics::of_map(cli, &inputs.vpk(), &inputs.map, &std::env::temp_dir()).map_err(|e| format!("{e:#}")),
        None => Err(no_cli(inputs)),
    };
    let collider_bytes = collider.as_ref().ok().map(physics::Physics::to_awmh);
    stage("the collider from the map's physics");

    let source = Source { map: inputs.map.clone(), game_build: game_build(&inputs.game), export_hash: format!("{export_hash:016x}") };
    let scene_bytes = scene.to_bytes();
    if let Some(dir) = inputs.out.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut sections: Vec<(SectionKind, &[u8])> = vec![(SectionKind::Scene, &scene_bytes), (SectionKind::Geometry, &geometry.bytes), (SectionKind::Textures, &textures_section)];
    if let Some(bytes) = &collider_bytes {
        sections.push((SectionKind::Collider, bytes));
    }
    nades_pack::write(&inputs.out, source.clone(), &sections).with_context(|| format!("writing {}", inputs.out.display()))?;
    stage("write the pack");
    let file_size = std::fs::metadata(&inputs.out)?.len();
    Ok(Report {
        sections: sections.iter().map(|(k, b)| (*k, b.len() as u64)).collect(),
        collider,
        scene,
        source,
        file_size,
        stages,
        texcoord2_outside,
        missing_uv0,
        missing_frames,
        game_meshes: asked_game.then(|| (meshes.values().filter(|&&l| l).count(), meshes.len())),
        game_files: game.len(),
        decompiled: lump.runs,
        exported: exported.unwrap_or_default(),
        pak_entries: game.pak_entries(),
        fallbacks,
        missing,
    })
}

/// An image's encoded bytes in the glb.
fn image_bytes<'b>(doc: &gltf::Document, buffers: &'b [Cow<'_, [u8]>], image: usize) -> Result<&'b [u8]> {
    let view = doc.images.get(image).and_then(|i| i.buffer_view).with_context(|| format!("image {image} is not in the glb"))?;
    let v = doc.buffer_views.get(view).with_context(|| format!("image {image} names view {view}"))?;
    buffers.get(v.buffer).and_then(|b| b.get(v.byte_offset..v.byte_offset + v.byte_length)).with_context(|| format!("image {image} lies outside its buffer"))
}

/// Where one base colour image's texture comes from: the game file its name finds, else the one its materials'
/// `TextureParams` name, else the export's WebP (with why no game file would do). `Err` says why none would do.
fn plan_base_color(doc: &gltf::Document, buffers: &[Cow<'_, [u8]>], index: &GameIndex, image: usize, users: &[usize]) -> Result<(Planned, Option<String>), String> {
    let name = doc.images[image].name.clone().unwrap_or_default();
    let label = if name.is_empty() { format!("image {image}") } else { name.clone() };
    let webp = image_bytes(doc, buffers, image).map_err(|e| format!("{label}: {e:#}"))?;
    let is_webp = doc.images[image].mime_type.as_deref() == Some("image/webp") || webp.starts_with(b"RIFF");
    let expected = if is_webp { Some(textures::webp_size(webp).map_err(|e| format!("{label}: {e:#}"))?) } else { None };
    let try_file = |file: &GameFile| -> Result<(nades_source2::Vtex, Vec<u8>, TextureFormat, u32), String> {
        let bytes = index.read(file).map_err(|e| format!("{e:#}"))?;
        let vtex = nades_source2::parse(&bytes).map_err(|e| format!("{}: {e}", index.name(file)))?;
        let (format, first) = textures::fits(&vtex, expected).map_err(|e| format!("{}: {e}", index.name(file)))?;
        Ok((vtex, bytes, format, first))
    };
    let fill = |file: &GameFile, first: u32| match file {
        GameFile::Disk(path) => Fill::GameFile { path: path.clone(), first },
        GameFile::Pak(entry) => Fill::Pak { entry: entry.clone(), first },
    };
    let mut why = Vec::new();
    match index.by_name(&name) {
        ByName::Found(file) => match try_file(&file) {
            Ok((vtex, _, format, first)) => {
                let source = TextureSource::ByName { file: index.name(&file), first_mip: first };
                return Ok((textures::from_vtex(name, &vtex, format, true, Semantic::Color, source, fill(&file, first)), None));
            }
            Err(e) => why.push(e),
        },
        ByName::Ambiguous(n) => why.push(format!("{n} game files are named {name}")),
        ByName::Missing => why.push(format!("no game file is named {name}")),
        ByName::NotAGameName => {}
    }
    for &m in users {
        let Some(material) = doc.materials.get(m) else { continue };
        for param in textures::BASE_COLOR_PARAMS {
            let Some(path) = material.texture_param(param) else { continue };
            let file_name = path.rsplit('/').next().unwrap_or(path);
            if !name.is_empty() && file_name != name {
                continue;
            }
            let Some(file) = index.by_path(path) else {
                why.push(format!("{param} names {path}, which is neither unpacked nor in a pak01 archive the depot holds"));
                continue;
            };
            match try_file(&file) {
                Ok((vtex, _, format, first)) => {
                    let source = TextureSource::ByTextureParams { param: param.to_string(), file: index.name(&file), first_mip: first };
                    return Ok((textures::from_vtex(label, &vtex, format, true, Semantic::Color, source, fill(&file, first)), None));
                }
                Err(e) => why.push(e),
            }
        }
    }
    why.dedup();
    let why = if why.is_empty() { "the image is not named after a game texture".to_string() } else { why.join("; ") };
    if is_webp {
        return textures::from_webp(label, image as u32, webp.to_vec(), true, Semantic::Color).map(|p| (p, Some(why))).map_err(|e| format!("{e:#}"));
    }
    Err(format!("{label}: no game file ({why}) and a {} image the packer does not decode", doc.images[image].mime_type.as_deref().unwrap_or("non-WebP")))
}
