//! Spectral noise reduction for live audio.
//!
//! Short-time FFT (sqrt-Hann windows, 50 % overlap, so analysis × synthesis
//! sums to one). Per frequency bin the noise floor follows the smoothed
//! power down instantly and creeps up slowly (~3 dB/s), so it settles on the
//! background hiss without chasing speech or tones. Each bin is scaled by a
//! Wiener-like gain `1 − k·noise/power` (floored at −18 dB) that is smoothed
//! over time to avoid "musical noise".

use std::sync::Arc;

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

const OVER_SUBTRACT: f32 = 1.8;
const GAIN_FLOOR: f32 = 0.125; // −18 dB
const NOISE_BIAS: f32 = 2.0; // a minimum sits below the mean noise power

pub struct NoiseReducer {
    n: usize,
    hop: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    win: Vec<f32>,
    input: Vec<f32>,
    acc: Vec<f32>,
    buf: Vec<Complex32>,
    scratch: Vec<Complex32>,
    psd: Vec<f32>,
    noise: Vec<f32>,
    gain: Vec<f32>,
    rise: f32,
}

impl NoiseReducer {
    pub fn new(rate: f64) -> Self {
        // ~16 ms frames.
        let n = if rate < 24_000.0 { 256 } else { 512 };
        let hop = n / 2;
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        let scratch_len = fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len());
        let win = (0..n)
            .map(|i| (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).sqrt())
            .collect();
        let frames_per_s = rate / hop as f64;
        NoiseReducer {
            n,
            hop,
            fft,
            ifft,
            win,
            input: Vec::with_capacity(2 * n),
            acc: vec![0.0; n],
            buf: vec![Complex32::default(); n],
            scratch: vec![Complex32::default(); scratch_len],
            psd: vec![0.0; n],
            noise: vec![f32::MAX; n],
            gain: vec![1.0; n],
            rise: 10f64.powf(3.0 / 10.0 / frames_per_s) as f32,
        }
    }

    /// Feed audio; appends processed audio (delayed by one frame).
    pub fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        self.input.extend_from_slice(x);
        while self.input.len() >= self.n {
            self.frame();
            out.extend_from_slice(&self.acc[..self.hop]);
            self.acc.copy_within(self.hop.., 0);
            let n = self.n;
            self.acc[n - self.hop..].iter_mut().for_each(|v| *v = 0.0);
            self.input.drain(..self.hop);
        }
    }

    fn frame(&mut self) {
        for i in 0..self.n {
            self.buf[i] = Complex32::new(self.input[i] * self.win[i], 0.0);
        }
        self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);
        for k in 0..self.n {
            let p = self.buf[k].norm_sqr();
            self.psd[k] = 0.7 * self.psd[k] + 0.3 * p;
            self.noise[k] = if self.psd[k] < self.noise[k] { self.psd[k] } else { self.noise[k] * self.rise };
            let snr = p / (self.noise[k] * NOISE_BIAS + 1e-12);
            let g = (1.0 - OVER_SUBTRACT / snr.max(1e-6)).max(GAIN_FLOOR);
            self.gain[k] = 0.6 * self.gain[k] + 0.4 * g;
            self.buf[k] *= self.gain[k];
        }
        self.ifft.process_with_scratch(&mut self.buf, &mut self.scratch);
        let scale = 1.0 / self.n as f32;
        for i in 0..self.n {
            self.acc[i] += self.buf[i].re * scale * self.win[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_hiss_keeps_changing_signals() {
        // A tone keyed on and off every 0.4 s (like speech or data) in hiss.
        // A tone that never changes would be treated as noise, as in any
        // spectral noise reducer.
        let rate = 16_000.0f32;
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        let mut noise = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            ((rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.6
        };
        let on = |i: usize| ((i as f32 / rate / 0.4) as usize).is_multiple_of(2);
        let tone = |i: usize| if on(i) { 0.3 * (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin() } else { 0.0 };
        let n = rate as usize * 6;
        let clean: Vec<f32> = (0..n).map(tone).collect();
        let x: Vec<f32> = clean.iter().map(|t| t + noise()).collect();
        let mut nr = NoiseReducer::new(rate as f64);
        let mut y = Vec::new();
        for chunk in x.chunks(800) {
            nr.process(chunk, &mut y);
        }
        // Output sample i lines up with input sample i. Judge the settled
        // second half, away from the keying edges.
        let mid = |i: usize| ((i as f32 / rate) % 0.4 - 0.2).abs() < 0.12;
        let (mut noise_in, mut noise_out, mut nn) = (0f64, 0f64, 0);
        let (mut sig_in, mut sig_out, mut ns) = (0f64, 0f64, 0);
        for i in n / 2..y.len() {
            if !mid(i) {
                continue;
            }
            if on(i) {
                // Tone kept: correlation with the clean tone.
                sig_in += (x[i] * clean[i]) as f64;
                sig_out += (y[i] * clean[i]) as f64;
                ns += 1;
            } else {
                noise_in += (x[i] * x[i]) as f64;
                noise_out += (y[i] * y[i]) as f64;
                nn += 1;
            }
        }
        let hiss_cut = 10.0 * (noise_in / noise_out).log10();
        let tone_loss = 20.0 * (sig_in / sig_out).log10();
        assert!(nn > 1000 && ns > 1000);
        assert!(hiss_cut > 10.0, "hiss only reduced by {hiss_cut:.1} dB");
        assert!(tone_loss < 2.0, "tone reduced by {tone_loss:.1} dB");
    }
}
