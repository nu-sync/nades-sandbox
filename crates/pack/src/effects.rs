//! The shared effects pack (`<packs>/shared/effects.pack`, `nades-pack --effects`): what CS2 draws its smoke and fire
//! with, the same on every map, read from the game's `pak01` once. One section, Effects: an [`Effects`] (postcard).

use serde::{Deserialize, Serialize};

/// The smoke noise's texels a side.
pub const NOISE_SIZE: usize = 128;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effects {
    /// `materials/dev/noise/worley_perlin` (the smoke material's `g_tSmokeNoiseTex`, 128³ RGBA8) as one channel of
    /// R + 0.95 G, the only mix the smoke shader reads, x fastest: 128³ bytes.
    pub noise: Vec<u8>,
    /// The fire's flipbooks.
    pub flipbooks: Vec<Flipbook>,
}

/// One flipbook as an atlas of `cell`-texel frames, `columns` × `rows`, in sequence order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flipbook {
    /// e.g. `fire_small_sim_a`.
    pub name: String,
    pub cell: u32,
    pub columns: u32,
    pub rows: u32,
    /// Each sequence's first frame and its frame count.
    pub sequences: Vec<(u32, u32)>,
    /// RGBA8, straight alpha, the first row the top: (columns × cell) × (rows × cell) texels.
    pub rgba: Vec<u8>,
}

impl Effects {
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("effects always serialize")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }

    /// Whether every part has the size it says.
    pub fn validate(&self) -> Result<(), String> {
        if self.noise.len() != NOISE_SIZE * NOISE_SIZE * NOISE_SIZE {
            return Err(format!("the noise holds {} bytes, not 128³", self.noise.len()));
        }
        for f in &self.flipbooks {
            let want = (f.columns * f.cell) as usize * (f.rows * f.cell) as usize * 4;
            if f.rgba.len() != want {
                return Err(format!("{}: {} bytes, {want} expected", f.name, f.rgba.len()));
            }
            if f.sequences.iter().any(|&(first, count)| first + count > f.columns * f.rows) {
                return Err(format!("{}: a sequence runs past its atlas", f.name));
            }
        }
        Ok(())
    }
}
