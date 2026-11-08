//! Demodulators for live listening: one channel cut out of the capture and
//! turned into mono audio.
//!
//! | mode | channel filter | audio rate | notes |
//! |------|----------------|------------|-------|
//! | NFM  | ±8 kHz         | ~16 kHz    | DC-blocked, 3.8 kHz low-pass |
//! | AM   | ±5 kHz         | ~16 kHz    | envelope, carrier removed, AGC |
//! | USB/LSB | 3 kHz side | ~16 kHz    | AGC |
//! | WFM  | ±100 kHz       | ~34 kHz    | 50 µs de-emphasis (UK/EU), 13 kHz low-pass |

use num_complex::{Complex, Complex32};
use serde::{Deserialize, Serialize};

use crate::dsp::ddc::{audio_decimation, decimation_for, fits, lowpass, Ddc};
use crate::dsp::fm::FmDemod;
use super::nr::NoiseReducer;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Nfm,
    Wfm,
    Am,
    Usb,
    Lsb,
}

impl Mode {
    /// Occupied bandwidth (also drawn on the waterfall).
    pub fn bandwidth_hz(self) -> f64 {
        match self {
            Mode::Nfm => 12_500.0,
            Mode::Wfm => 200_000.0,
            Mode::Am => 10_000.0,
            Mode::Usb | Mode::Lsb => 3_000.0,
        }
    }

    /// Channel filter centre relative to the dial frequency.
    fn shift_hz(self) -> f64 {
        match self {
            Mode::Usb => 1_500.0,
            Mode::Lsb => -1_500.0,
            _ => 0.0,
        }
    }

    fn cutoff_hz(self) -> f64 {
        match self {
            Mode::Nfm => 8_000.0,
            Mode::Wfm => 100_000.0,
            Mode::Am => 5_000.0,
            Mode::Usb | Mode::Lsb => 1_500.0,
        }
    }

    pub fn id(self) -> u8 {
        match self {
            Mode::Nfm => 0,
            Mode::Wfm => 1,
            Mode::Am => 2,
            Mode::Usb => 3,
            Mode::Lsb => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    pub freq_hz: u64,
    pub mode: Mode,
    /// Mute below this channel level (dBFS). Ignored for WFM.
    pub squelch_db: Option<f32>,
    /// Spectral noise reduction.
    pub nr: bool,
}

impl Params {
    /// Whether this channel fits inside a capture.
    pub fn in_band(&self, center_hz: u64, sample_rate: u32) -> bool {
        let target = self.freq_hz as f64 + self.mode.shift_hz();
        target > 0.0 && fits(center_hz, sample_rate, target as u64, self.mode.bandwidth_hz())
    }
}

/// Real FIR low-pass + integer decimation.
struct RealDecim {
    taps: Vec<f32>,
    hist: Vec<f32>,
    decim: usize,
    count: usize,
}

impl RealDecim {
    fn new(n: usize, cutoff: f64, decim: usize) -> Self {
        let taps = lowpass(n, cutoff);
        RealDecim { hist: vec![0.0; taps.len() - 1], taps, decim, count: 0 }
    }

    fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        let keep = self.taps.len() - 1;
        for &v in x {
            self.hist.push(v);
            self.count += 1;
            if self.count == self.decim {
                self.count = 0;
                let w = &self.hist[self.hist.len() - self.taps.len()..];
                out.push(w.iter().zip(&self.taps).map(|(a, b)| a * b).sum());
            }
        }
        let drop = self.hist.len() - keep;
        self.hist.drain(..drop);
    }
}

pub struct Demod {
    mode: Mode,
    squelch_db: Option<f32>,
    ddc: Ddc,
    pub out_rate: f64,
    fm: Option<FmDemod>,
    ssb: Option<(Complex<f64>, Complex<f64>)>,
    deemph: Option<(f32, f32)>,
    dc: (f32, f32),
    agc: Option<(f32, f32)>,
    lp: RealDecim,
    nr: Option<NoiseReducer>,
    iq: Vec<Complex32>,
    mid: Vec<f32>,
    post: Vec<f32>,
}

impl Demod {
    pub fn new(p: &Params, center_hz: u64, in_rate: u32) -> Self {
        let m = p.mode;
        let (decim, mid_rate) = if m == Mode::Wfm {
            let d = decimation_for(in_rate, 200_000);
            (d, in_rate as f64 / d as f64)
        } else {
            let (d, r) = audio_decimation(in_rate);
            (d, r as f64)
        };
        let offset = p.freq_hz as f64 + m.shift_hz() - center_hz as f64;
        let ddc = Ddc::with_cutoff(in_rate, offset, decim, m.cutoff_hz());
        let (final_decim, lp) = if m == Mode::Wfm {
            // Strong cut below 17 kHz: the 19 kHz stereo pilot and 38 kHz
            // subcarrier mustn't alias into the audio.
            let d = (mid_rate / 34_000.0).round().max(1.0) as usize;
            (d, RealDecim::new(255, 13_000.0 / mid_rate, d))
        } else {
            let cut = if matches!(m, Mode::Usb | Mode::Lsb) { 2_800.0 } else { 3_800.0 };
            (3, RealDecim::new(95, cut / mid_rate, 3))
        };
        let ssb = matches!(m, Mode::Usb | Mode::Lsb).then(|| {
            let w = std::f64::consts::TAU * m.shift_hz() / mid_rate;
            (Complex::new(1.0, 0.0), Complex::new(w.cos(), w.sin()))
        });
        Demod {
            mode: m,
            squelch_db: p.squelch_db,
            ddc,
            out_rate: mid_rate / final_decim as f64,
            fm: match m {
                Mode::Nfm => Some(FmDemod::new(mid_rate as u32, 5_000.0)),
                Mode::Wfm => Some(FmDemod::new(mid_rate as u32, 75_000.0)),
                _ => None,
            },
            ssb,
            // 50 µs de-emphasis as a one-pole low-pass.
            deemph: (m == Mode::Wfm).then(|| (1.0 - (-1.0 / (50e-6 * mid_rate)).exp() as f32, 0.0)),
            dc: (0.0, 0.0),
            agc: matches!(m, Mode::Am | Mode::Usb | Mode::Lsb).then(|| ((-1.0 / (0.4 * mid_rate)).exp() as f32, 1e-3)),
            nr: p.nr.then(|| NoiseReducer::new(mid_rate / final_decim as f64)),
            lp,
            iq: Vec::new(),
            mid: Vec::new(),
            post: Vec::new(),
        }
    }

    /// Feed a cu8 block: returns (channel level dBFS, squelch open, audio).
    pub fn process(&mut self, data: &[u8], out: &mut Vec<f32>) -> (f32, bool) {
        self.iq.clear();
        self.ddc.process_cu8(data, &mut self.iq);
        let power = self.iq.iter().map(|z| z.norm_sqr()).sum::<f32>() / self.iq.len().max(1) as f32;
        let level = 10.0 * (power + 1e-12).log10();

        self.mid.clear();
        match self.mode {
            Mode::Nfm | Mode::Wfm => self.fm.as_mut().unwrap().process_f32(&self.iq, &mut self.mid),
            Mode::Am => self.mid.extend(self.iq.iter().map(|z| z.norm())),
            Mode::Usb | Mode::Lsb => {
                let (p, step) = self.ssb.as_mut().unwrap();
                for z in &self.iq {
                    self.mid.push(z.re * p.re as f32 - z.im * p.im as f32);
                    *p *= *step;
                }
                *p /= p.norm();
            }
        }
        if let Some((a, y)) = self.deemph.as_mut() {
            for v in self.mid.iter_mut() {
                *y += *a * (*v - *y);
                *v = *y;
            }
        }
        // DC block: removes the AM carrier and any FM tuning offset.
        let (x1, y1) = &mut self.dc;
        for v in self.mid.iter_mut() {
            let y = *v - *x1 + 0.998 * *y1;
            *x1 = *v;
            *y1 = y;
            *v = y;
        }
        if let Some((decay, env)) = self.agc.as_mut() {
            for v in self.mid.iter_mut() {
                *env = v.abs().max(*env * *decay).max(1e-5);
                *v *= 0.3 / *env;
            }
        }
        let start = out.len();
        match self.nr.as_mut() {
            Some(nr) => {
                self.post.clear();
                self.lp.process(&self.mid, &mut self.post);
                nr.process(&self.post, out);
            }
            None => self.lp.process(&self.mid, out),
        }
        let open = match (self.mode, self.squelch_db) {
            (Mode::Wfm, _) | (_, None) => true,
            (_, Some(sq)) => level >= sq,
        };
        if !open {
            out[start..].iter_mut().for_each(|v| *v = 0.0);
        }
        (level, open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::to_cu8;
    use std::f64::consts::TAU;

    /// One second of cu8 at `rate` from a complex baseband generator.
    fn iq(rate: u32, secs: f64, f: impl Fn(f64) -> (f64, f64)) -> Vec<u8> {
        let n = (rate as f64 * secs) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let (re, im) = f(i as f64 / rate as f64);
            v.push(to_cu8(re as f32));
            v.push(to_cu8(im as f32));
        }
        v
    }

    /// Dominant frequency of `x` by zero crossings (fine for one clean tone).
    fn tone_hz(x: &[f32], rate: f64) -> f64 {
        let x = &x[x.len() / 3..];
        let zc = x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        zc as f64 / 2.0 / (x.len() as f64 / rate)
    }

    fn run(mode: Mode, carrier_off: f64, rate: u32, sig: impl Fn(f64) -> (f64, f64)) -> (Vec<f32>, f64, f32) {
        let center = 100_000_000u64;
        let p = Params { freq_hz: (center as f64 + carrier_off) as u64, mode, squelch_db: None, nr: false };
        let mut d = Demod::new(&p, center, rate);
        let data = iq(rate, 1.0, sig);
        let mut out = Vec::new();
        let mut level = 0.0;
        for chunk in data.chunks(2 * 102_400) {
            level = d.process(chunk, &mut out).0;
        }
        (out, d.out_rate, level)
    }

    #[test]
    fn nfm_recovers_tone() {
        let rate = 2_048_000;
        let off = 250_000.0;
        // 1 kHz tone, 3 kHz deviation.
        let (out, r, level) = run(Mode::Nfm, off, rate, |t| {
            let ph = TAU * off * t + 3.0 * (TAU * 1000.0 * t).sin();
            (0.5 * ph.cos(), 0.5 * ph.sin())
        });
        assert!((r - 17_066.7).abs() < 1.0, "rate {r}");
        assert!((tone_hz(&out, r) - 1000.0).abs() < 15.0, "{}", tone_hz(&out, r));
        assert!(level > -10.0, "level {level}");
    }

    #[test]
    fn am_and_ssb_recover_tone() {
        let rate = 1_024_000;
        let off = -100_000.0;
        let (out, r, _) = run(Mode::Am, off, rate, |t| {
            let a = 0.3 * (1.0 + 0.6 * (TAU * 700.0 * t).sin());
            let ph = TAU * off * t;
            (a * ph.cos(), a * ph.sin())
        });
        assert!((tone_hz(&out, r) - 700.0).abs() < 15.0, "AM {}", tone_hz(&out, r));
        // USB: a 1.2 kHz audio tone sits 1.2 kHz above the dial.
        let (out, r, _) = run(Mode::Usb, off, rate, |t| {
            let ph = TAU * (off + 1200.0) * t;
            (0.3 * ph.cos(), 0.3 * ph.sin())
        });
        assert!((tone_hz(&out, r) - 1200.0).abs() < 15.0, "USB {}", tone_hz(&out, r));
        let (out, r, _) = run(Mode::Lsb, off, rate, |t| {
            let ph = TAU * (off - 900.0) * t;
            (0.3 * ph.cos(), 0.3 * ph.sin())
        });
        assert!((tone_hz(&out, r) - 900.0).abs() < 15.0, "LSB {}", tone_hz(&out, r));
    }

    #[test]
    fn wfm_recovers_tone_and_drops_pilot() {
        let rate = 2_048_000;
        let off = 400_000.0;
        // 2 kHz tone (40 kHz deviation) plus a 19 kHz pilot (7.5 kHz).
        let (out, r, _) = run(Mode::Wfm, off, rate, |t| {
            let ph = TAU * off * t
                + (40_000.0 / 2_000.0) * (TAU * 2_000.0 * t).sin()
                + (7_500.0 / 19_000.0) * (TAU * 19_000.0 * t).sin();
            (0.5 * ph.cos(), 0.5 * ph.sin())
        });
        assert!((r - 34_133.3).abs() < 1.0, "rate {r}");
        assert!((tone_hz(&out, r) - 2000.0).abs() < 20.0, "{}", tone_hz(&out, r));
    }

    #[test]
    fn squelch_mutes_noise() {
        let center = 100_000_000u64;
        let p = Params { freq_hz: center + 50_000, mode: Mode::Nfm, squelch_db: Some(-30.0), nr: true };
        let mut d = Demod::new(&p, center, 1_024_000);
        assert!((d.out_rate - 51_200.0 / 3.0).abs() < 1e-6);
        let quiet = iq(1_024_000, 0.2, |_| (0.0, 0.0));
        let mut out = Vec::new();
        let (level, open) = d.process(&quiet, &mut out);
        assert!(!open && level < -30.0);
        assert!(out.iter().all(|&v| v == 0.0));
    }
}
