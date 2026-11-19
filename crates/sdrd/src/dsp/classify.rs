//! Signal classifier (M9): finds the signals in the main receiver's capture
//! and labels each one as a carrier, CW, voice, digital voice, data, broadcast
//! FM or OFDM (DAB / DVB-T and similar), for labels on the waterfall and the
//! spectrum's "what's this?" readout.
//!
//! It works from short spectra rather than demodulating anything, so one
//! pass covers every signal in the capture for little CPU:
//!
//! 1. **Frames.** 20 short spectra a second (Hann window, bins about 250 Hz
//!    wide). Only the first few FFTs of each frame period are computed; the
//!    rest of the samples are skipped, which keeps it cheap on a small PC.
//! 2. **Detection.** Every second, the last 3 s of frames are averaged and
//!    max-held. Bins well above the noise floor (estimated per region, so
//!    the dongle's roll-off and a wide multiplex don't fool it) are grouped
//!    into signals.
//! 3. **Features.** Per signal: occupied bandwidth, SNR, duty cycle, on/off
//!    keying rate, level steadiness, how flat its top is, how much its shape
//!    changes from frame to frame, drift, and how long its transmissions last.
//! 4. **Scoring.** A small hand-set model (soft rules per class, normalised
//!    to shares) turns the features into class scores. These are kept per
//!    signal over time, so a label settles and doesn't flicker; bursty
//!    signals (packet, telemetry) are remembered for a while after they stop.
//!
//! The browser adds context the server doesn't have (band plan segments and
//! bookmarks near the signal) on top of these scores; see
//! `web/src/lib/signals.ts`.
//!
//! Signals are sent to WebSocket clients as JSON (`{"type":"signals",...}`),
//! about once a second while anyone is watching.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use serde::Serialize;
use tokio::sync::{broadcast, watch};

use crate::radio::IqBlock;

/// Frames per second.
const FPS: u32 = 20;
/// At most this many FFTs are averaged per frame; the rest are skipped.
const FFTS_PER_FRAME: u32 = 3;
/// Frames looked at in each analysis (3 s).
const WINDOW: usize = 60;
/// Analyse every this many frames (1 s).
const EVERY: usize = 20;
/// Don't analyse with fewer frames than this after a retune.
const MIN_FRAMES: usize = 20;
/// Bins this wide, roughly.
const TARGET_BIN_HZ: f64 = 250.0;
/// Share of the capture at each edge that isn't used (anti-alias roll-off).
const EDGE: f64 = 0.03;
/// A signal: its average this far above the floor (dB) ...
const MEAN_DB: f32 = 5.0;
/// ... or its max-hold this far.
const MAX_DB: f32 = 12.0;
/// Bursty signals are remembered this long after last heard (ms) ...
const HOLD_BURSTY_MS: u64 = 120_000;
/// ... others this long.
const HOLD_MS: u64 = 15_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Unmodulated carrier (beacon, birdie, spur, idle repeater).
    Carrier,
    /// On/off keyed carrier: Morse.
    Cw,
    /// Analogue voice: NFM, AM or SSB.
    Voice,
    /// Digital voice: DMR, D-STAR, M17, Fusion, dPMR, ...
    DigitalVoice,
    /// Bursty data: packet / APRS, telemetry, ISM sensors, pagers, LoRa.
    Data,
    /// Wideband FM broadcast.
    BroadcastFm,
    /// Wide, flat and continuous: DAB, DVB-T, LTE and other OFDM.
    Ofdm,
}

pub const CLASSES: [Class; 7] = [
    Class::Carrier,
    Class::Cw,
    Class::Voice,
    Class::DigitalVoice,
    Class::Data,
    Class::BroadcastFm,
    Class::Ofdm,
];
const N: usize = CLASSES.len();

/// What one analysis window measured for a signal (plus its history).
#[derive(Clone, Debug, Default, Serialize)]
pub struct Features {
    /// Occupied bandwidth (Hz), between the points 26 dB below the peak.
    pub bw_hz: f32,
    /// Peak above the noise floor while on (dB, per bin).
    pub snr_db: f32,
    /// Share of the window it was on.
    pub duty: f32,
    /// On/off changes per second.
    pub keying: f32,
    /// Spread of its level while on (dB).
    pub level_sd_db: f32,
    /// Ripple across the middle of its spectrum (dB); small = flat top.
    /// None for signals too narrow to tell.
    pub ripple_db: Option<f32>,
    /// How much its width changes from frame to frame (0 = not at all).
    pub shape_var: f32,
    /// Spread of its centre over the window (Hz).
    pub drift_hz: f32,
    /// Typical length of its completed transmissions (s), if any were seen.
    pub burst_s: Option<f32>,
    /// Longest it has been on in one go (s), so far.
    pub longest_s: f32,
    /// A stronger signal it keys on and off with (Hz): it's probably a spur
    /// or harmonic of that one.
    pub keyed_with_hz: Option<u64>,
}

/// A signal as sent to the browser.
#[derive(Clone, Debug, Serialize)]
pub struct Signal {
    pub id: u32,
    /// Centre (Hz).
    pub hz: u64,
    pub lo_hz: u64,
    pub hi_hz: u64,
    /// On in the last analysis.
    pub active: bool,
    pub first_ms: u64,
    pub last_ms: u64,
    /// Best class and its share of the scores (0–1), or None when nothing
    /// fits well.
    pub class: Option<Class>,
    pub confidence: f32,
    /// Share per class, in [`CLASSES`] order (sums to 1).
    pub scores: [f32; N],
    pub features: Features,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub center_hz: u64,
    pub sample_rate: u32,
    pub bin_hz: f32,
    pub ts: u64,
    pub classes: [Class; N],
    pub signals: Vec<Signal>,
}

// -------------------------------------------------------------- the model

/// 1 at or below `a`, 0 at or above `b`.
fn below(x: f32, a: f32, b: f32) -> f32 {
    ((b - x) / (b - a)).clamp(0.0, 1.0)
}
/// 0 at or below `a`, 1 at or above `b`.
fn above(x: f32, a: f32, b: f32) -> f32 {
    ((x - a) / (b - a)).clamp(0.0, 1.0)
}
/// 1 between `b` and `c`, falling to 0 at `a` and `d`.
fn between(x: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    above(x, a, b).min(below(x, c, d))
}
/// A condition that matters with weight `1 - floor`: never quite zero, so
/// one feature that's off doesn't rule a class out completely.
fn soft(cond: f32, floor: f32) -> f32 {
    floor + (1.0 - floor) * cond
}

/// Raw score per class: how well the features fit each (0–1, unnormalised).
pub fn score(f: &Features) -> [f32; N] {
    let bw = f.bw_hz;
    let narrow = below(bw, 1_300.0, 2_500.0);
    let steady = above(f.duty, 0.8, 0.95) * below(f.keying, 0.4, 1.2);
    let keyed = above(f.keying, 0.6, 2.0);
    let level_steady = below(f.level_sd_db, 1.5, 4.0);
    let flat = f.ripple_db.map_or(0.3, |r| below(r, 2.5, 5.0));
    let bursts = f.burst_s.map_or(0.0, |b| below(b, 1.5, 3.0));
    let long = above(f.longest_s, 1.5, 3.0);
    let continuous = above(f.duty, 0.7, 0.95);
    let lively = above(f.shape_var, 0.08, 0.25);

    // A narrow line keyed with a much stronger signal: a spur of it, so a
    // carrier rather than Morse or data of its own.
    let spur = if f.keyed_with_hz.is_some() { narrow } else { 0.0 };

    let carrier = soft(narrow, 0.02) * soft(steady.max(spur), 0.05) * soft(level_steady, 0.3);
    // Morse elements are short (a dash at 12 wpm is 0.3 s); steady keying
    // in long blocks is something else (data, or a spur of it).
    let elements = f.burst_s.map_or(0.5, |b| below(b, 0.4, 0.9));
    let cw = soft(narrow, 0.02) * soft(keyed, 0.05) * soft(elements, 0.1) * soft(1.0 - spur, 0.1);
    // Speech changes the spectrum syllable by syllable (FM) or its level
    // (AM, SSB); a steady tone or image (SSTV) doesn't.
    let voice = soft(between(bw, 1_500.0, 2_500.0, 20_000.0, 30_000.0), 0.02)
        * soft(long, 0.2)
        * soft(1.0 - flat, 0.15)
        * soft(lively.max(1.0 - level_steady), 0.1)
        * soft(1.0 - bursts, 0.3);
    let digital_voice = soft(between(bw, 4_000.0, 6_000.0, 14_000.0, 18_000.0), 0.02)
        * soft(flat, 0.1)
        * soft(long, 0.15)
        * soft(level_steady, 0.3)
        * soft(1.0 - lively, 0.3);
    let data = soft(between(bw, 800.0, 1_500.0, 1_000_000.0, 3_000_000.0), 0.05)
        * soft(bursts.max(0.5 * keyed * (1.0 - narrow)), 0.06)
        * soft(1.0 - continuous, 0.3)
        * soft(1.0 - spur, 0.3);
    let broadcast = soft(between(bw, 50_000.0, 90_000.0, 300_000.0, 400_000.0), 0.02)
        * soft(continuous, 0.1)
        * soft(1.0 - flat, 0.4);
    let ofdm = soft(above(bw, 300_000.0, 800_000.0), 0.02) * soft(flat, 0.1) * soft(continuous, 0.1);
    [carrier, cw, voice, digital_voice, data, broadcast, ofdm]
}

/// Scores as shares (sum 1).
fn normalise(s: [f32; N]) -> [f32; N] {
    let sum: f32 = s.iter().sum::<f32>().max(1e-9);
    s.map(|v| v / sum)
}

/// Below this raw score nothing fits well enough to name.
const MIN_FIT: f32 = 0.2;
/// Nor when the best class has less than this share.
const MIN_SHARE: f32 = 0.4;

// ------------------------------------------------------------ the tracker

struct Track {
    id: u32,
    lo_hz: f64,
    hi_hz: f64,
    first_ms: u64,
    last_ms: u64,
    active: bool,
    /// Smoothed shares and raw fit.
    shares: [f32; N],
    fit: f32,
    updates: u32,
    features: Features,
    /// Completed transmissions (s), newest last.
    bursts: VecDeque<f32>,
    longest_s: f32,
}

impl Track {
    fn centre(&self) -> f64 {
        (self.lo_hz + self.hi_hz) / 2.0
    }

    fn overlaps(&self, lo: f64, hi: f64) -> bool {
        // Allow a little slack so a drifting or narrow signal stays the same track.
        let slack = 1_000f64.max(0.1 * (hi - lo));
        lo <= self.hi_hz + slack && hi >= self.lo_hz - slack
    }

    fn best(&self) -> (Option<Class>, f32) {
        let (i, &share) = self
            .shares
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .expect("classes");
        if self.fit < MIN_FIT || share < MIN_SHARE {
            (None, share)
        } else {
            (Some(CLASSES[i]), share)
        }
    }

    fn bursty(&self) -> bool {
        !self.bursts.is_empty() && self.longest_s < 4.0
    }

    fn signal(&self) -> Signal {
        let (class, confidence) = self.best();
        Signal {
            id: self.id,
            hz: self.centre().round() as u64,
            lo_hz: self.lo_hz.round() as u64,
            hi_hz: self.hi_hz.round() as u64,
            active: self.active,
            first_ms: self.first_ms,
            last_ms: self.last_ms,
            class,
            confidence: (confidence * 100.0).round() / 100.0,
            scores: self.shares.map(|v| (v * 1000.0).round() / 1000.0),
            features: self.features.clone(),
        }
    }
}

/// What the detector found for one signal in one window.
struct Detection {
    lo_hz: f64,
    hi_hz: f64,
    features: Features,
    /// Lengths (s) of transmissions that ended in the newest part of the window.
    ended: Vec<f32>,
    /// Its current run if it's on at the end of the window (s).
    ongoing_s: Option<f32>,
    /// When it was on, frame by frame.
    on: Vec<bool>,
}

// ----------------------------------------------------------- the analyser

pub struct Classifier {
    size: usize,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    norm: f32,
    lut: [f32; 256],
    buf: Vec<Complex32>,
    scratch: Vec<Complex32>,
    acc: Vec<f32>,
    acc_n: u32,
    /// Samples into the current frame period, and its length.
    pos: usize,
    period: usize,
    frames: VecDeque<Vec<f32>>,
    since: usize,
    center_hz: u64,
    sample_rate: u32,
    tracks: Vec<Track>,
    next_id: u32,
}

impl Default for Classifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Classifier {
    pub fn new() -> Self {
        Classifier {
            size: 0,
            fft: FftPlanner::new().plan_fft_forward(1),
            window: Vec::new(),
            norm: 1.0,
            lut: std::array::from_fn(|i| (i as f32 - 127.5) / 127.5),
            buf: Vec::new(),
            scratch: Vec::new(),
            acc: Vec::new(),
            acc_n: 0,
            pos: 0,
            period: 1,
            frames: VecDeque::new(),
            since: 0,
            center_hz: 0,
            sample_rate: 0,
            tracks: Vec::new(),
            next_id: 1,
        }
    }

    fn setup(&mut self, center_hz: u64, sample_rate: u32) {
        let size = ((sample_rate as f64 / TARGET_BIN_HZ) as usize).next_power_of_two().clamp(512, 16384);
        if size != self.size {
            self.size = size;
            self.fft = FftPlanner::new().plan_fft_forward(size);
            self.window = (0..size)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / size as f32).cos())
                .collect();
            let sum: f32 = self.window.iter().sum();
            self.norm = sum * sum;
            self.scratch = vec![Complex32::default(); self.fft.get_inplace_scratch_len()];
        }
        self.period = (sample_rate as usize / FPS as usize).max(size);
        self.center_hz = center_hz;
        self.sample_rate = sample_rate;
        self.reset();
        // Tracks outside the new capture can't be followed.
        let (lo, hi) = self.usable();
        self.tracks.retain(|t| t.hi_hz > lo && t.lo_hz < hi);
        for t in &mut self.tracks {
            t.active = false;
        }
    }

    /// Start afresh (after a gap in the samples).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.acc = vec![0.0; self.size];
        self.acc_n = 0;
        self.pos = 0;
        self.frames.clear();
        self.since = 0;
    }

    fn bin_hz(&self) -> f64 {
        self.sample_rate as f64 / self.size as f64
    }

    /// The part of the capture that's analysed (Hz).
    fn usable(&self) -> (f64, f64) {
        let half = self.sample_rate as f64 * (0.5 - EDGE);
        (self.center_hz as f64 - half, self.center_hz as f64 + half)
    }

    /// Feed a block; returns a report each time an analysis is due.
    pub fn process(&mut self, block: &IqBlock, now_ms: u64) -> Option<Report> {
        if block.center_hz != self.center_hz || block.sample_rate != self.sample_rate {
            self.setup(block.center_hz, block.sample_rate);
        }
        let mut report = None;
        let use_len = self.size * FFTS_PER_FRAME as usize;
        for pair in block.data.chunks_exact(2) {
            if self.pos < use_len {
                let i = self.buf.len();
                let w = self.window[i];
                self.buf.push(Complex32::new(self.lut[pair[0] as usize] * w, self.lut[pair[1] as usize] * w));
                if self.buf.len() == self.size {
                    self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);
                    for (a, c) in self.acc.iter_mut().zip(self.buf.iter()) {
                        *a += c.norm_sqr();
                    }
                    self.buf.clear();
                    self.acc_n += 1;
                }
            }
            self.pos += 1;
            if self.pos >= self.period {
                self.pos = 0;
                self.buf.clear();
                if self.end_frame() {
                    report = Some(self.analyse(now_ms));
                }
            }
        }
        report
    }

    /// Store the frame; true when an analysis is due.
    fn end_frame(&mut self) -> bool {
        if self.acc_n == 0 {
            return false;
        }
        let n = self.size;
        let scale = 1.0 / (self.acc_n as f32 * self.norm);
        let mut frame = if self.frames.len() >= WINDOW {
            self.frames.pop_front().expect("full")
        } else {
            vec![0.0; n]
        };
        for (k, v) in frame.iter_mut().enumerate() {
            // fftshift: lowest frequency first.
            *v = self.acc[(k + n / 2) % n] * scale + 1e-20;
        }
        self.frames.push_back(frame);
        self.acc.iter_mut().for_each(|v| *v = 0.0);
        self.acc_n = 0;
        self.since += 1;
        if self.since >= EVERY && self.frames.len() >= MIN_FRAMES {
            self.since = 0;
            return true;
        }
        false
    }

    fn analyse(&mut self, now_ms: u64) -> Report {
        let found = self.detect();
        self.update_tracks(found, now_ms);
        let mut signals: Vec<Signal> = self.tracks.iter().map(Track::signal).collect();
        signals.sort_by_key(|s| s.hz);
        Report {
            kind: "signals",
            center_hz: self.center_hz,
            sample_rate: self.sample_rate,
            bin_hz: self.bin_hz() as f32,
            ts: now_ms,
            classes: CLASSES,
            signals,
        }
    }

    fn detect(&self) -> Vec<Detection> {
        let n = self.size;
        let nf = self.frames.len();
        let frame_s = self.period as f32 / self.sample_rate as f32;
        let bin_hz = self.bin_hz();
        let mut mean = vec![0f32; n];
        let mut max = vec![0f32; n];
        for f in &self.frames {
            for k in 0..n {
                mean[k] += f[k];
                max[k] = max[k].max(f[k]);
            }
        }
        mean.iter_mut().for_each(|v| *v /= nf as f32);
        let db = |v: f32| 10.0 * v.log10();
        let mean_db: Vec<f32> = mean.iter().map(|&v| db(v)).collect();
        let max_db: Vec<f32> = max.iter().map(|&v| db(v)).collect();

        let k0 = (n as f64 * EDGE).ceil() as usize;
        let k1 = n - k0;
        let floor = noise_floor(&mean_db[k0..k1]);
        let floor_db = |k: usize| floor[k - k0];
        let floor_lin: Vec<f32> = (0..n)
            .map(|k| if (k0..k1).contains(&k) { 10f32.powf(floor_db(k) / 10.0) } else { 1.0 })
            .collect();

        // Bins above the floor, grouped into regions.
        let hot: Vec<bool> = (0..n)
            .map(|k| {
                (k0..k1).contains(&k)
                    && (mean_db[k] - floor_db(k) > MEAN_DB || max_db[k] - floor_db(k) > MAX_DB)
            })
            .collect();
        let regions = merge_regions(runs(&hot, 2));

        let dc = n / 2;
        // Level over a region per frame (dB above its floor), and when it's on.
        let gate = |a: usize, b: usize| -> (Vec<f32>, Vec<bool>) {
            let nb = b - a + 1;
            let reg_floor: f32 = floor_lin[a..=b].iter().sum();
            let series: Vec<f32> = self
                .frames
                .iter()
                .map(|f| db(f[a..=b].iter().sum::<f32>() / reg_floor))
                .collect();
            let mut sorted = series.clone();
            sorted.sort_by(f32::total_cmp);
            let p90 = sorted[(sorted.len() * 9 / 10).min(sorted.len() - 1)];
            let noise_thr = 3.0 + 9.0 / ((nb as f32) * FFTS_PER_FRAME as f32).sqrt();
            let thr = noise_thr.min(0.5 * p90).max(1.5);
            let on = series.iter().map(|&v| v > thr).collect();
            (series, on)
        };

        // Pieces close together that key on and off together are one signal:
        // the sidelobes of an OOK sensor, the two tones of wide FSK. Weakest
        // first, each joins the strongest piece within reach it keys with
        // (and anything between the two).
        let max_gap = (25_000.0 / bin_hz) as usize;
        let peak_max = |a: usize, b: usize| (a..=b).map(|k| max_db[k] - floor_db(k)).fold(f32::MIN, f32::max);
        let mut joined: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        let mut peaks: Vec<f32> = Vec::new();
        for (a, b) in regions {
            joined.push((a, b, gate(a, b).1));
            peaks.push(peak_max(a, b));
        }
        let gap = |x: (usize, usize), y: (usize, usize)| {
            if x.1 < y.0 {
                y.0 - x.1 - 1
            } else if y.1 < x.0 {
                x.0 - y.1 - 1
            } else {
                0
            }
        };
        while joined.len() <= 200 {
            let mut order: Vec<usize> = (0..joined.len()).collect();
            order.sort_by(|&x, &y| peaks[x].total_cmp(&peaks[y]));
            let pair = order.iter().find_map(|&i| {
                let span = (joined[i].0, joined[i].1);
                (0..joined.len())
                    .filter(|&j| j != i && peaks[j] >= peaks[i])
                    .filter(|&j| gap(span, (joined[j].0, joined[j].1)) <= max_gap)
                    .filter(|&j| keyed_together(&joined[i].2, &joined[j].2))
                    .max_by(|&x, &y| peaks[x].total_cmp(&peaks[y]))
                    .map(|j| (i, j))
            });
            let Some((i, j)) = pair else { break };
            let lo = joined[i].0.min(joined[j].0);
            let hi = joined[i].1.max(joined[j].1);
            // Fold every piece inside the new span into j.
            let mut on = joined[j].2.clone();
            let mut keep = Vec::new();
            let mut keep_peaks = Vec::new();
            let mut at = 0;
            for (k, r) in joined.iter().enumerate() {
                if k == j {
                    at = keep.len();
                    keep.push((lo, hi, Vec::new()));
                    keep_peaks.push(peaks[k]);
                } else if r.0 >= lo && r.1 <= hi {
                    for (x, y) in on.iter_mut().zip(&r.2) {
                        *x |= *y;
                    }
                } else {
                    keep.push(r.clone());
                    keep_peaks.push(peaks[k]);
                }
            }
            keep[at].2 = on;
            joined = keep;
            peaks = keep_peaks;
        }

        // A weak sliver right next to a far stronger signal is part of it
        // (FM sidebands poking above the noise), not a signal of its own.
        let peak_of = |a: usize, b: usize| (a..=b).map(|k| mean_db[k] - floor_db(k)).fold(f32::MIN, f32::max);
        let sliver = (1_500.0 / bin_hz).ceil() as usize;
        let mut i = 0;
        while i < joined.len() {
            let (a, b) = (joined[i].0, joined[i].1);
            if b - a + 1 <= sliver {
                let pk = peak_of(a, b);
                let absorbs = |j: usize| {
                    let (na, nb) = (joined[j].0, joined[j].1);
                    let gap = if na > b { na - b - 1 } else { a - nb - 1 };
                    peak_of(na, nb) >= pk + 20.0 && gap <= 2.max((nb - na + 1) / 2)
                };
                let into = if i > 0 && absorbs(i - 1) {
                    Some(i - 1)
                } else if i + 1 < joined.len() && absorbs(i + 1) {
                    Some(i + 1)
                } else {
                    None
                };
                if let Some(j) = into {
                    joined[j].0 = joined[j].0.min(a);
                    joined[j].1 = joined[j].1.max(b);
                    joined.remove(i);
                    continue;
                }
            }
            i += 1;
        }

        let mut out = Vec::new();
        for (a, b, _) in joined {
            let nb = b - a + 1;
            let (series, on) = gate(a, b);
            let on_count = on.iter().filter(|&&o| o).count();
            if on_count == 0 || (nb == 1 && on_count < 2) {
                continue;
            }

            // Spectrum while on.
            let mut avg = vec![0f32; nb];
            for (f, &o) in self.frames.iter().zip(&on) {
                if o {
                    for (i, v) in avg.iter_mut().enumerate() {
                        *v += f[a + i];
                    }
                }
            }
            let ex: Vec<f32> = (0..nb).map(|i| db(avg[i] / on_count as f32 / floor_lin[a + i])).collect();
            let (ip, &peak) = ex.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)).expect("bins");
            let cut = (peak - 26.0).max(3.0);
            let lo = ex.iter().position(|&v| v > cut).unwrap_or(ip);
            let hi = ex.iter().rposition(|&v| v > cut).unwrap_or(ip);
            let (oa, ob) = (a + lo, a + hi);
            let occ = ob - oa + 1;
            // The dongle's DC spike: a narrow steady line at the very centre.
            if occ <= 3 && (oa..=ob).contains(&dc) {
                continue;
            }
            // Weak and a bin or two wide: more likely noise than a signal.
            if peak < if occ <= 3 { 8.0 } else { 5.0 } {
                continue;
            }
            let bw = occ as f64 * bin_hz;

            // Ripple across the middle 60 % of the occupied band.
            let ripple_db = (occ >= 10).then(|| {
                let m0 = lo + occ / 5;
                let m1 = hi - occ / 5;
                let mid = &ex[m0..=m1];
                let mu = mid.iter().sum::<f32>() / mid.len() as f32;
                (mid.iter().map(|v| (v - mu).powi(2)).sum::<f32>() / mid.len() as f32).sqrt()
            });

            // Per on-frame: level, centroid and rms width of the excess power.
            let mut levels = Vec::new();
            let mut cents = Vec::new();
            let mut widths = Vec::new();
            for (t, f) in self.frames.iter().enumerate() {
                if !on[t] {
                    continue;
                }
                levels.push(series[t]);
                let mut p = 0f64;
                let mut m1 = 0f64;
                let mut m2 = 0f64;
                for k in oa..=ob {
                    let e = (f[k] - floor_lin[k]).max(0.0) as f64;
                    let x = k as f64;
                    p += e;
                    m1 += e * x;
                    m2 += e * x * x;
                }
                if p > 0.0 {
                    let c = m1 / p;
                    cents.push(c);
                    widths.push(((m2 / p) - c * c).max(0.0).sqrt() + 0.5);
                }
            }
            let level_sd_db = sd(&levels.iter().map(|&v| v as f64).collect::<Vec<_>>()) as f32;
            let drift_hz = (sd(&cents) * bin_hz) as f32;
            let shape_var = if widths.len() >= 3 && occ >= 4 {
                (sd(&widths) / mean_of(&widths)) as f32
            } else {
                0.0
            };

            // Transmissions: runs of on frames.
            let changes = on.windows(2).filter(|w| w[0] != w[1]).count();
            let keying = changes as f32 / (nf as f32 * frame_s);
            let mut ended = Vec::new();
            let mut longest = 0usize;
            let mut start = None;
            for t in 0..=nf {
                let o = t < nf && on[t];
                match (o, start) {
                    (true, None) => start = Some(t),
                    (false, Some(s)) => {
                        longest = longest.max(t - s);
                        // Completed, and ended in the newest analysis step
                        // (so each is counted once across overlapping windows).
                        if s > 0 && t < nf && t + EVERY >= nf {
                            ended.push((t - s) as f32 * frame_s);
                        }
                        start = None;
                    }
                    _ => {}
                }
            }
            let ongoing_s = on[nf - 1].then(|| {
                let s = on.iter().rposition(|&o| !o).map_or(0, |i| i + 1);
                (nf - s) as f32 * frame_s
            });

            let lo_hz = self.center_hz as f64 - self.sample_rate as f64 / 2.0 + oa as f64 * bin_hz;
            let hi_hz = lo_hz + bw;
            out.push(Detection {
                lo_hz,
                hi_hz,
                features: Features {
                    bw_hz: bw as f32,
                    snr_db: (peak * 10.0).round() / 10.0,
                    duty: on_count as f32 / nf as f32,
                    keying,
                    level_sd_db,
                    ripple_db,
                    shape_var,
                    drift_hz,
                    burst_s: None,
                    longest_s: longest as f32 * frame_s,
                    keyed_with_hz: None,
                },
                ended,
                ongoing_s,
                on,
            });
        }
        // Narrow lines that key with a much stronger signal.
        for i in 0..out.len() {
            if out[i].features.bw_hz > 2_500.0 {
                continue;
            }
            let with = (0..out.len())
                .filter(|&j| j != i && out[j].features.snr_db >= out[i].features.snr_db + 10.0)
                .filter(|&j| keyed_together(&out[i].on, &out[j].on))
                .max_by(|&x, &y| out[x].features.snr_db.total_cmp(&out[y].features.snr_db));
            out[i].features.keyed_with_hz = with.map(|j| ((out[j].lo_hz + out[j].hi_hz) / 2.0).round() as u64);
        }
        out
    }

    fn update_tracks(&mut self, found: Vec<Detection>, now_ms: u64) {
        for t in &mut self.tracks {
            t.active = false;
        }
        for d in found {
            let idx = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.overlaps(d.lo_hz, d.hi_hz))
                .min_by(|a, b| {
                    let c = (d.lo_hz + d.hi_hz) / 2.0;
                    (a.1.centre() - c).abs().total_cmp(&(b.1.centre() - c).abs())
                })
                .map(|(i, _)| i);
            let i = match idx {
                Some(i) if !self.tracks[i].active => i,
                // Already matched this round: two signals now where one was.
                _ => {
                    self.tracks.push(Track {
                        id: self.next_id,
                        lo_hz: d.lo_hz,
                        hi_hz: d.hi_hz,
                        first_ms: now_ms,
                        last_ms: now_ms,
                        active: false,
                        shares: [1.0 / N as f32; N],
                        fit: 0.0,
                        updates: 0,
                        features: Features::default(),
                        bursts: VecDeque::new(),
                        longest_s: 0.0,
                    });
                    self.next_id = self.next_id.wrapping_add(1).max(1);
                    self.tracks.len() - 1
                }
            };
            let t = &mut self.tracks[i];
            t.active = true;
            t.last_ms = now_ms;
            // Follow its edges (a drifting carrier, a wider over).
            let k = if t.updates == 0 { 1.0 } else { 0.3 };
            t.lo_hz += k * (d.lo_hz - t.lo_hz);
            t.hi_hz += k * (d.hi_hz - t.hi_hz);
            for b in d.ended {
                if t.bursts.len() == 8 {
                    t.bursts.pop_front();
                }
                t.bursts.push_back(b);
            }
            t.longest_s = t.longest_s.max(d.features.longest_s).max(d.ongoing_s.unwrap_or(0.0));
            let mut f = d.features;
            f.longest_s = t.longest_s;
            f.burst_s = median(t.bursts.iter().copied());
            let raw = score(&f);
            let fit = raw.iter().copied().fold(0.0, f32::max);
            let shares = normalise(raw);
            // Smooth the scores. The first look sets them; then quick for a few, steadier with a history.
            let a = match t.updates {
                0 => 1.0,
                1..=2 => 0.6,
                _ => 0.25,
            };
            for (s, n) in t.shares.iter_mut().zip(shares) {
                *s += a * (n - *s);
            }
            t.fit += a * (fit - t.fit);
            t.updates += 1;
            t.features = f;
        }
        self.tracks.retain(|t| {
            let hold = if t.bursty() { HOLD_BURSTY_MS } else { HOLD_MS };
            now_ms.saturating_sub(t.last_ms) <= hold
        });
    }
}

/// Noise floor per bin (dB): a low percentile in blocks across the capture,
/// interpolated, capped a little above the floor of the whole capture so a
/// wide signal filling a block isn't taken for noise.
fn noise_floor(db: &[f32]) -> Vec<f32> {
    let n = db.len();
    let pct = |v: &[f32], p: f32| {
        let mut s = v.to_vec();
        s.sort_by(f32::total_cmp);
        s[((s.len() as f32 * p) as usize).min(s.len() - 1)]
    };
    let global = pct(db, 0.15);
    let blocks = 32.min(n.max(1));
    let bl = n.div_ceil(blocks);
    let floors: Vec<(f32, f32)> = db
        .chunks(bl)
        .enumerate()
        .map(|(i, c)| ((i * bl) as f32 + c.len() as f32 / 2.0, pct(c, 0.3).min(global + 6.0)))
        .collect();
    (0..n)
        .map(|k| {
            let x = k as f32;
            let j = floors.iter().position(|&(c, _)| c > x).unwrap_or(floors.len());
            if j == 0 {
                floors[0].1
            } else if j == floors.len() {
                floors[j - 1].1
            } else {
                let (c0, v0) = floors[j - 1];
                let (c1, v1) = floors[j];
                v0 + (v1 - v0) * (x - c0) / (c1 - c0)
            }
        })
        .collect()
}

/// Two gates that switch on and off together (within a frame), and aren't
/// simply always on.
fn keyed_together(x: &[bool], y: &[bool]) -> bool {
    let n = x.len();
    let count = |v: &[bool]| v.iter().filter(|&&o| o).count();
    let (cx, cy) = (count(x), count(y));
    let busy = |c: usize| c * 10 >= n * 9;
    if cx == 0 || cy == 0 || busy(cx) || busy(cy) {
        return false;
    }
    // Within a couple of frames: a weak piece's gate is ragged at the edges.
    let near = |v: &[bool], t: usize| v[t.saturating_sub(2)..(t + 3).min(n)].iter().any(|&o| o);
    let hits = (0..n).filter(|&t| x[t] && near(y, t)).count() + (0..n).filter(|&t| y[t] && near(x, t)).count();
    hits * 4 >= (cx + cy) * 3
}

/// Runs of true, joined across gaps of up to `gap` false.
fn runs(hot: &[bool], gap: usize) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (k, &h) in hot.iter().enumerate() {
        if !h {
            continue;
        }
        match out.last_mut() {
            Some((_, e)) if k - *e <= gap + 1 => *e = k,
            _ => out.push((k, k)),
        }
    }
    out
}

/// Join neighbouring regions separated by a gap small next to both (the
/// dips between the lines of an FM broadcast), but not neighbouring channels.
fn merge_regions(regions: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (a, b) in regions {
        if let Some(last) = out.last_mut() {
            let gap = a - last.1 - 1;
            let wa = last.1 - last.0 + 1;
            let wb = b - a + 1;
            if gap as f32 <= 0.1 * wa.min(wb) as f32 {
                last.1 = b;
                continue;
            }
        }
        out.push((a, b));
    }
    out
}

fn mean_of(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn sd(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = mean_of(v);
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

fn median(it: impl Iterator<Item = f32>) -> Option<f32> {
    let mut v: Vec<f32> = it.collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Run the classifier on its own thread while anyone is watching the
/// spectrum (`watching` has receivers); the latest report is published as
/// JSON on `out`.
pub fn spawn(
    mut rx: broadcast::Receiver<Arc<IqBlock>>,
    watching: broadcast::Sender<bytes::Bytes>,
    out: watch::Sender<Arc<str>>,
) {
    std::thread::Builder::new()
        .name("classifier".into())
        .spawn(move || {
            let mut c = Classifier::new();
            let mut idle = true;
            loop {
                match rx.blocking_recv() {
                    Ok(block) => {
                        if watching.receiver_count() == 0 {
                            if !idle {
                                c = Classifier::new();
                                out.send_replace(Arc::from(""));
                                idle = true;
                            }
                            continue;
                        }
                        idle = false;
                        if let Some(r) = c.process(&block, now_ms()) {
                            match serde_json::to_string(&r) {
                                Ok(s) => {
                                    out.send_replace(Arc::from(s));
                                }
                                Err(e) => tracing::warn!("classifier report: {e}"),
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("classifier lagged {n} blocks");
                        c.reset();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .expect("spawn classifier thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the simulator at `center_hz` for `secs`, returning the last report.
    fn run(center_hz: u64, rate: u32, secs: f64) -> Report {
        let mut gen = crate::radio::synth_for_tests();
        let mut c = Classifier::new();
        let mut last = None;
        let block_s = 0.05;
        let steps = (secs / block_s).round() as usize;
        for i in 0..steps {
            let data = gen(center_hz, rate, block_s);
            let block = IqBlock { center_hz, sample_rate: rate, data };
            if let Some(r) = c.process(&block, (i as f64 * block_s * 1000.0) as u64) {
                last = Some(r);
            }
        }
        last.expect("a report")
    }

    fn near(r: &Report, hz: u64, tol: u64) -> Option<&Signal> {
        r.signals.iter().find(|s| s.lo_hz <= hz + tol && s.hi_hz + tol >= hz)
    }

    fn dump(r: &Report) -> String {
        r.signals
            .iter()
            .map(|s| {
                format!(
                    "{:.4} MHz {:?} {:.2} bw {:.0} {:?}\n",
                    s.hz as f64 / 1e6,
                    s.class,
                    s.confidence,
                    s.features.bw_hz,
                    s.features
                )
            })
            .collect()
    }

    fn expect(r: &Report, hz: u64, tol: u64, class: Class) {
        let s = near(r, hz, tol).unwrap_or_else(|| panic!("nothing at {hz}:\n{}", dump(r)));
        assert_eq!(s.class, Some(class), "at {hz}:\n{}", dump(r));
    }

    #[test]
    fn soft_rules() {
        assert_eq!(below(1.0, 2.0, 3.0), 1.0);
        assert_eq!(below(3.5, 2.0, 3.0), 0.0);
        assert_eq!(above(2.5, 2.0, 3.0), 0.5);
        assert_eq!(between(10.0, 1.0, 2.0, 20.0, 30.0), 1.0);
        assert_eq!(between(25.0, 1.0, 2.0, 20.0, 30.0), 0.5);
        let s = normalise([1.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!((s[1] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn floor_ignores_a_wide_signal() {
        // Noise at -60 with a signal at -30 over 70 % of the capture.
        let db: Vec<f32> = (0..1000).map(|k| if (150..850).contains(&k) { -30.0 } else { -60.0 }).collect();
        let f = noise_floor(&db);
        assert!(f.iter().all(|&v| v < -50.0), "{:?}", &f[400..410]);
    }

    #[test]
    fn regions_join_lines_but_not_channels() {
        // Two wide pieces 3 bins apart join; two narrow ones 3 bins apart don't.
        assert_eq!(merge_regions(vec![(0, 99), (103, 200)]), vec![(0, 200)]);
        assert_eq!(merge_regions(vec![(0, 9), (13, 22)]), vec![(0, 9), (13, 22)]);
        assert_eq!(runs(&[true, false, false, true, false, false, false, true], 2), vec![(0, 3), (7, 7)]);
    }

    #[test]
    fn steady_carriers() {
        // 437.55: a drifting (Doppler) carrier.
        let r = run(437_500_000, 256_000, 6.0);
        expect(&r, 437_550_000, 12_000, Class::Carrier);
    }

    #[test]
    fn cw_beacon() {
        // GB3SIM keys its text every 20 s from t = 0.
        let r = run(144_450_000, 256_000, 8.0);
        expect(&r, 144_470_000, 1_000, Class::Cw);
    }

    #[test]
    fn broadcast_fm() {
        let r = run(96_000_000, 1_024_000, 5.0);
        expect(&r, 96_100_000, 20_000, Class::BroadcastFm);
    }

    #[test]
    fn dab_multiplex() {
        let r = run(222_064_000, 2_048_000, 4.0);
        expect(&r, 222_064_000, 100_000, Class::Ofdm);
    }

    #[test]
    fn digital_voice() {
        // M17 from t = 2 s.
        let r = run(145_375_000, 256_000, 7.0);
        expect(&r, 145_375_000, 3_000, Class::DigitalVoice);
    }

    #[test]
    fn analogue_voice() {
        // The repeater talks for the first 6 s of every 30.
        let r = run(145_712_500, 256_000, 5.0);
        expect(&r, 145_712_500, 3_000, Class::Voice);
    }

    #[test]
    fn data_bursts() {
        // SRD telemetry: on 0.8 s of every second.
        let r = run(434_200_000, 256_000, 8.0);
        expect(&r, 434_200_000, 3_000, Class::Data);
        // APRS packets every 10 s, and remembered between them.
        let r = run(144_800_000, 256_000, 25.0);
        expect(&r, 144_800_000, 3_000, Class::Data);
    }

    #[test]
    fn spurs_key_with_their_transmitter() {
        // The SRD's close-in spurs (every 50 kHz) key on and off with it.
        let r = run(434_200_000, 512_000, 6.0);
        let s = near(&r, 434_250_000, 500).unwrap_or_else(|| panic!("no spur:\n{}", dump(&r)));
        let with = s.features.keyed_with_hz.expect("keyed with the SRD");
        assert!(with.abs_diff(434_200_000) < 2_000, "{with}");
        expect(&r, 434_250_000, 500, Class::Carrier);
        expect(&r, 434_200_000, 3_000, Class::Data);
    }

    #[test]
    fn sstv_is_one_signal_and_not_voice() {
        // Images from t = 3 s: steady tones, not speech.
        let r = run(145_800_000, 256_000, 9.0);
        let around: Vec<_> = r.signals.iter().filter(|s| s.hz.abs_diff(145_800_000) < 15_000).collect();
        assert_eq!(around.len(), 1, "{}", dump(&r));
        assert_ne!(around[0].class, Some(Class::Voice), "{}", dump(&r));
    }

    #[test]
    fn ook_sensor_is_one_signal() {
        // Two Nexus sensors on 433.92 (OOK: sidelobes either side), at 1 s and 8.5 s.
        // Also from the ISM preset (434.0 MHz, 1.024 MS/s), as the UI shows it.
        for (centre, rate, secs) in [(433_920_000, 256_000, 3.0), (433_920_000, 256_000, 10.0), (434_000_000, 1_024_000, 10.0)] {
            let r = run(centre, rate, secs);
            let around: Vec<_> = r.signals.iter().filter(|s| s.hz.abs_diff(433_920_000) < 80_000).collect();
            assert_eq!(around.len(), 1, "after {secs} s:
{}", dump(&r));
            assert_eq!(around[0].class, Some(Class::Data), "{}", dump(&r));
        }
    }

    #[test]
    fn quiet_band_has_nothing() {
        let r = run(150_000_000, 256_000, 4.0);
        assert!(r.signals.is_empty(), "{}", dump(&r));
    }
}
