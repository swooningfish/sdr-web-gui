//! RDS (Radio Data System) on broadcast FM: station name (PS), programme
//! type (PTY), programme identifier (PI) and radio text (RT).
//!
//! Decoder, working on the FM multiplex (the demodulator output before
//! de-emphasis):
//! 1. Mix the 57 kHz subcarrier to 0 Hz and low-pass in two stages (the
//!    stereo difference signal reaches 53 kHz, 4 kHz away), ~25 kHz out.
//! 2. Costas loop for the BPSK carrier phase.
//! 3. Biphase matched filter over one bit (1187.5 bit/s); the sampling
//!    phase is the one with the most energy, from a slowly decaying
//!    histogram over the bit period (follows sample-clock drift).
//! 4. Differential decoding, then block sync on the offset words
//!    (A, B, C/C', D) using the (26,16) code's syndrome; no error correction,
//!    blocks that don't check are dropped.
//! 5. Groups 0A/0B (PS, PTY) and 2A/2B (RT).
//!
//! The encoder (`encode`) is used by the tests and the simulated receiver.

use num_complex::Complex32;

use crate::dsp::ddc::lowpass;

pub const BIT_RATE: f64 = 1187.5;
pub const SUBCARRIER_HZ: f64 = 57_000.0;

/// g(x) = x^10 + x^8 + x^7 + x^5 + x^4 + x^3 + 1
const POLY: u32 = 0x5B9;
const OFFSET_A: u16 = 0x0FC;
const OFFSET_B: u16 = 0x198;
const OFFSET_C: u16 = 0x168;
const OFFSET_CP: u16 = 0x350;
const OFFSET_D: u16 = 0x1B4;

/// Remainder of a 26-bit block divided by g(x). A good block leaves its
/// offset word.
fn syndrome(block: u32) -> u16 {
    let mut reg = block & 0x3FF_FFFF;
    for i in (10..26).rev() {
        if reg & (1 << i) != 0 {
            reg ^= POLY << (i - 10);
        }
    }
    (reg & 0x3FF) as u16
}

/// Block position in a group (0 = A … 3 = D) for an offset word.
fn position(s: u16) -> Option<usize> {
    match s {
        OFFSET_A => Some(0),
        OFFSET_B => Some(1),
        OFFSET_C | OFFSET_CP => Some(2),
        OFFSET_D => Some(3),
        _ => None,
    }
}

/// What the station is sending, as far as decoded.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct RdsInfo {
    /// Programme identifier, 4 hex digits.
    pub pi: Option<String>,
    /// Station name (8 characters), once all of it has arrived.
    pub ps: Option<String>,
    pub pty: Option<u8>,
    /// Radio text (up to 64 characters).
    pub rt: Option<String>,
    /// Groups decoded so far.
    pub groups: u32,
}

/// Real FIR with integer decimation on complex samples.
struct CDecim {
    taps: Vec<f32>,
    hist: Vec<Complex32>,
    decim: usize,
    count: usize,
}

impl CDecim {
    fn new(n: usize, cutoff: f64, decim: usize) -> Self {
        let taps = lowpass(n, cutoff);
        CDecim { hist: vec![Complex32::default(); taps.len() - 1], taps, decim, count: 0 }
    }

    fn push(&mut self, x: Complex32, out: &mut Vec<Complex32>) {
        self.hist.push(x);
        self.count += 1;
        if self.count == self.decim {
            self.count = 0;
            let w = &self.hist[self.hist.len() - self.taps.len()..];
            let mut acc = Complex32::default();
            for (s, t) in w.iter().zip(&self.taps) {
                acc += s * *t;
            }
            out.push(acc);
        }
        if self.hist.len() > 4 * self.taps.len() {
            let drop = self.hist.len() - (self.taps.len() - 1);
            self.hist.drain(..drop);
        }
    }
}

pub struct RdsDecoder {
    // Mixer at the input rate.
    nco: Complex32,
    nco_step: Complex32,
    stage1: CDecim,
    stage2: CDecim,
    s1: Vec<Complex32>,
    s2: Vec<Complex32>,
    // Costas loop.
    phase: f32,
    freq: f32,
    alpha: f32,
    beta: f32,
    power: f32,
    // Bit timing.
    spb: f64,
    half: usize,
    ring: Vec<f32>,
    ring_pos: usize,
    sum_a: f32,
    sum_b: f32,
    n: u64,
    hist: Vec<f32>,
    last_cycle: u64,
    prev_sign: bool,
    // Blocks.
    reg: u32,
    bits: u64,
    synced: bool,
    next_pos: usize,
    bits_to_block: u32,
    bad: u32,
    last_hit: Option<(u64, usize)>,
    group: [Option<u16>; 4],
    // Decoded data.
    pi: Option<u16>,
    pty: Option<u8>,
    ps: [u8; 8],
    ps_seen: u8,
    ps_done: Option<String>,
    rt: [u8; 64],
    rt_ab: Option<bool>,
    rt_seen: u16,
    groups: u32,
    changed: bool,
}

impl RdsDecoder {
    /// `rate`: multiplex sample rate (must be above ~120 kHz).
    pub fn new(rate: f64) -> Self {
        let d1 = (rate / 50_000.0).floor().max(1.0) as usize;
        let r1 = rate / d1 as f64;
        let d2 = (r1 / 24_000.0).round().max(1.0) as usize;
        let rate2 = r1 / d2 as f64;
        let w = -std::f64::consts::TAU * SUBCARRIER_HZ / rate;
        // Costas loop, ~30 Hz bandwidth, damping 0.707.
        let bn_t = 30.0 / rate2;
        let zeta = 0.707;
        let th = bn_t / (zeta + 0.25 / zeta);
        let d = 1.0 + 2.0 * zeta * th + th * th;
        let spb = rate2 / BIT_RATE;
        let half = (spb / 2.0).round() as usize;
        RdsDecoder {
            nco: Complex32::new(1.0, 0.0),
            nco_step: Complex32::new(w.cos() as f32, w.sin() as f32),
            // Stage 1: keep ±12 kHz, nothing folds below 4 kHz.
            stage1: CDecim::new(8 * d1 + 1, 12_000.0 / rate, d1),
            // Stage 2: the RDS band (±2.4 kHz), clear of the stereo signal at 4 kHz.
            stage2: CDecim::new((5.5 * r1 / 1_400.0) as usize | 1, 2_700.0 / r1, d2),
            s1: Vec::new(),
            s2: Vec::new(),
            phase: 0.0,
            freq: 0.0,
            alpha: (4.0 * zeta * th / d) as f32,
            beta: (4.0 * th * th / d) as f32,
            power: 1e-6,
            spb,
            half,
            ring: vec![0.0; 2 * half],
            ring_pos: 0,
            sum_a: 0.0,
            sum_b: 0.0,
            n: 0,
            hist: vec![0.0; 16],
            last_cycle: u64::MAX,
            prev_sign: false,
            reg: 0,
            bits: 0,
            synced: false,
            next_pos: 0,
            bits_to_block: 0,
            bad: 0,
            last_hit: None,
            group: [None; 4],
            pi: None,
            pty: None,
            ps: [b' '; 8],
            ps_seen: 0,
            ps_done: None,
            rt: [b' '; 64],
            rt_ab: None,
            rt_seen: 0,
            groups: 0,
            changed: false,
        }
    }

    /// Feed multiplex samples.
    pub fn process(&mut self, mpx: &[f32]) {
        self.s1.clear();
        for &x in mpx {
            let m = self.nco * x;
            self.nco *= self.nco_step;
            self.stage1.push(m, &mut self.s1);
        }
        self.nco /= self.nco.norm();
        self.s2.clear();
        for i in 0..self.s1.len() {
            let v = self.s1[i];
            self.stage2.push(v, &mut self.s2);
        }
        for i in 0..self.s2.len() {
            let z = self.s2[i];
            self.symbol_sample(z);
        }
    }

    /// The decoded info, if anything changed since the last call.
    pub fn take(&mut self) -> Option<RdsInfo> {
        if !self.changed {
            return None;
        }
        self.changed = false;
        Some(self.info())
    }

    pub fn info(&self) -> RdsInfo {
        let rt = (self.rt_seen != 0).then(|| {
            let end = self.rt.iter().position(|&c| c == 0x0D).unwrap_or(64);
            text(&self.rt[..end]).trim_end().to_string()
        });
        RdsInfo {
            pi: self.pi.map(|p| format!("{p:04X}")),
            ps: self.ps_done.clone(),
            pty: self.pty,
            rt: rt.filter(|s| !s.is_empty()),
            groups: self.groups,
        }
    }

    fn symbol_sample(&mut self, z: Complex32) {
        // Costas loop.
        let rot = Complex32::from_polar(1.0, -self.phase);
        let y = z * rot;
        self.power = 0.999 * self.power + 0.001 * y.norm_sqr();
        let err = (y.re * y.im / self.power.max(1e-12)).clamp(-1.0, 1.0);
        self.freq += self.beta * err;
        self.phase += self.freq + self.alpha * err;
        if self.phase > std::f32::consts::PI {
            self.phase -= std::f32::consts::TAU;
        } else if self.phase < -std::f32::consts::PI {
            self.phase += std::f32::consts::TAU;
        }

        // Biphase matched filter: first half-bit minus second half-bit.
        let h = self.half;
        let x = y.re;
        let oldest_a = self.ring[(self.ring_pos + h) % (2 * h)]; // leaves half B into half A
        let oldest = self.ring[self.ring_pos]; // leaves the window
        self.sum_a += oldest_a - oldest;
        self.sum_b += x - oldest_a;
        self.ring[self.ring_pos] = x;
        self.ring_pos = (self.ring_pos + 1) % (2 * h);
        if self.n.is_multiple_of(4096) {
            // Re-add the sums now and then so rounding errors can't build up.
            self.sum_a = (0..h).map(|i| self.ring[(self.ring_pos + i) % (2 * h)]).sum();
            self.sum_b = (h..2 * h).map(|i| self.ring[(self.ring_pos + i) % (2 * h)]).sum();
        }
        let mf = self.sum_a - self.sum_b;

        // Bit timing: energy per phase of the bit period.
        let t = self.n as f64 / self.spb;
        self.n += 1;
        let cycle = t.floor() as u64;
        let bins = self.hist.len();
        let bin = ((t - t.floor()) * bins as f64) as usize % bins;
        self.hist[bin] = 0.997 * self.hist[bin] + 0.003 * mf.abs();
        let best = (0..bins).max_by(|&a, &b| self.hist[a].total_cmp(&self.hist[b])).unwrap();
        if bin == best && cycle != self.last_cycle {
            self.last_cycle = cycle;
            let sign = mf > 0.0;
            let bit = sign != self.prev_sign;
            self.prev_sign = sign;
            self.bit(bit);
        }
    }

    fn bit(&mut self, bit: bool) {
        self.reg = ((self.reg << 1) | bit as u32) & 0x3FF_FFFF;
        self.bits += 1;
        if !self.synced {
            if let Some(pos) = position(syndrome(self.reg)) {
                if let Some((at, p)) = self.last_hit {
                    if self.bits - at == 26 && (p + 1) % 4 == pos {
                        self.synced = true;
                        self.bad = 0;
                        self.group = [None; 4];
                        self.next_pos = (pos + 1) % 4;
                        self.bits_to_block = 26;
                        self.block(pos, (self.reg >> 10) as u16);
                        return;
                    }
                }
                self.last_hit = Some((self.bits, pos));
            }
            return;
        }
        self.bits_to_block -= 1;
        if self.bits_to_block > 0 {
            return;
        }
        self.bits_to_block = 26;
        let pos = self.next_pos;
        self.next_pos = (pos + 1) % 4;
        let ok = position(syndrome(self.reg)) == Some(pos);
        if ok {
            self.bad = self.bad.saturating_sub(1);
            self.block(pos, (self.reg >> 10) as u16);
        } else {
            self.bad += 2;
            if pos == 3 {
                self.finish_group();
            }
            if self.bad > 20 {
                self.synced = false;
                self.last_hit = None;
            }
        }
    }

    fn block(&mut self, pos: usize, info: u16) {
        self.group[pos] = Some(info);
        if pos == 3 {
            self.finish_group();
        }
    }

    fn finish_group(&mut self) {
        let g = std::mem::take(&mut self.group);
        let Some(b) = g[1] else { return };
        self.groups += 1;
        let pi = g[0];
        if pi.is_some() && pi != self.pi {
            if self.pi.is_some() {
                // Another station: start afresh.
                self.ps_seen = 0;
                self.ps_done = None;
                self.rt = [b' '; 64];
                self.rt_seen = 0;
            }
            self.pi = pi;
            self.changed = true;
        }
        let pty = ((b >> 5) & 0x1F) as u8;
        if self.pty != Some(pty) {
            self.pty = Some(pty);
            self.changed = true;
        }
        let kind = b >> 12;
        let version_b = b & 0x0800 != 0;
        match (kind, version_b) {
            (0, _) => {
                if let Some(d) = g[3] {
                    let i = (b & 0x3) as usize * 2;
                    self.ps[i] = (d >> 8) as u8;
                    self.ps[i + 1] = d as u8;
                    self.ps_seen |= 1 << (b & 0x3);
                    if self.ps_seen == 0xF {
                        let ps = text(&self.ps);
                        if self.ps_done.as_deref() != Some(&ps) {
                            self.ps_done = Some(ps);
                            self.changed = true;
                        }
                    }
                }
            }
            (2, _) => {
                let ab = b & 0x10 != 0;
                if self.rt_ab.is_some_and(|x| x != ab) {
                    self.rt = [b' '; 64];
                    self.rt_seen = 0;
                }
                self.rt_ab = Some(ab);
                let seg = (b & 0xF) as usize;
                let before = self.rt;
                if !version_b {
                    if let (Some(c), Some(d)) = (g[2], g[3]) {
                        let i = seg * 4;
                        self.rt[i..i + 4].copy_from_slice(&[(c >> 8) as u8, c as u8, (d >> 8) as u8, d as u8]);
                        self.rt_seen |= 1 << seg;
                    }
                } else if let Some(d) = g[3] {
                    let i = seg * 2;
                    self.rt[i..i + 2].copy_from_slice(&[(d >> 8) as u8, d as u8]);
                    self.rt_seen |= 1 << seg;
                }
                if self.rt != before {
                    self.changed = true;
                }
            }
            _ => {}
        }
    }
}

/// RDS characters → text (the basic set matches ASCII; others become '?').
fn text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&c| match c {
            0x20..=0x7E => c as char,
            0x00 => ' ',
            _ => '?',
        })
        .collect()
}

/// Encoding, for the tests and the simulated receiver.
pub mod encode {
    use super::*;

    fn block(info: u16, offset: u16) -> u32 {
        let check = syndrome((info as u32) << 10) ^ offset;
        ((info as u32) << 10) | check as u32
    }

    /// Groups 0A (station name, 4 groups) and 2A (radio text) for one station,
    /// as data bits (before differential coding).
    pub fn groups(pi: u16, pty: u8, ps: &str, rt: &str) -> Vec<bool> {
        let ps: Vec<u8> = format!("{ps:<8}").bytes().take(8).collect();
        let mut rt: Vec<u8> = rt.bytes().take(64).collect();
        if rt.len() < 64 {
            rt.push(0x0D);
        }
        while !rt.len().is_multiple_of(4) {
            rt.push(b' ');
        }
        let mut words: Vec<[u16; 4]> = Vec::new();
        for seg in 0..4u16 {
            let b = (pty as u16) << 5 | seg; // group 0A, TP 0
            let c = 0xE0CD; // alternative frequencies: none
            let d = (ps[seg as usize * 2] as u16) << 8 | ps[seg as usize * 2 + 1] as u16;
            words.push([pi, b, c, d]);
        }
        for (seg, chunk) in rt.chunks(4).enumerate() {
            let b = 0x2000 | (pty as u16) << 5 | seg as u16; // group 2A, text A
            let c = (chunk[0] as u16) << 8 | chunk[1] as u16;
            let d = (chunk[2] as u16) << 8 | chunk[3] as u16;
            words.push([pi, b, c, d]);
        }
        let mut bits = Vec::new();
        for w in words {
            for (info, off) in w.iter().zip([OFFSET_A, OFFSET_B, OFFSET_C, OFFSET_D]) {
                let blk = block(*info, off);
                for i in (0..26).rev() {
                    bits.push(blk & (1 << i) != 0);
                }
            }
        }
        bits
    }

    /// Differential coding: each transmitted bit is the data bit XOR the
    /// previous transmitted bit.
    pub fn differential(bits: &[bool]) -> Vec<bool> {
        let mut prev = false;
        bits.iter()
            .map(|&b| {
                prev ^= b;
                prev
            })
            .collect()
    }

    /// Biphase baseband (±1) at time `t` seconds into a repeating stream of
    /// differentially coded bits.
    pub fn baseband(coded: &[bool], t: f64, bit_rate: f64) -> f64 {
        let pos = t * bit_rate;
        let bit = coded[(pos.floor() as usize) % coded.len()];
        let first_half = pos - pos.floor() < 0.5;
        if bit == first_half {
            1.0
        } else {
            -1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn checkwords_leave_the_offset() {
        for (info, off) in [(0x1234u16, OFFSET_A), (0xC204, OFFSET_B), (0, OFFSET_D), (0xFFFF, OFFSET_CP)] {
            let check = syndrome((info as u32) << 10) ^ off;
            let blk = ((info as u32) << 10) | check as u32;
            assert_eq!(syndrome(blk), off);
            assert_eq!(syndrome(blk ^ (1 << 13)), syndrome(1 << 13) ^ off, "single bit error changes the syndrome");
            assert_ne!(syndrome(blk ^ (1 << 13)), off);
        }
    }

    /// Multiplex: a 1 kHz mono tone, the 19 kHz pilot, the 38 kHz stereo
    /// difference signal and RDS (sample clock `ppm` off).
    fn mpx(rate: f64, secs: f64, ppm: f64, rds_amp: f64) -> Vec<f32> {
        let coded = encode::differential(&encode::groups(0xC204, 10, "TEST FM", "Now playing: a test tone"));
        let k = 1.0 + ppm * 1e-6;
        (0..(rate * secs) as usize)
            .map(|i| {
                let t = i as f64 / rate * k;
                let audio = 0.4 * (TAU * 1000.0 * t).sin();
                let pilot = 0.09 * (TAU * 19_000.0 * t).sin();
                let stereo = 0.2 * (TAU * 3_000.0 * t).sin() * (TAU * 38_000.0 * t).sin();
                let rds = rds_amp * encode::baseband(&coded, t, BIT_RATE) * (TAU * SUBCARRIER_HZ * t).sin();
                (audio + pilot + stereo + rds) as f32
            })
            .collect()
    }

    fn run(rate: f64, secs: f64, ppm: f64, amp: f64) -> RdsInfo {
        let mut d = RdsDecoder::new(rate);
        for chunk in mpx(rate, secs, ppm, amp).chunks(10_240) {
            d.process(chunk);
        }
        d.info()
    }

    #[test]
    fn decodes_station_name_and_text() {
        let info = run(204_800.0, 4.0, 0.0, 0.04);
        assert_eq!(info.pi.as_deref(), Some("C204"));
        assert_eq!(info.ps.as_deref(), Some("TEST FM "));
        assert_eq!(info.pty, Some(10));
        assert_eq!(info.rt.as_deref(), Some("Now playing: a test tone"));
    }

    #[test]
    fn copes_with_clock_error_and_other_rates() {
        let info = run(250_000.0, 4.0, 60.0, 0.03);
        assert_eq!(info.ps.as_deref(), Some("TEST FM "), "{info:?}");
        let info = run(200_000.0, 4.0, -45.0, 0.03);
        assert_eq!(info.ps.as_deref(), Some("TEST FM "), "{info:?}");
    }

    #[test]
    fn no_rds_no_output() {
        let info = run(204_800.0, 2.0, 0.0, 0.0);
        assert_eq!(info.ps, None);
        assert_eq!(info.groups, 0);
    }
}
