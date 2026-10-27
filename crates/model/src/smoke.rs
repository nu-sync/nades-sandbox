//! The game's voxel-log encoding, owned here like the VTEX reader: a port of show-me-the-nades' `tracks::voxel_frames`
//! and `voxels::voxel_frames_with_emitters`, with their tests.
//!
//! A smoke's `m_VoxelFrameData` is an append-only log of frames, one per step of the server's cloud simulation (every
//! 0.1 s):
//!
//! ```text
//! frame   = u16 number, u16 payload length, payload            (little-endian)
//! payload = u8 fading, u8 flags,
//!           flags & 1: u8 n, n × emitter (u8 x, y, z, level, f32 hold)
//!           flags & 2: u16 n, n × brick (u16 index, u64 solid mask)
//!           u8 n, n × 20-byte hole
//! ```
//!
//! Coordinates index a 32³ grid of 20-unit voxels whose minimum corner is `m_vSmokeDetonationPos - 320` on every axis.
//! A brick is 4³ voxels; its index is a Morton code over the 8³ brick grid and each mask bit a Morton code within the
//! brick, x lowest. Set bits are world geometry, not smoke: the cloud is what the simulation fills from the emitters
//! through open voxels.

/// One emitter as networked: a cell of the 32³ grid, its level, and how long it holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitter {
    pub cell: [u8; 3],
    pub level: u8,
    pub hold: f32,
}

/// One decoded frame of a smoke's voxel log.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub number: u16,
    pub fading: bool,
    /// The emitters it sends (none for most frames).
    pub emitters: Vec<Emitter>,
    /// (brick index, solid mask).
    pub bricks: Vec<(u16, u64)>,
    pub holes: usize,
}

/// A frame whose payload does not match its declared length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the voxel log's frame at byte {at} does not match its declared length")]
pub struct DecodeError {
    /// Where the frame starts.
    pub at: usize,
}

/// Every complete frame; a frame still being written when the projectile went away ends the log.
pub fn decode(bytes: &[u8]) -> Result<Vec<Frame>, DecodeError> {
    let mut frames = vec![];
    let mut at = 0;
    while at + 4 <= bytes.len() {
        let number = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let len = usize::from(u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]));
        let Some(payload) = bytes.get(at + 4..at + 4 + len) else {
            break;
        };
        frames.push(payload_of(number, payload).ok_or(DecodeError { at })?);
        at += 4 + len;
    }
    Ok(frames)
}

fn payload_of(number: u16, p: &[u8]) -> Option<Frame> {
    let (&fading, rest) = p.split_first()?;
    let (&flags, mut rest) = rest.split_first()?;
    let mut emitters = vec![];
    if flags & 1 != 0 {
        let (&n, after) = rest.split_first()?;
        let n = usize::from(n);
        emitters = after
            .get(..n * 8)?
            .chunks_exact(8)
            .map(|e| Emitter { cell: [e[0], e[1], e[2]], level: e[3], hold: f32::from_le_bytes([e[4], e[5], e[6], e[7]]) })
            .collect();
        rest = &after[n * 8..];
    }
    let mut bricks = vec![];
    if flags & 2 != 0 {
        let n = usize::from(u16::from_le_bytes([*rest.first()?, *rest.get(1)?]));
        let body = rest.get(2..2 + n * 10)?;
        bricks = body
            .chunks_exact(10)
            .map(|b| (u16::from_le_bytes([b[0], b[1]]), u64::from_le_bytes(b[2..10].try_into().expect("ten-byte chunk"))))
            .collect();
        rest = &rest[2 + n * 10..];
    }
    let (&holes, rest) = rest.split_first()?;
    let holes = usize::from(holes);
    (rest.len() == holes * 20).then_some(Frame { number, fading: fading != 0, emitters, bricks, holes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(number: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = number.to_le_bytes().to_vec();
        out.extend((payload.len() as u16).to_le_bytes());
        out.extend(payload);
        out
    }

    /// A frame with two emitters and one brick.
    fn sending() -> Vec<u8> {
        let mut first = vec![0, 3, 2];
        first.extend([16, 16, 13, 5, 0, 0, 0, 0, 12, 15, 16, 5, 0, 0, 128, 63]);
        first.extend(1u16.to_le_bytes());
        first.extend(286u16.to_le_bytes());
        first.extend(0x0f0f_0f0f_0000_0000u64.to_le_bytes());
        first.push(0);
        first
    }

    #[test]
    fn voxel_log_decodes_emitters_bricks_and_empty_frames() {
        let mut log = frame(0, &sending());
        log.extend(frame(1, &[0, 0, 0]));
        log.extend(frame(2, &[1, 0, 0]));

        let frames = decode(&log).unwrap();

        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].emitters.len(), 2);
        assert_eq!(frames[0].bricks, vec![(286, 0x0f0f_0f0f_0000_0000)]);
        assert_eq!((frames[1].number, frames[1].fading), (1, false));
        assert!(frames[2].fading);
    }

    #[test]
    fn voxel_log_rejects_a_payload_longer_than_its_contents() {
        let log = frame(0, &[0, 0, 0, 9]);
        assert_eq!(decode(&log), Err(DecodeError { at: 0 }));
    }

    #[test]
    fn voxel_log_ignores_a_frame_still_being_written() {
        let mut log = frame(0, &[0, 0, 0]);
        log.extend([1, 0, 3, 0, 0]);
        assert_eq!(decode(&log).unwrap().len(), 1);
    }

    #[test]
    fn emitters_come_with_the_frame_that_sends_them() {
        let mut log = frame(0, &[0, 2, 0, 0, 0]);
        log.extend(frame(1, &sending()));
        log.extend(frame(2, &[1, 1, 1, 3, 4, 5, 6, 0, 0, 0, 64, 0]));

        let frames = decode(&log).unwrap();

        assert_eq!(frames.len(), 3);
        assert!(frames[0].emitters.is_empty());
        assert_eq!(frames[1].bricks, vec![(286, 0x0f0f_0f0f_0000_0000)]);
        assert_eq!(frames[1].emitters, vec![Emitter { cell: [16, 16, 13], level: 5, hold: 0.0 }, Emitter { cell: [12, 15, 16], level: 5, hold: 1.0 }]);
        assert_eq!(frames[2].emitters, vec![Emitter { cell: [3, 4, 5], level: 6, hold: 2.0 }]);
        assert!(frames[2].fading);
    }

    #[test]
    fn a_malformed_frame_is_reported_where_it_starts() {
        let mut log = frame(0, &[0, 0, 0]);
        log.extend(frame(1, &[0, 0, 0, 9]));
        assert_eq!(decode(&log), Err(DecodeError { at: 7 }));
    }
}
