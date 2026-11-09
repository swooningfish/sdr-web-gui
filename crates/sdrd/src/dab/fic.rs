//! FIC decoding: de-puncturing, Viterbi, energy dispersal, FIB CRC and the
//! FIGs that name the ensemble and its services. Also the matching encoder
//! steps, used by `encode`.

use std::collections::BTreeMap;

use super::FIC_BLOCK;

/// Information bits per FIC block (three FIBs).
pub(crate) const BLOCK_BITS: usize = 768;
/// Mother code bits per FIC block, tail included: (768 + 6) × 4.
const MOTHER: usize = (BLOCK_BITS + 6) * 4;

/// Puncturing vectors PI_15, PI_16 and the tail vector (MSB first; 1 = sent).
const PI_15: u32 = 0b1110_1110_1110_1110_1110_1110_1110_1100;
const PI_16: u32 = 0b1110_1110_1110_1110_1110_1110_1110_1110;
const PI_X: u32 = 0b1100_1100_1100_1100_1100_1100;

/// Which mother code bits of a FIC block are sent (mode I: 21 × 128 bits at
/// PI_16, 3 × 128 at PI_15, then the 24 tail bits at PI_X).
pub(crate) fn sent() -> Vec<bool> {
    (0..MOTHER)
        .map(|i| {
            if i < 3072 {
                let pi = if i / 128 < 21 { PI_16 } else { PI_15 };
                (pi >> (31 - i % 32)) & 1 == 1
            } else {
                (PI_X >> (23 - (i - 3072))) & 1 == 1
            }
        })
        .collect()
}

/// Encoder outputs (x0, x1, x2, x3) for input `a` with the previous six
/// inputs in `state` (bit 0 = the latest). Generators 133, 171, 145, 133
/// (octal), i.e. 1+D²+D³+D⁵+D⁶, 1+D+D²+D³+D⁶, 1+D+D⁴+D⁶ and the first again.
#[inline]
fn outputs(a: u8, state: u8) -> [u8; 4] {
    let d = |n: u32| if n == 0 { a } else { (state >> (n - 1)) & 1 };
    let x0 = d(0) ^ d(2) ^ d(3) ^ d(5) ^ d(6);
    let x1 = d(0) ^ d(1) ^ d(2) ^ d(3) ^ d(6);
    let x2 = d(0) ^ d(1) ^ d(4) ^ d(6);
    [x0, x1, x2, x0]
}

/// Convolutional encoding with the six tail bits (for the encoder).
pub(crate) fn conv_encode(bits: &[u8]) -> Vec<u8> {
    let mut state = 0u8;
    let mut out = Vec::with_capacity((bits.len() + 6) * 4);
    for &a in bits.iter().chain(&[0u8; 6]) {
        out.extend_from_slice(&outputs(a, state));
        state = ((state << 1) | a) & 0x3F;
    }
    out
}

/// Viterbi decoder for the K=7 rate 1/4 mother code. `soft`: one value per
/// mother code bit, positive for 0 and negative for 1 (0 = punctured).
/// The encoder starts and ends (after the tail) in state 0.
pub(crate) fn viterbi(soft: &[f32]) -> Vec<u8> {
    let steps = soft.len() / 4;
    // Expected outputs per (state, input), as ±1 (+1 for a 0 bit).
    let mut expect = [[[0f32; 4]; 2]; 64];
    for (s, e) in expect.iter_mut().enumerate() {
        for (a, ea) in e.iter_mut().enumerate() {
            let o = outputs(a as u8, s as u8);
            for (x, &bit) in ea.iter_mut().zip(&o) {
                *x = if bit == 0 { 1.0 } else { -1.0 };
            }
        }
    }
    let mut metric = [f32::NEG_INFINITY; 64];
    metric[0] = 0.0;
    // Per step, per state: which of the two predecessors won (its top bit).
    let mut choice = vec![0u64; steps];
    for (t, c) in choice.iter_mut().enumerate() {
        let r = &soft[t * 4..t * 4 + 4];
        let mut next = [f32::NEG_INFINITY; 64];
        for (n, slot) in next.iter_mut().enumerate() {
            let a = n & 1;
            let mut best = f32::NEG_INFINITY;
            let mut pick = 0u64;
            for top in 0..2 {
                let s = (n >> 1) | (top << 5);
                let e = &expect[s][a];
                let m = metric[s] + e[0] * r[0] + e[1] * r[1] + e[2] * r[2] + e[3] * r[3];
                if m > best {
                    best = m;
                    pick = top as u64;
                }
            }
            *slot = best;
            *c |= pick << n;
        }
        metric = next;
    }
    let mut state = 0usize;
    let mut bits = vec![0u8; steps];
    for t in (0..steps).rev() {
        bits[t] = (state & 1) as u8;
        let top = ((choice[t] >> state) & 1) as usize;
        state = (state >> 1) | (top << 5);
    }
    bits.truncate(steps.saturating_sub(6));
    bits
}

/// Energy dispersal sequence (x⁹ + x⁵ + 1, all ones to start), restarted
/// for each FIC block.
pub(crate) fn prbs(n: usize) -> Vec<u8> {
    let mut sr = [1u8; 9];
    (0..n)
        .map(|_| {
            let b = sr[8] ^ sr[4];
            sr.copy_within(0..8, 1);
            sr[0] = b;
            b
        })
        .collect()
}

/// CRC-16-CCITT (x¹⁶ + x¹² + x⁵ + 1), preset to ones, sent inverted.
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    !crc
}

/// EBU Latin character set (ETSI TS 101 756 annex C) as Unicode.
const EBU_LATIN: [u16; 256] = [
    0x00, 0x118, 0x12e, 0x172, 0x102, 0x116, 0x10e, 0x218, 0x21a, 0x10a, 0x0a, 0x0b, 0x120, 0x139, 0x17b, 0x143,
    0x105, 0x119, 0x12f, 0x173, 0x103, 0x117, 0x10f, 0x219, 0x21b, 0x10b, 0x147, 0x11a, 0x121, 0x13a, 0x17c, 0x82,
    0x20, 0x21, 0x22, 0x23, 0x142, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f,
    0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f,
    0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x5b, 0x16e, 0x5d, 0x141, 0x5f,
    0x104, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f,
    0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0xab, 0x16f, 0xbb, 0x13d, 0x126,
    0xe1, 0xe0, 0xe9, 0xe8, 0xed, 0xec, 0xf3, 0xf2, 0xfa, 0xf9, 0xd1, 0xc7, 0x15e, 0xdf, 0xa1, 0x178,
    0xe2, 0xe4, 0xea, 0xeb, 0xee, 0xef, 0xf4, 0xf6, 0xfb, 0xfc, 0xf1, 0xe7, 0x15f, 0x11f, 0x131, 0xff,
    0x136, 0x145, 0xa9, 0x122, 0x11e, 0x11b, 0x148, 0x151, 0x150, 0x20ac, 0xa3, 0x24, 0x100, 0x112, 0x12a, 0x16a,
    0x137, 0x146, 0x13b, 0x123, 0x13c, 0x130, 0x144, 0x171, 0x170, 0xbf, 0x13e, 0xb0, 0x101, 0x113, 0x12b, 0x16b,
    0xc1, 0xc0, 0xc9, 0xc8, 0xcd, 0xcc, 0xd3, 0xd2, 0xda, 0xd9, 0x158, 0x10c, 0x160, 0x17d, 0xd0, 0x13f,
    0xc2, 0xc4, 0xca, 0xcb, 0xce, 0xcf, 0xd4, 0xd6, 0xdb, 0xdc, 0x159, 0x10d, 0x161, 0x17e, 0x111, 0x140,
    0xc3, 0xc5, 0xc6, 0x152, 0x177, 0xdd, 0xd5, 0xd8, 0xde, 0x14a, 0x154, 0x106, 0x15a, 0x179, 0x164, 0xf0,
    0xe3, 0xe5, 0xe6, 0x153, 0x175, 0xfd, 0xf5, 0xf8, 0xfe, 0x14b, 0x155, 0x107, 0x15b, 0x17a, 0x165, 0x127,
];

/// A 16-byte label in FIG 1's character set (0 = EBU Latin, 15 = UTF-8).
fn label_text(charset: u8, raw: &[u8]) -> String {
    let s = if charset == 15 {
        String::from_utf8_lossy(raw).into_owned()
    } else {
        raw.iter()
            .map(|&b| char::from_u32(EBU_LATIN[b as usize] as u32).unwrap_or(' '))
            .collect()
    };
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// Byte for `c` in EBU Latin (for the encoder); unknown characters become spaces.
pub(crate) fn ebu_byte(c: char) -> u8 {
    EBU_LATIN.iter().position(|&u| u as u32 == c as u32 && u >= 0x20).map_or(b' ', |p| p as u8)
}

/// What the FIC said about the multiplex.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ensemble {
    pub eid: Option<u16>,
    pub label: Option<String>,
    /// Programme services by service id: label.
    pub services: BTreeMap<u16, String>,
    /// FIBs that passed / failed their CRC.
    pub fibs_ok: u32,
    pub fibs_bad: u32,
}

impl Ensemble {
    /// Decode one FIC block (2304 soft bits, positive = 0).
    pub(crate) fn add_block(&mut self, soft: &[f32], sent: &[bool], prbs: &[u8]) {
        debug_assert_eq!(soft.len(), FIC_BLOCK);
        let mut mother = vec![0f32; MOTHER];
        let mut it = soft.iter();
        for (m, &s) in mother.iter_mut().zip(sent) {
            if s {
                *m = *it.next().unwrap_or(&0.0);
            }
        }
        let bits = viterbi(&mother);
        for f in 0..3 {
            let mut fib = [0u8; 32];
            for (i, byte) in fib.iter_mut().enumerate() {
                for b in 0..8 {
                    let k = f * 256 + i * 8 + b;
                    *byte = (*byte << 1) | (bits[k] ^ prbs[k]);
                }
            }
            if crc16(&fib[..30]) == u16::from_be_bytes([fib[30], fib[31]]) {
                self.fibs_ok += 1;
                self.fib(&fib[..30]);
            } else {
                self.fibs_bad += 1;
            }
        }
    }

    /// The FIGs in one FIB's 30 data bytes.
    fn fib(&mut self, d: &[u8]) {
        let mut i = 0;
        while i < d.len() {
            let h = d[i];
            if h == 0xFF {
                break;
            }
            let (kind, len) = (h >> 5, (h & 0x1F) as usize);
            let end = i + 1 + len;
            if len == 0 || end > d.len() {
                break;
            }
            let body = &d[i + 1..end];
            match kind {
                0 => self.fig0(body),
                1 => self.fig1(body),
                _ => {}
            }
            i = end;
        }
    }

    /// FIG 0: only extension 0 (the ensemble id) is needed.
    fn fig0(&mut self, d: &[u8]) {
        if d.len() >= 3 && d[0] & 0x1F == 0 {
            self.eid = Some(u16::from_be_bytes([d[1], d[2]]));
        }
    }

    /// FIG 1: labels. Extension 0 names the ensemble, 1 a programme service.
    fn fig1(&mut self, d: &[u8]) {
        if d.len() < 19 {
            return;
        }
        let (charset, other, ext) = (d[0] >> 4, d[0] & 0x08 != 0, d[0] & 0x07);
        if other {
            return; // another ensemble's
        }
        let id = u16::from_be_bytes([d[1], d[2]]);
        let label = label_text(charset, &d[3..19]);
        if label.is_empty() {
            return;
        }
        match ext {
            0 => {
                self.eid.get_or_insert(id);
                self.label = Some(label);
            }
            1 => {
                self.services.insert(id, label);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puncturing_leaves_a_block() {
        assert_eq!(sent().iter().filter(|&&s| s).count(), FIC_BLOCK);
    }

    #[test]
    fn energy_dispersal_starts_as_in_the_standard() {
        let p = prbs(16);
        assert_eq!(p, [0, 0, 0, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0]);
    }

    #[test]
    fn crc_known_value() {
        // CRC-16/GENIBUS (CCITT, preset ones, inverted) of "123456789".
        assert_eq!(crc16(b"123456789"), 0xD64E);
    }

    #[test]
    fn viterbi_corrects_errors_through_puncturing() {
        let bits: Vec<u8> = (0..BLOCK_BITS).map(|i| ((i * 7919 + i / 3) % 5 < 2) as u8).collect();
        let coded = conv_encode(&bits);
        let mask = sent();
        let mut soft: Vec<f32> = coded
            .iter()
            .zip(&mask)
            .map(|(&b, &s)| if s { if b == 0 { 1.0 } else { -1.0 } } else { 0.0 })
            .collect();
        // Flip a sprinkling of the sent bits.
        for i in (5..MOTHER).step_by(97) {
            if mask[i] {
                soft[i] = -soft[i];
            }
        }
        assert_eq!(viterbi(&soft), bits);
    }

    #[test]
    fn labels_in_both_charsets() {
        assert_eq!(label_text(0, b"Radio $ \\ ok    "), "Radio \u{142} \u{16e} ok");
        assert_eq!(label_text(15, "Caf\u{e9} FM".as_bytes()), "Café FM");
        assert_eq!(ebu_byte('A'), b'A');
        assert_eq!(ebu_byte('$'), 0xAB);
        assert_eq!(ebu_byte('\u{e9}'), 0x82);
    }

    #[test]
    fn reads_labels_from_figs() {
        let mut e = Ensemble::default();
        let mut fib = vec![0x05u8, 0x00, 0xC1, 0x81, 0x00, 0x00];
        let mut fig1 = vec![0x35u8, 0x01, 0xC2, 0x21];
        fig1.extend_from_slice(b"Sim Radio One   ");
        fig1.extend_from_slice(&[0xFF, 0x00]);
        fig1[0] = 0x20 | (fig1.len() as u8 - 1);
        fib.extend_from_slice(&fig1);
        fib.push(0xFF);
        fib.resize(30, 0);
        e.fib(&fib);
        assert_eq!(e.eid, Some(0xC181));
        assert_eq!(e.services.get(&0xC221).map(String::as_str), Some("Sim Radio One"));
    }
}
