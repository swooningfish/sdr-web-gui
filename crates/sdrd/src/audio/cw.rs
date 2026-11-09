//! Morse (CW) decoder for live audio, plus the keying generator used by the
//! tests and the simulated receiver.
//!
//! Works on demodulated audio, so it reads both a CW signal heard through the
//! CW/SSB demodulator and the tone ident an FM repeater sends at the end of an
//! over (heard through NFM).
//!
//! - Short FFTs (~15 ms, hop ~3.75 ms) over 300–1500 Hz.
//! - Tone lock: each hop, the strongest bin scores a point if it stands well
//!   clear of the band's median (a tone, not voice or hiss); scores fade over
//!   about a quarter of a second and the best-scoring bin is the tone we follow.
//! - Keying: the tone's level against a noise floor and peak tracker, with
//!   hysteresis and a two-hop debounce.
//! - When the lock moves to a new tone, the last 3 s are decoded again on
//!   it, so the first letters of an ident aren't lost.
//! - Words are shown once complete; ones made only of E/T/I or the same
//!   letter repeated (what speech and hiss produce) are dropped.
//! - Timing: the dot length is fitted to the recent marks (each a dot or a
//!   dash of three dots), so 5–40 WPM work without setting the speed
//!   (letter gap 3 dots, word gap 7).

use std::collections::VecDeque;
use std::sync::Arc;

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

const MIN_TONE_HZ: f64 = 300.0;
const MAX_TONE_HZ: f64 = 1_500.0;
/// A bin this far above the band's median power scores as a tone (dB).
const TONAL_DB: f32 = 12.0;
/// The followed tone must stand this far above the median to key down, and
/// this far to stay down (dB). Hiss rarely does either for long.
const KEY_DOWN_DB: f32 = 8.0;
const KEY_HOLD_DB: f32 = 5.0;
/// Key level must be this far above the noise floor to count as a signal (dB).
const MIN_SNR_DB: f32 = 8.0;

const MORSE: &[(&str, char)] = &[
    (".-", 'A'), ("-...", 'B'), ("-.-.", 'C'), ("-..", 'D'), (".", 'E'), ("..-.", 'F'),
    ("--.", 'G'), ("....", 'H'), ("..", 'I'), (".---", 'J'), ("-.-", 'K'), (".-..", 'L'),
    ("--", 'M'), ("-.", 'N'), ("---", 'O'), (".--.", 'P'), ("--.-", 'Q'), (".-.", 'R'),
    ("...", 'S'), ("-", 'T'), ("..-", 'U'), ("...-", 'V'), (".--", 'W'), ("-..-", 'X'),
    ("-.--", 'Y'), ("--..", 'Z'),
    ("-----", '0'), (".----", '1'), ("..---", '2'), ("...--", '3'), ("....-", '4'),
    (".....", '5'), ("-....", '6'), ("--...", '7'), ("---..", '8'), ("----.", '9'),
    (".-.-.-", '.'), ("--..--", ','), ("..--..", '?'), ("-..-.", '/'), ("-...-", '='),
    ("-....-", '-'), (".-.-.", '+'), (".--.-.", '@'), (".----.", '\''), ("-.--.", '('),
    ("-.--.-", ')'), ("---...", ':'), ("-.-.-.", ';'), (".-..-.", '"'), ("-.-.--", '!'),
];

fn decode_symbol(s: &str) -> Option<char> {
    MORSE.iter().find(|(code, _)| *code == s).map(|(_, c)| *c)
}

fn encode_char(c: char) -> Option<&'static str> {
    let c = c.to_ascii_uppercase();
    MORSE.iter().find(|(_, ch)| *ch == c).map(|(code, _)| *code)
}

/// Whether a decoded word looks like Morse rather than speech or noise
/// keying the decoder: those come out as runs of one-element letters (E, T,
/// I), lone letters, or the same letter over and over.
fn plausible(w: &str) -> bool {
    if w.is_empty() || w.chars().all(|c| matches!(c, 'E' | 'T' | 'I')) {
        return false;
    }
    if w.len() == 1 {
        // Single letters that mean something on their own ("K" = over).
        return w.chars().all(|c| c.is_ascii_digit() || matches!(c, 'K' | 'R' | '?'));
    }
    let b = w.as_bytes();
    !b.windows(4).any(|x| x.iter().all(|&c| c == x[0]))
}

/// Key-down intervals `(start, end)` in seconds for `text` at `wpm` (PARIS
/// timing: one dot = 1.2 / wpm s), and the total length including a trailing
/// word gap.
pub fn keying(text: &str, wpm: f64) -> (Vec<(f64, f64)>, f64) {
    let dot = 1.2 / wpm;
    let mut t = 0.0;
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        for (ci, c) in word.chars().enumerate() {
            let Some(code) = encode_char(c) else { continue };
            if ci > 0 {
                t += 2.0 * dot; // letter gap is 3 dots; 1 already added
            }
            for el in code.chars() {
                let len = if el == '-' { 3.0 * dot } else { dot };
                out.push((t, t + len));
                t += len + dot;
            }
        }
        t += 6.0 * dot; // word gap 7 dots
    }
    (out, t)
}

/// Text decoded since the last call, and the estimated speed.
pub struct CwText {
    pub text: String,
    pub wpm: f32,
}

pub struct CwDecoder {
    n: usize,
    hop: usize,
    hop_secs: f64,
    fft: Arc<dyn Fft<f32>>,
    win: Vec<f32>,
    input: Vec<f32>,
    buf: Vec<Complex32>,
    scratch: Vec<Complex32>,
    lo_bin: usize,
    hi_bin: usize,
    power: Vec<f32>,
    score: Vec<f32>,
    tone: Option<usize>,
    /// The last 3 s of band powers (bins lo-1..=hi+1) and medians,
    /// replayed when the tone lock moves so the first letters aren't lost.
    history: VecDeque<(Vec<f32>, f32)>,
    history_len: usize,
    /// Hops still to skip while the filters ahead of us settle.
    warmup: usize,
    noise_db: f32,
    peak_db: f32,
    /// Debounced key state and how long it has lasted (s).
    key: bool,
    run: f64,
    pending: Option<(bool, u8)>,
    dot: f64,
    marks: VecDeque<f64>,
    symbol: String,
    /// Letters of the word being sent; shown once the word ends.
    word: String,
    words: usize,
    out: String,
}

impl CwDecoder {
    pub fn new(rate: f64) -> Self {
        let n = ((rate * 0.015) as usize).next_power_of_two().max(64);
        let hop = n / 4;
        let mut planner = FftPlanner::new();
        let win = (0..n)
            .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()) as f32)
            .collect();
        let bin_hz = rate / n as f64;
        let lo_bin = (MIN_TONE_HZ / bin_hz).ceil().max(3.0) as usize;
        let hi_bin = ((MAX_TONE_HZ / bin_hz).ceil() as usize).min(n / 2 - 3);
        let hop_secs = hop as f64 / rate;
        CwDecoder {
            n,
            hop,
            hop_secs,
            fft: planner.plan_fft_forward(n),
            win,
            input: Vec::with_capacity(2 * n),
            buf: vec![Complex32::default(); n],
            scratch: Vec::new(),
            lo_bin,
            hi_bin,
            power: vec![0.0; n / 2],
            score: vec![0.0; n / 2],
            tone: None,
            history: VecDeque::new(),
            history_len: (3.0 / hop_secs) as usize,
            warmup: (0.25 / hop_secs) as usize,
            noise_db: f32::NAN,
            peak_db: -200.0,
            key: false,
            run: 0.0,
            pending: None,
            dot: 0.06, // 20 WPM to start with
            marks: VecDeque::new(),
            symbol: String::new(),
            word: String::new(),
            words: 0,
            out: String::new(),
        }
    }

    pub fn wpm(&self) -> f32 {
        (1.2 / self.dot) as f32
    }

    /// Feed audio; decoded words collect until `take`.
    pub fn process(&mut self, audio: &[f32]) {
        self.input.extend_from_slice(audio);
        while self.input.len() >= self.n {
            self.analyse();
            self.input.drain(..self.hop);
        }
    }

    /// Text decoded since the last call: whole words, each after the first
    /// with a leading space.
    pub fn take(&mut self) -> Option<CwText> {
        if self.out.is_empty() {
            return None;
        }
        Some(CwText { text: std::mem::take(&mut self.out), wpm: self.wpm() })
    }

    fn analyse(&mut self) {
        if self.warmup > 0 {
            self.warmup -= 1;
            return;
        }
        for (i, b) in self.buf.iter_mut().enumerate() {
            *b = Complex32::new(self.input[i] * self.win[i], 0.0);
        }
        self.scratch.resize(self.fft.get_inplace_scratch_len(), Complex32::default());
        self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);
        for k in self.lo_bin - 2..=self.hi_bin + 2 {
            self.power[k] = self.buf[k].norm_sqr();
        }

        // Tone lock.
        let band = &self.power[self.lo_bin..=self.hi_bin];
        let mut sorted: Vec<f32> = band.to_vec();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let median = sorted[sorted.len() / 2].max(1e-12);
        let (kmax, pmax) = band
            .iter()
            .enumerate()
            .fold((0, 0.0f32), |acc, (i, &p)| if p > acc.1 { (i, p) } else { acc });
        let kmax = kmax + self.lo_bin;
        for s in self.score.iter_mut() {
            *s *= 0.985;
        }
        // A tone is a narrow peak: well above the bins two either side (a
        // Hann-windowed tone is down 6 dB one bin away and far more at two).
        // A click or the DC blocker settling makes a smooth slope instead.
        let peak = pmax > 4.0 * self.power[kmax - 2].max(self.power[kmax + 2]);
        if peak && 10.0 * (pmax / median).log10() > TONAL_DB {
            self.score[kmax] += 1.0;
        }
        let smoothed = |k: usize| self.score[k - 1] * 0.5 + self.score[k] + self.score[k + 1] * 0.5;
        let best = (self.lo_bin..=self.hi_bin).max_by(|&a, &b| smoothed(a).total_cmp(&smoothed(b))).unwrap();
        let tone = match self.tone {
            Some(t) if smoothed(best) < 1.5 * smoothed(t) => Some(t),
            _ if smoothed(best) >= 3.0 => Some(best),
            other => other,
        };

        let snapshot = self.power[self.lo_bin - 1..=self.hi_bin + 1].to_vec();
        if self.history.len() == self.history_len {
            self.history.pop_front();
        }
        self.history.push_back((snapshot, median));

        match (self.tone, tone) {
            (_, None) => {}
            // Same tone, or one sitting between two bins hopping across.
            (Some(old), Some(new)) if old.abs_diff(new) <= 1 => {
                self.tone = Some(new);
                let (p, m) = self.history.back().cloned().unwrap();
                self.key_hop(new, &p, m);
            }
            (_, Some(new)) => {
                // A new tone: decode the last few seconds again on it.
                self.tone = Some(new);
                self.reset_keying();
                let hist = std::mem::take(&mut self.history);
                for (p, m) in &hist {
                    self.key_hop(new, p, *m);
                }
                self.history = hist;
            }
        }
    }

    fn reset_keying(&mut self) {
        self.noise_db = f32::NAN;
        self.peak_db = -200.0;
        self.key = false;
        self.run = 0.0;
        self.pending = None;
        // Whatever was keying the old tone (often speech) says nothing about
        // this one's speed.
        self.dot = 0.06;
        self.marks.clear();
        self.symbol.clear();
        self.word.clear();
    }

    /// Key state for one hop, from the powers around tone bin `t`.
    fn key_hop(&mut self, t: usize, p: &[f32], median: f32) {
        let i = t - (self.lo_bin - 1);
        // The tone bin and its neighbours (a detuned tone spreads over two).
        let level = 10.0 * (p[i - 1] + p[i] + p[i + 1] + 1e-12).log10();
        let tonal = 10.0 * (p[i].max(p[i - 1]).max(p[i + 1]) / median).log10();
        if self.noise_db.is_nan() || level < self.noise_db {
            self.noise_db = level;
        } else {
            // Rise quickly in the gaps (the floor changes when a carrier comes
            // or goes) but hardly at all under a long dash.
            let rate = if self.key { 0.0005 } else { 0.02 };
            self.noise_db += rate * (level - self.noise_db);
        }
        if level > self.peak_db {
            self.peak_db = level;
        } else {
            self.peak_db = (self.peak_db - 0.02).max(self.noise_db);
        }
        let mid = self.noise_db + (self.peak_db - self.noise_db) / 2.0;
        let raw = if self.key {
            tonal > KEY_HOLD_DB && level > mid - 1.5
        } else {
            tonal > KEY_DOWN_DB && level > mid + 1.5
        } && self.peak_db - self.noise_db > MIN_SNR_DB;
        self.step(raw);
    }

    /// One hop of (raw) key state: debounce, then time the marks and spaces.
    fn step(&mut self, raw: bool) {
        self.run += self.hop_secs;
        if raw == self.key {
            self.pending = None;
        } else {
            let n = match self.pending {
                Some((s, n)) if s == raw => n + 1,
                _ => 1,
            };
            if n < 2 {
                self.pending = Some((raw, n));
            } else {
                // The change started a hop ago.
                let len = self.run - self.hop_secs;
                self.pending = None;
                if self.key {
                    self.mark(len);
                }
                self.key = raw;
                self.run = self.hop_secs;
            }
        }
        // Finish a letter, then the word, while the key stays up.
        if !self.key {
            if self.run > 2.0 * self.dot && !self.symbol.is_empty() {
                self.letter();
            }
            if self.run > 5.0 * self.dot && !self.word.is_empty() {
                self.end_word();
            }
        }
    }

    fn mark(&mut self, len: f64) {
        // A click, not an element (a dot at 60 WPM is 20 ms).
        if len < 0.3 * self.dot || len < 0.02 {
            return;
        }
        if self.marks.len() == 24 {
            self.marks.pop_front();
        }
        self.marks.push_back(len);
        self.estimate_dot();
        self.symbol.push(if len > 2.0 * self.dot { '-' } else { '.' });
        if self.symbol.len() > 7 {
            self.symbol.clear(); // not Morse
        }
    }

    /// Dot length that best explains the recent marks as dots (1) or dashes
    /// (3), in the log domain. Each mark's misfit is capped, so a stray click
    /// can't drag the estimate; ties (all marks alike) go to the candidate
    /// nearest the previous estimate.
    fn estimate_dot(&mut self) {
        let prev = self.dot.ln();
        let logs: Vec<f64> = self.marks.iter().map(|m| m.ln()).collect();
        let mut best = (f64::MAX, self.dot);
        let mut d: f64 = 0.025;
        while d <= 0.25 {
            let (ld, l3) = (d.ln(), (3.0 * d).ln());
            let cost: f64 = logs.iter().map(|&l| (l - ld).abs().min((l - l3).abs()).min(0.5)).sum::<f64>()
                + 0.05 * (ld - prev).abs();
            if cost < best.0 {
                best = (cost, d);
            }
            d *= 1.02;
        }
        self.dot = best.1;
    }

    fn letter(&mut self) {
        if let Some(c) = decode_symbol(&self.symbol) {
            self.word.push(c);
            if self.word.len() >= 24 {
                self.end_word();
            }
        }
        self.symbol.clear();
    }

    fn end_word(&mut self) {
        let w = std::mem::take(&mut self.word);
        if plausible(&w) {
            if self.words > 0 {
                self.out.push(' ');
            }
            self.out.push_str(&w);
            self.words += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// A keyed tone with 5 ms raised-cosine edges plus white noise.
    pub(crate) fn morse_audio(text: &str, wpm: f64, tone: f64, rate: f64, noise: f64) -> Vec<f32> {
        let (keys, total) = keying(text, wpm);
        let n = ((total + 0.5) * rate) as usize;
        let mut rng = 0x1234_5678_9abc_def0u64;
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
        let edge = 0.005;
        let mut k = 0;
        (0..n)
            .map(|i| {
                let t = i as f64 / rate - 0.25;
                while k < keys.len() && keys[k].1 + edge < t {
                    k += 1;
                }
                let env = keys.get(k).map_or(0.0, |&(a, b)| {
                    if t < a || t > b + edge {
                        0.0
                    } else if t < a + edge {
                        0.5 - 0.5 * (std::f64::consts::PI * (t - a) / edge).cos()
                    } else if t > b {
                        0.5 + 0.5 * (std::f64::consts::PI * (t - b) / edge).cos()
                    } else {
                        1.0
                    }
                });
                (0.3 * env * (TAU * tone * i as f64 / rate).sin() + noise * gauss()) as f32
            })
            .collect()
    }

    fn decode(audio: &[f32], rate: f64) -> (String, f32) {
        let mut d = CwDecoder::new(rate);
        let mut text = String::new();
        for chunk in audio.chunks(853) {
            d.process(chunk);
            if let Some(t) = d.take() {
                text.push_str(&t.text);
            }
        }
        (text, d.wpm())
    }

    #[test]
    fn keying_follows_paris_timing() {
        // PARIS is exactly 50 dots including the word gap.
        let (_, total) = keying("PARIS", 20.0);
        assert!((total - 50.0 * 0.06).abs() < 1e-9, "{total}");
    }

    #[test]
    fn decodes_clean_morse_at_several_speeds() {
        let rate = 17_066.67;
        for wpm in [12.0, 20.0, 30.0] {
            let audio = morse_audio("CQ CQ DE M0ABC M0ABC K", wpm, 700.0, rate, 0.01);
            let (text, est) = decode(&audio, rate);
            assert!(text.contains("DE M0ABC M0ABC K"), "{wpm} WPM: {text:?}");
            assert!((est as f64 - wpm).abs() < wpm * 0.2, "{wpm} WPM estimated as {est}");
        }
    }

    #[test]
    fn decodes_noisy_off_tune_morse() {
        let rate = 17_066.67;
        // About 3 dB SNR in a 3 kHz bandwidth (the noise you'd hear on
        // SSB), tone well off the usual 700 Hz.
        let audio = morse_audio("TEST DE G4XYZ 599 TU", 18.0, 1_020.0, rate, 0.22);
        let (text, _) = decode(&audio, rate);
        assert!(text.contains("G4XYZ 599 TU"), "{text:?}");
    }

    #[test]
    fn noise_alone_decodes_nothing() {
        let rate = 17_066.67;
        let audio = morse_audio("", 20.0, 700.0, rate, 0.1);
        let audio: Vec<f32> = audio.iter().cycle().take(rate as usize * 4).copied().collect();
        let (text, _) = decode(&audio, rate);
        assert!(text.trim().len() <= 1, "{text:?}");
    }

    #[test]
    fn speech_then_repeater_ident() {
        // Four seconds of "speech" (two gliding tones with a syllable
        // rhythm), then the ident on a 1 kHz tone: no junk, whole callsign.
        let rate = 17_066.67;
        let mut audio: Vec<f32> = (0..(4.0 * rate) as usize)
            .scan((0.0f64, 0.0f64), |(p1, p2), i| {
                let t = i as f64 / rate;
                let f1 = 220.0 + 90.0 * (TAU * 0.6 * t).sin();
                let f2 = 2.7 * f1 + 300.0 * (TAU * 0.23 * t).sin();
                *p1 = (*p1 + TAU * f1 / rate) % TAU;
                *p2 = (*p2 + TAU * f2 / rate) % TAU;
                let env = (0.5 + 0.5 * (TAU * 3.1 * t).sin()).powi(2);
                Some((0.4 * env * (0.7 * p1.sin() + 0.3 * p2.sin())) as f32)
            })
            .collect();
        audio.extend(morse_audio("GB3SR", 18.0, 1_000.0, rate, 0.01));
        let (text, _) = decode(&audio, rate);
        assert_eq!(text, "GB3SR");
    }

    #[test]
    fn junk_words_are_dropped() {
        assert!(!plausible("E"));
        assert!(!plausible("ETIE"));
        assert!(!plausible("TTTTTO"));
        assert!(plausible("K"));
        assert!(!plausible("W"));
        assert!(plausible("TEST"));
        assert!(plausible("599"));
    }
}
