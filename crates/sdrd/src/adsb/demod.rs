//! Mode S demodulator for cu8 IQ at any rate from 2 MS/s up.
//!
//! Replies are pulse-position modulated at 1 Mbit/s: each bit is a 0.5 µs
//! pulse in the first (1) or second (0) half of its microsecond, after a
//! preamble of four pulses at 0, 1, 3.5 and 4.5 µs. Rather than assume a
//! whole number of samples per half-bit (2 MS/s only), the magnitude is
//! integrated over each half-bit with fractional sample edges, and a few
//! sub-sample timing offsets are tried for every preamble; the CRC says
//! which one decoded correctly.

use std::collections::HashSet;

use super::crc;
use super::message::{df, len_for, Frame};

/// Sub-sample timing offsets tried per preamble (in samples).
const PHASES: [f64; 5] = [0.0, -0.25, 0.25, -0.5, 0.5];

pub struct Demod {
    rate: u32,
    /// Samples per microsecond.
    sps: f64,
    /// Magnitudes carried over from the previous block.
    tail: Vec<f32>,
    /// Starts before this index of the next buffer were already searched.
    skip: usize,
    noise: f32,
    lut: [f32; 256],
    pub stats: Stats,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    pub preambles: u64,
    pub good: u64,
    pub corrected: u64,
}

/// Running integral of the magnitude, for averages over fractional spans.
struct Integral<'a> {
    mag: &'a [f32],
    cum: &'a [f64],
}

impl Integral<'_> {
    #[inline]
    fn at(&self, x: f64) -> f64 {
        let k = (x.floor() as usize).min(self.mag.len() - 1);
        self.cum[k] + (x - k as f64) * self.mag[k] as f64
    }

    /// Mean magnitude over [a, b) (sample positions).
    #[inline]
    fn mean(&self, a: f64, b: f64) -> f32 {
        ((self.at(b) - self.at(a)) / (b - a)) as f32
    }
}

impl Demod {
    pub fn new(rate: u32) -> Self {
        let mut lut = [0f32; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            let x = i as f32 - 127.4;
            *v = x * x;
        }
        Demod {
            rate,
            sps: rate as f64 / 1e6,
            tail: Vec::new(),
            skip: 0,
            noise: 2.0,
            lut,
            stats: Stats::default(),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Samples needed after a start position to hold a long message.
    fn span(&self) -> usize {
        ((8.0 + 112.0 + 1.0) * self.sps).ceil() as usize + 2
    }

    /// Demodulate a block of interleaved cu8 IQ. `known` holds addresses seen
    /// recently, which short replies (address in the parity) must match.
    pub fn feed(&mut self, iq: &[u8], known: &HashSet<u32>, out: &mut Vec<Frame>) {
        let mut mag = std::mem::take(&mut self.tail);
        mag.reserve(iq.len() / 2);
        for p in iq.chunks_exact(2) {
            mag.push((self.lut[p[0] as usize] + self.lut[p[1] as usize]).sqrt());
        }
        let span = self.span();
        if mag.len() <= span + 1 {
            self.tail = mag;
            return;
        }

        // Noise floor: the block's median-ish level (mean of the lower half).
        let n = mag.len();
        let step = (n / 4096).max(1);
        let mut sample: Vec<f32> = mag.iter().step_by(step).copied().collect();
        let mid = sample.len() / 2;
        sample.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
        self.noise = 0.7 * self.noise + 0.3 * sample[mid].max(0.5);

        let mut cum = Vec::with_capacity(n + 1);
        let mut acc = 0f64;
        cum.push(0.0);
        for &m in &mag {
            acc += m as f64;
            cum.push(acc);
        }
        let it = Integral { mag: &mag, cum: &cum };
        let sps = self.sps;
        let half = 0.5 * sps;
        let end = n - span;
        let threshold = self.noise * 2.2;

        let mut s = self.skip;
        while s < end {
            // Cheap test first: a pulse here and one 1 µs later.
            if mag[s].max(mag[s + 1]) < threshold || it.mean(s as f64 + sps, s as f64 + sps + half) < threshold {
                s += 1;
                continue;
            }
            match self.try_at(&it, s as f64, known) {
                Some((frame, bits)) => {
                    out.push(frame);
                    // Skip past this message.
                    s += ((8.0 + bits as f64) * sps) as usize;
                }
                None => s += 1,
            }
        }
        self.skip = s - end;
        self.tail = mag[end..].to_vec();
    }

    /// Check for a preamble at `start` and decode what follows.
    fn try_at(&mut self, it: &Integral, start: f64, known: &HashSet<u32>) -> Option<(Frame, usize)> {
        let sps = self.sps;
        let chip = |p: f64, us: f64| it.mean(p + us * sps, p + (us + 0.5) * sps);
        let pre = |p: f64| {
            let hi = [chip(p, 0.0), chip(p, 1.0), chip(p, 3.5), chip(p, 4.5)];
            let hmin = hi.iter().copied().fold(f32::MAX, f32::min);
            let hmean = hi.iter().sum::<f32>() / 4.0;
            // The half-microsecond gaps between pulses smear into the
            // neighbouring samples at ~2 samples/µs, so they only need to be
            // lower; the long quiet stretches must be clearly quiet.
            let gaps = (chip(p, 0.5) + chip(p, 4.0)) / 2.0;
            let quiet = (it.mean(p + 1.5 * sps, p + 3.5 * sps) * 2.0 + it.mean(p + 5.0 * sps, p + 8.0 * sps) * 3.0) / 5.0;
            let ok = hmean > 1.05 * gaps && hmin > 1.6 * quiet && hmean > 2.2 * quiet && hmean > 2.5 * self.noise;
            (ok, hmean, hmean / quiet.max(0.01))
        };
        let (ok0, _, _) = pre(start);
        if !ok0 {
            // Try the neighbouring phases before giving up on this sample.
            if !PHASES[1..].iter().any(|d| pre(start + d).0) {
                return None;
            }
        }
        self.stats.preambles += 1;

        let mut best_ap: Option<(Frame, usize, f32)> = None;
        for &d in &PHASES {
            let p = start + d;
            let (ok, level, _) = pre(p);
            if !ok {
                continue;
            }
            let bit_at = |k: usize| chip(p, 8.0 + k as f64) > chip(p, 8.5 + k as f64);
            let mut bytes = [0u8; 14];
            for k in 0..5 {
                if bit_at(k) {
                    bytes[0] |= 0x80 >> k;
                }
            }
            let dfv = df(&bytes);
            if !matches!(dfv, 0 | 4 | 5 | 11 | 16 | 17 | 18 | 20 | 21) {
                continue;
            }
            let len = len_for(dfv);
            let mut weak = 0f32;
            for k in 5..len * 8 {
                let (a, b) = (chip(p, 8.0 + k as f64), chip(p, 8.5 + k as f64));
                if a > b {
                    bytes[k / 8] |= 0x80 >> (k % 8);
                }
                weak += (a - b).abs() / (a + b).max(1e-3);
            }
            let rssi = 20.0 * (level / 127.5).max(1e-6).log10();
            let msg = &bytes[..len];
            let r = crc::residual(msg);
            match dfv {
                17 | 18 => {
                    let mut corrected = false;
                    if r != 0 {
                        if !crc::fix_one_bit(&mut bytes) || df(&bytes) != dfv {
                            continue;
                        }
                        corrected = true;
                    }
                    self.stats.good += 1;
                    self.stats.corrected += corrected as u64;
                    return Some((Frame { bytes: bytes[..14].to_vec(), rssi, corrected }, 112));
                }
                11 => {
                    // Interrogator id in the low 7 bits; a stray all-call
                    // only counts from aircraft we already know.
                    let icao = (bytes[1] as u32) << 16 | (bytes[2] as u32) << 8 | bytes[3] as u32;
                    if r == 0 || (r < 0x80 && known.contains(&icao)) {
                        self.stats.good += 1;
                        return Some((Frame { bytes: bytes[..7].to_vec(), rssi, corrected: false }, 56));
                    }
                }
                _ => {
                    // Address/parity: only for known aircraft, best phase wins.
                    let conf = weak / (len * 8) as f32;
                    if known.contains(&r) && conf > 0.3 && best_ap.as_ref().is_none_or(|b| conf > b.2) {
                        best_ap = Some((Frame { bytes: msg.to_vec(), rssi, corrected: false }, len * 8, conf));
                    }
                }
            }
        }
        best_ap.map(|(f, bits, _)| {
            self.stats.good += 1;
            (f, bits)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::encode;
    use super::*;

    /// IQ (cu8) with the messages at the given start times (µs), a carrier
    /// `offset_hz` off centre, at amplitude `amp`, plus noise.
    fn synth(rate: u32, msgs: &[(f64, Vec<u8>)], amp: f64, offset_hz: f64, noise: f64) -> Vec<u8> {
        let total_us = msgs.iter().map(|m| m.0).fold(0.0, f64::max) + 300.0;
        let n = (total_us * rate as f64 / 1e6) as usize;
        let mut on = vec![0f64; n];
        for (t0, m) in msgs {
            for (a, b) in encode::pulses(m) {
                // Fractional coverage of each sample by the pulse.
                let (sa, sb) = ((t0 + a) * rate as f64 / 1e6, (t0 + b) * rate as f64 / 1e6);
                let mut k = sa.floor() as usize;
                while (k as f64) < sb && k < n {
                    let cover = (sb.min(k as f64 + 1.0) - sa.max(k as f64)).max(0.0);
                    on[k] += cover;
                    k += 1;
                }
            }
        }
        let mut rng = 12345u64;
        let mut uni = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let mut out = Vec::with_capacity(n * 2);
        for (k, &g) in on.iter().enumerate() {
            let ph = std::f64::consts::TAU * offset_hz * k as f64 / rate as f64 + 0.7;
            let i = amp * g.min(1.0) * ph.cos() + noise * (uni() + uni() + uni());
            let q = amp * g.min(1.0) * ph.sin() + noise * (uni() + uni() + uni());
            out.push((i * 127.5 + 127.5).round().clamp(0.0, 255.0) as u8);
            out.push((q * 127.5 + 127.5).round().clamp(0.0, 255.0) as u8);
        }
        out
    }

    fn run(rate: u32, iq: &[u8], block: usize) -> Vec<Frame> {
        let mut d = Demod::new(rate);
        let mut out = Vec::new();
        let known: HashSet<u32> = [0x40ABCD].into_iter().collect();
        for c in iq.chunks(block) {
            d.feed(c, &known, &mut out);
        }
        // Flush with silence so the tail gets searched.
        d.feed(&vec![127u8; 4000], &known, &mut out);
        out
    }

    #[test]
    fn decodes_at_common_rates() {
        let msgs: Vec<(f64, Vec<u8>)> = vec![
            (50.0, encode::ident(0x40ABCD, "TEST1", "A3").to_vec()),
            (400.0, encode::position(0x40ABCD, 51.5, -0.1, 12000, false).to_vec()),
            (731.3, encode::velocity(0x40ABCD, 250.0, 45.0, 640).to_vec()),
            (1100.7, encode::all_call(0x40ABCD).to_vec()),
        ];
        for rate in [2_000_000, 2_048_000, 2_400_000] {
            for offset in [0.0, 150e3] {
                let iq = synth(rate, &msgs, 0.5, offset, 0.03);
                // Odd block sizes, so messages straddle block edges.
                let frames = run(rate, &iq, 2 * 777);
                let got: Vec<Vec<u8>> = frames.iter().map(|f| f.bytes.clone()).collect();
                for (_, m) in &msgs {
                    assert!(got.contains(m), "rate {rate} offset {offset}: missing {m:02x?}, got {got:02x?}");
                }
                assert_eq!(frames.len(), msgs.len(), "rate {rate}: duplicates or junk {got:02x?}");
            }
        }
    }

    #[test]
    fn noise_alone_decodes_nothing() {
        let iq = synth(2_400_000, &[], 0.0, 0.0, 0.08);
        let mut big = Vec::new();
        for _ in 0..200 {
            big.extend_from_slice(&iq);
        }
        assert!(run(2_400_000, &big, 65536).is_empty());
    }
}
