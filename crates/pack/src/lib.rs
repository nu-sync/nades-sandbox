//! The per-map pack: `~/.cache/nades/packs/<map>.pack`, one file per map, written once from the map's export and the
//! game's own files (`nades-pack <map>`, crates/packer) and memory-mapped at start-up. It is derived from Valve's assets, so
//! it never leaves this machine.
//!
//! Layout: a 16-byte header (magic, format version, the table's length), the table of contents (postcard), then the
//! sections, each starting on a 16 KiB boundary (Apple silicon's page size) so that a texture's or a buffer's bytes
//! can go to the GPU straight from the mapping. A file cut short, or written by another format version, is refused.
//! SPEC.md ("The pack") lists what each section holds.
//!
//! Format 3's packs hold three sections: Scene (a [`scene::Scene`], postcard), Geometry and Textures, whose bytes the
//! scene's spans and mips point into, and, when the packer had Source 2 Viewer's CLI, the two colliders: Collider (line
//! of sight) and GrenadeCollider (what a grenade bounces off, S40). The baked light, the sky and the post-processing are records in the Scene
//! section, and their textures (lightmaps, the sky's cube map) are in the Textures section with the rest, so one
//! upload path serves every texture. [`scene`] documents that interface; [`Pack::scene`], [`Pack::geometry`] and
//! [`Pack::textures`] read it.

pub mod effects;
pub mod scene;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAGIC: [u8; 8] = *b"NADESPK\0";
/// Bumped whenever a section's layout changes: an old pack is then rebuilt, never misread.
pub const VERSION: u32 = 3;
/// Sections start on this boundary.
pub const ALIGN: u64 = 16 * 1024;
const HEADER: usize = 16;

/// What a section holds. SPEC.md describes each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SectionKind {
    /// Draws, materials, the texture table, the baked light, the sun, the sky, the 3D skybox and the
    /// post-processing: the scene description ([`scene::Scene`], postcard).
    Scene,
    /// Vertex, index and instance buffers in the layouts the shaders read ([`scene::Draw`]).
    Geometry,
    /// Texture mips as GPU blocks (BC1/BC3/BC4/BC5/BC6H/BC7) or RGBA8, rows padded for buffer-to-texture copies, in
    /// chunks ([`scene::Texture`]): the materials', the lightmaps' and the sky's.
    Textures,
    /// The collision mesh's triangles, for line of sight: awpy's AWMH layout ("AWMH", version 1, the vertex and
    /// triangle counts, 3 × f32 a vertex, 3 × u32 a triangle), made from the map's physics (crates/packer, physics.rs).
    Collider,
    /// The shared effects pack's one section: an [`effects::Effects`] (postcard).
    Effects,
    /// What a grenade bounces off, in the Collider's AWMH layout: the map's physics welded but not clustered, its
    /// grenade clips kept and its player clips, sky, glass, windows and `passbullets` left out (S40).
    GrenadeCollider,
}

/// Where the pack came from, so a new export or a game update rebuilds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The map, e.g. `de_cache`.
    pub map: String,
    /// The game build the files came from (the depot's manifest id).
    pub game_build: String,
    /// A hash of the export it was built from.
    pub export_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub kind: SectionKind,
    pub offset: u64,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contents {
    pub source: Source,
    pub sections: Vec<Section>,
}

#[derive(Debug, Error)]
pub enum PackError {
    #[error("pack i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a pack (wrong magic)")]
    Magic,
    #[error("pack format {found}, this build reads {VERSION}: rebuild it")]
    Version { found: u32 },
    #[error("pack is cut short or its table is broken")]
    Truncated,
    #[error("pack table: {0}")]
    Table(#[from] postcard::Error),
    #[error("pack scene: {0}")]
    Scene(String),
}

/// A pack, mapped. Its sections are slices of the mapping.
pub struct Pack {
    map: Mmap,
    contents: Contents,
}

impl Pack {
    pub fn open(path: &Path) -> Result<Self, PackError> {
        let file = File::open(path)?;
        // SAFETY: packs are written once to a temporary name and renamed into place (`write`), and nothing writes to
        // them afterwards, so the mapping does not change under the reader
        let map = unsafe { Mmap::map(&file)? };
        if map.len() < HEADER {
            return Err(PackError::Truncated);
        }
        if map[0..8] != MAGIC {
            return Err(PackError::Magic);
        }
        let version = u32::from_le_bytes(map[8..12].try_into().expect("4 bytes"));
        if version != VERSION {
            return Err(PackError::Version { found: version });
        }
        let table_len = u32::from_le_bytes(map[12..16].try_into().expect("4 bytes")) as usize;
        let table = map.get(HEADER..HEADER + table_len).ok_or(PackError::Truncated)?;
        let contents: Contents = postcard::from_bytes(table)?;
        if contents.sections.iter().any(|s| s.offset.checked_add(s.len).is_none_or(|end| end > map.len() as u64)) {
            return Err(PackError::Truncated);
        }
        Ok(Self { map, contents })
    }

    pub fn contents(&self) -> &Contents {
        &self.contents
    }

    /// A section's bytes, straight from the mapping.
    pub fn section(&self, kind: SectionKind) -> Option<&[u8]> {
        let s = self.contents.sections.iter().find(|s| s.kind == kind)?;
        Some(&self.map[s.offset as usize..(s.offset + s.len) as usize])
    }

    /// The scene description, checked against the Geometry and Textures sections ([`scene::Scene::validate`]): every
    /// span and mip it names lies inside them.
    pub fn scene(&self) -> Result<scene::Scene, PackError> {
        let bytes = self.section(SectionKind::Scene).ok_or_else(|| PackError::Scene("the pack has no Scene section".into()))?;
        let scene = scene::Scene::from_bytes(bytes).map_err(|e| PackError::Scene(format!("unreadable: {e}")))?;
        scene.validate(self.geometry().len() as u64, self.textures().len() as u64).map_err(|e| PackError::Scene(e.0))?;
        Ok(scene)
    }

    /// The Geometry section (empty when the pack has none): the bytes [`scene::Draw`]'s spans point into.
    pub fn geometry(&self) -> &[u8] {
        self.section(SectionKind::Geometry).unwrap_or_default()
    }

    /// The Collider section, when the pack has one.
    pub fn collider(&self) -> Option<&[u8]> {
        self.section(SectionKind::Collider)
    }

    /// The GrenadeCollider section, when the pack has one.
    pub fn grenade_collider(&self) -> Option<&[u8]> {
        self.section(SectionKind::GrenadeCollider)
    }

    /// The effects of the shared effects pack, checked.
    pub fn effects(&self) -> Result<effects::Effects, PackError> {
        let bytes = self.section(SectionKind::Effects).ok_or_else(|| PackError::Scene("the pack has no Effects section".into()))?;
        let effects = effects::Effects::from_bytes(bytes).map_err(|e| PackError::Scene(format!("unreadable effects: {e}")))?;
        effects.validate().map_err(PackError::Scene)?;
        Ok(effects)
    }

    /// The Textures section (empty when the pack has none): the bytes [`scene::Texture`]'s chunks and mips point into.
    pub fn textures(&self) -> &[u8] {
        self.section(SectionKind::Textures).unwrap_or_default()
    }
}

/// Writes a pack: the header, the table, then each section on an `ALIGN` boundary. It goes to a temporary name first
/// and is renamed into place, so a reader never maps a half-written file.
pub fn write(path: &Path, source: Source, sections: &[(SectionKind, &[u8])]) -> Result<(), PackError> {
    // the table's own size moves the first section, so lay the sections out after a table sized for these offsets
    let mut contents = Contents { source, sections: Vec::with_capacity(sections.len()) };
    let mut first = ALIGN;
    loop {
        contents.sections.clear();
        let mut at = first;
        for (kind, bytes) in sections {
            contents.sections.push(Section { kind: *kind, offset: at, len: bytes.len() as u64 });
            at = (at + bytes.len() as u64).div_ceil(ALIGN) * ALIGN;
        }
        let table_end = (HEADER + postcard::to_allocvec(&contents)?.len()) as u64;
        if table_end <= first {
            break;
        }
        first = table_end.div_ceil(ALIGN) * ALIGN;
    }
    let table = postcard::to_allocvec(&contents)?;
    let partial = path.with_extension("pack.partial");
    {
        let mut out = BufWriter::new(File::create(&partial)?);
        out.write_all(&MAGIC)?;
        out.write_all(&VERSION.to_le_bytes())?;
        out.write_all(&(table.len() as u32).to_le_bytes())?;
        out.write_all(&table)?;
        let mut at = (HEADER + table.len()) as u64;
        for (section, (_, bytes)) in contents.sections.iter().zip(sections) {
            out.write_all(&vec![0; (section.offset - at) as usize])?;
            out.write_all(bytes)?;
            at = section.offset + section.len;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    }
    std::fs::rename(&partial, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source { map: "de_test".into(), game_build: "1".into(), export_hash: "abc".into() }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nades-pack-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("map.pack")
    }

    #[test]
    fn sections_come_back_whole_and_page_aligned() {
        let path = scratch("round-trip");
        let geometry: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        let scene = b"draws".to_vec();
        write(&path, source(), &[(SectionKind::Scene, &scene), (SectionKind::Geometry, &geometry)]).unwrap();
        let pack = Pack::open(&path).unwrap();
        assert_eq!(pack.contents().source, source());
        assert_eq!(pack.section(SectionKind::Scene), Some(&scene[..]));
        assert_eq!(pack.section(SectionKind::Geometry), Some(&geometry[..]));
        assert_eq!(pack.section(SectionKind::Collider), None);
        assert!(pack.contents().sections.iter().all(|s| s.offset % ALIGN == 0));
    }

    #[test]
    fn refuses_another_format_a_foreign_file_and_a_cut_short_one() {
        let path = scratch("refusals");
        write(&path, source(), &[(SectionKind::Scene, &[1, 2, 3])]).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[8] = 9;
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(Pack::open(&path), Err(PackError::Version { found: 9 })));
        bytes[0] = b'X';
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(Pack::open(&path), Err(PackError::Magic)));
        write(&path, source(), &[(SectionKind::Scene, &[1, 2, 3])]).unwrap();
        let whole = std::fs::read(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() - 1]).unwrap();
        assert!(matches!(Pack::open(&path), Err(PackError::Truncated)));
    }
}
