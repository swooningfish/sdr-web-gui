//! DAB (Eureka-147, ETSI EN 300 401), transmission mode I: enough of a
//! receiver to read a multiplex's station list from its Fast Information
//! Channel (FIC), for the band scan. No audio.
//!
//! Receiving (`ofdm`): find each frame by its null symbol, measure the
//! fractional frequency error from the cyclic prefixes, the whole-carrier
//! error and the fine timing from the phase reference symbol, then
//! differentially demodulate the three FIC symbols and undo the frequency
//! interleaving. Decoding (`fic`): split the soft bits into four FIC blocks,
//! de-puncture, Viterbi-decode the rate 1/4 K=7 code, undo the energy
//! dispersal, check each FIB's CRC and read FIG 0/0 (ensemble id), 1/0
//! (ensemble label) and 1/1 (programme service labels).
//!
//! `encode` builds whole frames from a list of services, for the tests and
//! the simulated receiver.

use num_complex::Complex32;

pub mod encode;
mod fic;
mod ofdm;

pub use ofdm::{decode, null_depth};

/// Sample rate the receiver works at (mode I is defined at 2.048 MS/s).
pub const RATE: u32 = 2_048_000;
/// Frame, null symbol, symbol, useful part and guard interval, in samples.
pub(crate) const T_F: usize = 196_608;
pub(crate) const T_NULL: usize = 2_656;
pub(crate) const T_S: usize = 2_552;
pub(crate) const T_U: usize = 2_048;
pub(crate) const T_G: usize = T_S - T_U;
/// OFDM symbols per frame (after the null) and carriers per symbol.
pub(crate) const L: usize = 76;
pub(crate) const K: usize = 1_536;
/// Symbols 1–3 carry the FIC.
pub(crate) const FIC_SYMBOLS: usize = 3;
/// Soft bits per FIC block (four per frame in mode I).
pub(crate) const FIC_BLOCK: usize = 2_304;

/// Band III channels (centre frequencies).
pub const CHANNELS: &[(&str, u64)] = &[
    ("5A", 174_928_000),
    ("5B", 176_640_000),
    ("5C", 178_352_000),
    ("5D", 180_064_000),
    ("6A", 181_936_000),
    ("6B", 183_648_000),
    ("6C", 185_360_000),
    ("6D", 187_072_000),
    ("7A", 188_928_000),
    ("7B", 190_640_000),
    ("7C", 192_352_000),
    ("7D", 194_064_000),
    ("8A", 195_936_000),
    ("8B", 197_648_000),
    ("8C", 199_360_000),
    ("8D", 201_072_000),
    ("9A", 202_928_000),
    ("9B", 204_640_000),
    ("9C", 206_352_000),
    ("9D", 208_064_000),
    ("10A", 209_936_000),
    ("10B", 211_648_000),
    ("10C", 213_360_000),
    ("10D", 215_072_000),
    ("11A", 216_928_000),
    ("11B", 218_640_000),
    ("11C", 220_352_000),
    ("11D", 222_064_000),
    ("12A", 223_936_000),
    ("12B", 225_648_000),
    ("12C", 227_360_000),
    ("12D", 229_072_000),
    ("13A", 230_784_000),
    ("13B", 232_496_000),
    ("13C", 234_208_000),
    ("13D", 235_776_000),
    ("13E", 237_488_000),
    ("13F", 239_200_000),
];

/// Phase reference symbol, mode I: rows (k_min, k_max, i, n) of EN 300 401
/// table 23; the phase of carrier k is π/2·(h[i][k − k_min] + n).
const PRS_ROWS: [(i16, i16, u8, u8); 48] = [
    (-768, -737, 0, 1),
    (-736, -705, 1, 2),
    (-704, -673, 2, 0),
    (-672, -641, 3, 1),
    (-640, -609, 0, 3),
    (-608, -577, 1, 2),
    (-576, -545, 2, 2),
    (-544, -513, 3, 3),
    (-512, -481, 0, 2),
    (-480, -449, 1, 1),
    (-448, -417, 2, 2),
    (-416, -385, 3, 3),
    (-384, -353, 0, 1),
    (-352, -321, 1, 2),
    (-320, -289, 2, 3),
    (-288, -257, 3, 3),
    (-256, -225, 0, 2),
    (-224, -193, 1, 2),
    (-192, -161, 2, 2),
    (-160, -129, 3, 1),
    (-128, -97, 0, 1),
    (-96, -65, 1, 3),
    (-64, -33, 2, 1),
    (-32, -1, 3, 2),
    (1, 32, 0, 3),
    (33, 64, 3, 1),
    (65, 96, 2, 1),
    (97, 128, 1, 1),
    (129, 160, 0, 2),
    (161, 192, 3, 2),
    (193, 224, 2, 1),
    (225, 256, 1, 0),
    (257, 288, 0, 2),
    (289, 320, 3, 2),
    (321, 352, 2, 3),
    (353, 384, 1, 3),
    (385, 416, 0, 0),
    (417, 448, 3, 2),
    (449, 480, 2, 1),
    (481, 512, 1, 3),
    (513, 544, 0, 3),
    (545, 576, 3, 3),
    (577, 608, 2, 3),
    (609, 640, 1, 0),
    (641, 672, 0, 3),
    (673, 704, 3, 0),
    (705, 736, 2, 1),
    (737, 768, 1, 1),
];

/// h[i][j] (table 24); each row is a 16-value pattern repeated twice.
const H: [[u8; 16]; 4] = [
    [0, 2, 0, 0, 0, 0, 1, 1, 2, 0, 0, 0, 2, 2, 1, 1],
    [0, 3, 2, 3, 0, 1, 3, 0, 2, 1, 2, 3, 2, 3, 3, 0],
    [0, 0, 0, 2, 0, 2, 1, 3, 2, 2, 0, 2, 2, 0, 1, 3],
    [0, 1, 2, 1, 0, 3, 3, 2, 2, 3, 2, 1, 2, 1, 3, 2],
];

/// FFT bin of carrier `k` (no shift: negative carriers at the top).
#[inline]
pub(crate) fn bin(k: i32) -> usize {
    k.rem_euclid(T_U as i32) as usize
}

/// Phase reference symbol, by FFT bin (zero where there's no carrier).
pub(crate) fn prs() -> Vec<Complex32> {
    let mut out = vec![Complex32::default(); T_U];
    for &(lo, hi, i, n) in &PRS_ROWS {
        for k in lo..=hi {
            let h = H[i as usize][((k - lo) % 16) as usize];
            let phi = std::f32::consts::FRAC_PI_2 * (h + n) as f32;
            out[bin(k as i32)] = Complex32::from_polar(1.0, phi);
        }
    }
    out
}

/// Frequency interleaving: the carrier (−768…768, not 0) that carries QPSK
/// symbol `n` of an OFDM symbol (EN 300 401 §14.6.1).
pub(crate) fn interleaver() -> Vec<i32> {
    let mut out = Vec::with_capacity(K);
    let mut p = 0usize;
    for _ in 0..T_U {
        if (256..=256 + K).contains(&p) && p != T_U / 2 {
            out.push(p as i32 - (T_U / 2) as i32);
        }
        p = (13 * p + 511) % T_U;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaver_covers_every_carrier_once() {
        let map = interleaver();
        assert_eq!(map.len(), K);
        let mut seen = vec![false; T_U];
        for &k in &map {
            assert!(k != 0 && (-768..=768).contains(&k), "{k}");
            assert!(!seen[bin(k)], "carrier {k} twice");
            seen[bin(k)] = true;
        }
    }

    #[test]
    fn reference_symbol_has_every_carrier() {
        let p = prs();
        assert_eq!(p.iter().filter(|z| z.norm() > 0.5).count(), K);
        assert_eq!(p[0].norm(), 0.0);
        assert_eq!(p[bin(769)].norm(), 0.0);
    }

    #[test]
    fn channels_are_sorted_and_in_band_iii() {
        assert_eq!(CHANNELS.len(), 38);
        assert!(CHANNELS.windows(2).all(|w| w[0].1 < w[1].1));
        assert_eq!(CHANNELS.iter().find(|c| c.0 == "11D").unwrap().1, 222_064_000);
    }
}
