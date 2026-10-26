//! VPK version 2 archives: how CS2 ships its files. A map's own VPK (`maps/<map>.vpk`) is one file that holds its
//! directory and its data; the game's `pak01_dir.vpk` holds only the directory, and the data sits in numbered archives
//! beside it (`pak01_000.vpk`, `pak01_001.vpk`, …).
//!
//! The layout, from Valve's format as ValveResourceFormat's `ValvePak` reads it:
//! - a 28-byte header: the signature `0x55AA1234`, the version (2), the directory tree's size, then the sizes of the
//!   data, archive-MD5, other-MD5 and signature sections that follow the tree;
//! - the tree, three levels of NUL-terminated strings, each level ended by an empty string: an extension, then the
//!   folders with files of that extension, then the file names in each folder. A single space stands for an empty
//!   folder (a file at the root) or an empty extension.
//! - after each file name, its entry: CRC-32 of the whole file (u32), preload bytes (u16), archive index (u16),
//!   offset (u32), length (u32) and the terminator `0xFFFF` (u16), then the preload bytes themselves.
//!
//! A file's contents are its preload bytes followed by `length` bytes at `offset` in its archive. Archive index
//! `0x7FFF` means the directory file itself, counting `offset` from the end of the tree (the 28-byte header plus the
//! tree); any other index names `<name>_NNN.vpk`, where `<name>` is the directory file's name without its `_dir`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use thiserror::Error;

/// The first four bytes of every VPK directory file.
pub const SIGNATURE: u32 = 0x55AA_1234;
/// The archive index of files stored in the directory file itself.
pub const IN_DIRECTORY: u16 = 0x7FFF;
/// Version 2's header: seven u32 fields.
pub const HEADER_LEN: usize = 28;
const ENTRY_LEN: usize = 18;
const TERMINATOR: u16 = 0xFFFF;

#[derive(Debug, Error)]
pub enum VpkError {
    #[error("vpk i/o on {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("not a VPK (signature {0:#010x})")]
    Signature(u32),
    #[error("VPK version {0} (only version 2 is read)")]
    Version(u32),
    #[error("the VPK directory is malformed: {0}")]
    Tree(&'static str),
    #[error("no {0} in the VPK")]
    Missing(String),
    #[error("{entry} lies outside {archive}")]
    OutOfArchive { entry: String, archive: PathBuf },
    #[error("{entry}: CRC-32 {found:#010x}, the directory says {expected:#010x}")]
    Crc { entry: String, expected: u32, found: u32 },
}

/// One file in the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The file's path inside the archive, folders joined by `/`, e.g. `maps/de_cache/lightmaps/irradiance.vtex_c`.
    pub path: String,
    /// CRC-32 (IEEE, as zlib computes it) of the file's whole contents: preload and archive bytes.
    pub crc: u32,
    /// Where the preload bytes are in the directory file: they come first in the file's contents.
    pub preload: Range<usize>,
    /// The archive holding the rest: [`IN_DIRECTORY`], or the `NNN` of `<name>_NNN.vpk`.
    pub archive: u16,
    /// Where the rest starts in that archive (after the tree, for [`IN_DIRECTORY`]).
    pub offset: u32,
    /// How many bytes of the file are in the archive.
    pub length: u32,
}

impl Entry {
    /// The file's size: preload and archive bytes.
    pub fn size(&self) -> u64 {
        self.preload.len() as u64 + u64::from(self.length)
    }
}

/// A parsed directory: every entry, and where the directory file's own data starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory {
    pub entries: Vec<Entry>,
    /// Offset in the directory file of the data stored there (archive [`IN_DIRECTORY`]): the header plus the tree.
    pub data_start: u64,
    by_path: HashMap<String, usize>,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Reads the NUL-terminated string at `*at` inside `tree`, moving past its terminator.
fn string<'a>(tree: &'a [u8], at: &mut usize) -> Result<&'a str, VpkError> {
    let rest = tree.get(*at..).ok_or(VpkError::Tree("a name runs past the tree"))?;
    let end = rest.iter().position(|&b| b == 0).ok_or(VpkError::Tree("a name runs past the tree"))?;
    let s = std::str::from_utf8(&rest[..end]).map_err(|_| VpkError::Tree("a name is not UTF-8"))?;
    *at += end + 1;
    Ok(s)
}

/// The full path of `name` with extension `ext` in `folder`, where a single space stands for an empty folder or
/// extension.
fn join(folder: &str, name: &str, ext: &str) -> String {
    let file = if ext == " " || ext.is_empty() { name.to_string() } else { format!("{name}.{ext}") };
    if folder == " " || folder.is_empty() { file } else { format!("{folder}/{file}") }
}

/// Parses a version 2 directory from the start of a directory file (the whole file, or at least its header and tree).
pub fn parse(bytes: &[u8]) -> Result<Directory, VpkError> {
    let signature = u32_at(bytes, 0).ok_or(VpkError::Tree("shorter than a header"))?;
    if signature != SIGNATURE {
        return Err(VpkError::Signature(signature));
    }
    let version = u32_at(bytes, 4).ok_or(VpkError::Tree("shorter than a header"))?;
    if version != 2 {
        return Err(VpkError::Version(version));
    }
    let tree_len = u32_at(bytes, 8).ok_or(VpkError::Tree("shorter than a header"))? as usize;
    if bytes.len() < HEADER_LEN {
        return Err(VpkError::Tree("shorter than a header"));
    }
    let tree_end = HEADER_LEN.checked_add(tree_len).filter(|&end| end <= bytes.len()).ok_or(VpkError::Tree("the tree runs past the file"))?;
    let tree = &bytes[..tree_end];
    let mut at = HEADER_LEN;
    let mut entries = Vec::new();
    loop {
        let ext = string(tree, &mut at)?;
        if ext.is_empty() {
            break;
        }
        loop {
            let folder = string(tree, &mut at)?;
            if folder.is_empty() {
                break;
            }
            loop {
                let name = string(tree, &mut at)?;
                if name.is_empty() {
                    break;
                }
                let field = |off: usize| u32_at(tree, at + off).ok_or(VpkError::Tree("an entry runs past the tree"));
                let half = |off: usize| u16_at(tree, at + off).ok_or(VpkError::Tree("an entry runs past the tree"));
                let crc = field(0)?;
                let preload_len = half(4)? as usize;
                let archive = half(6)?;
                let offset = field(8)?;
                let length = field(12)?;
                if half(16)? != TERMINATOR {
                    return Err(VpkError::Tree("an entry's terminator is not 0xFFFF"));
                }
                at += ENTRY_LEN;
                let preload = at..at + preload_len;
                if preload.end > tree_end {
                    return Err(VpkError::Tree("preload bytes run past the tree"));
                }
                at = preload.end;
                entries.push(Entry { path: join(folder, name, ext), crc, preload, archive, offset, length });
            }
        }
    }
    let by_path = entries.iter().enumerate().map(|(i, e)| (e.path.clone(), i)).collect();
    Ok(Directory { entries, data_start: tree_end as u64, by_path })
}

impl Directory {
    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.by_path.get(path).map(|&i| &self.entries[i])
    }
}

/// CRC-32 (IEEE 802.3, reflected, as zlib and VPK directories compute it).
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

/// An open VPK: its directory file, mapped, and the directory parsed from it.
pub struct Vpk {
    path: PathBuf,
    map: Mmap,
    directory: Directory,
}

impl Vpk {
    /// Opens a directory file: a map's `<map>.vpk`, or the game's `pak01_dir.vpk`.
    pub fn open(path: &Path) -> Result<Self, VpkError> {
        let io = |source| VpkError::Io { path: path.to_path_buf(), source };
        let file = File::open(path).map_err(io)?;
        // SAFETY: the game's VPKs are written by Steam when the game updates, never while this reads them; a file
        // changed underneath would at worst give wrong bytes, which the CRC check catches
        let map = unsafe { Mmap::map(&file) }.map_err(io)?;
        let directory = parse(&map)?;
        Ok(Self { path: path.to_path_buf(), map, directory })
    }

    pub fn directory(&self) -> &Directory {
        &self.directory
    }

    pub fn entries(&self) -> &[Entry] {
        &self.directory.entries
    }

    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.directory.get(path)
    }

    /// The numbered archive `index` beside this directory file: `pak01_dir.vpk`'s archive 3 is `pak01_003.vpk`.
    pub fn archive_path(&self, index: u16) -> PathBuf {
        archive_path(&self.path, index)
    }

    /// A file's contents. A file stored whole in the directory file (no preload bytes) is borrowed from the mapping;
    /// anything else is read into memory.
    pub fn read(&self, path: &str) -> Result<Cow<'_, [u8]>, VpkError> {
        let entry = self.entry(path).ok_or_else(|| VpkError::Missing(path.to_string()))?;
        self.read_entry(entry)
    }

    /// [`Vpk::read`], then a check of the contents against the directory's CRC-32.
    pub fn read_checked(&self, path: &str) -> Result<Cow<'_, [u8]>, VpkError> {
        let entry = self.entry(path).ok_or_else(|| VpkError::Missing(path.to_string()))?;
        let bytes = self.read_entry(entry)?;
        let found = crc32(&bytes);
        if found != entry.crc {
            return Err(VpkError::Crc { entry: entry.path.clone(), expected: entry.crc, found });
        }
        Ok(bytes)
    }

    pub fn read_entry(&self, entry: &Entry) -> Result<Cow<'_, [u8]>, VpkError> {
        let preload = &self.map[entry.preload.clone()];
        let length = entry.length as usize;
        if entry.archive == IN_DIRECTORY {
            let start = self.directory.data_start as usize + entry.offset as usize;
            let rest = self.map.get(start..start + length).ok_or_else(|| VpkError::OutOfArchive { entry: entry.path.clone(), archive: self.path.clone() })?;
            if preload.is_empty() {
                return Ok(Cow::Borrowed(rest));
            }
            return Ok(Cow::Owned([preload, rest].concat()));
        }
        let archive = self.archive_path(entry.archive);
        let io = |source| VpkError::Io { path: archive.clone(), source };
        let mut file = File::open(&archive).map_err(io)?;
        let size = file.metadata().map_err(io)?.len();
        if u64::from(entry.offset) + u64::from(entry.length) > size {
            return Err(VpkError::OutOfArchive { entry: entry.path.clone(), archive });
        }
        let mut bytes = Vec::with_capacity(preload.len() + length);
        bytes.extend_from_slice(preload);
        bytes.resize(preload.len() + length, 0);
        file.seek(SeekFrom::Start(u64::from(entry.offset))).map_err(io)?;
        file.read_exact(&mut bytes[preload.len()..]).map_err(io)?;
        Ok(Cow::Owned(bytes))
    }
}

/// The numbered archive `index` beside the directory file `dir`: `<name>_NNN.vpk`, `<name>` being the directory
/// file's name without `_dir`.
pub fn archive_path(dir: &Path, index: u16) -> PathBuf {
    let stem = dir.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let base = stem.strip_suffix("_dir").unwrap_or(stem);
    dir.with_file_name(format!("{base}_{index:03}.vpk"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One file to put in a synthetic VPK: where it goes in the tree and how its bytes are split.
    struct Stored<'a> {
        ext: &'a str,
        folder: &'a str,
        name: &'a str,
        preload: &'a [u8],
        /// The bytes after the preload, and the archive that holds them.
        rest: &'a [u8],
        archive: u16,
    }

    /// A version 2 directory file and its numbered archives, laid out as Valve's tools write them: files grouped by
    /// extension, then by folder.
    fn build(files: &[Stored]) -> (Vec<u8>, HashMap<u16, Vec<u8>>) {
        let mut tree = Vec::new();
        let mut inline = Vec::new();
        let mut archives: HashMap<u16, Vec<u8>> = HashMap::new();
        let mut exts: Vec<&str> = files.iter().map(|f| f.ext).collect();
        exts.dedup();
        for ext in exts {
            tree.extend_from_slice(ext.as_bytes());
            tree.push(0);
            let mut folders: Vec<&str> = files.iter().filter(|f| f.ext == ext).map(|f| f.folder).collect();
            folders.dedup();
            for folder in folders {
                tree.extend_from_slice(folder.as_bytes());
                tree.push(0);
                for f in files.iter().filter(|f| f.ext == ext && f.folder == folder) {
                    tree.extend_from_slice(f.name.as_bytes());
                    tree.push(0);
                    let data = if f.archive == IN_DIRECTORY { &mut inline } else { archives.entry(f.archive).or_default() };
                    let offset = data.len() as u32;
                    data.extend_from_slice(f.rest);
                    tree.extend_from_slice(&crc32(&[f.preload, f.rest].concat()).to_le_bytes());
                    tree.extend_from_slice(&(f.preload.len() as u16).to_le_bytes());
                    tree.extend_from_slice(&f.archive.to_le_bytes());
                    tree.extend_from_slice(&offset.to_le_bytes());
                    tree.extend_from_slice(&(f.rest.len() as u32).to_le_bytes());
                    tree.extend_from_slice(&TERMINATOR.to_le_bytes());
                    tree.extend_from_slice(f.preload);
                }
                tree.push(0);
            }
            tree.push(0);
        }
        tree.push(0);
        let mut dir = Vec::new();
        for field in [SIGNATURE, 2, tree.len() as u32, inline.len() as u32, 0, 0, 0] {
            dir.extend_from_slice(&field.to_le_bytes());
        }
        dir.extend_from_slice(&tree);
        dir.extend_from_slice(&inline);
        (dir, archives)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nades-vpk-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_files_from_the_directory_file_and_from_numbered_archives() {
        let lightmap: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 256) as u8).collect();
        let files = [
            Stored { ext: "vtex_c", folder: "maps/de_test/lightmaps", name: "irradiance", preload: &[], rest: &lightmap, archive: IN_DIRECTORY },
            Stored { ext: "vtex_c", folder: "maps/de_test/lightmaps", name: "direct_light_shadows", preload: b"head", rest: b"tail", archive: IN_DIRECTORY },
            Stored { ext: "vtex_c", folder: "materials/concrete", name: "wall_color", preload: b"pre", rest: b"from archive one", archive: 1 },
            Stored { ext: "txt", folder: " ", name: "readme", preload: b"preload only", rest: b"", archive: IN_DIRECTORY },
            Stored { ext: " ", folder: "scripts", name: "no_extension", preload: &[], rest: b"plain", archive: 0 },
        ];
        let (dir_bytes, archives) = build(&files);
        let dir = scratch("archives");
        let dir_path = dir.join("pak01_dir.vpk");
        std::fs::write(&dir_path, &dir_bytes).unwrap();
        for (index, bytes) in &archives {
            std::fs::write(dir.join(format!("pak01_{index:03}.vpk")), bytes).unwrap();
        }

        let vpk = Vpk::open(&dir_path).unwrap();
        let paths: Vec<&str> = vpk.entries().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "maps/de_test/lightmaps/irradiance.vtex_c",
                "maps/de_test/lightmaps/direct_light_shadows.vtex_c",
                "materials/concrete/wall_color.vtex_c",
                "readme.txt",
                "scripts/no_extension",
            ]
        );
        // a file stored whole in the directory file comes straight from the mapping
        let irradiance = vpk.read_checked("maps/de_test/lightmaps/irradiance.vtex_c").unwrap();
        assert!(matches!(irradiance, Cow::Borrowed(_)));
        assert_eq!(&irradiance[..], &lightmap[..]);
        assert_eq!(vpk.entry("maps/de_test/lightmaps/irradiance.vtex_c").unwrap().size(), 5000);
        assert_eq!(&vpk.read_checked("maps/de_test/lightmaps/direct_light_shadows.vtex_c").unwrap()[..], b"headtail");
        assert_eq!(&vpk.read_checked("materials/concrete/wall_color.vtex_c").unwrap()[..], b"prefrom archive one");
        assert_eq!(&vpk.read_checked("readme.txt").unwrap()[..], b"preload only");
        assert_eq!(&vpk.read_checked("scripts/no_extension").unwrap()[..], b"plain");
        assert_eq!(vpk.archive_path(1), dir.join("pak01_001.vpk"));
        assert!(matches!(vpk.read("maps/de_test/missing.vtex_c"), Err(VpkError::Missing(_))));
    }

    #[test]
    fn a_map_vpk_names_its_archives_after_itself() {
        assert_eq!(archive_path(Path::new("/game/maps/de_cache.vpk"), 0), Path::new("/game/maps/de_cache_000.vpk"));
        assert_eq!(archive_path(Path::new("/game/pak01_dir.vpk"), 12), Path::new("/game/pak01_012.vpk"));
    }

    #[test]
    fn refuses_a_foreign_file_another_version_and_a_broken_tree() {
        let (good, _) = build(&[Stored { ext: "kv3", folder: "maps", name: "a", preload: &[], rest: b"x", archive: IN_DIRECTORY }]);
        assert!(parse(&good).is_ok());
        let mut foreign = good.clone();
        foreign[0] = 0;
        assert!(matches!(parse(&foreign), Err(VpkError::Signature(_))));
        let mut v1 = good.clone();
        v1[4] = 1;
        assert!(matches!(parse(&v1), Err(VpkError::Version(1))));
        // the tree says it is longer than the file
        let mut long = good.clone();
        long[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(parse(&long), Err(VpkError::Tree(_))));
        // an entry whose terminator was overwritten
        let mut broken = good.clone();
        let terminator = HEADER_LEN + "kv3\0maps\0a\0".len() + 16;
        broken[terminator] = 0;
        assert!(matches!(parse(&broken), Err(VpkError::Tree(_))));
        // cut inside the tree
        assert!(matches!(parse(&good[..HEADER_LEN + 6]), Err(VpkError::Tree(_))));
    }

    #[test]
    fn a_wrong_crc_or_an_entry_past_its_archive_is_an_error() {
        let files = [
            Stored { ext: "vtex_c", folder: "maps", name: "a", preload: &[], rest: b"0123456789", archive: IN_DIRECTORY },
            Stored { ext: "vtex_c", folder: "maps", name: "b", preload: &[], rest: b"abc", archive: 0 },
        ];
        let (mut dir_bytes, archives) = build(&files);
        let dir = scratch("errors");
        let dir_path = dir.join("de_test.vpk");
        // change one byte of a's contents: the CRC no longer matches
        let last = dir_bytes.len() - 1;
        dir_bytes[last] ^= 0xFF;
        std::fs::write(&dir_path, &dir_bytes).unwrap();
        // b's archive is cut short
        std::fs::write(dir.join("de_test_000.vpk"), &archives[&0][..2]).unwrap();
        let vpk = Vpk::open(&dir_path).unwrap();
        assert!(vpk.read("maps/a.vtex_c").is_ok());
        assert!(matches!(vpk.read_checked("maps/a.vtex_c"), Err(VpkError::Crc { .. })));
        assert!(matches!(vpk.read("maps/b.vtex_c"), Err(VpkError::OutOfArchive { .. })));
    }

    /// The real archive the M1 pack takes its lightmap from: `cargo test --release -p nades-source2 -- --ignored`.
    #[test]
    #[ignore = "reads the game's files in ~/.cache/nades/cs2 (Valve's, never in the repository)"]
    fn reads_de_cache_irradiance_from_the_game_files() {
        let home = std::env::var_os("HOME").expect("HOME");
        let path = Path::new(&home).join(".cache/nades/cs2/game/csgo/maps/de_cache.vpk");
        let vpk = Vpk::open(&path).unwrap();
        let entry = vpk.entry("maps/de_cache/lightmaps/irradiance.vtex_c").unwrap();
        assert_eq!(entry.archive, IN_DIRECTORY);
        let bytes = vpk.read_checked(&entry.path).unwrap();
        assert!(matches!(bytes, Cow::Borrowed(_)));
        let texture = crate::parse(&bytes).unwrap();
        println!("{} entries; irradiance {} bytes, {:?}", vpk.entries().len(), bytes.len(), texture);
        assert_eq!(texture.format, crate::Format::Bc6h);
    }
}
