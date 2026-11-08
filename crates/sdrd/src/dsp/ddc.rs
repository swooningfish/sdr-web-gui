//! Digital down-converter: moves one frequency in the wideband cu8 capture to
//! 0 Hz, low-pass filters it and decimates to a narrow complex stream. Used to
//! hand each decoder (rtl_433 now, Direwolf and SSTV later) only its own slice
//! of the spectrum, so several can share one dongle.

use num_complex::{Complex, Complex32};

/// Integer decimation that keeps the output rate at or above `min_out`.
pub fn decimation_for(in_rate: u32, min_out: u32) -> usize {
    (in_rate / min_out.max(1)).max(1) as usize
}

/// Decimation giving an exact integer audio rate between 44 and 64 kHz (as
/// close to 48 kHz as possible), so external decoders get a true rate.
/// Falls back to the nearest rate at or above 44 kHz.
pub fn audio_decimation(in_rate: u32) -> (usize, u32) {
    let lo = (in_rate / 64_000).max(1);
    let hi = (in_rate / 44_000).max(1);
    (lo..=hi)
        .filter(|d| in_rate.is_multiple_of(*d))
        .map(|d| (d as usize, in_rate / d))
        .min_by_key(|(_, r)| r.abs_diff(48_000))
        .unwrap_or_else(|| {
            let d = hi.max(1);
            (d as usize, in_rate / d)
        })
}

/// Whether a channel `bw_hz` wide at `target_hz` fits inside the capture,
/// staying clear of the band edges where the dongle's own filter rolls off.
pub fn fits(center_hz: u64, sample_rate: u32, target_hz: u64, bw_hz: f64) -> bool {
    let offset = (target_hz as f64 - center_hz as f64).abs();
    offset + bw_hz / 2.0 <= sample_rate as f64 * 0.45
}

pub struct Ddc {
    decim: usize,
    taps: Vec<f32>,
    /// Mixer: phasor rotated by `step` every input sample.
    phasor: Complex<f64>,
    step: Complex<f64>,
    /// Mixed input samples; the newest `taps.len() - 1` are kept between calls.
    hist: Vec<Complex32>,
    /// Input samples since the last output.
    count: usize,
    lut: [f32; 256],
}

impl Ddc {
    /// `offset_hz` is the frequency (relative to the capture centre) to move to
    /// 0 Hz. The output is filtered to ±40 % of the output rate.
    pub fn new(in_rate: u32, offset_hz: f64, decim: usize) -> Self {
        let out_rate = in_rate as f64 / decim.max(1) as f64;
        Self::with_cutoff(in_rate, offset_hz, decim, 0.4 * out_rate)
    }

    /// Like `new`, but filtered to ±`cutoff_hz` (e.g. a 12.5 kHz NFM channel).
    pub fn with_cutoff(in_rate: u32, offset_hz: f64, decim: usize, cutoff_hz: f64) -> Self {
        let decim = decim.max(1);
        let taps = if decim == 1 {
            vec![1.0]
        } else {
            lowpass(16 * decim + 1, cutoff_hz / in_rate as f64)
        };
        let w = -std::f64::consts::TAU * offset_hz / in_rate as f64;
        let mut lut = [0f32; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = (i as f32 - 127.5) / 127.5;
        }
        Ddc {
            decim,
            hist: vec![Complex32::default(); taps.len() - 1],
            taps,
            phasor: Complex::new(1.0, 0.0),
            step: Complex::new(w.cos(), w.sin()),
            count: 0,
            lut,
        }
    }

    /// Feed interleaved cu8 samples; appends complex output (±1.0 full scale).
    pub fn process_cu8(&mut self, data: &[u8], out: &mut Vec<Complex32>) {
        let keep = self.taps.len() - 1;
        for pair in data.chunks_exact(2) {
            let x = Complex::new(self.lut[pair[0] as usize] as f64, self.lut[pair[1] as usize] as f64);
            let m = x * self.phasor;
            self.phasor *= self.step;
            self.hist.push(Complex32::new(m.re as f32, m.im as f32));
            self.count += 1;
            if self.count == self.decim {
                self.count = 0;
                let win = &self.hist[self.hist.len() - self.taps.len()..];
                let mut acc = Complex32::default();
                // Taps are symmetric, so no need to reverse them.
                for (s, t) in win.iter().zip(&self.taps) {
                    acc += s * *t;
                }
                out.push(acc);
            }
        }
        // Stop rounding errors from slowly changing the mixer's amplitude.
        self.phasor /= self.phasor.norm();
        let drop = self.hist.len() - keep;
        self.hist.drain(..drop);
    }
}

/// Windowed-sinc (Blackman) low-pass, unity gain at DC. `cutoff` is a fraction
/// of the sample rate.
fn lowpass(n: usize, cutoff: f64) -> Vec<f32> {
    let m = (n - 1) as f64;
    let mut taps: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - m / 2.0;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (std::f64::consts::TAU * cutoff * x).sin() / (std::f64::consts::PI * x)
            };
            let a = std::f64::consts::TAU * i as f64 / m;
            let w = 0.42 - 0.5 * a.cos() + 0.08 * (2.0 * a).cos();
            sinc * w
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.iter_mut().for_each(|t| *t /= sum);
    taps.into_iter().map(|t| t as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::to_cu8;

    fn tone(rate: u32, hz: f64, amp: f64, n: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let ph = std::f64::consts::TAU * hz * i as f64 / rate as f64;
            v.push(to_cu8((amp * ph.cos()) as f32));
            v.push(to_cu8((amp * ph.sin()) as f32));
        }
        v
    }

    fn power(x: &[Complex32]) -> f32 {
        x.iter().map(|c| c.norm_sqr()).sum::<f32>() / x.len() as f32
    }

    #[test]
    fn passes_target_and_rejects_neighbours() {
        let rate = 2_048_000;
        let decim = decimation_for(rate, 250_000);
        assert_eq!(decim, 8);
        let mut out = Vec::new();

        // Tone exactly on the target (+300 kHz) comes out near DC at full level.
        let mut d = Ddc::new(rate, 300_000.0, decim);
        d.process_cu8(&tone(rate, 300_000.0, 0.5, 65_536), &mut out);
        assert_eq!(out.len(), 65_536 / 8);
        let p_in = power(&out[100..]);
        assert!((p_in - 0.25).abs() < 0.03, "passband power {p_in}");
        // ...and it's at DC: consecutive samples barely rotate.
        let rot = (out[500] * out[499].conj()).arg().abs();
        assert!(rot < 0.01, "residual rotation {rot}");

        // A tone 400 kHz away from the target is attenuated by > 50 dB.
        out.clear();
        let mut d = Ddc::new(rate, 300_000.0, decim);
        d.process_cu8(&tone(rate, -100_000.0, 0.5, 65_536), &mut out);
        let p_out = power(&out[100..]);
        assert!(10.0 * (p_out / p_in).log10() < -50.0, "stopband {p_out}");
    }

    #[test]
    fn block_boundaries_do_not_matter() {
        let rate = 1_024_000;
        let data = tone(rate, 12_345.0, 0.4, 20_000);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        Ddc::new(rate, 10_000.0, 4).process_cu8(&data, &mut a);
        let mut d = Ddc::new(rate, 10_000.0, 4);
        for chunk in data.chunks(2 * 777) {
            d.process_cu8(chunk, &mut b);
        }
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).norm() < 1e-4);
        }
    }

    #[test]
    fn audio_rates_are_exact() {
        for &r in crate::radio::SAMPLE_RATES {
            let (d, a) = audio_decimation(r);
            assert_eq!(r % d as u32, 0, "{r}");
            assert_eq!(r / d as u32, a);
            assert!((44_000..=64_000).contains(&a), "{r} -> {a}");
        }
        assert_eq!(audio_decimation(2_048_000), (40, 51_200));
        assert_eq!(audio_decimation(2_400_000), (50, 48_000));
    }

    #[test]
    fn band_fit() {
        assert!(fits(434_000_000, 1_024_000, 433_920_000, 200_000.0));
        assert!(!fits(145_300_000, 2_048_000, 433_920_000, 200_000.0));
        assert!(!fits(433_920_000, 250_000, 434_100_000, 200_000.0));
    }
}
