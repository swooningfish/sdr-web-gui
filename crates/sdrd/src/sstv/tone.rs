//! Audio → instantaneous tone frequency. The audio is mixed down by 1900 Hz
//! (the middle of the SSTV range), low-pass filtered as a complex signal so
//! only the positive-frequency tone remains, decimated by 2, and the phase
//! step between samples gives the frequency. Amplitude doesn't matter, so it
//! works the same for FM and SSB.

use num_complex::{Complex, Complex32};

use crate::dsp::ddc::lowpass;

const CENTER_HZ: f64 = 1900.0;
const DECIM: usize = 2;

pub struct ToneDemod {
    rate_out: f64,
    taps: Vec<f32>,
    hist: Vec<Complex32>,
    count: usize,
    phasor: Complex<f64>,
    step: Complex<f64>,
    prev: Complex32,
    /// DC blocker state (an off-tune FM carrier shows up as DC).
    dc_x: f32,
    dc_y: f32,
}

impl ToneDemod {
    pub fn new(audio_rate: f64) -> Self {
        // ±2.2 kHz keeps tone changes sharp (Robot 36 chroma pixels are
        // 0.14 ms) while rejecting the mirrored negative-frequency tones,
        // which land at -3 kHz and below.
        let taps = lowpass(321, 2200.0 / audio_rate);
        let w = -std::f64::consts::TAU * CENTER_HZ / audio_rate;
        ToneDemod {
            rate_out: audio_rate / DECIM as f64,
            hist: vec![Complex32::default(); taps.len() - 1],
            taps,
            count: 0,
            phasor: Complex::new(1.0, 0.0),
            step: Complex::new(w.cos(), w.sin()),
            prev: Complex32::new(1.0, 0.0),
            dc_x: 0.0,
            dc_y: 0.0,
        }
    }

    /// Rate of the frequency samples produced.
    pub fn rate(&self) -> f64 {
        self.rate_out
    }

    pub fn process(&mut self, audio: &[f32], out: &mut Vec<f32>) {
        let keep = self.taps.len() - 1;
        let scale = (self.rate_out / std::f64::consts::TAU) as f32;
        for &x in audio {
            let y = x - self.dc_x + 0.999 * self.dc_y;
            self.dc_x = x;
            self.dc_y = y;
            let m = self.phasor * y as f64;
            self.phasor *= self.step;
            self.hist.push(Complex32::new(m.re as f32, m.im as f32));
            self.count += 1;
            if self.count == DECIM {
                self.count = 0;
                let win = &self.hist[self.hist.len() - self.taps.len()..];
                let mut acc = Complex32::default();
                for (s, t) in win.iter().zip(&self.taps) {
                    acc += s * *t;
                }
                let f = (acc * self.prev.conj()).arg() * scale + CENTER_HZ as f32;
                self.prev = acc;
                out.push(f);
            }
        }
        self.phasor /= self.phasor.norm();
        let drop = self.hist.len() - keep;
        self.hist.drain(..drop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sstv::encode::synth;

    #[test]
    fn tracks_tone_steps() {
        let rate = 48_000.0;
        let audio = synth(&[(1500.0, 0.05), (2300.0, 0.05), (1200.0, 0.05)], rate, 0.5);
        let mut d = ToneDemod::new(rate);
        let mut f = Vec::new();
        d.process(&audio, &mut f);
        let r = d.rate();
        let at = |t: f64| f[(t * r) as usize];
        assert!((at(0.03) - 1500.0).abs() < 5.0, "{}", at(0.03));
        assert!((at(0.08) - 2300.0).abs() < 5.0, "{}", at(0.08));
        assert!((at(0.13) - 1200.0).abs() < 5.0, "{}", at(0.13));
    }
}
