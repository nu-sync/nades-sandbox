//! The pack's textures: each one the game's own blocks where a `.vtex_c` stands for it, else the export's image
//! decoded to RGBA8; then their place in the Textures section (chunks of ≈32 MiB) and the bytes themselves.
//!
//! - **Base colours** follow show-me-the-nades' `scripts/bc_textures.py`: the image's name is the game texture's
//!   (`<name>.vtex`), found once under `game/csgo`; mip 0 is the first of the file's mips whose longest side is at
//!   most 1024, as the export resized it, and that level's size must be the export image's. When the name finds no
//!   single file, the materials that use the image name its source in their `TextureParams` (`g_tColor`,
//!   `g_tColor1` or `g_tLayer1Color`, by shader), by its full path.
//! - **Every other material texture** (normal maps, the layers' colours and height maps, occlusion, masks) is the file
//!   its material's `TextureParams` names, read from the unpacked game folder or else from the game's `pak01`
//!   archives in the depot, from the first mip that fits its role's size ([`crate::materials::DETAIL_SIZE`],
//!   [`crate::materials::DATA_SIZE`]).
//! - **The lightmaps** come from the map's VPK, **the sky** (a BC6H cube map) from `pak01`, whole.
//!
//! Nothing is made up: a texture with neither a game file nor an export image is left out and reported, and its
//! slot drawn with a neutral default.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use memmap2::Mmap;
use nades_pack::scene::{CHUNK_ALIGN, Chunk, Mip, ROW_ALIGN, Semantic, Texture, TextureFormat, TextureSource, align_up, mip_chain};
use nades_source2::vpk::Vpk;
use nades_source2::{Format, Vtex};
use rayon::prelude::*;

/// Base colour images are resized by the export so that their longest side fits this.
pub const MAX_SIZE: u32 = 1024;
/// Chunks close once they would pass this.
pub const CHUNK_TARGET: u64 = 32 << 20;
/// The `TextureParams` entries that hold a material's base colour, by shader: `g_tColor` (csgo_complex,
/// csgo_static_overlay, csgo_vertexlitgeneric, csgo_lightmappedgeneric, csgo_foliage, …), `g_tColor1`
/// (csgo_environment and csgo_environment_blend's first layer) and `g_tLayer1Color` (csgo_simple_3layer_parallax).
pub const BASE_COLOR_PARAMS: [&str; 3] = ["g_tColor", "g_tColor1", "g_tLayer1Color"];

/// The game's files: every `.vtex_c` under the unpacked game folder, by file name, and the game's `pak01` archives
/// (the directory and whichever numbered archives the depot holds).
pub struct GameIndex {
    root: PathBuf,
    /// Each texture's stem: the unpacked folder's files, then `pak01`'s entries the depot holds that are not unpacked.
    by_stem: HashMap<String, Vec<GameFile>>,
    pak: Option<Vpk>,
}

/// What an image's name finds under the game folder.
#[derive(Debug, PartialEq, Eq)]
pub enum ByName {
    Found(GameFile),
    /// More than one file has this name.
    Ambiguous(usize),
    Missing,
    /// The image is not named after a game texture (no `.vtex` name, or no name).
    NotAGameName,
}

/// Where a game file's bytes are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameFile {
    /// A file under the unpacked game folder.
    Disk(PathBuf),
    /// An entry of `pak01`.
    Pak(String),
}

impl GameIndex {
    /// Indexes `root` (an unpacked `game/csgo`, when there is one) and `pak` (`pak01_dir.vpk`, when it exists): the
    /// textures of both by name, an unpacked file standing for the `pak01` entry at its path.
    pub fn scan(root: Option<&Path>, pak: Option<&Path>) -> Result<Self> {
        let mut by_stem: HashMap<String, Vec<GameFile>> = HashMap::new();
        let mut unpacked: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut stack: Vec<PathBuf> = root.filter(|r| r.is_dir()).map(Path::to_path_buf).into_iter().collect();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).with_context(|| format!("listing {}", dir.display()))? {
                let path = entry?.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Some(stem) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".vtex_c")) {
                    if let Some(root) = root
                        && let Ok(relative) = path.strip_prefix(root)
                    {
                        unpacked.insert(relative.to_string_lossy().replace('\\', "/"));
                    }
                    by_stem.entry(stem.to_string()).or_default().push(GameFile::Disk(path.clone()));
                }
            }
        }
        let pak = match pak {
            Some(p) if p.is_file() => Some(Vpk::open(p).with_context(|| format!("the game's archive {}", p.display()))?),
            _ => None,
        };
        if let Some(pak) = &pak {
            for entry in pak.entries() {
                let Some(stem) = entry.path.rsplit('/').next().and_then(|n| n.strip_suffix(".vtex_c")) else { continue };
                if unpacked.contains(&entry.path) || !held(pak, entry) {
                    continue;
                }
                by_stem.entry(stem.to_string()).or_default().push(GameFile::Pak(entry.path.clone()));
            }
        }
        Ok(Self { root: root.map(Path::to_path_buf).unwrap_or_default(), by_stem, pak })
    }

    /// An index over the given files (relative to `root`), for tests.
    pub fn of(root: &Path, files: &[&str]) -> Self {
        let mut by_stem: HashMap<String, Vec<GameFile>> = HashMap::new();
        for f in files {
            let path = root.join(f);
            let stem = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".vtex_c")).expect("a .vtex_c").to_string();
            by_stem.entry(stem).or_default().push(GameFile::Disk(path));
        }
        Self { root: root.to_path_buf(), by_stem, pak: None }
    }

    pub fn len(&self) -> usize {
        self.by_stem.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_stem.is_empty()
    }

    /// Entries in `pak01`'s directory (0 without it).
    pub fn pak_entries(&self) -> usize {
        self.pak.as_ref().map_or(0, |p| p.entries().len())
    }

    /// Every path in `pak01`'s directory.
    pub fn pak_paths(&self) -> Box<dyn Iterator<Item = String> + '_> {
        match &self.pak {
            Some(pak) => Box::new(pak.entries().iter().map(|e| e.path.clone())),
            None => Box::new(std::iter::empty()),
        }
    }

    /// The file an export image's name stands for: `<stem>.vtex` → the one `<stem>.vtex_c`.
    pub fn by_name(&self, image_name: &str) -> ByName {
        let Some(stem) = image_name.strip_suffix(".vtex").map(|n| n.rsplit('/').next().unwrap_or(n)) else { return ByName::NotAGameName };
        match self.by_stem.get(stem).map(Vec::as_slice) {
            Some([one]) => ByName::Found(one.clone()),
            Some(many) if many.len() > 1 => ByName::Ambiguous(many.len()),
            _ => ByName::Missing,
        }
    }

    /// The file a `TextureParams` path names (`materials/…/x.vtex` → `materials/…/x.vtex_c`): unpacked, or in
    /// `pak01`.
    pub fn by_path(&self, param: &str) -> Option<GameFile> {
        self.find(param)
    }

    /// A game path's compiled file (`materials/…/x.vtex`, or any path with `_c` already): under the unpacked folder,
    /// or in `pak01` when the depot holds the archive it is in.
    pub fn find(&self, path: &str) -> Option<GameFile> {
        let compiled = if path.ends_with("_c") { path.to_string() } else { format!("{path}_c") };
        if !self.root.as_os_str().is_empty() {
            let disk = self.root.join(&compiled);
            if disk.is_file() {
                return Some(GameFile::Disk(disk));
            }
        }
        let pak = self.pak.as_ref()?;
        let entry = pak.entry(&compiled)?;
        held(pak, entry).then_some(GameFile::Pak(compiled))
    }

    /// A game file's path relative to the game folder: what the scene records.
    pub fn name(&self, file: &GameFile) -> String {
        match file {
            GameFile::Disk(path) => self.relative(path),
            GameFile::Pak(entry) => entry.clone(),
        }
    }

    /// A game file's bytes.
    pub fn read(&self, file: &GameFile) -> Result<Vec<u8>> {
        match file {
            GameFile::Disk(path) => std::fs::read(path).with_context(|| format!("reading {}", path.display())),
            GameFile::Pak(entry) => Ok(self.pak.as_ref().context("no pak01")?.read_checked(entry).with_context(|| format!("{entry} in pak01"))?.into_owned()),
        }
    }

    /// `path` relative to the game folder, for the scene's records.
    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy().replace('\\', "/")
    }
}

/// Whether the depot holds an entry's bytes: in the directory file, or in a numbered archive that is there.
fn held(pak: &Vpk, entry: &nades_source2::vpk::Entry) -> bool {
    entry.length == 0 || entry.archive == nades_source2::vpk::IN_DIRECTORY || pak.archive_path(entry.archive).is_file()
}

/// The pack's format for a game texture, or none.
pub fn pack_format(format: Format) -> TextureFormat {
    match format {
        Format::Dxt1 => TextureFormat::Bc1,
        Format::Dxt5 => TextureFormat::Bc3,
        Format::Bc7 => TextureFormat::Bc7,
        Format::Rgba8888 => TextureFormat::Rgba8,
        Format::Bc6h => TextureFormat::Bc6h,
        Format::Ati1n => TextureFormat::Bc4,
        Format::Ati2n => TextureFormat::Bc5,
    }
}

/// The pack's format for a game texture used as colour, or none.
pub fn colour_format(format: Format) -> Option<TextureFormat> {
    match format {
        Format::Dxt1 | Format::Dxt5 | Format::Bc7 | Format::Rgba8888 => Some(pack_format(format)),
        Format::Bc6h | Format::Ati1n | Format::Ati2n => None,
    }
}

/// The first of a texture's mips whose longest side is at most `max` (or its last mip).
pub fn first_fitting_mip(width: u32, height: u32, mips: u32, max: u32) -> u32 {
    let mut first = 0;
    while width.max(height) >> first > max && first + 1 < mips {
        first += 1;
    }
    first
}

/// Whether game texture `vtex` can stand for an export image of `expected` size: its format, its first mip that fits
/// [`MAX_SIZE`], and that mip's size. Returns the format and that mip, or why not.
pub fn fits(vtex: &Vtex, expected: Option<(u32, u32)>) -> Result<(TextureFormat, u32), String> {
    if !vtex.is_2d() {
        return Err("a cube, volume or layered texture".into());
    }
    let format = colour_format(vtex.format).ok_or_else(|| format!("{:?} is not a colour format", vtex.format))?;
    let first = first_fitting_mip(vtex.width, vtex.height, vtex.mips, MAX_SIZE);
    let (w, h) = vtex.mip_dimensions(first);
    if format != TextureFormat::Rgba8 && (w % 4 != 0 || h % 4 != 0) {
        return Err(format!("mip {first} is {w}x{h}: not whole 4x4 blocks"));
    }
    if let Some((ew, eh)) = expected
        && (ew, eh) != (w, h)
    {
        return Err(format!("the export's image is {ew}x{eh}, the game's mip {first} {w}x{h}"));
    }
    Ok((format, first))
}

/// Whether game texture `vtex` can be used as `semantic`, and from which mip so that its longest side fits
/// `max_size`: returns the format and that mip, or why not. Mips below a whole 4×4 block are never the first one.
pub fn usable(vtex: &Vtex, semantic: Semantic, max_size: u32) -> Result<(TextureFormat, u32), String> {
    let (cube, volume) = (semantic == Semantic::Sky, matches!(semantic, Semantic::ProbeIrradiance | Semantic::ProbeShadows));
    let shape = if cube { vtex.is_cube() } else if volume { vtex.is_volume() } else { vtex.is_2d() };
    if !shape {
        return Err(format!("flags {:#x}, depth {}: not a {}", vtex.flags, vtex.depth, if cube { "cube map" } else if volume { "volume" } else { "2D texture" }));
    }
    let format = pack_format(vtex.format);
    let allowed = match semantic {
        Semantic::Color => colour_format(vtex.format).is_some(),
        Semantic::NormalHemiOct | Semantic::Data => !matches!(vtex.format, Format::Bc6h),
        Semantic::NormalRgb => false,
        Semantic::Irradiance | Semantic::Sky | Semantic::ProbeIrradiance => vtex.format == Format::Bc6h,
        Semantic::ProbeShadows => nades_pack::scene::shadow_channels(format).is_some(),
        Semantic::IrradianceDirection => vtex.format == Format::Dxt1,
        Semantic::Shadows => nades_pack::scene::shadow_channels(format).is_some(),
    };
    if !allowed {
        return Err(format!("{:?} cannot hold {semantic:?}", vtex.format));
    }
    let mut first = if volume { 0 } else { first_fitting_mip(vtex.width, vtex.height, vtex.mips, max_size) };
    let whole = |m: u32| {
        let (w, h) = vtex.mip_dimensions(m);
        format == TextureFormat::Rgba8 || (w % 4 == 0 && h % 4 == 0)
    };
    while first > 0 && !whole(first) {
        first -= 1;
    }
    if !whole(first) {
        let (w, h) = vtex.mip_dimensions(first);
        return Err(format!("mip {first} is {w}x{h}: not whole 4x4 blocks"));
    }
    Ok((format, first))
}

/// Where one texture's bytes come from.
pub enum Fill {
    /// A game `.vtex_c` on disk, from mip `first` on.
    GameFile { path: PathBuf, first: u32 },
    /// A game `.vtex_c` in `pak01`, from mip `first` on (read again when the section is filled).
    Pak { entry: String, first: u32 },
    /// Already in memory as a VTEX (a lightmap, from a map's VPK), from mip `first` on.
    Vtex { bytes: Vec<u8>, first: u32 },
    /// The export's WebP, decoded and mipmapped here.
    Webp { bytes: Vec<u8> },
}

impl Fill {
    /// The source's first mip level that the pack keeps.
    fn first(&self) -> u32 {
        match self {
            Fill::GameFile { first, .. } | Fill::Pak { first, .. } | Fill::Vtex { first, .. } => *first,
            Fill::Webp { .. } => 0,
        }
    }
}

/// A texture that will be in the pack: its record (chunk and offsets set by [`assign_chunks`]) and its bytes' source.
pub struct Planned {
    pub texture: Texture,
    pub fill: Fill,
}

/// Plans a texture from a game VTEX: every level from the fill's first, every face (a volume: its first level alone).
pub fn from_vtex(name: String, vtex: &Vtex, format: TextureFormat, srgb: bool, semantic: Semantic, source: TextureSource, fill: Fill) -> Planned {
    let first = fill.first();
    let (width, height) = vtex.mip_dimensions(first);
    let layers = vtex.slices(first);
    let levels = if vtex.is_volume() { 1 } else { vtex.mips - first };
    let (mips, _) = mip_chain(format, width, height, levels, layers);
    Planned { texture: Texture { name, format, srgb, semantic, width, height, layers, volume: vtex.is_volume(), chunk: 0, mips, source }, fill }
}

/// Plans an RGBA8 texture from the export's WebP: a full mip chain.
pub fn from_webp(name: String, image: u32, webp: Vec<u8>, srgb: bool, semantic: Semantic) -> Result<Planned> {
    let (width, height) = webp_size(&webp)?;
    let levels = 32 - width.max(height).leading_zeros();
    let (mips, _) = mip_chain(TextureFormat::Rgba8, width, height, levels, 1);
    Ok(Planned { texture: Texture { name, format: TextureFormat::Rgba8, srgb, semantic, width, height, layers: 1, volume: false, chunk: 0, mips, source: TextureSource::ExportImage { image } }, fill: Fill::Webp { bytes: webp } })
}

pub fn webp_size(webp: &[u8]) -> Result<(u32, u32)> {
    let decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(webp)).context("a WebP header")?;
    Ok(decoder.dimensions())
}

/// Decodes a WebP to RGBA8, rows top first.
pub fn decode_webp(webp: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(webp)).context("a WebP header")?;
    let (width, height) = decoder.dimensions();
    let mut pixels = vec![0u8; decoder.output_buffer_size().context("a WebP too large to decode")?];
    decoder.read_image(&mut pixels).context("decoding a WebP")?;
    if decoder.has_alpha() {
        return Ok((width, height, pixels));
    }
    Ok((width, height, pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()))
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(l: f32) -> u8 {
    let s = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The next mip of an RGBA8 image: each texel the mean of a 2×2 square (the last row or column repeated for odd
/// sizes), colour averaged in linear light when `srgb`, alpha always linearly.
pub fn half(rgba: &[u8], width: u32, height: u32, srgb: bool) -> (u32, u32, Vec<u8>) {
    let (w, h) = (width as usize, height as usize);
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let table: Vec<f32> = (0..=255u8).map(srgb_to_linear).collect();
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            let texels = [(2 * x, 2 * y), (2 * x + 1, 2 * y), (2 * x, 2 * y + 1), (2 * x + 1, 2 * y + 1)].map(|(sx, sy)| (sy.min(h - 1) * w + sx.min(w - 1)) * 4);
            for c in 0..4 {
                let o = (y * nw + x) * 4 + c;
                out[o] = if srgb && c < 3 {
                    linear_to_srgb(texels.iter().map(|&t| table[rgba[t + c] as usize]).sum::<f32>() / 4.0)
                } else {
                    ((texels.iter().map(|&t| u32::from(rgba[t + c])).sum::<u32>() + 2) / 4) as u8
                };
            }
        }
    }
    (nw as u32, nh as u32, out)
}

/// Places textures in chunks, in their order: a chunk closes before a texture that would take it past `target`, so
/// a texture larger than `target` has a chunk of its own. Sets each texture's chunk and its mips' offsets in it.
pub fn assign_chunks(textures: &mut [Texture], target: u64) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut used = 0u64;
    for t in textures.iter_mut() {
        let size = t.mips.last().map_or(0, |m| m.offset + m.len());
        if chunks.is_empty() || (used > 0 && used + size > target) {
            if let Some(last) = chunks.last_mut() {
                last.len = align_up(used, CHUNK_ALIGN);
            }
            let offset = chunks.last().map_or(0, |c| c.offset + c.len);
            chunks.push(Chunk { offset, len: 0 });
            used = 0;
        }
        t.chunk = chunks.len() as u32 - 1;
        for m in &mut t.mips {
            m.offset += used;
        }
        used = align_up(used + size, u64::from(ROW_ALIGN));
    }
    if let Some(last) = chunks.last_mut() {
        last.len = align_up(used.max(1), CHUNK_ALIGN);
    }
    chunks
}

/// Copies a mip's tightly packed rows (every layer, one after the other) into its padded place.
fn put_rows(dst: &mut [u8], mip: &Mip, tight: &[u8], row_bytes: usize) -> Result<()> {
    let rows = (mip.rows * mip.layers) as usize;
    ensure!(tight.len() == row_bytes * rows, "{} bytes for {rows} rows of {row_bytes}", tight.len());
    for (r, row) in tight.chunks_exact(row_bytes).enumerate() {
        let at = r * mip.bytes_per_row as usize;
        dst[at..at + row_bytes].copy_from_slice(row);
    }
    Ok(())
}

/// Writes one texture's mips into `dst`, its bytes from its first mip's offset on.
fn fill_one(t: &Texture, fill: &Fill, game: &GameIndex, dst: &mut [u8]) -> Result<()> {
    let base = t.mips[0].offset;
    let place = |m: &Mip| (m.offset - base) as usize..(m.offset - base + m.len()) as usize;
    let vtex_mips = |file: &[u8], first: u32, dst: &mut [u8]| -> Result<()> {
        let vtex = nades_source2::parse(file)?;
        ensure!(vtex.slices(first) == t.layers, "{} slices, the record says {}", vtex.slices(first), t.layers);
        for (j, m) in t.mips.iter().enumerate() {
            let level = first + j as u32;
            let mut tight = vec![0u8; vtex.mip_bytes(level)];
            vtex.read_mip(file, level, &mut tight)?;
            let row_bytes = (m.width.div_ceil(t.format.block_dim()) * t.format.block_bytes()) as usize;
            put_rows(&mut dst[place(m)], m, &tight, row_bytes).with_context(|| format!("mip {level}"))?;
        }
        Ok(())
    };
    match fill {
        Fill::GameFile { path, first } => {
            let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
            // SAFETY: the game's files are not written while the pack is built
            let map = unsafe { Mmap::map(&file) }.with_context(|| format!("mapping {}", path.display()))?;
            vtex_mips(&map, *first, dst).with_context(|| format!("{}", path.display()))
        }
        Fill::Pak { entry, first } => {
            let bytes = game.read(&GameFile::Pak(entry.clone()))?;
            vtex_mips(&bytes, *first, dst).with_context(|| entry.clone())
        }
        Fill::Vtex { bytes, first } => vtex_mips(bytes, *first, dst),
        Fill::Webp { bytes } => {
            let (mut w, mut h, mut rgba) = decode_webp(bytes)?;
            ensure!((w, h) == (t.width, t.height), "the WebP decoded to {w}x{h}, not {}x{}", t.width, t.height);
            for (level, m) in t.mips.iter().enumerate() {
                if level > 0 {
                    (w, h, rgba) = half(&rgba, w, h, t.srgb);
                }
                put_rows(&mut dst[place(m)], m, &rgba, w as usize * 4)?;
            }
            Ok(())
        }
    }
}

/// The Textures section: every texture's mips at their chunk and offset, filled in parallel.
pub fn fill(planned: &[Planned], chunks: &[Chunk], game: &GameIndex) -> Result<Vec<u8>> {
    let len = chunks.last().map_or(0, |c| c.offset + c.len) as usize;
    let mut section = vec![0u8; len];
    // carve the section into one slice per texture: in chunk order, the textures' ranges never overlap
    let mut order: Vec<usize> = (0..planned.len()).collect();
    let start = |i: usize| chunks[planned[i].texture.chunk as usize].offset + planned[i].texture.mips[0].offset;
    order.sort_by_key(|&i| start(i));
    let mut rest: &mut [u8] = &mut section;
    let mut consumed = 0u64;
    let mut jobs = Vec::with_capacity(planned.len());
    for i in order {
        let t = &planned[i].texture;
        let (from, size) = (start(i), t.mips.last().map_or(0, |m| m.offset + m.len()) - t.mips[0].offset);
        ensure!(from >= consumed, "textures {i} and the one before overlap");
        let (_, tail) = std::mem::take(&mut rest).split_at_mut((from - consumed) as usize);
        let (slot, tail) = tail.split_at_mut(size as usize);
        rest = tail;
        consumed = from + size;
        jobs.push((i, slot));
    }
    jobs.into_par_iter().try_for_each(|(i, slot)| fill_one(&planned[i].texture, &planned[i].fill, game, slot).with_context(|| format!("texture {} ({})", i, planned[i].texture.name)))?;
    Ok(section)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_name_finds_one_game_file_or_says_why_not() {
        let root = Path::new("/game/csgo");
        let index = GameIndex::of(root, &["materials/a/wall_color_psd_1.vtex_c", "materials/b/dup_tga_2.vtex_c", "materials/c/dup_tga_2.vtex_c"]);
        assert_eq!(index.by_name("wall_color_psd_1.vtex"), ByName::Found(GameFile::Disk(root.join("materials/a/wall_color_psd_1.vtex_c"))));
        assert_eq!(index.by_name("dup_tga_2.vtex"), ByName::Ambiguous(2));
        assert_eq!(index.by_name("gone_psd_3.vtex"), ByName::Missing);
        assert_eq!(index.by_name("default_ao_tga_559f1ac6_orm_1658198406.png"), ByName::NotAGameName);
        assert_eq!(index.by_name(""), ByName::NotAGameName);
        assert_eq!(index.relative(&root.join("materials/a/x.vtex_c")), "materials/a/x.vtex_c");
        // without the folder on disk or pak01, a path finds nothing
        assert_eq!(index.find("materials/a/wall_color_psd_1.vtex"), None);
    }

    fn vtex(format: Format, width: u32, height: u32, mips: u32) -> Vtex {
        Vtex { width, height, depth: 1, format, mips, flags: 0, stored: vec![0; mips as usize], lz4: true, data_start: 0 }
    }

    #[test]
    fn the_first_mip_that_fits_1024_must_be_the_exports_size() {
        assert_eq!(first_fitting_mip(2048, 2048, 12, 1024), 1);
        assert_eq!(first_fitting_mip(4096, 1024, 13, 1024), 2);
        assert_eq!(first_fitting_mip(512, 512, 10, 1024), 0);
        // a chain that stops early keeps its last mip
        assert_eq!(first_fitting_mip(4096, 4096, 2, 1024), 1);
        assert_eq!(fits(&vtex(Format::Bc7, 2048, 1024, 12), Some((1024, 512))), Ok((TextureFormat::Bc7, 1)));
        assert_eq!(fits(&vtex(Format::Dxt1, 512, 512, 10), None), Ok((TextureFormat::Bc1, 0)));
        assert!(fits(&vtex(Format::Bc7, 2048, 2048, 12), Some((512, 512))).is_err(), "a size the export did not make");
        assert!(fits(&vtex(Format::Ati1n, 1024, 1024, 11), None).is_err(), "a mask is not a colour");
        assert!(fits(&vtex(Format::Bc7, 1024, 6, 11), None).is_err(), "partial blocks");
        let mut cube = vtex(Format::Bc7, 256, 256, 9);
        cube.flags = nades_source2::flags::CUBE;
        assert!(fits(&cube, None).is_err());
    }

    #[test]
    fn each_role_takes_the_formats_it_can_read_from_the_mip_that_fits_it() {
        // a 1024² height map kept at 512, a normal map at 1024
        assert_eq!(usable(&vtex(Format::Bc7, 1024, 1024, 11), Semantic::Data, 512), Ok((TextureFormat::Bc7, 1)));
        assert_eq!(usable(&vtex(Format::Bc7, 1024, 1024, 11), Semantic::NormalHemiOct, 1024), Ok((TextureFormat::Bc7, 0)));
        assert_eq!(usable(&vtex(Format::Ati1n, 64, 64, 7), Semantic::Data, 512), Ok((TextureFormat::Bc4, 0)));
        // a chain of small mips never starts below a block: a 4x2 texture cannot be cut down
        assert!(usable(&vtex(Format::Bc7, 4, 2, 1), Semantic::Data, 512).is_err());
        assert_eq!(usable(&vtex(Format::Bc7, 16, 16, 5), Semantic::Data, 2), Ok((TextureFormat::Bc7, 2)));
        // the lightmaps' and the sky's formats, and only theirs
        assert!(usable(&vtex(Format::Bc6h, 64, 64, 1), Semantic::Irradiance, 8192).is_ok());
        assert!(usable(&vtex(Format::Dxt1, 64, 64, 1), Semantic::IrradianceDirection, 8192).is_ok());
        assert!(usable(&vtex(Format::Ati1n, 64, 64, 1), Semantic::Shadows, 8192).is_ok());
        assert!(usable(&vtex(Format::Ati2n, 64, 64, 1), Semantic::Shadows, 8192).is_ok());
        assert!(usable(&vtex(Format::Bc7, 64, 64, 1), Semantic::Shadows, 8192).is_ok());
        assert!(usable(&vtex(Format::Bc6h, 64, 64, 1), Semantic::Shadows, 8192).is_err());
        assert!(usable(&vtex(Format::Bc6h, 64, 64, 1), Semantic::Color, 1024).is_err());
        let mut cube = vtex(Format::Bc6h, 64, 64, 7);
        assert!(usable(&cube, Semantic::Sky, 1024).is_err(), "a sky is a cube map");
        cube.flags = nades_source2::flags::CUBE;
        assert_eq!(usable(&cube, Semantic::Sky, 1024), Ok((TextureFormat::Bc6h, 0)));
        assert!(usable(&cube, Semantic::Irradiance, 1024).is_err(), "a cube map is no lightmap");
    }

    #[test]
    fn mips_average_colour_in_linear_light_and_alpha_as_it_is() {
        // a 2x2 of black and white: the sRGB mean of linear 0.5 is 188, alpha's plain mean 128
        let rgba = [0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255];
        let (w, h, srgb) = half(&rgba, 2, 2, true);
        assert_eq!((w, h, srgb), (1, 1, vec![188, 188, 188, 128]));
        let (_, _, linear) = half(&rgba, 2, 2, false);
        assert_eq!(linear, vec![128, 128, 128, 128]);
        // odd sizes repeat their last column: 3x1 → 1x1 from texels 0 and 1
        let (w, h, odd) = half(&[10, 10, 10, 255, 20, 20, 20, 255, 250, 250, 250, 255], 3, 1, false);
        assert_eq!((w, h, odd), (1, 1, vec![15, 15, 15, 255]));
    }

    fn texture(format: TextureFormat, size: u32, levels: u32) -> Texture {
        let (mips, _) = mip_chain(format, size, size, levels, 1);
        Texture { name: String::new(), format, srgb: false, semantic: Semantic::Color, width: size, height: size, layers: 1, volume: false, chunk: 0, mips, source: TextureSource::ExportImage { image: 0 } }
    }

    #[test]
    fn chunks_hold_whole_textures_on_16_kib_pages() {
        // 64 MiB of BC6H alone, then 1 MiB textures, 32 to a chunk
        let mut textures = vec![texture(TextureFormat::Bc6h, 8192, 1)];
        textures.extend((0..40).map(|_| texture(TextureFormat::Bc7, 1024, 1)));
        let chunks = assign_chunks(&mut textures, CHUNK_TARGET);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], Chunk { offset: 0, len: 64 << 20 });
        assert_eq!(chunks[1], Chunk { offset: 64 << 20, len: 32 << 20 });
        assert_eq!(chunks[2], Chunk { offset: 96 << 20, len: 8 << 20 });
        assert_eq!((textures[1].chunk, textures[32].chunk, textures[33].chunk), (1, 1, 2));
        assert_eq!((textures[2].mips[0].offset, textures[33].mips[0].offset), (1 << 20, 0));
    }

    #[test]
    fn the_section_holds_each_mip_in_its_padded_rows() {
        // an RGBA8 4x2 image from a lossless WebP: its two mips land at their offsets, rows padded to 256 bytes
        let pixels: Vec<u8> = (0..8u8).flat_map(|i| [i * 30, 255 - i * 30, i, 255]).collect();
        let mut webp = Vec::new();
        image_webp::WebPEncoder::new(&mut webp).encode(&pixels, 4, 2, image_webp::ColorType::Rgba8).unwrap();
        let planned = vec![from_webp("tiny".into(), 0, webp, true, Semantic::Color).unwrap()];
        let mut textures: Vec<Texture> = planned.iter().map(|p| p.texture.clone()).collect();
        let chunks = assign_chunks(&mut textures, CHUNK_TARGET);
        let planned: Vec<Planned> = planned.into_iter().zip(textures).map(|(p, t)| Planned { texture: t, fill: p.fill }).collect();
        let section = fill(&planned, &chunks, &GameIndex::of(Path::new("/nowhere"), &[])).unwrap();
        let t = &planned[0].texture;
        assert_eq!((t.width, t.height, t.mips.len()), (4, 2, 3));
        assert_eq!(&section[0..16], &pixels[0..16]);
        assert_eq!(&section[256..272], &pixels[16..32]);
        assert_eq!(&section[16..256], &[0u8; 240][..]);
        let mip1 = t.mips[1].offset as usize;
        let (_, _, expected) = half(&pixels, 4, 2, true);
        assert_eq!(&section[mip1..mip1 + 8], &expected[..]);
    }
}
