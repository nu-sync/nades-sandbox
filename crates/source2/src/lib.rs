//! Readers for CS2's own files on this machine (`~/.cache/nades/cs2`, never committed or shared).
//!
//! - [`vpk`]: VPK version 2 archives, the containers CS2 ships its files in (a map's lightmaps live in its own VPK).
//! - VTEX (`.vtex_c`) textures, below.
//!
//! VTEX textures: CS2 stores them as GPU blocks with their mip chains (a census of the local cache:
//! BC7 for 74% of pixels, BC1 19.5%, BC4 6.5%; lightmaps BC6H), so a pack can upload them as they are. The layout,
//! from ValveResourceFormat's `Resource.cs` and `Texture.cs` and checked against Source 2 Viewer on 296 textures of
//! de_cache (swarm 3, lane 06):
//! - a resource header: the data's offset (u32), two versions (u16, u16), then the block table's offset (u32,
//!   relative to its own field) and its block count (u32);
//! - block entries of 12 bytes: a four-character name, an offset relative to the offset field, a size;
//! - the DATA block: the VTEX header (version 1, flags, size, depth, format, mip count) and extra data entries, one of
//!   which (type 4, COMPRESSED_MIP_SIZE) lists each mip's stored size;
//! - the mips from the data offset on, smallest first, each LZ4 (block format) when its stored size is below its
//!   decoded size, raw otherwise. COMPRESSED_MIP_SIZE starts with the compression method (1 LZ4, 0 none); a texture
//!   without that entry stores every mip raw.
//! - a cube map (the skies) holds its six faces in each mip, one after the other in the order +X, −X, +Y, −Y, +Z, −Z
//!   (Source 2 Viewer uploads them in file order as OpenGL's faces, which are wgpu's layers 0–5); the mip is compressed
//!   as a whole.

pub mod vpk;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VtexError {
    #[error("not a VTEX resource: {0}")]
    Malformed(&'static str),
    #[error("VTEX version {0} (only 1 is known)")]
    Version(u16),
    #[error("texture format {0} has no block layout here")]
    Format(u8),
    #[error("mip {mip} is outside the file")]
    MipOutOfFile { mip: u32 },
    #[error("mip {mip}: LZ4 gave {got} bytes, {want} expected")]
    Lz4 { mip: u32, got: usize, want: usize },
    #[error("mip {mip} is stored in {stored} bytes, uncompressed, but decodes to {want}")]
    MipSize { mip: u32, stored: usize, want: usize },
}

/// The block layouts CS2 uses, by the format byte in the VTEX header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// BC1 (DXT1): colour, 8 bytes a block.
    Dxt1,
    /// BC3 (DXT5): colour and alpha, 16 bytes a block.
    Dxt5,
    /// Uncompressed RGBA, 4 bytes a texel.
    Rgba8888,
    /// BC6H: half-float colour (the lightmaps' irradiance), 16 bytes a block.
    Bc6h,
    /// BC7: colour, 16 bytes a block.
    Bc7,
    /// BC5 (ATI2N): two channels, 16 bytes a block.
    Ati2n,
    /// BC4 (ATI1N): one channel (masks, the baked sun shadows), 8 bytes a block.
    Ati1n,
}

impl Format {
    pub fn from_byte(byte: u8) -> Result<Self, VtexError> {
        Ok(match byte {
            1 => Self::Dxt1,
            2 => Self::Dxt5,
            4 => Self::Rgba8888,
            19 => Self::Bc6h,
            20 => Self::Bc7,
            21 => Self::Ati2n,
            27 => Self::Ati1n,
            other => return Err(VtexError::Format(other)),
        })
    }

    /// Texels along a block's side: 4 for the BC formats, 1 for RGBA.
    pub fn block_dim(self) -> u32 {
        if self == Self::Rgba8888 { 1 } else { 4 }
    }

    pub fn block_bytes(self) -> usize {
        match self {
            Self::Dxt1 | Self::Ati1n => 8,
            Self::Rgba8888 => 4,
            Self::Dxt5 | Self::Bc6h | Self::Bc7 | Self::Ati2n => 16,
        }
    }
}

/// VTEX header flags (ValveResourceFormat's `VTexFlags`).
pub mod flags {
    pub const SUGGEST_CLAMP_S: u16 = 0x01;
    pub const SUGGEST_CLAMP_T: u16 = 0x02;
    pub const SUGGEST_CLAMP_U: u16 = 0x04;
    pub const NO_LOD: u16 = 0x08;
    pub const CUBE: u16 = 0x10;
    pub const VOLUME: u16 = 0x20;
    /// An array of `depth` layers; the lightmaps are arrays of one.
    pub const ARRAY: u16 = 0x40;
}

/// A VTEX texture's header and where its mips are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vtex {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub format: Format,
    pub mips: u32,
    pub flags: u16,
    /// Each mip's stored size, index 0 the largest.
    pub stored: Vec<u32>,
    /// Whether a mip stored in fewer bytes than it decodes to is LZ4 (COMPRESSED_MIP_SIZE's method 1).
    pub lz4: bool,
    /// Where the mip data starts in the file.
    pub data_start: usize,
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, VtexError> {
    bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or(VtexError::Malformed("cut short"))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, VtexError> {
    bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or(VtexError::Malformed("cut short"))
}

/// Reads a `.vtex_c` file's header. The mips stay where they are until `read_mip`.
pub fn parse(bytes: &[u8]) -> Result<Vtex, VtexError> {
    let data_start = u32_at(bytes, 0)? as usize;
    let table = 8 + u32_at(bytes, 8)? as usize;
    let blocks = u32_at(bytes, 12)? as usize;
    let mut data = None;
    for i in 0..blocks {
        let entry = table + i * 12;
        if bytes.get(entry..entry + 4) == Some(b"DATA") {
            data = Some(entry + 4 + u32_at(bytes, entry + 4)? as usize);
        }
    }
    let at = data.ok_or(VtexError::Malformed("no DATA block"))?;
    let version = u16_at(bytes, at)?;
    if version != 1 {
        return Err(VtexError::Version(version));
    }
    let flags = u16_at(bytes, at + 2)?;
    let width = u32::from(u16_at(bytes, at + 20)?);
    let height = u32::from(u16_at(bytes, at + 22)?);
    let depth = u32::from(u16_at(bytes, at + 24)?);
    let format = Format::from_byte(*bytes.get(at + 26).ok_or(VtexError::Malformed("cut short"))?)?;
    let mips = u32::from(*bytes.get(at + 27).ok_or(VtexError::Malformed("cut short"))?);
    let extras = u32_at(bytes, at + 36)? as usize;
    let mut stored = None;
    let mut lz4 = false;
    for i in 0..extras {
        let entry = at + 40 + i * 12;
        // COMPRESSED_MIP_SIZE: the method (u32: 1 LZ4, 0 none), the array's offset (relative to its own field) and
        // its count, then one u32 per mip
        if u32_at(bytes, entry)? == 4 {
            let extra = entry + 4 + u32_at(bytes, entry + 4)? as usize;
            lz4 = match u32_at(bytes, extra)? {
                0 => false,
                1 => true,
                _ => return Err(VtexError::Malformed("an unknown mip compression method")),
            };
            let array = extra + 4 + u32_at(bytes, extra + 4)? as usize;
            let count = u32_at(bytes, extra + 8)? as usize;
            stored = Some((0..count).map(|k| u32_at(bytes, array + k * 4)).collect::<Result<Vec<_>, _>>()?);
        }
    }
    let mut vtex = Vtex { width, height, depth, format, mips, flags, stored: Vec::new(), lz4, data_start };
    vtex.stored = match stored {
        Some(sizes) if sizes.len() == mips as usize => sizes,
        Some(_) => return Err(VtexError::Malformed("COMPRESSED_MIP_SIZE does not list every mip")),
        // no COMPRESSED_MIP_SIZE: every mip is stored raw
        None => (0..mips).map(|m| vtex.mip_bytes(m) as u32).collect(),
    };
    if vtex.stored.is_empty() {
        return Err(VtexError::Malformed("no mips"));
    }
    Ok(vtex)
}

impl Vtex {
    /// A plain 2D texture: not a cube or a volume, and one layer (an array of one, like the lightmaps, counts).
    pub fn is_2d(&self) -> bool {
        self.flags & (flags::CUBE | flags::VOLUME) == 0 && self.depth <= 1
    }

    /// A cube map: six faces in each mip.
    pub fn is_cube(&self) -> bool {
        self.flags & flags::CUBE != 0 && self.flags & flags::VOLUME == 0 && self.depth <= 1
    }

    /// Faces (or layers) in each mip: 6 for a cube map, 1 for a plain 2D texture.
    pub fn faces(&self) -> u32 {
        if self.is_cube() { 6 } else { 1 }
    }

    /// A volume texture: `depth` slices, halved with each mip (the light probes' atlases).
    pub fn is_volume(&self) -> bool {
        self.flags & flags::VOLUME != 0
    }

    /// 2D slices in mip `mip`: a volume's depth at that mip, a cube's six faces, or one.
    pub fn slices(&self, mip: u32) -> u32 {
        if self.is_volume() { (self.depth >> mip).max(1) } else { self.faces() }
    }

    /// Mip `mip`'s width and height in texels.
    pub fn mip_dimensions(&self, mip: u32) -> (u32, u32) {
        ((self.width >> mip).max(1), (self.height >> mip).max(1))
    }

    /// Mip `mip`'s decoded size, all of its faces or slices: [`Vtex::mip_size`] × [`Vtex::slices`].
    pub fn mip_bytes(&self, mip: u32) -> usize {
        self.mip_size(mip) * self.slices(mip) as usize
    }

    /// The decoded size of one face of mip `mip`: whole blocks per row and column.
    pub fn mip_size(&self, mip: u32) -> usize {
        let dim = self.format.block_dim();
        let columns = (self.width >> mip).max(1).div_ceil(dim) as usize;
        let rows = (self.height >> mip).max(1).div_ceil(dim) as usize;
        columns * rows * self.format.block_bytes()
    }

    /// Mip `mip`'s byte range in the file: the smaller mips come before it.
    pub fn mip_range(&self, mip: u32) -> (usize, usize) {
        let before: usize = self.stored.iter().skip(mip as usize + 1).map(|&s| s as usize).sum();
        (self.data_start + before, self.stored[mip as usize] as usize)
    }

    /// Decodes mip `mip` into `out` (`out.len() == mip_bytes(mip)`, every face): a copy, or LZ4 when it is stored
    /// smaller.
    pub fn read_mip(&self, file: &[u8], mip: u32, out: &mut [u8]) -> Result<(), VtexError> {
        let (at, stored) = self.mip_range(mip);
        let source = file.get(at..at + stored).ok_or(VtexError::MipOutOfFile { mip })?;
        if stored == out.len() {
            out.copy_from_slice(source);
            return Ok(());
        }
        if !self.lz4 {
            return Err(VtexError::MipSize { mip, stored, want: out.len() });
        }
        let got = lz4_flex::block::decompress_into(source, out).map_err(|_| VtexError::Lz4 { mip, got: 0, want: out.len() })?;
        if got == out.len() { Ok(()) } else { Err(VtexError::Lz4 { mip, got, want: out.len() }) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.vtex_c` laid out as CS2 writes one: a resource header, a block table with DATA, the VTEX header, a
    /// COMPRESSED_MIP_SIZE extra (LZ4), then the mips smallest first.
    fn vtex_file(format: u8, width: u16, height: u16, mips: &[Vec<u8>], stored_sizes: &[u32]) -> Vec<u8> {
        vtex_file_with(format, (width, height, 1), 0, mips, Some((1, stored_sizes)))
    }

    /// The same, with the header's flags and depth, and COMPRESSED_MIP_SIZE's method and sizes, or no such entry.
    fn vtex_file_with(format: u8, (width, height, depth): (u16, u16, u16), flags: u16, mips: &[Vec<u8>], sizes: Option<(u32, &[u32])>) -> Vec<u8> {
        let mut f = vec![0u8; 16];
        // block table at 16: one entry, DATA, its block right after the table
        f[8..12].copy_from_slice(&(16u32 - 8).to_le_bytes());
        f[12..16].copy_from_slice(&1u32.to_le_bytes());
        let entry = f.len();
        f.extend_from_slice(b"DATA");
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        let data = f.len();
        f[entry + 4..entry + 8].copy_from_slice(&((data - (entry + 4)) as u32).to_le_bytes());
        // VTEX header: version 1, flags, size, depth, format, mip count; the extra entries at +40
        let mut header = vec![0u8; 40];
        header[0..2].copy_from_slice(&1u16.to_le_bytes());
        header[2..4].copy_from_slice(&flags.to_le_bytes());
        header[20..22].copy_from_slice(&width.to_le_bytes());
        header[22..24].copy_from_slice(&height.to_le_bytes());
        header[24..26].copy_from_slice(&depth.to_le_bytes());
        header[26] = format;
        header[27] = mips.len() as u8;
        header[36..40].copy_from_slice(&u32::from(sizes.is_some()).to_le_bytes());
        f.extend_from_slice(&header);
        if let Some((method, stored_sizes)) = sizes {
            // the extra entry: type 4, offset to its data (relative to the offset field), size
            let extra = f.len();
            f.extend_from_slice(&4u32.to_le_bytes());
            f.extend_from_slice(&8u32.to_le_bytes());
            f.extend_from_slice(&0u32.to_le_bytes());
            // the extra's data: the method, the array's offset (relative to its own field), the count, then the sizes
            let extra_data = f.len();
            assert_eq!(extra_data, extra + 4 + 8);
            f.extend_from_slice(&method.to_le_bytes());
            f.extend_from_slice(&8u32.to_le_bytes());
            f.extend_from_slice(&(stored_sizes.len() as u32).to_le_bytes());
            for size in stored_sizes {
                f.extend_from_slice(&size.to_le_bytes());
            }
        }
        let start = f.len();
        f[0..4].copy_from_slice(&(start as u32).to_le_bytes());
        for mip in mips.iter().rev() {
            f.extend_from_slice(mip);
        }
        f
    }

    #[test]
    fn reads_a_bc7_texture_with_raw_and_lz4_mips() {
        // 8x8 BC7: mip 0 is 2x2 blocks (64 bytes, stored LZ4), mip 1 one block (16 bytes, stored raw)
        let mip0: Vec<u8> = (0..64).map(|i| (i % 7) as u8).collect();
        let mip1: Vec<u8> = (0..16).map(|i| 200 - i as u8).collect();
        let packed0 = lz4_flex::block::compress(&mip0);
        assert!(packed0.len() < mip0.len());
        let file = vtex_file(20, 8, 8, &[packed0.clone(), mip1.clone()], &[packed0.len() as u32, 16]);
        let vtex = parse(&file).unwrap();
        assert_eq!((vtex.width, vtex.height, vtex.depth, vtex.format, vtex.mips), (8, 8, 1, Format::Bc7, 2));
        assert_eq!((vtex.mip_size(0), vtex.mip_size(1)), (64, 16));
        let mut out0 = vec![0; 64];
        let mut out1 = vec![0; 16];
        vtex.read_mip(&file, 0, &mut out0).unwrap();
        vtex.read_mip(&file, 1, &mut out1).unwrap();
        assert_eq!((out0, out1), (mip0, mip1));
    }

    #[test]
    fn sizes_bc1_and_bc6h_blocks_and_rounds_small_mips_up_to_a_block() {
        let file = vtex_file(1, 6, 4, &[vec![0; 16]], &[16]);
        let vtex = parse(&file).unwrap();
        assert_eq!(vtex.format, Format::Dxt1);
        // 6x4 is 2x1 blocks of 8 bytes; a 1x1 mip is still one block
        assert_eq!((vtex.mip_size(0), vtex.mip_size(3)), (16, 8));
        let lightmap = parse(&vtex_file(19, 8, 8, &[vec![0; 64]], &[64])).unwrap();
        assert_eq!((lightmap.format, lightmap.mip_size(0)), (Format::Bc6h, 64));
    }

    #[test]
    fn reads_raw_mips_with_or_without_compressed_mip_sizes() {
        // an 8x8 BC6H array of one layer, as the lightmaps are: no COMPRESSED_MIP_SIZE, so every mip is raw
        let mip0: Vec<u8> = (0..64).map(|i| i as u8).collect();
        let mip1: Vec<u8> = (100..116).collect();
        let file = vtex_file_with(19, (8, 8, 1), flags::ARRAY, &[mip0.clone(), mip1.clone()], None);
        let vtex = parse(&file).unwrap();
        assert_eq!((vtex.stored.clone(), vtex.lz4, vtex.is_2d()), (vec![64, 16], false, true));
        let mut out = vec![0; 64];
        vtex.read_mip(&file, 0, &mut out).unwrap();
        assert_eq!(out, mip0);
        let mut out = vec![0; 16];
        vtex.read_mip(&file, 1, &mut out).unwrap();
        assert_eq!(out, mip1);
        // method 0 lists the sizes but compresses nothing: a mip stored smaller than it decodes is an error
        let packed = lz4_flex::block::compress(&mip0);
        let file = vtex_file_with(20, (8, 8, 1), 0, std::slice::from_ref(&packed), Some((0, &[packed.len() as u32])));
        let vtex = parse(&file).unwrap();
        assert_eq!(vtex.read_mip(&file, 0, &mut [0; 64]), Err(VtexError::MipSize { mip: 0, stored: packed.len(), want: 64 }));
    }

    #[test]
    fn tells_a_plain_texture_from_a_cube_a_volume_and_an_unknown_compression() {
        let one = |flags, depth| parse(&vtex_file_with(20, (4, 4, depth), flags, &[vec![0; 16]], Some((1, &[16])))).unwrap();
        assert!(one(0, 1).is_2d());
        assert!(!one(flags::CUBE, 1).is_2d());
        assert!(!one(flags::VOLUME, 4).is_2d());
        assert!(!one(flags::ARRAY, 2).is_2d());
        assert_eq!(one(0, 1).mip_dimensions(1), (2, 2));
        let unknown = vtex_file_with(20, (4, 4, 1), 0, &[vec![0; 16]], Some((2, &[16])));
        assert_eq!(parse(&unknown), Err(VtexError::Malformed("an unknown mip compression method")));
        let short = vtex_file_with(20, (8, 8, 1), 0, &[vec![0; 64], vec![0; 16]], Some((1, &[64])));
        assert_eq!(parse(&short), Err(VtexError::Malformed("COMPRESSED_MIP_SIZE does not list every mip")));
    }

    #[test]
    fn a_cube_map_holds_six_faces_in_each_mip() {
        // a 4x4 BC6H cube, two mips: each mip is six one-block faces (16 bytes each), mip 0 stored LZ4 as a whole
        let mip0: Vec<u8> = (0..96).map(|i| (i / 16) as u8).collect();
        let mip1: Vec<u8> = (0..96).map(|i| 100 + (i / 16) as u8).collect();
        let packed = lz4_flex::block::compress(&mip0);
        let file = vtex_file_with(19, (4, 4, 1), flags::CUBE, &[packed.clone(), mip1.clone()], Some((1, &[packed.len() as u32, 96])));
        let vtex = parse(&file).unwrap();
        assert!(vtex.is_cube() && !vtex.is_2d());
        assert_eq!((vtex.faces(), vtex.mip_size(0), vtex.mip_bytes(0), vtex.mip_bytes(1)), (6, 16, 96, 96));
        let mut out = vec![0; 96];
        vtex.read_mip(&file, 0, &mut out).unwrap();
        assert_eq!(out, mip0);
        vtex.read_mip(&file, 1, &mut out).unwrap();
        assert_eq!(out, mip1);
        // without COMPRESSED_MIP_SIZE every face of every mip is raw
        let raw = vtex_file_with(19, (4, 4, 1), flags::CUBE, std::slice::from_ref(&mip0), None);
        assert_eq!(parse(&raw).unwrap().stored, vec![96]);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        assert_eq!(parse(&[0; 8]), Err(VtexError::Malformed("cut short")));
        assert_eq!(parse(&vtex_file(99, 4, 4, &[vec![0; 8]], &[8])), Err(VtexError::Format(99)));
        let file = vtex_file(20, 4, 4, &[vec![0; 16]], &[16]);
        let vtex = parse(&file).unwrap();
        assert_eq!(vtex.read_mip(&file[..file.len() - 1], 0, &mut [0; 16]), Err(VtexError::MipOutOfFile { mip: 0 }));
    }
}
