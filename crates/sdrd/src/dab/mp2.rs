//! MPEG-1 Layer II frames for the simulated DAB stations: mono, 48 kHz,
//! 128 kbit/s (one 384-byte frame per 24 ms logical frame). Only the lowest
//! of the 32 sub-bands (0–750 Hz) carries sound, and its samples are written
//! directly, so no analysis filterbank is needed: a tone of f Hz there plays
//! as f Hz.

use super::dabplus::Bits;

/// Bytes per frame (and per logical frame of a 128 kbit/s sub-channel).
pub const FRAME_BYTES: usize = 384;
/// Sub-band 0 samples per frame (12 granules of 3), at 1500 per second.
pub const SAMPLES: usize = 36;
pub const SUBBAND_RATE: f64 = 1_500.0;

/// One frame from sub-band 0's samples (−1…1).
pub fn frame(samples: &[f32; SAMPLES]) -> Vec<u8> {
    let mut b = Bits::new();
    // Header: sync, MPEG-1, Layer II, no CRC, 128 kbit/s, 48 kHz, mono.
    b.put(0xFFF, 12);
    b.put(1, 1);
    b.put(0b10, 2);
    b.put(1, 1);
    b.put(8, 4);
    b.put(1, 2);
    b.put(0, 2);
    b.put(0b11, 2);
    b.put(0, 2);
    b.put(0, 4);
    // Bit allocation (table B.2a, 27 sub-bands): sub-band 0 at 15 levels
    // (index 3, 4 bits a sample), the rest none.
    b.put(3, 4);
    for sb in 1..27 {
        b.put(0, if sb < 11 { 4 } else if sb < 23 { 3 } else { 2 });
    }
    b.put(2, 2); // scfsi: one scale factor for the frame
    b.put(3, 6); // scale factor 1.0
    for &s in samples {
        b.put((7.0 + 7.0 * s.clamp(-1.0, 1.0)).round() as u32, 4);
    }
    let mut out = b.finish();
    out.resize(FRAME_BYTES, 0);
    out
}

/// `n` frames of a tune: `notes` (Hz) in turn, each for `note_frames`
/// frames, at amplitude `amp`.
pub fn tune(notes: &[f64], note_frames: usize, n: usize, amp: f32) -> Vec<Vec<u8>> {
    let mut phase = 0f64;
    (0..n)
        .map(|f| {
            let hz = notes[(f / note_frames) % notes.len()];
            let mut s = [0f32; SAMPLES];
            for v in s.iter_mut() {
                *v = amp * phase.sin() as f32;
                phase = (phase + std::f64::consts::TAU * hz / SUBBAND_RATE) % std::f64::consts::TAU;
            }
            frame(&s)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout() {
        let f = frame(&[0.0; SAMPLES]);
        assert_eq!(f.len(), FRAME_BYTES);
        assert_eq!(&f[..4], &[0xFF, 0xFD, 0x84, 0xC0]);
        // 32 + 88 allocation + 2 + 6 + 36 × 4 bits = 272 bits = 34 bytes; the rest is padding.
        assert!(f[34..].iter().all(|&b| b == 0));
        assert_eq!(tune(&[500.0], 4, 9, 0.5).len(), 9);
    }
}
