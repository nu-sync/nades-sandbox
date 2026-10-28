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
    /// The grenades' world models, which the game draws a thrown grenade with (empty in a pack from before them).
    pub grenades: Vec<GrenadeModel>,
}

/// A grenade's world model in Hammer units, z up, its long axis along z as held: its parts, each with its base colour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrenadeModel {
    /// hegrenade, flashbang, smokegrenade, molotov, incendiary or decoy.
    pub name: String,
    pub parts: Vec<GrenadePart>,
}

/// One mesh of a model and its texture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrenadePart {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// The base colour, RGBA8 sRGB, `width` × `height`, the first row the top.
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// An effects pack from before the grenades.
#[derive(Deserialize)]
struct EffectsV1 {
    noise: Vec<u8>,
    flipbooks: Vec<Flipbook>,
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

    /// The effects, from this build's layout or the one before the grenades (then with none).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, postcard::Error> {
        match postcard::from_bytes::<Self>(bytes) {
            Ok(effects) => Ok(effects),
            Err(e) => postcard::from_bytes::<EffectsV1>(bytes).map(|v1| Self { noise: v1.noise, flipbooks: v1.flipbooks, grenades: Vec::new() }).map_err(|_| e),
        }
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
        for g in &self.grenades {
            for p in &g.parts {
                let n = p.positions.len();
                if p.normals.len() != n || p.uvs.len() != n || p.indices.len() % 3 != 0 || p.indices.iter().any(|&i| i as usize >= n) || p.rgba.len() != (p.width * p.height * 4) as usize {
                    return Err(format!("{}: a part's vertices, indices or texture do not agree", g.name));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flipbook() -> Flipbook {
        Flipbook { name: "f".into(), cell: 1, columns: 1, rows: 1, sequences: vec![(0, 1)], rgba: vec![1, 2, 3, 4] }
    }

    #[test]
    fn packs_with_and_without_the_grenades_read_both_ways() {
        let part = GrenadePart { positions: vec![[0.0; 3]; 3], normals: vec![[0.0, 0.0, 1.0]; 3], uvs: vec![[0.0; 2]; 3], indices: vec![0, 1, 2], width: 1, height: 1, rgba: vec![9; 4] };
        let new = Effects { noise: vec![7; 8], flipbooks: vec![flipbook()], grenades: vec![GrenadeModel { name: "smokegrenade".into(), parts: vec![part] }] };
        assert_eq!(Effects::from_bytes(&new.to_bytes()).unwrap(), new);
        // a pack from before the grenades reads with none
        #[derive(Serialize)]
        struct Old {
            noise: Vec<u8>,
            flipbooks: Vec<Flipbook>,
        }
        let old = postcard::to_allocvec(&Old { noise: vec![7; 8], flipbooks: vec![flipbook()] }).unwrap();
        assert_eq!(Effects::from_bytes(&old).unwrap().grenades, []);
        // and a build from before them reads a new pack, the grenades left over at its end
        let read: EffectsV1 = postcard::from_bytes(&new.to_bytes()).unwrap();
        assert_eq!((read.noise.len(), read.flipbooks.len()), (8, 1));
    }
}
