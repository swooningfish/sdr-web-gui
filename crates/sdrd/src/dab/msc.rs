//! Main Service Channel: a sub-channel's bits out of each CIF, time
//! de-interleaving over 16 CIFs, de-puncturing (EEP or UEP protection),
//! Viterbi and energy dispersal, giving one logical frame of bytes per CIF
//! (every 24 ms). Also the matching transmitter steps for the tests and the
//! simulated receiver.

use super::fic::{conv_encode, prbs, viterbi};

/// Bits per capacity unit, and CUs per CIF.
pub const CU_BITS: usize = 64;
pub const CIF_CUS: usize = 864;
pub const CIF_BITS: usize = CU_BITS * CIF_CUS;

/// Time interleaving: bit i is delayed by `MAP[i % 16]` CIFs in the
/// transmitter and the rest of 16 in the receiver.
const MAP: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];

/// Puncturing vectors PI_1…PI_24 (MSB first; PI_x keeps x + 8 of 32).
const PI: [u32; 24] = [
    0xC888_8888, 0xC888_C888, 0xC8C8_C888, 0xC8C8_C8C8, 0xCCC8_C8C8, 0xCCC8_CCC8, 0xCCCC_CCC8, 0xCCCC_CCCC,
    0xECCC_CCCC, 0xECCC_ECCC, 0xECEC_ECCC, 0xECEC_ECEC, 0xEEEC_ECEC, 0xEEEC_EEEC, 0xEEEE_EEEC, 0xEEEE_EEEE,
    0xFEEE_EEEE, 0xFEEE_FEEE, 0xFEFE_FEEE, 0xFEFE_FEFE, 0xFFFE_FEFE, 0xFFFE_FFFE, 0xFFFF_FFFE, 0xFFFF_FFFF,
];
/// Tail (24 bits).
const PI_X: u32 = 0b1100_1100_1100_1100_1100_1100;

/// Short-form (UEP) protection, by table index: bit rate (kbit/s), size
/// (CUs) and four (block count, PI) pairs (EN 300 401 table 8).
type Uep = (u16, u16, [(u8, u8); 4]);
pub const UEP: [Uep; 64] = [
    (32, 16, [(3, 5), (4, 3), (17, 2), (0, 0)]),
    (32, 21, [(3, 11), (3, 6), (18, 5), (0, 0)]),
    (32, 24, [(3, 15), (4, 9), (14, 6), (3, 8)]),
    (32, 29, [(3, 22), (4, 13), (14, 8), (3, 13)]),
    (32, 35, [(3, 24), (5, 17), (13, 12), (3, 17)]),
    (48, 24, [(4, 5), (3, 4), (26, 2), (3, 3)]),
    (48, 29, [(3, 9), (4, 6), (26, 4), (3, 6)]),
    (48, 35, [(3, 15), (4, 10), (26, 6), (3, 9)]),
    (48, 42, [(3, 24), (4, 14), (26, 8), (3, 15)]),
    (48, 52, [(3, 24), (5, 18), (25, 13), (3, 18)]),
    (56, 29, [(6, 5), (10, 4), (23, 2), (3, 3)]),
    (56, 35, [(6, 9), (10, 6), (23, 4), (3, 5)]),
    (56, 42, [(6, 16), (12, 7), (21, 6), (3, 9)]),
    (56, 52, [(6, 23), (10, 13), (23, 8), (3, 13)]),
    (64, 32, [(6, 5), (9, 3), (31, 2), (2, 3)]),
    (64, 42, [(6, 11), (9, 6), (33, 5), (0, 0)]),
    (64, 48, [(6, 16), (12, 8), (27, 6), (3, 9)]),
    (64, 58, [(6, 23), (10, 13), (29, 8), (3, 13)]),
    (64, 70, [(6, 24), (11, 18), (28, 12), (3, 18)]),
    (80, 40, [(6, 6), (10, 3), (41, 2), (3, 3)]),
    (80, 52, [(6, 11), (10, 6), (41, 5), (3, 6)]),
    (80, 58, [(6, 16), (11, 8), (40, 6), (3, 7)]),
    (80, 70, [(6, 23), (10, 13), (41, 8), (3, 13)]),
    (80, 84, [(6, 24), (10, 17), (41, 12), (3, 18)]),
    (96, 48, [(7, 5), (9, 4), (53, 2), (3, 4)]),
    (96, 58, [(7, 9), (10, 6), (52, 4), (3, 6)]),
    (96, 70, [(6, 16), (12, 9), (51, 6), (3, 10)]),
    (96, 84, [(6, 22), (10, 12), (53, 9), (3, 12)]),
    (96, 104, [(6, 24), (13, 18), (50, 13), (3, 19)]),
    (112, 58, [(14, 5), (17, 4), (50, 2), (3, 5)]),
    (112, 70, [(11, 9), (21, 6), (49, 4), (3, 8)]),
    (112, 84, [(11, 16), (23, 8), (47, 6), (3, 9)]),
    (112, 104, [(11, 23), (21, 12), (49, 9), (3, 14)]),
    (128, 64, [(12, 5), (19, 3), (62, 2), (3, 4)]),
    (128, 84, [(11, 11), (21, 6), (61, 5), (3, 7)]),
    (128, 96, [(11, 16), (22, 9), (60, 6), (3, 10)]),
    (128, 116, [(11, 22), (21, 12), (61, 9), (3, 14)]),
    (128, 140, [(11, 24), (20, 17), (62, 13), (3, 19)]),
    (160, 80, [(11, 5), (19, 4), (87, 2), (3, 4)]),
    (160, 104, [(11, 11), (23, 6), (83, 5), (3, 9)]),
    (160, 116, [(11, 16), (24, 8), (82, 6), (3, 11)]),
    (160, 140, [(11, 22), (21, 11), (85, 9), (3, 13)]),
    (160, 168, [(11, 24), (22, 18), (84, 12), (3, 19)]),
    (192, 96, [(11, 6), (20, 4), (110, 2), (3, 5)]),
    (192, 116, [(11, 10), (22, 6), (108, 4), (3, 9)]),
    (192, 140, [(11, 16), (24, 10), (106, 6), (3, 11)]),
    (192, 168, [(11, 22), (20, 13), (110, 9), (3, 13)]),
    (192, 208, [(11, 24), (21, 20), (109, 13), (3, 24)]),
    (224, 116, [(12, 8), (22, 6), (131, 2), (3, 6)]),
    (224, 140, [(12, 12), (26, 8), (127, 4), (3, 11)]),
    (224, 168, [(11, 16), (20, 10), (134, 7), (3, 9)]),
    (224, 208, [(11, 24), (22, 16), (132, 10), (3, 15)]),
    (224, 232, [(11, 24), (24, 20), (130, 12), (3, 20)]),
    (256, 128, [(11, 6), (24, 5), (154, 2), (3, 5)]),
    (256, 168, [(11, 12), (24, 9), (154, 5), (3, 10)]),
    (256, 192, [(11, 16), (27, 10), (151, 7), (3, 10)]),
    (256, 232, [(11, 24), (22, 14), (156, 10), (3, 13)]),
    (256, 280, [(11, 24), (26, 19), (152, 14), (3, 18)]),
    (320, 160, [(11, 8), (26, 5), (200, 2), (3, 6)]),
    (320, 208, [(11, 13), (25, 9), (201, 5), (3, 10)]),
    (320, 280, [(11, 24), (26, 17), (200, 9), (3, 17)]),
    (384, 192, [(11, 8), (27, 6), (247, 2), (3, 7)]),
    (384, 280, [(11, 16), (24, 9), (250, 7), (3, 10)]),
    (384, 416, [(12, 24), (28, 20), (245, 14), (3, 23)]),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    /// Short form: index into [`UEP`].
    Uep(u8),
    /// Long form, option A or B, level 1–4.
    EepA(u8),
    EepB(u8),
    Unknown,
}

/// How a service's audio is coded (FIG 0/2 ASCTy).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioKind {
    /// MPEG-1/2 Layer II ("DAB").
    Mp2,
    /// HE-AAC v2 in superframes ("DAB+").
    #[serde(rename = "dab+")]
    DabPlus,
    Other,
}

/// A sub-channel of the MSC (FIG 0/1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubChannel {
    pub id: u8,
    /// Start address and size in CUs.
    pub start: u16,
    pub size: u16,
    pub protection: Protection,
}

impl SubChannel {
    /// Bit rate (kbit/s) and puncturing: (blocks of 128 mother code bits, PI).
    fn coding(&self) -> Option<(usize, Vec<(usize, u8)>)> {
        let s = self.size as usize;
        let (b, blocks) = match self.protection {
            Protection::Uep(i) => {
                let (rate, size, rows) = UEP.get(i as usize)?;
                if *size as usize != s {
                    return None;
                }
                (*rate as usize, rows.iter().filter(|r| r.0 > 0).map(|r| (r.0 as usize, r.1)).collect())
            }
            Protection::EepA(level) => {
                let b = match level {
                    1 => s * 2 / 3,
                    2 => s,
                    3 => s * 4 / 3,
                    4 => s * 2,
                    _ => return None,
                };
                if b == 0 || b % 8 != 0 {
                    return None;
                }
                let n = b / 8;
                let rows = match level {
                    1 => vec![(6 * n - 3, 24), (3, 23)],
                    2 if n == 1 => vec![(5, 13), (1, 12)],
                    2 => vec![(2 * n - 3, 14), (4 * n + 3, 13)],
                    3 => vec![(6 * n - 3, 8), (3, 7)],
                    _ => vec![(4 * n - 3, 3), (2 * n + 3, 2)],
                };
                (b, rows)
            }
            Protection::EepB(level) => {
                let b = match level {
                    1 => s * 32 / 27,
                    2 => s * 32 / 21,
                    3 => s * 16 / 9,
                    4 => s * 32 / 15,
                    _ => return None,
                };
                if b == 0 || b % 32 != 0 {
                    return None;
                }
                let (p1, p2) = [(10, 9), (6, 5), (4, 3), (2, 1)][level as usize - 1];
                (b, vec![(24 * b / 32 - 3, p1), (3, p2)])
            }
            Protection::Unknown => return None,
        };
        (blocks.iter().map(|r| r.0).sum::<usize>() * 4 == 3 * b).then_some((b, blocks))
    }

    /// Audio bit rate, kbit/s.
    pub fn bitrate(&self) -> Option<u32> {
        self.coding().map(|c| c.0 as u32)
    }
}

/// Which mother code bits are sent, for this puncturing.
fn sent(blocks: &[(usize, u8)]) -> Vec<bool> {
    let mut out = Vec::new();
    for &(n, pi) in blocks {
        let v = PI[pi as usize - 1];
        for i in 0..n * 128 {
            out.push((v >> (31 - i % 32)) & 1 == 1);
        }
    }
    out.extend((0..24).map(|i| (PI_X >> (23 - i)) & 1 == 1));
    out
}

/// Bits (MSB first) to bytes.
fn pack(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8).map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | b)).collect()
}

/// Decodes one sub-channel from successive CIFs.
pub struct SubDecoder {
    sub: SubChannel,
    sent: Vec<bool>,
    prbs: Vec<u8>,
    hist: Vec<Vec<f32>>,
    idx: usize,
    filled: usize,
    mother: Vec<f32>,
}

impl SubDecoder {
    pub fn new(sub: &SubChannel) -> Option<SubDecoder> {
        let (b, blocks) = sub.coding()?;
        let n = sub.size as usize * CU_BITS;
        if (sub.start as usize + sub.size as usize) > CIF_CUS {
            return None;
        }
        let sent = sent(&blocks);
        Some(SubDecoder {
            sub: sub.clone(),
            prbs: prbs(24 * b),
            hist: vec![vec![0.0; n]; 16],
            idx: 0,
            filled: 0,
            mother: vec![0.0; sent.len()],
            sent,
        })
    }

    #[cfg(test)]
    pub fn subchannel(&self) -> &SubChannel {
        &self.sub
    }

    /// Feed one CIF's soft bits (positive = 0); returns the logical frame
    /// 16 CIFs later, once the de-interleaver is full.
    pub fn push(&mut self, cif: &[f32]) -> Option<Vec<u8>> {
        let start = self.sub.start as usize * CU_BITS;
        let data = &cif[start..start + self.hist[0].len()];
        let mut out = vec![0f32; data.len()];
        for (i, (o, &d)) in out.iter_mut().zip(data).enumerate() {
            *o = self.hist[(self.idx + MAP[i & 15]) & 15][i];
            self.hist[self.idx][i] = d;
        }
        self.idx = (self.idx + 1) & 15;
        self.filled += 1;
        if self.filled <= 16 {
            return None;
        }
        let mut it = out.iter();
        for (m, &s) in self.mother.iter_mut().zip(&self.sent) {
            *m = if s { *it.next().unwrap_or(&0.0) } else { 0.0 };
        }
        let mut bits = viterbi(&self.mother);
        for (b, p) in bits.iter_mut().zip(&self.prbs) {
            *b ^= p;
        }
        Some(pack(&bits))
    }
}

/// Transmitter: one logical frame (3 × bit rate bytes) to the sub-channel's
/// bits for one CIF, before time interleaving.
pub(crate) fn protect(sub: &SubChannel, frame: &[u8]) -> Vec<u8> {
    let (b, blocks) = sub.coding().expect("valid sub-channel");
    assert_eq!(frame.len(), 3 * b, "a logical frame is 3 bytes per kbit/s");
    let bits: Vec<u8> = frame
        .iter()
        .flat_map(|&byte| (0..8).rev().map(move |i| (byte >> i) & 1))
        .zip(prbs(24 * b))
        .map(|(x, p)| x ^ p)
        .collect();
    let coded = conv_encode(&bits);
    let mut out: Vec<u8> = coded.iter().zip(sent(&blocks)).filter(|(_, s)| *s).map(|(&c, _)| c).collect();
    out.resize(sub.size as usize * CU_BITS, 0);
    out
}

/// Transmitter: time-interleave a repeating sequence of protected frames
/// (as `protect` returns): CIF `t` takes bit i from frame t − MAP[i % 16].
pub(crate) fn interleave_loop(frames: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let n = frames.len();
    (0..n)
        .map(|t| {
            (0..frames[0].len()).map(|i| frames[(t + n * 16 - MAP[i & 15]) % n][i]).collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puncturing_vectors_keep_x_plus_8() {
        for (i, v) in PI.iter().enumerate() {
            assert_eq!(v.count_ones() as usize, i + 9, "PI_{}", i + 1);
        }
    }

    #[test]
    fn every_protection_fills_its_size() {
        let check = |p: Protection, size: u16| {
            let s = SubChannel { id: 0, start: 0, size, protection: p };
            let (_, blocks) = s.coding().unwrap_or_else(|| panic!("{p:?} {size}"));
            sent(&blocks).iter().filter(|&&x| x).count()
        };
        for (i, u) in UEP.iter().enumerate() {
            // Some rows leave a few padding bits at the end.
            let kept = check(Protection::Uep(i as u8), u.1);
            let pad = (u.1 as usize * 64).checked_sub(kept).unwrap_or_else(|| panic!("UEP {i} overflows"));
            assert!(pad <= 12 && pad % 4 == 0, "UEP {i}: {pad} bits of padding");
        }
        for b in [8, 16, 32, 48, 64, 96, 128, 192, 256] {
            for (level, size) in [(1, b * 3 / 2), (2, b), (3, b * 3 / 4), (4, b / 2)] {
                if b * 3 % 2 == 0 && b * 3 % 4 == 0 {
                    assert_eq!(check(Protection::EepA(level), size as u16), size * 64, "EEP {level}-A {b}");
                }
            }
        }
        for b in [32, 64, 96, 128, 192] {
            for (level, size) in [(1, b * 27 / 32), (2, b * 21 / 32), (3, b * 9 / 16), (4, b * 15 / 32)] {
                assert_eq!(check(Protection::EepB(level), size as u16), size * 64, "EEP {level}-B {b}");
            }
        }
    }

    #[test]
    fn a_subchannel_survives_interleaving_and_noise() {
        let sub = SubChannel { id: 3, start: 100, size: 96, protection: Protection::EepA(3) }; // 128 kbit/s
        assert_eq!(sub.bitrate(), Some(128));
        let frames: Vec<Vec<u8>> = (0..20u32).map(|t| (0..384u32).map(|i| (i * 31 + t * 7) as u8).collect()).collect();
        let tx = interleave_loop(&frames.iter().map(|f| protect(&sub, f)).collect::<Vec<_>>());
        let mut dec = SubDecoder::new(&sub).unwrap();
        let mut got = Vec::new();
        let mut rng = 5u64;
        for t in 0..60 {
            let mut cif = vec![0f32; CIF_BITS];
            for (i, &b) in tx[t % tx.len()].iter().enumerate() {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                let noise = ((rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 1.6;
                cif[100 * 64 + i] = if b == 0 { 1.0 } else { -1.0 } + noise;
            }
            if let Some(f) = dec.push(&cif) {
                got.push((t, f));
            }
        }
        assert_eq!(got.len(), 60 - 16);
        for (t, f) in got {
            assert_eq!(f, frames[(t - 16) % frames.len()], "CIF {t}");
        }
    }
}
