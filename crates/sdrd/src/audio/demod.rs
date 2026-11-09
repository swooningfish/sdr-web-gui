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
        let p = Params { freq_hz: (center as f64 + carrier_off) as u64, mode, squelch_db: None, nr: false, cw: false };
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
        let p = Params { freq_hz: center + 50_000, mode: Mode::Nfm, squelch_db: Some(-30.0), nr: true, cw: false };
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
        let p = Params { freq_hz: center + off as u64, mode: Mode::Cw, squelch_db: None, nr: false, cw: true };
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
        let p = Params { freq_hz: center + off as u64, mode: Mode::Nfm, squelch_db: None, nr: false, cw: true };
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
        let p = Params { freq_hz: center + off as u64, mode: Mode::Wfm, squelch_db: None, nr: false, cw: false };
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
}
