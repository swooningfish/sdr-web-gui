//! Mode I OFDM receiver for the FIC: frame sync on the null symbol,
//! frequency and timing from the phase reference, differential QPSK.

use std::f64::consts::TAU;
use std::sync::Arc;

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use super::fic::{prbs, sent, Ensemble, BLOCK_BITS};
use super::{bin, interleaver, prs, FIC_BLOCK, FIC_SYMBOLS, K, L, RATE, T_F, T_G, T_NULL, T_S, T_U};

/// Furthest whole-carrier (1 kHz) frequency error searched for.
const MAX_CARRIERS: i32 = 40;
/// How far into the guard interval the FFT window starts, so echoes and
/// small timing errors stay inside the cyclic prefix.
const BACKOFF: usize = 48;
/// Null symbol power as a share of the average power: below this it counts
/// as a null.
const NULL_RATIO: f64 = 0.6;

fn to_complex(cu8: &[u8]) -> Vec<Complex32> {
    cu8.chunks_exact(2)
        .map(|p| Complex32::new((p[0] as f32 - 127.5) / 127.5, (p[1] as f32 - 127.5) / 127.5))
        .collect()
}

/// Running sum of power, for the power over any window in O(1).
fn power_sums(z: &[Complex32]) -> Vec<f64> {
    let mut cum = Vec::with_capacity(z.len() + 1);
    let mut acc = 0f64;
    cum.push(0.0);
    for s in z {
        acc += s.norm_sqr() as f64;
        cum.push(acc);
    }
    cum
}

/// Start of the quietest null-symbol-long window in `lo..=hi`.
fn quietest(cum: &[f64], lo: usize, hi: usize) -> usize {
    (lo..=hi)
        .min_by(|&a, &b| {
            let pa = cum[a + T_NULL] - cum[a];
            let pb = cum[b + T_NULL] - cum[b];
            pa.total_cmp(&pb)
        })
        .unwrap_or(lo)
}

/// How deep the deepest null-symbol-long dip in a capture is: its power
/// as a share of the average, and that as a signal-to-noise estimate (dB).
/// A DAB multiplex dips to the noise every 96 ms; noise alone doesn't.
pub fn null_depth(cu8: &[u8]) -> Option<(f64, f64)> {
    let z = to_complex(cu8);
    if z.len() < T_F + T_NULL {
        return None;
    }
    let cum = power_sums(&z);
    let mean = cum[z.len()] / z.len() as f64;
    let at = quietest(&cum, 0, T_F);
    let null = (cum[at + T_NULL] - cum[at]) / T_NULL as f64;
    let ratio = null / mean.max(1e-12);
    // A second dip one frame on, if the capture is long enough.
    let ratio = if at + T_F + T_NULL + 64 <= z.len() {
        let next = quietest(&cum, at + T_F - 64, (at + T_F + 64).min(z.len() - T_NULL));
        ratio.max((cum[next + T_NULL] - cum[next]) / T_NULL as f64 / mean.max(1e-12))
    } else {
        ratio
    };
    let snr = 10.0 * (1.0 / ratio.max(1e-6) - 1.0).max(1e-6).log10();
    Some((ratio, snr))
}

struct Rx {
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    prs: Vec<Complex32>,
    /// Phase steps between neighbouring reference carriers, by carrier.
    prs_diff: Vec<(i32, Complex32)>,
    map: Vec<i32>,
    sent: Vec<bool>,
    prbs: Vec<u8>,
    buf: Vec<Complex32>,
}

impl Rx {
    fn new() -> Rx {
        let mut planner = FftPlanner::new();
        let prs = prs();
        let prs_diff = (-768..768)
            .filter(|&k| k != -1 && k != 0)
            .map(|k| (k, prs[bin(k)] * prs[bin(k + 1)].conj()))
            .collect();
        Rx {
            fft: planner.plan_fft_forward(T_U),
            ifft: planner.plan_fft_inverse(T_U),
            prs,
            prs_diff,
            map: interleaver(),
            sent: sent(),
            prbs: prbs(BLOCK_BITS),
            buf: vec![Complex32::default(); T_U],
        }
    }

    /// FFT of `T_U` samples from `start`, with `hz` mixed out.
    fn spectrum(&mut self, z: &[Complex32], start: usize, hz: f64) -> Vec<Complex32> {
        let w = -TAU * hz / RATE as f64;
        let step = num_complex::Complex64::from_polar(1.0, w);
        let mut ph = num_complex::Complex64::from_polar(1.0, (w * start as f64) % TAU);
        for (n, out) in self.buf.iter_mut().enumerate() {
            *out = z[start + n] * Complex32::new(ph.re as f32, ph.im as f32);
            ph *= step;
        }
        let mut out = self.buf.clone();
        self.fft.process(&mut out);
        out
    }

    /// Fractional frequency error (within ±500 Hz) from the cyclic prefixes
    /// of the frame's symbols.
    fn fine_freq(z: &[Complex32], prs_start: usize) -> f64 {
        let mut acc = Complex32::default();
        for l in 0..L {
            let s = prs_start + l * T_S;
            if s + T_S > z.len() {
                break;
            }
            for n in 64..T_G - 64 {
                acc += z[s + n + T_U] * z[s + n].conj();
            }
        }
        acc.arg() as f64 / TAU * RATE as f64 / T_U as f64
    }

    /// Whole-carrier offset of the reference symbol's spectrum: the shift at
    /// which neighbouring carriers' phase steps best match the reference's.
    fn carrier_offset(&self, f: &[Complex32]) -> i32 {
        (-MAX_CARRIERS..=MAX_CARRIERS)
            .max_by(|&a, &b| self.match_at(f, a).total_cmp(&self.match_at(f, b)))
            .unwrap_or(0)
    }

    fn match_at(&self, f: &[Complex32], s: i32) -> f32 {
        let mut acc = Complex32::default();
        for &(k, r) in &self.prs_diff {
            acc += f[bin(k + s)] * f[bin(k + 1 + s)].conj() * r.conj();
        }
        acc.norm()
    }

    /// Timing error (samples) of an FFT window over the reference symbol:
    /// the peak of the channel impulse response. Positive = the window
    /// started early.
    fn timing(&mut self, f: &[Complex32], shift: i32) -> i32 {
        let mut h = vec![Complex32::default(); T_U];
        for k in -768..=768i32 {
            if k != 0 {
                h[bin(k)] = f[bin(k + shift)] * self.prs[bin(k)].conj();
            }
        }
        self.ifft.process(&mut h);
        let (peak, _) = h
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm_sqr().total_cmp(&b.1.norm_sqr()))
            .unwrap_or((0, &Complex32::default()));
        let p = peak as i32;
        if p > (T_U / 2) as i32 {
            p - T_U as i32
        } else {
            p
        }
    }

    /// Soft bits of symbols 1..=`symbols` (3072 each: real parts, then
    /// imaginary) of the frame whose reference symbol starts at `prs_start`,
    /// and the frequency error found (Hz). Symbols 1–3 are the FIC, 4–75
    /// the four CIFs of the MSC.
    fn frame(&mut self, z: &[Complex32], prs_start: usize, symbols: usize) -> Option<(Vec<f32>, f64)> {
        let frac = Self::fine_freq(z, prs_start);
        let w0 = prs_start + T_G - BACKOFF;
        if w0 + T_U > z.len() {
            return None;
        }
        let f0 = self.spectrum(z, w0, frac);
        let shift = self.carrier_offset(&f0);
        let hz = frac + shift as f64 * (RATE as f64 / T_U as f64);
        let f0 = self.spectrum(z, w0, hz);
        // Line the window up with the strongest path, then back off again.
        let err = self.timing(&f0, 0);
        let start = (w0 as i64 + err as i64 - BACKOFF as i64).max(0) as usize;
        if start + symbols * T_S + T_U > z.len() {
            return None;
        }
        let mut prev = self.spectrum(z, start, hz);
        let mut soft = Vec::with_capacity(symbols * 2 * K);
        for l in 1..=symbols {
            let cur = self.spectrum(z, start + l * T_S, hz);
            let r: Vec<Complex32> = self.map.iter().map(|&k| cur[bin(k)] * prev[bin(k)].conj()).collect();
            let scale = r.iter().map(|v| v.norm()).sum::<f32>() / K as f32;
            let scale = if scale > 0.0 { 1.0 / scale } else { 0.0 };
            soft.extend(r.iter().map(|v| v.re * scale));
            soft.extend(r.iter().map(|v| v.im * scale));
            prev = cur;
        }
        Some((soft, hz))
    }
}

/// What decoding a capture found.
#[derive(Debug, Default)]
pub struct Decoded {
    pub ensemble: Ensemble,
    /// Frames demodulated.
    pub frames: usize,
    /// Frequency error of the multiplex against the tuning (Hz).
    pub freq_error_hz: Option<f64>,
}

/// Read the station list from a capture (cu8 at 2.048 MS/s, tuned to the
/// multiplex's centre).
pub fn decode(cu8: &[u8]) -> Decoded {
    let z = to_complex(cu8);
    let mut out = Decoded::default();
    if z.len() < T_F + T_NULL {
        return out;
    }
    let cum = power_sums(&z);
    let mean = cum[z.len()] / z.len() as f64;
    let is_null = |at: usize| (cum[at + T_NULL] - cum[at]) / (T_NULL as f64) < NULL_RATIO * mean;
    let mut rx = Rx::new();
    let mut at = quietest(&cum, 0, T_F);
    loop {
        if at + T_NULL + (FIC_SYMBOLS + 2) * T_S > z.len() {
            break;
        }
        if is_null(at) {
            if let Some((soft, hz)) = rx.frame(&z, at + T_NULL, FIC_SYMBOLS) {
                out.frames += 1;
                out.freq_error_hz = Some(hz);
                for block in soft.chunks_exact(FIC_BLOCK) {
                    out.ensemble.add_block(block, &rx.sent, &rx.prbs);
                }
            }
        }
        // The next null is a frame on; the sample clock may have drifted a little.
        let lo = at + T_F - 128;
        let hi = (at + T_F + 128).min(z.len().saturating_sub(T_NULL));
        if lo > hi {
            break;
        }
        at = quietest(&cum, lo, hi);
    }
    out
}

/// Where the FIC ends and the CIFs start in a frame's soft bits.
pub const FIC_SOFT: usize = FIC_SYMBOLS * 2 * K;

/// A receiver that follows a multiplex as samples arrive: finds each frame,
/// decodes the FIC into `ensemble` and hands over every frame's soft bits
/// (for the MSC).
pub struct Stream {
    rx: Rx,
    buf: Vec<Complex32>,
    /// Start of the next null symbol in `buf`, once locked on.
    next: Option<usize>,
    misses: u32,
    pub ensemble: Ensemble,
    /// Frequency error of the multiplex against the tuning (Hz), last frame.
    pub freq_error_hz: Option<f64>,
    /// Frames demodulated since the start.
    pub frames: u64,
}

impl Default for Stream {
    fn default() -> Self {
        Self::new()
    }
}

impl Stream {
    pub fn new() -> Stream {
        Stream {
            rx: Rx::new(),
            buf: Vec::new(),
            next: None,
            misses: 0,
            ensemble: Ensemble::default(),
            freq_error_hz: None,
            frames: 0,
        }
    }

    /// Whether frames are being found.
    pub fn locked(&self) -> bool {
        self.next.is_some() && self.misses == 0 && self.frames > 0
    }

    /// Feed cu8 samples (2.048 MS/s, tuned to the multiplex); `frame` gets
    /// each frame's soft bits: the FIC ([`FIC_SOFT`]), then four CIFs.
    pub fn push(&mut self, cu8: &[u8], mut frame: impl FnMut(&[f32])) {
        self.buf.extend(to_complex(cu8));
        loop {
            let Some(at) = self.next else {
                // Look for a null in the first frame's worth.
                if self.buf.len() < 2 * T_F + T_NULL {
                    return;
                }
                let cum = power_sums(&self.buf[..2 * T_F]);
                let mean = cum[2 * T_F] / (2 * T_F) as f64;
                let at = quietest(&cum, 0, T_F);
                if (cum[at + T_NULL] - cum[at]) / (T_NULL as f64) < NULL_RATIO * mean {
                    self.next = Some(at);
                    self.misses = 0;
                } else {
                    self.buf.drain(..T_F);
                }
                continue;
            };
            if self.buf.len() < at + T_F + T_NULL + 256 {
                return;
            }
            let cum = power_sums(&self.buf[at..at + T_F + T_NULL + 256]);
            let mean = cum[T_F] / T_F as f64;
            if cum[T_NULL] / (T_NULL as f64) < NULL_RATIO * mean {
                if let Some((soft, hz)) = self.rx.frame(&self.buf, at + T_NULL, L - 1) {
                    for block in soft[..FIC_SOFT].chunks_exact(FIC_BLOCK) {
                        self.ensemble.add_block(block, &self.rx.sent, &self.rx.prbs);
                    }
                    self.freq_error_hz = Some(hz);
                    self.frames += 1;
                    self.misses = 0;
                    frame(&soft);
                }
            } else {
                self.misses += 1;
                if self.misses > 3 {
                    // Lost it: look afresh.
                    self.next = None;
                    self.buf.drain(..at + T_F);
                    continue;
                }
            }
            // The next null is a frame on; the sample clock may drift a little.
            let next = quietest(&cum, T_F - 128, T_F + 128);
            self.buf.drain(..at + next);
            self.next = Some(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::encode::{frame, Spec};
    use super::*;
    use crate::radio::to_cu8;

    const SERVICES: &[(u16, &str)] = &[(0xC221, "Sim Radio One"), (0xC222, "Sim Jazz"), (0xC4A5, "Caf\u{e9} Classics")];

    /// `frames` frames of a multiplex, starting `skip` samples in, shifted
    /// by `hz`, with noise at `snr_db` and quantised to cu8 like a dongle.
    fn capture(frames: usize, skip: usize, hz: f64, snr_db: f64) -> Vec<u8> {
        let spec = Spec { eid: 0xC181, label: "SIM DAB", services: SERVICES, audio: &[] };
        let one = frame(&spec, 1);
        let amp = 0.2;
        let noise = amp * 10f64.powf(-snr_db / 20.0) / std::f64::consts::SQRT_2;
        let mut rng = 0x1234_5678_9abc_def0u64;
        let mut gauss = || {
            let mut u = || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng >> 11) as f64 / (1u64 << 53) as f64
            };
            (u() + u() + u() + u() - 2.0) * 1.732
        };
        let mut out = Vec::new();
        for i in skip..skip + frames * T_F {
            let s = one[i % T_F];
            let ph = TAU * hz * i as f64 / RATE as f64;
            let r = Complex32::new(ph.cos() as f32, ph.sin() as f32) * s * amp as f32;
            out.push(to_cu8(r.re + (noise * gauss()) as f32));
            out.push(to_cu8(r.im + (noise * gauss()) as f32));
        }
        out
    }

    #[test]
    fn reads_the_station_list() {
        let d = decode(&capture(6, 70_000, 0.0, 30.0));
        assert!(d.frames >= 4, "{d:?}");
        assert_eq!(d.ensemble.fibs_bad, 0, "{d:?}");
        assert_eq!(d.ensemble.eid, Some(0xC181));
        assert_eq!(d.ensemble.label.as_deref(), Some("SIM DAB"));
        let names: Vec<&str> = d.ensemble.services.values().map(String::as_str).collect();
        assert_eq!(names, ["Sim Radio One", "Sim Jazz", "Caf\u{e9} Classics"]);
    }

    #[test]
    fn copes_with_tuning_error_and_noise() {
        // 7.3 kHz off (a dongle about 33 ppm out at 222 MHz), 8 dB SNR.
        let d = decode(&capture(8, 12_345, 7_310.0, 8.0));
        let hz = d.freq_error_hz.unwrap();
        assert!((hz - 7_310.0).abs() < 30.0, "measured {hz} Hz");
        assert_eq!(d.ensemble.label.as_deref(), Some("SIM DAB"), "{d:?}");
        assert_eq!(d.ensemble.services.len(), 3, "{d:?}");
        let d = decode(&capture(8, 150_000, -21_480.0, 10.0));
        assert_eq!(d.ensemble.services.len(), 3, "{d:?}");
    }

    #[test]
    fn streams_a_subchannel() {
        use super::super::encode::{frames, Audio};
        use super::super::msc::{AudioKind, Protection, SubChannel, SubDecoder, CIF_BITS};
        let sub = SubChannel { id: 5, start: 200, size: 96, protection: Protection::EepA(3) };
        let logical: Vec<Vec<u8>> = (0..8u32).map(|t| (0..384u32).map(|i| (i ^ (t * 37)) as u8).collect()).collect();
        let audio = [Audio { sid: 0xC221, sub: sub.clone(), kind: AudioKind::Mp2, frames: logical.clone() }];
        let spec = Spec { eid: 0xC181, label: "SIM DAB", services: &SERVICES[..1], audio: &audio };
        let tx = frames(&spec, 2, 9);
        // 12 frames, 3.1 kHz off, 15 dB SNR, fed in 50 ms blocks.
        let mut cu8 = Vec::new();
        let mut rng = 77u64;
        for i in 1_000..12 * T_F {
            let s = tx[(i / T_F) % 2][i % T_F] * 0.2;
            let ph = TAU * 3_100.0 * i as f64 / RATE as f64;
            let r = Complex32::new(ph.cos() as f32, ph.sin() as f32) * s;
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let n = ((rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.06;
            cu8.push(to_cu8(r.re + n));
            cu8.push(to_cu8(r.im - n));
        }
        let mut stream = Stream::new();
        let mut dec = SubDecoder::new(&sub).unwrap();
        let mut out = Vec::new();
        for block in cu8.chunks(204_800) {
            stream.push(block, |soft| {
                for cif in soft[FIC_SOFT..].chunks_exact(CIF_BITS) {
                    out.extend(dec.push(cif));
                }
            });
        }
        assert!(stream.locked());
        assert_eq!(stream.ensemble.audio_of(0xC221).map(|a| a.0), Some(sub));
        assert!(out.len() >= 20, "{} logical frames", out.len());
        // Consecutive frames from the 8-frame loop, in order.
        let first = logical.iter().position(|f| *f == out[0]).expect("a sent frame");
        for (k, f) in out.iter().enumerate() {
            assert_eq!(*f, logical[(first + k) % 8], "logical frame {k}");
        }
    }

    #[test]
    fn timing_follows_the_reference_symbol() {
        let z = to_complex(&capture(2, 0, 1_500.0, 25.0));
        let mut rx = Rx::new();
        let sent = sent();
        let p = prbs(BLOCK_BITS);
        // The null guess off either way by more than the back-off.
        for off in [-150i64, -60, 90, 200] {
            let (soft, _) = rx.frame(&z, (T_F as i64 + T_NULL as i64 + off) as usize, FIC_SYMBOLS).unwrap();
            let mut e = Ensemble::default();
            for block in soft.chunks_exact(FIC_BLOCK) {
                e.add_block(block, &sent, &p);
            }
            assert_eq!((e.fibs_ok, e.fibs_bad), (12, 0), "offset {off}");
        }
    }

    #[test]
    fn noise_is_not_a_multiplex() {
        let mut rng = 99u64;
        let noise: Vec<u8> = (0..T_F * 4)
            .map(|_| {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                120 + (rng >> 60) as u8
            })
            .collect();
        let (ratio, _) = null_depth(&noise).unwrap();
        assert!(ratio > 0.8, "{ratio}");
        assert_eq!(decode(&noise).frames, 0);
        let (ratio, snr) = null_depth(&capture(2, 1000, 0.0, 15.0)).unwrap();
        assert!(ratio < 0.2 && snr > 8.0, "{ratio} {snr}");
    }
}
