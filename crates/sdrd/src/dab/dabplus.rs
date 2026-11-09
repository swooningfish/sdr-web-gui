//! DAB+ (ETSI TS 102 563): five logical frames make a superframe, protected
//! by Reed-Solomon RS(120,110) in columns and found by its firecode. Its
//! header gives the AAC format and where each access unit (AU) starts; each
//! AU has a CRC. AUs go to the AAC decoder wrapped as LOAS/LATM, which
//! (unlike ADTS) can say the frames are 960 samples long.

use std::collections::VecDeque;
use std::sync::OnceLock;

use super::fic::crc16;

struct Gf {
    exp: [u8; 512],
    log: [u8; 256],
}

/// GF(2⁸) with x⁸ + x⁴ + x³ + x² + 1, α = 2.
fn gf() -> &'static Gf {
    static GF: OnceLock<Gf> = OnceLock::new();
    GF.get_or_init(|| {
        let mut g = Gf { exp: [0; 512], log: [0; 256] };
        let mut x: u16 = 1;
        for i in 0..255 {
            g.exp[i] = x as u8;
            g.log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= 0x11D;
            }
        }
        for i in 255..512 {
            g.exp[i] = g.exp[i - 255];
        }
        g
    })
}

fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let g = gf();
    g.exp[g.log[a as usize] as usize + g.log[b as usize] as usize]
}

fn div(a: u8, b: u8) -> u8 {
    if a == 0 {
        return 0;
    }
    let g = gf();
    g.exp[(g.log[a as usize] as usize + 255 - g.log[b as usize] as usize) % 255]
}

fn alpha(n: usize) -> u8 {
    gf().exp[n % 255]
}

/// Evaluate a polynomial (lowest degree first) at x.
fn eval(p: &[u8], x: u8) -> u8 {
    p.iter().rev().fold(0, |acc, &c| mul(acc, x) ^ c)
}

const RS_N: usize = 120;
const RS_K: usize = 110;
const PARITY: usize = RS_N - RS_K;

/// RS(120,110) parity for 110 data bytes (generator roots α⁰…α⁹; byte 0
/// is the highest power), for the tests and the simulator.
#[cfg(test)]
pub(crate) fn rs_parity(data: &[u8]) -> [u8; PARITY] {
    // g(x) = Π (x − αʲ), highest power first.
    let mut g = vec![1u8];
    for j in 0..PARITY {
        let mut n = vec![0u8; g.len() + 1];
        for (i, &c) in g.iter().enumerate() {
            n[i] ^= c;
            n[i + 1] ^= mul(c, alpha(j));
        }
        g = n;
    }
    let mut rem = [0u8; PARITY];
    for &d in data {
        let f = d ^ rem[0];
        rem.copy_within(1.., 0);
        rem[PARITY - 1] = 0;
        for i in 0..PARITY {
            rem[i] ^= mul(f, g[i + 1]);
        }
    }
    rem
}

/// Correct a 120-byte codeword in place; the number of bytes fixed, or
/// None if there were too many errors.
pub(crate) fn rs_correct(cw: &mut [u8]) -> Option<usize> {
    debug_assert_eq!(cw.len(), RS_N);
    let synd: Vec<u8> = (0..PARITY)
        .map(|j| cw.iter().enumerate().fold(0u8, |acc, (pos, &b)| acc ^ mul(b, alpha(j * (RS_N - 1 - pos)))))
        .collect();
    if synd.iter().all(|&s| s == 0) {
        return Some(0);
    }
    // Berlekamp–Massey: the error locator Λ (lowest degree first).
    let (mut c, mut b) = (vec![1u8], vec![1u8]);
    let (mut l, mut m, mut bd) = (0usize, 1usize, 1u8);
    for n in 0..PARITY {
        let mut d = synd[n];
        for i in 1..=l.min(c.len() - 1) {
            d ^= mul(c[i], synd[n - i]);
        }
        if d == 0 {
            m += 1;
            continue;
        }
        let coef = div(d, bd);
        let mut next = c.clone();
        if next.len() < b.len() + m {
            next.resize(b.len() + m, 0);
        }
        for (i, &bi) in b.iter().enumerate() {
            next[i + m] ^= mul(coef, bi);
        }
        if 2 * l <= n {
            b = c;
            l = n + 1 - l;
            bd = d;
            m = 1;
        } else {
            m += 1;
        }
        c = next;
    }
    c.truncate(l + 1);
    if l > PARITY / 2 {
        return None;
    }
    // Ω = S·Λ mod x^10; Λ' (formal derivative: odd terms).
    let mut omega = vec![0u8; PARITY];
    for (i, &ci) in c.iter().enumerate() {
        for (j, &sj) in synd.iter().enumerate() {
            if i + j < PARITY {
                omega[i + j] ^= mul(ci, sj);
            }
        }
    }
    let deriv: Vec<u8> = c.iter().enumerate().skip(1).map(|(i, &ci)| if i % 2 == 1 { ci } else { 0 }).collect();
    let mut fixed = 0;
    for pos in 0..RS_N {
        let x = alpha(RS_N - 1 - pos);
        let xinv = div(1, x);
        if eval(&c, xinv) != 0 {
            continue;
        }
        let den = eval(&deriv, xinv);
        if den == 0 {
            return None;
        }
        cw[pos] ^= mul(x, div(eval(&omega, xinv), den));
        fixed += 1;
    }
    (fixed == l).then_some(fixed)
}

/// Firecode check word: x¹⁶+x¹⁴+x¹³+x¹²+x¹¹+x⁵+x³+x²+x+1, preset 0.
fn firecode(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x782F } else { crc << 1 };
        }
    }
    crc
}

/// The AAC format from a superframe header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// 48 kHz (else 32 kHz) output.
    pub dac_48k: bool,
    /// HE-AAC (SBR).
    pub sbr: bool,
    /// Stereo core.
    pub stereo: bool,
    /// Parametric stereo (HE-AAC v2).
    pub ps: bool,
}

impl Format {
    fn from_byte(b: u8) -> Format {
        Format { dac_48k: b & 0x40 != 0, sbr: b & 0x20 != 0, stereo: b & 0x10 != 0, ps: b & 0x08 != 0 }
    }

    #[cfg(test)]
    fn byte(&self) -> u8 {
        (self.dac_48k as u8) << 6 | (self.sbr as u8) << 5 | (self.stereo as u8) << 4 | (self.ps as u8) << 3
    }

    /// "HE-AAC v2" etc.
    pub fn codec(&self) -> &'static str {
        match (self.sbr, self.ps) {
            (true, true) => "HE-AAC v2",
            (true, false) => "HE-AAC",
            _ => "AAC-LC",
        }
    }

    fn num_aus(&self) -> usize {
        match (self.dac_48k, self.sbr) {
            (true, true) => 3,
            (true, false) => 6,
            (false, true) => 2,
            (false, false) => 4,
        }
    }

    fn first_au(&self) -> usize {
        match (self.dac_48k, self.sbr) {
            (true, true) => 6,
            (true, false) => 11,
            (false, true) => 5,
            (false, false) => 8,
        }
    }

    /// AAC sampling frequency index of the core (half the output with SBR).
    fn core_sr_index(&self) -> u32 {
        match (self.dac_48k, self.sbr) {
            (true, true) => 6,   // 24 kHz
            (true, false) => 3,  // 48 kHz
            (false, true) => 8,  // 16 kHz
            (false, false) => 5, // 32 kHz
        }
    }
}

/// A superframe's AUs.
#[derive(Debug)]
pub struct Superframe {
    pub format: Format,
    /// AUs whose CRC checked (CRC removed).
    pub aus: Vec<Vec<u8>>,
    pub bad_aus: usize,
    /// Bytes Reed-Solomon corrected.
    pub corrected: usize,
}

/// Finds superframes in a sub-channel's logical frames.
pub struct Superframes {
    frame_len: usize,
    frames: VecDeque<Vec<u8>>,
}

impl Superframes {
    /// `frame_len`: bytes per logical frame (3 × bit rate in kbit/s).
    pub fn new(frame_len: usize) -> Superframes {
        Superframes { frame_len, frames: VecDeque::new() }
    }

    pub fn push(&mut self, frame: Vec<u8>) -> Option<Superframe> {
        if frame.len() != self.frame_len || self.frame_len % 24 != 0 || self.frame_len == 0 {
            return None;
        }
        self.frames.push_back(frame);
        if self.frames.len() < 5 {
            return None;
        }
        let mut sf: Vec<u8> = self.frames.iter().flatten().copied().collect();
        let cols = sf.len() / RS_N;
        let mut corrected = 0;
        let mut cw = [0u8; RS_N];
        for col in 0..cols {
            for (pos, b) in cw.iter_mut().enumerate() {
                *b = sf[pos * cols + col];
            }
            if let Some(n) = rs_correct(&mut cw) {
                corrected += n;
                for (pos, &b) in cw.iter().enumerate() {
                    sf[pos * cols + col] = b;
                }
            }
        }
        if firecode(&sf[2..11]) != u16::from_be_bytes([sf[0], sf[1]]) || (sf[3] == 0 && sf[4] == 0) {
            self.frames.pop_front();
            return None;
        }
        self.frames.clear();
        let format = Format::from_byte(sf[2]);
        let n = format.num_aus();
        let mut starts = vec![format.first_au()];
        let packed = [
            (sf[3] as usize) << 4 | (sf[4] as usize) >> 4,
            (sf[4] as usize & 0x0F) << 8 | sf[5] as usize,
            (sf[6] as usize) << 4 | (sf[7] as usize) >> 4,
            (sf[7] as usize & 0x0F) << 8 | sf[8] as usize,
            (sf[9] as usize) << 4 | (sf[10] as usize) >> 4,
        ];
        starts.extend_from_slice(&packed[..n - 1]);
        starts.push(cols * RS_K);
        let mut out = Superframe { format, aus: Vec::new(), bad_aus: 0, corrected };
        for w in starts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b <= a + 2 || b > cols * RS_K {
                out.bad_aus += 1;
                continue;
            }
            let au = &sf[a..b];
            let (body, crc) = au.split_at(au.len() - 2);
            if crc16(body) == u16::from_be_bytes([crc[0], crc[1]]) {
                out.aus.push(body.to_vec());
            } else {
                out.bad_aus += 1;
            }
        }
        Some(out)
    }
}

pub(crate) struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    pub(crate) fn new() -> Bits {
        Bits { out: Vec::new(), acc: 0, n: 0 }
    }
    pub(crate) fn put(&mut self, v: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                self.out.push(self.acc as u8);
                self.acc = 0;
                self.n = 0;
            }
        }
    }
    pub(crate) fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.acc << (8 - self.n)) as u8);
        }
        self.out
    }
}

/// One AU as a LOAS frame (AudioSyncStream of one AudioMuxElement with its
/// StreamMuxConfig). The AudioSpecificConfig gives the core: AAC-LC,
/// 960-sample frames; SBR and PS are found in the data (implicit
/// signalling), as decoders handle DAB+.
pub fn loas(format: &Format, au: &[u8]) -> Vec<u8> {
    let mut b = Bits::new();
    b.put(0, 1); // useSameStreamMux
    b.put(0, 1); // audioMuxVersion
    b.put(1, 1); // allStreamsSameTimeFraming
    b.put(0, 6); // numSubFrames
    b.put(0, 4); // numProgram
    b.put(0, 3); // numLayer
    b.put(2, 5); // AudioSpecificConfig: AAC LC
    b.put(format.core_sr_index(), 4);
    b.put(if format.stereo { 2 } else { 1 }, 4);
    b.put(0b100, 3); // GASpecificConfig: 960 samples, no core coder, no extension
    b.put(0, 3); // frameLengthType
    b.put(0xFF, 8); // latmBufferFullness
    b.put(0, 1); // otherDataPresent
    b.put(0, 1); // crcCheckPresent
    let mut len = au.len();
    while len >= 255 {
        b.put(255, 8);
        len -= 255;
    }
    b.put(len as u32, 8);
    for &byte in au {
        b.put(byte as u32, 8);
    }
    let element = b.finish();
    let mut out = Vec::with_capacity(element.len() + 3);
    let head = (0x2B7u32 << 13) | element.len() as u32;
    out.extend_from_slice(&head.to_be_bytes()[1..]);
    out.extend_from_slice(&element);
    out
}

/// Build a superframe's five logical frames from AUs (for the tests).
#[cfg(test)]
pub(crate) fn build(format: Format, frame_len: usize, aus: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let cols = frame_len * 5 / RS_N;
    let mut info = vec![0u8; cols * RS_K];
    let n = format.num_aus();
    assert_eq!(aus.len(), n);
    let mut starts = vec![format.first_au()];
    for au in aus {
        let s = *starts.last().unwrap();
        info[s..s + au.len()].copy_from_slice(au);
        let crc = crc16(au);
        info[s + au.len()..s + au.len() + 2].copy_from_slice(&crc.to_be_bytes());
        starts.push(s + au.len() + 2);
    }
    assert_eq!(*starts.last().unwrap(), cols * RS_K, "the last AU fills the superframe");
    info[2] = format.byte();
    // 12-bit AU starts packed from byte 3; only as many as there are AUs.
    for (k, &s) in starts[1..n].iter().enumerate() {
        let bit = 24 + 12 * k;
        for j in 0..12 {
            if (s >> (11 - j)) & 1 == 1 {
                info[(bit + j) / 8] |= 0x80 >> ((bit + j) % 8);
            }
        }
    }
    let fc = firecode(&info[2..11]);
    info[..2].copy_from_slice(&fc.to_be_bytes());
    let mut sf = vec![0u8; frame_len * 5];
    sf[..info.len()].copy_from_slice(&info);
    for col in 0..cols {
        let data: Vec<u8> = (0..RS_K).map(|pos| sf[pos * cols + col]).collect();
        for (k, p) in rs_parity(&data).iter().enumerate() {
            sf[(RS_K + k) * cols + col] = *p;
        }
    }
    sf.chunks(frame_len).map(<[u8]>::to_vec).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reed_solomon_fixes_up_to_five_bytes() {
        let data: Vec<u8> = (0..RS_K as u32).map(|i| (i * 73 + 11) as u8).collect();
        let mut cw = data.clone();
        cw.extend_from_slice(&rs_parity(&data));
        let mut clean = cw.clone();
        assert_eq!(rs_correct(&mut clean), Some(0));
        for errs in 1..=5 {
            let mut bad = cw.clone();
            for e in 0..errs {
                bad[(e * 37 + 3) % RS_N] ^= (e as u8 + 1) * 29;
            }
            assert_eq!(rs_correct(&mut bad), Some(errs), "{errs} errors");
            assert_eq!(bad, cw);
        }
        let mut bad = cw.clone();
        for e in 0..8 {
            bad[e * 13] ^= 0x5A;
        }
        let r = rs_correct(&mut bad);
        assert!(r.is_none() || bad != cw, "eight errors can't be fixed");
    }

    #[test]
    fn superframe_round_trip() {
        let format = Format { dac_48k: false, sbr: true, stereo: false, ps: true };
        let frame_len = 3 * 64; // 64 kbit/s
        // The last AU runs to the end: 8 columns × 110 − 5 − (230 + 2) − 2.
        let aus = vec![vec![0x21; 230], (0..641).map(|i| i as u8).collect()];
        let frames = build(format, frame_len, &aus);
        let mut sf = Superframes::new(frame_len);
        // Start mid-way: a stray frame first, and a byte error in each frame.
        assert!(sf.push(vec![7; frame_len]).is_none());
        let mut got = None;
        for (i, mut f) in frames.into_iter().enumerate() {
            f[i * 11 + 20] ^= 0xFF;
            got = sf.push(f);
        }
        let got = got.expect("superframe");
        assert_eq!(got.format, format);
        assert_eq!(got.format.codec(), "HE-AAC v2");
        assert_eq!(got.bad_aus, 0);
        assert_eq!(got.corrected, 5);
        assert_eq!(got.aus, aus);
    }

    #[test]
    fn loas_header() {
        let f = Format { dac_48k: true, sbr: true, stereo: true, ps: false };
        let l = loas(&f, &[1, 2, 3]);
        assert_eq!((l[0] as u32) << 3 | (l[1] as u32) >> 5, 0x2B7);
        let len = ((l[1] as usize & 0x1F) << 8) | l[2] as usize;
        assert_eq!(len, l.len() - 3);
        // 16 bits of mux config, then AOT 2 (00010), index 6 (0110), 2 channels (0010), 100.
        let asc = u32::from_be_bytes([l[5], l[6], l[7], 0]) >> 16;
        assert_eq!(asc, 0b0001_0011_0001_0100);
    }
}
