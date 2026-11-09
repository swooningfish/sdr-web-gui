//! Demodulators for live listening: one channel cut out of the capture and
//! turned into mono audio.
//!
//! | mode | channel filter | audio rate | notes |
//! |------|----------------|------------|-------|
//! | NFM  | ±8 kHz         | ~16 kHz    | DC-blocked, 3.8 kHz low-pass |
//! | AM   | ±5 kHz         | ~16 kHz    | envelope, carrier removed, AGC |
//! | USB/LSB | 3 kHz side | ~16 kHz    | AGC |
//! | CW   | ±600 Hz        | ~16 kHz    | 700 Hz beat note (dial = carrier), 700 Hz band-pass, AGC |
//! | WFM  | ±100 kHz       | ~34 kHz    | 50 µs de-emphasis (UK/EU), 13 kHz low-pass, RDS |
//!
//! Optional Morse decoding (`Params::cw`) runs on the audio before AGC,
//! noise reduction and squelch, so it also reads FM repeater idents in NFM.
//!
//! Optional AFC (`Params::afc`, NFM and AM) follows a drifting signal: the
//! down-converter passes a wider window (about ±17 kHz), the power-weighted
//! mean frequency of that window is measured every 5 ms and the mixer is
//! nudged towards it, then a sharp filter cuts the channel back out. It only
//! moves while something coherent is in the window, so it holds its place
//! through gaps instead of wandering off on noise.

use num_complex::{Complex, Complex32};
use serde::{Deserialize, Serialize};

use crate::dsp::ddc::{audio_decimation, decimation_for, fits, lowpass, Ddc};
use crate::dsp::fm::FmDemod;
use super::cw::{CwDecoder, CwText};
use super::nr::NoiseReducer;
use super::rds::{RdsDecoder, RdsInfo};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Nfm,
    Wfm,
    Am,
    Usb,
    Lsb,
    Cw,
}

/// Beat note for CW: a carrier at the dial frequency is heard at this pitch.
pub const CW_PITCH_HZ: f64 = 700.0;

impl Mode {
    /// Occupied bandwidth (also drawn on the waterfall).
    pub fn bandwidth_hz(self) -> f64 {
        match self {
            Mode::Nfm => 12_500.0,
            Mode::Wfm => 200_000.0,
            Mode::Am => 10_000.0,
            Mode::Usb | Mode::Lsb => 3_000.0,
            Mode::Cw => 500.0,
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

    /// Product detector: how far the channel centre is moved up in the audio.
    fn bfo_hz(self) -> f64 {
        match self {
            Mode::Cw => CW_PITCH_HZ,
            m => m.shift_hz(),
        }
    }

    /// How far AFC may pull away from the dial frequency, if it works in this mode.
    pub fn afc_range_hz(self) -> Option<f64> {
        match self {
            Mode::Nfm => Some(50_000.0),
            Mode::Am => Some(10_000.0),
            _ => None,
        }
    }

    fn cutoff_hz(self) -> f64 {
        match self {
            Mode::Nfm => 8_000.0,
            Mode::Wfm => 100_000.0,
            Mode::Am => 5_000.0,
            Mode::Usb | Mode::Lsb => 1_500.0,
            Mode::Cw => 600.0,
        }
    }

    pub fn id(self) -> u8 {
        match self {
            Mode::Nfm => 0,
            Mode::Wfm => 1,
            Mode::Am => 2,
            Mode::Usb => 3,
            Mode::Lsb => 4,
            Mode::Cw => 5,
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
    /// Decode Morse from the audio.
    pub cw: bool,
    /// Automatic frequency control: follow a drifting signal (NFM and AM).
    pub afc: bool,
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

/// Two-pole band-pass (RBJ cookbook, constant 0 dB peak gain).
#[derive(Clone)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn bandpass(rate: f64, hz: f64, q: f64) -> Self {
        let w = std::f64::consts::TAU * hz / rate;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Biquad {
            b: [(alpha / a0) as f32, 0.0, (-alpha / a0) as f32],
            a: [(-2.0 * w.cos() / a0) as f32, ((1.0 - alpha) / a0) as f32],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn process(&mut self, v: &mut [f32]) {
        for s in v.iter_mut() {
            let y = self.b[0] * *s + self.b[1] * self.x[0] + self.b[2] * self.x[1]
                - self.a[0] * self.y[0]
                - self.a[1] * self.y[1];
            self.x = [*s, self.x[0]];
            self.y = [y, self.y[0]];
            *s = y;
        }
    }
}

/// Complex FIR low-pass at the channel rate (no decimation).
struct ComplexFir {
    taps: Vec<f32>,
    hist: Vec<Complex32>,
}

impl ComplexFir {
    fn new(n: usize, cutoff: f64) -> Self {
        let taps = lowpass(n, cutoff);
        ComplexFir { hist: vec![Complex32::default(); taps.len() - 1], taps }
    }

    fn process(&mut self, x: &[Complex32], out: &mut Vec<Complex32>) {
        let n = self.taps.len();
        for &v in x {
            self.hist.push(v);
            let w = &self.hist[self.hist.len() - n..];
            let mut acc = Complex32::default();
            for (s, t) in w.iter().zip(&self.taps) {
                acc += s * *t;
            }
            out.push(acc);
        }
        let drop = self.hist.len() - (n - 1);
        self.hist.drain(..drop);
    }
}

/// AFC lock threshold on |R1| / R0 (lag-1 autocorrelation over power) in the
/// measurement window. Noise alone gives about 0.44 there (the window's
/// filter is gentle; never above 0.55 in tests), a signal 3 dB under the
/// noise about 0.62, a clean NFM or AM signal over 0.9.
const AFC_LOCK: f64 = 0.6;
/// Fraction of the measured error corrected per 5 ms step (time constant ~15 ms).
const AFC_GAIN: f64 = 0.3;

/// Power-weighted mean frequency (Hz) of a stretch of baseband, and how
/// coherent it is (0 noise … 1 a single tone).
fn mean_frequency(z: &[Complex32], rate: f64) -> (f64, f64) {
    let (mut r0, mut r1) = (0.0f64, Complex::<f64>::new(0.0, 0.0));
    for w in z.windows(2) {
        let c = w[1] * w[0].conj();
        r1 += Complex::new(c.re as f64, c.im as f64);
        r0 += w[1].norm_sqr() as f64;
    }
    if r0 <= 0.0 {
        return (0.0, 0.0);
    }
    (r1.arg() * rate / std::f64::consts::TAU, r1.norm() / r0)
}

/// Automatic frequency control state for one listener.
struct Afc {
    in_rate: u32,
    mid_rate: f64,
    /// Mixer offset of the dial frequency from the capture centre.
    base_hz: f64,
    /// Correction on top of the dial frequency.
    offset_hz: f64,
    /// Allowed correction, both ways.
    lo_hz: f64,
    hi_hz: f64,
    /// Input bytes per measurement step.
    step_bytes: usize,
    locked: bool,
    chan: ComplexFir,
    wide: Vec<Complex32>,
}

impl Afc {
    fn set(&mut self, ddc: &mut Ddc, offset_hz: f64) {
        self.offset_hz = offset_hz.clamp(self.lo_hz, self.hi_hz);
        ddc.set_offset(self.in_rate, self.base_hz + self.offset_hz);
    }

    fn process(&mut self, ddc: &mut Ddc, data: &[u8], out: &mut Vec<Complex32>) {
        self.wide.clear();
        let mut locked = false;
        for part in data.chunks(self.step_bytes) {
            let start = self.wide.len();
            ddc.process_cu8(part, &mut self.wide);
            let (err, coh) = mean_frequency(&self.wide[start..], self.mid_rate);
            if self.wide.len() - start >= 16 && coh >= AFC_LOCK {
                locked = true;
                self.set(ddc, self.offset_hz + AFC_GAIN * err);
            }
        }
        self.locked = locked;
        self.chan.process(&self.wide, out);
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
    bandpass: Vec<Biquad>,
    /// Morse: its own low-pass/decimator on the audio before AGC, and the decoder.
    cw: Option<(RealDecim, Vec<f32>, CwDecoder)>,
    rds: Option<RdsDecoder>,
    afc: Option<Afc>,
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
        let afc = m.afc_range_hz().filter(|_| p.afc).map(|range| {
            // Keep the moved channel inside the capture.
            let edge = in_rate as f64 * 0.45 - m.bandwidth_hz() / 2.0;
            Afc {
                in_rate,
                mid_rate,
                base_hz: offset,
                offset_hz: 0.0,
                lo_hz: (-range).max(-edge - offset).min(0.0),
                hi_hz: range.min(edge - offset).max(0.0),
                step_bytes: 2 * (in_rate as usize / 200).max(1),
                locked: false,
                chan: ComplexFir::new(63, m.cutoff_hz() / mid_rate),
                wide: Vec::new(),
            }
        });
        // With AFC the down-converter passes a wider window to measure in.
        let cutoff = if afc.is_some() { 0.35 * mid_rate } else { m.cutoff_hz() };
        let ddc = Ddc::with_cutoff(in_rate, offset, decim, cutoff);
        let (final_decim, lp) = if m == Mode::Wfm {
            // Strong cut below 17 kHz: the 19 kHz stereo pilot and 38 kHz
            // subcarrier mustn't alias into the audio.
            let d = (mid_rate / 34_000.0).round().max(1.0) as usize;
            (d, RealDecim::new(255, 13_000.0 / mid_rate, d))
        } else {
            let cut = match m {
                Mode::Usb | Mode::Lsb => 2_800.0,
                Mode::Cw => 1_200.0,
                _ => 3_800.0,
            };
            (3, RealDecim::new(95, cut / mid_rate, 3))
        };
        let ssb = matches!(m, Mode::Usb | Mode::Lsb | Mode::Cw).then(|| {
            let w = std::f64::consts::TAU * m.bfo_hz() / mid_rate;
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
            agc: matches!(m, Mode::Am | Mode::Usb | Mode::Lsb | Mode::Cw)
                .then(|| ((-1.0 / (0.4 * mid_rate)).exp() as f32, 1e-3)),
            nr: p.nr.then(|| NoiseReducer::new(mid_rate / final_decim as f64)),
            bandpass: if m == Mode::Cw {
                // Two in a row: about 250 Hz wide, steep enough to drop neighbours.
                vec![Biquad::bandpass(mid_rate / final_decim as f64, CW_PITCH_HZ, 4.0); 2]
            } else {
                Vec::new()
            },
            cw: (p.cw && m != Mode::Wfm).then(|| {
                // Flat to ~1.8 kHz, so the noise the decoder sees has no slope.
                let lp = RealDecim::new(191, 2_400.0 / mid_rate, final_decim);
                (lp, Vec::new(), CwDecoder::new(mid_rate / final_decim as f64))
            }),
            rds: (m == Mode::Wfm && mid_rate >= 150_000.0).then(|| RdsDecoder::new(mid_rate)),
            afc,
            lp,
            iq: Vec::new(),
            mid: Vec::new(),
            post: Vec::new(),
        }
    }

    /// Feed a cu8 block: returns (channel level dBFS, squelch open, audio).
    pub fn process(&mut self, data: &[u8], out: &mut Vec<f32>) -> (f32, bool) {
        self.iq.clear();
        match self.afc.as_mut() {
            Some(afc) => afc.process(&mut self.ddc, data, &mut self.iq),
            None => self.ddc.process_cu8(data, &mut self.iq),
        }
        let power = self.iq.iter().map(|z| z.norm_sqr()).sum::<f32>() / self.iq.len().max(1) as f32;
        let level = 10.0 * (power + 1e-12).log10();

        self.mid.clear();
        match self.mode {
            Mode::Nfm | Mode::Wfm => self.fm.as_mut().unwrap().process_f32(&self.iq, &mut self.mid),
            Mode::Am => self.mid.extend(self.iq.iter().map(|z| z.norm())),
            Mode::Usb | Mode::Lsb | Mode::Cw => {
                let (p, step) = self.ssb.as_mut().unwrap();
                for z in &self.iq {
                    self.mid.push(z.re * p.re as f32 - z.im * p.im as f32);
                    *p *= *step;
                }
                *p /= p.norm();
            }
        }
        if let Some(rds) = self.rds.as_mut() {
            rds.process(&self.mid); // the multiplex, before de-emphasis
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
        // Morse is read before the AGC, which would pump the hiss up between
        // letters, and before the CW band-pass, whose peak looks like a tone.
        if let Some((lp, buf, cw)) = self.cw.as_mut() {
            buf.clear();
            lp.process(&self.mid, buf);
            cw.process(buf);
        }
        if let Some((decay, env)) = self.agc.as_mut() {
            for v in self.mid.iter_mut() {
                *env = v.abs().max(*env * *decay).max(1e-5);
                *v *= 0.3 / *env;
            }
        }
        let start = out.len();
        self.post.clear();
        self.lp.process(&self.mid, &mut self.post);
        for bp in self.bandpass.iter_mut() {
            bp.process(&mut self.post);
        }
        match self.nr.as_mut() {
            Some(nr) => nr.process(&self.post, out),
            None => out.extend_from_slice(&self.post),
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

    /// AFC correction (Hz from the dial frequency) and whether it is locked
    /// on, when AFC is on.
    pub fn afc(&self) -> Option<(f64, bool)> {
        self.afc.as_ref().map(|a| (a.offset_hz, a.locked))
    }

    /// Start AFC from a correction found earlier (e.g. by the demodulator this
    /// one replaces), so changing squelch or retuning the dongle keeps the lock.
    pub fn set_afc(&mut self, offset_hz: f64) {
        if let Some(a) = self.afc.as_mut() {
            a.set(&mut self.ddc, offset_hz);
        }
    }

    /// Morse decoded since the last call.
    pub fn take_cw(&mut self) -> Option<CwText> {
        self.cw.as_mut()?.2.take()
    }

    /// RDS, when something new has been decoded.
    pub fn take_rds(&mut self) -> Option<RdsInfo> {
        self.rds.as_mut()?.take()
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
        let p = Params { freq_hz: (center as f64 + carrier_off) as u64, mode, squelch_db: None, nr: false, cw: false, afc: false };
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
        let p = Params { freq_hz: center + 50_000, mode: Mode::Nfm, squelch_db: Some(-30.0), nr: true, cw: false, afc: false };
        let mut d = Demod::new(&p, center, 1_024_000);
        assert!((d.out_rate - 51_200.0 / 3.0).abs() < 1e-6);
        let quiet = iq(1_024_000, 0.2, |_| (0.0, 0.0));
        let mut out = Vec::new();
        let (level, open) = d.process(&quiet, &mut out);
        assert!(!open && level < -30.0);
        assert!(out.iter().all(|&v| v == 0.0));
    }
    /// cu8 from a phase function evaluated sample by sample (stateful).
    fn iq_fm(rate: u32, secs: f64, mut inst_hz: impl FnMut(f64) -> f64) -> Vec<u8> {
        let n = (rate as f64 * secs) as usize;
        let mut v = Vec::with_capacity(n * 2);
        let mut ph = 0.0f64;
        for i in 0..n {
            ph = (ph + TAU * inst_hz(i as f64 / rate as f64) / rate as f64) % TAU;
            v.push(to_cu8((0.4 * ph.cos()) as f32));
            v.push(to_cu8((0.4 * ph.sin()) as f32));
        }
        v
    }

    #[test]
    fn cw_mode_gives_a_700_hz_note() {
        let off = 30_000.0;
        let (out, r, _) = run(Mode::Cw, off, 1_024_000, |t| {
            let ph = TAU * off * t;
            (0.3 * ph.cos(), 0.3 * ph.sin())
        });
        assert!((tone_hz(&out, r) - CW_PITCH_HZ).abs() < 15.0, "{}", tone_hz(&out, r));
    }

    #[test]
    fn reads_a_beacon_in_cw_mode_after_a_long_gap() {
        // Like the simulated beacon: hard-keyed carrier, the message twice
        // with six seconds of hiss between.
        let rate = 1_024_000u32;
        let center = 144_000_000u64;
        let off = 470_000.0 - 400_000.0;
        let (keys, total) = crate::audio::cw::keying("VVV DE GB3SIM IO91", 16.0);
        let period = total + 6.0;
        let n = (rate as f64 * (2.0 * period + 0.5)) as usize;
        let mut rng = 0x2545_F491_4F6C_DD1Du64;
        let mut noise = || {
            let mut s = 0.0;
            for _ in 0..4 {
                rng ^= rng >> 12;
                rng ^= rng << 25;
                rng ^= rng >> 27;
                s += (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64;
            }
            (s - 2.0) * 1.732 * 0.02
        };
        let mut data = Vec::with_capacity(2 * n);
        let mut k = 0;
        for i in 0..n {
            let t = i as f64 / rate as f64;
            let rel = (t - 0.3).rem_euclid(period);
            if rel < 0.001 {
                k = 0;
            }
            while k < keys.len() && keys[k].1 < rel {
                k += 1;
            }
            let on = keys.get(k).is_some_and(|&(a, b)| rel >= a && rel < b);
            let ph = TAU * off * t;
            let a = if on { 0.08 } else { 0.0 };
            data.push(to_cu8((a * ph.cos() + noise()) as f32));
            data.push(to_cu8((a * ph.sin() + noise()) as f32));
        }
        let p = Params { freq_hz: center + off as u64, mode: Mode::Cw, squelch_db: None, nr: false, cw: true, afc: false };
        let mut d = Demod::new(&p, center, rate);
        let (mut out, mut text) = (Vec::new(), String::new());
        for chunk in data.chunks(2 * 51_200) {
            d.process(chunk, &mut out);
            if let Some(t) = d.take_cw() {
                text.push_str(&t.text);
            }
        }
        assert_eq!(text, "VVV DE GB3SIM IO91 VVV DE GB3SIM IO91");
    }

    #[test]
    fn reads_a_repeater_ident_in_nfm() {
        let rate = 1_024_000;
        let center = 145_000_000u64;
        let off = 125_000.0;
        let (keys, total) = crate::audio::cw::keying("GB3XX QRV", 18.0);
        let mut k = 0;
        let mut tone_ph = 0.0f64;
        // Carrier on throughout; a 1 kHz tone at 2.5 kHz deviation keyed with the ident.
        let data = iq_fm(rate, total + 0.6, |t| {
            let t0 = t - 0.3;
            while k < keys.len() && keys[k].1 < t0 {
                k += 1;
            }
            let on = keys.get(k).is_some_and(|&(a, b)| t0 >= a && t0 < b);
            tone_ph = (tone_ph + TAU * 1000.0 / rate as f64) % TAU;
            off + if on { 2_500.0 * tone_ph.sin() } else { 0.0 }
        });
        let p = Params { freq_hz: center + off as u64, mode: Mode::Nfm, squelch_db: None, nr: false, cw: true, afc: false };
        let mut d = Demod::new(&p, center, rate);
        let (mut out, mut text) = (Vec::new(), String::new());
        for chunk in data.chunks(2 * 51_200) {
            d.process(chunk, &mut out);
            if let Some(t) = d.take_cw() {
                text.push_str(&t.text);
            }
        }
        assert!(text.contains("GB3XX QRV"), "{text:?}");
    }

    #[test]
    fn wfm_decodes_rds() {
        use crate::audio::rds::{encode, BIT_RATE, SUBCARRIER_HZ};
        let rate = 2_048_000;
        let center = 98_000_000u64;
        let off = 300_000.0;
        let coded = encode::differential(&encode::groups(0xC5A1, 4, "SIM FM", "Hello from the test"));
        let data = iq_fm(rate, 3.5, |t| {
            let mpx = 0.45 * (TAU * 1000.0 * t).sin()
                + 0.09 * (TAU * 19_000.0 * t).sin()
                + 0.04 * encode::baseband(&coded, t, BIT_RATE) * (TAU * SUBCARRIER_HZ * t).sin();
            off + 75_000.0 * mpx
        });
        let p = Params { freq_hz: center + off as u64, mode: Mode::Wfm, squelch_db: None, nr: false, cw: false, afc: false };
        let mut d = Demod::new(&p, center, rate);
        let mut out = Vec::new();
        let mut last = None;
        for chunk in data.chunks(2 * 102_400) {
            d.process(chunk, &mut out);
            if let Some(r) = d.take_rds() {
                last = Some(r);
            }
        }
        let r = last.expect("no RDS decoded");
        assert_eq!(r.pi.as_deref(), Some("C5A1"));
        assert_eq!(r.ps.as_deref(), Some("SIM FM  "));
        assert_eq!(r.pty, Some(4));
        assert_eq!(r.rt.as_deref(), Some("Hello from the test"));
    }

    /// cu8 of a carrier whose centre (Hz from the capture centre) follows
    /// `centre(t)`, FM-modulated by a 1 kHz tone at 3 kHz deviation, plus
    /// white noise of `sigma` per component.
    fn drifting_fm(rate: u32, secs: f64, amp: f64, sigma: f64, centre: impl Fn(f64) -> f64) -> Vec<u8> {
        let n = (rate as f64 * secs) as usize;
        let mut v = Vec::with_capacity(n * 2);
        let mut ph = 0.0f64;
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        let mut gauss = || {
            let mut s = 0.0;
            for _ in 0..4 {
                rng ^= rng >> 12;
                rng ^= rng << 25;
                rng ^= rng >> 27;
                s += (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64;
            }
            (s - 2.0) * 1.732
        };
        for i in 0..n {
            let t = i as f64 / rate as f64;
            let f = centre(t) + 3000.0 * (TAU * 1000.0 * t).cos();
            ph = (ph + TAU * f / rate as f64) % TAU;
            v.push(to_cu8((amp * ph.cos() + sigma * gauss()) as f32));
            v.push(to_cu8((amp * ph.sin() + sigma * gauss()) as f32));
        }
        v
    }

    fn afc_params(center: u64, off: f64, mode: Mode) -> Params {
        Params { freq_hz: (center as f64 + off) as u64, mode, squelch_db: None, nr: false, cw: false, afc: true }
    }

    #[test]
    fn afc_follows_a_wandering_fm_signal() {
        let (rate, center, dial) = (2_048_000u32, 100_000_000u64, 200_000.0);
        // Swings ±20 kHz either side of the dial, once every two seconds.
        let drift = |t: f64| 20_000.0 * (TAU * 0.5 * t).sin();
        let data = drifting_fm(rate, 2.0, 0.3, 0.05, |t| dial + drift(t));
        let mut d = Demod::new(&afc_params(center, dial, Mode::Nfm), center, rate);
        let (mut out, mut worst) = (Vec::new(), 0.0f64);
        for (i, chunk) in data.chunks(2 * 102_400).enumerate() {
            d.process(chunk, &mut out);
            let (off, locked) = d.afc().unwrap();
            let t = (i + 1) as f64 * 0.05;
            if t > 0.2 {
                assert!(locked, "lost lock at {t:.2} s");
                worst = worst.max((off - drift(t)).abs());
            }
        }
        assert!(worst < 2_500.0, "tracking error up to {worst:.0} Hz");
        // Still a clean 1 kHz tone at the end.
        let r = d.out_rate;
        let tail = &out[out.len() - (r * 0.5) as usize..];
        assert!((tone_hz(tail, r) - 1000.0).abs() < 30.0, "{}", tone_hz(tail, r));
    }

    #[test]
    fn afc_pulls_in_an_off_channel_carrier_and_stays_in_range() {
        let (rate, center, dial) = (1_024_000u32, 100_000_000u64, -100_000.0);
        let data = drifting_fm(rate, 0.5, 0.3, 0.02, |_| dial + 9_000.0);
        let mut d = Demod::new(&afc_params(center, dial, Mode::Nfm), center, rate);
        let mut out = Vec::new();
        for chunk in data.chunks(2 * 51_200) {
            d.process(chunk, &mut out);
        }
        let (off, locked) = d.afc().unwrap();
        assert!(locked && (off - 9_000.0).abs() < 200.0, "{off} {locked}");
        // AM may only move 10 kHz.
        let data = drifting_fm(rate, 0.5, 0.3, 0.02, |_| dial + 15_000.0);
        let mut d = Demod::new(&afc_params(center, dial, Mode::Am), center, rate);
        for chunk in data.chunks(2 * 51_200) {
            d.process(chunk, &mut out);
        }
        assert_eq!(d.afc().unwrap().0, 10_000.0);
    }

    #[test]
    fn afc_holds_still_on_noise() {
        let (rate, center) = (2_048_000u32, 100_000_000u64);
        let data = drifting_fm(rate, 1.0, 0.0, 0.15, |_| 0.0);
        let mut d = Demod::new(&afc_params(center, 300_000.0, Mode::Nfm), center, rate);
        let mut out = Vec::new();
        for chunk in data.chunks(2 * 102_400) {
            d.process(chunk, &mut out);
            let (off, locked) = d.afc().unwrap();
            assert!(!locked && off == 0.0, "{off} {locked}");
        }
    }

    #[test]
    fn afc_is_only_for_nfm_and_am() {
        let p = Params { afc: true, ..afc_params(100_000_000, 0.0, Mode::Usb) };
        assert!(Demod::new(&p, 100_000_000, 1_024_000).afc().is_none());
        let p = Params { afc: false, ..afc_params(100_000_000, 0.0, Mode::Nfm) };
        assert!(Demod::new(&p, 100_000_000, 1_024_000).afc().is_none());
    }
}
