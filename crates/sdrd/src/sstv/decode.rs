//! SSTV decoder: frequency track in, images out.
//!
//! 1. VIS detection on 1 ms frequency averages: leader 1900 Hz, start bit,
//!    7 data bits + even parity (1100 = 1, 1300 = 0), stop bit, all 30 ms.
//! 2. Image decoding from the end of the VIS: for every sync period the sync
//!    pulse is searched around its predicted position (score = time at
//!    1200 Hz inside the pulse minus time at 1200 Hz in the porch after it,
//!    which pins its trailing edge). A least-squares line through the found
//!    positions gives the real line rate, which corrects slant from clock
//!    differences, and is used to place every scan.
//! 3. Several periods in a row without a sync means the signal is gone: the
//!    image ends early and is reported as partial.

use std::collections::VecDeque;

use super::modes::{by_vis, freq_to_value, yuv_to_rgb, Color, Mode};

pub struct Image {
    pub mode: &'static Mode,
    /// width × height × 3; lines not received stay black.
    pub rgb: Vec<u8>,
    /// Lines received.
    pub lines: usize,
    pub complete: bool,
}

pub enum Output {
    Started { mode: &'static Mode },
    /// Newly decoded rows (RGB, `mode.width` pixels each).
    Rows { session: u32, mode: &'static Mode, first_line: usize, rgb: Vec<u8> },
    Done { image: Image },
}

const SYNC_HZ: f32 = 1200.0;
const SYNC_TOL: f32 = 100.0;
const VIS_TOL: f32 = 80.0;
/// Periods without a sync before we give up on an image.
const MAX_MISSES: usize = 8;

struct ImgState {
    mode: &'static Mode,
    session: u32,
    /// Predicted first sync position (absolute sample index).
    a0: f64,
    /// Nominal samples per sync period.
    b0: f64,
    next: usize,
    meas: Vec<(f64, f64)>,
    misses: usize,
    rgb: Vec<u8>,
    lines: usize,
    // Robot 36 chroma bookkeeping.
    last_u: Vec<u8>,
    last_v: Vec<u8>,
    pending_y: Option<(usize, Vec<u8>)>,
}

impl ImgState {
    /// Predicted sync start for period `i`.
    fn fit(&self, i: usize) -> f64 {
        let i = i as f64;
        match self.meas.len() {
            0 => self.a0 + self.b0 * i,
            1 => self.meas[0].1 + self.b0 * (i - self.meas[0].0),
            n => {
                let n = n as f64;
                let mi = self.meas.iter().map(|m| m.0).sum::<f64>() / n;
                let mp = self.meas.iter().map(|m| m.1).sum::<f64>() / n;
                let (mut sxy, mut sxx) = (0.0, 0.0);
                for &(x, y) in &self.meas {
                    sxy += (x - mi) * (y - mp);
                    sxx += (x - mi) * (x - mi);
                }
                let b = if sxx > 0.0 { sxy / sxx } else { self.b0 };
                // Real clocks are within a fraction of a percent.
                let b = b.clamp(self.b0 * 0.99, self.b0 * 1.01);
                mp + b * (i - mi)
            }
        }
    }
}

pub struct Decoder {
    fs: f64,
    buf: Vec<f32>,
    /// Absolute index of `buf[0]`.
    base: usize,
    ms_len: f64,
    ms_next: f64,
    ms_sum: f32,
    ms_n: u32,
    ms_hist: VecDeque<f32>,
    ms_count: usize,
    cand: Option<(f32, usize, &'static Mode)>,
    cand_age: u32,
    img: Option<ImgState>,
    session: u32,
}

fn median(v: &[f32]) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s[s.len() / 2]
}

impl Decoder {
    /// `fs`: rate of the frequency samples (see `ToneDemod::rate`).
    pub fn new(fs: f64) -> Self {
        Decoder {
            fs,
            buf: Vec::new(),
            base: 0,
            ms_len: fs / 1000.0,
            ms_next: fs / 1000.0,
            ms_sum: 0.0,
            ms_n: 0,
            ms_hist: VecDeque::with_capacity(700),
            ms_count: 0,
            cand: None,
            cand_age: 0,
            img: None,
            session: 0,
        }
    }

    /// End an image in progress (e.g. the radio was retuned) as partial.
    pub fn abort(&mut self, out: &mut Vec<Output>) {
        if self.img.is_some() {
            self.finish(false, out);
        }
    }

    pub fn push(&mut self, freqs: &[f32], out: &mut Vec<Output>) {
        for &f in freqs {
            self.buf.push(f);
            if self.img.is_none() {
                let n = (self.base + self.buf.len() - 1) as f64;
                if n >= self.ms_next {
                    let mean = if self.ms_n > 0 { self.ms_sum / self.ms_n as f32 } else { f };
                    if self.ms_hist.len() == 700 {
                        self.ms_hist.pop_front();
                    }
                    self.ms_hist.push_back(mean);
                    self.ms_count += 1;
                    self.ms_next += self.ms_len;
                    self.ms_sum = 0.0;
                    self.ms_n = 0;
                    self.check_vis(out);
                }
                self.ms_sum += f;
                self.ms_n += 1;
            }
        }
        self.decode_ready(out);
        self.trim();
    }

    /// 1 ms means from `from_end` to `to_end` ms before now. `check_vis`
    /// makes the history contiguous first.
    fn ms_window(&self, from_end: usize, to_end: usize) -> &[f32] {
        let a = self.ms_hist.as_slices().0;
        let len = a.len();
        &a[len - from_end..len - to_end]
    }

    fn check_vis(&mut self, out: &mut Vec<Output>) {
        if self.ms_hist.len() < 600 {
            return;
        }
        self.ms_hist.make_contiguous();
        let near = |w: &[f32], hz: f32| -> Option<f32> {
            if w.is_empty() {
                return None;
            }
            let d = (median(w) - hz).abs();
            (d < VIS_TOL).then_some(d)
        };
        let matched = (|| {
            let mut score = near(self.ms_window(560, 310), 1900.0)?;
            score += near(self.ms_window(295, 275), SYNC_HZ)?;
            score += near(self.ms_window(25, 5), SYNC_HZ)?;
            let mut code = 0u8;
            let mut ones = 0;
            for j in 0..8 {
                let start = 30 + (8 - j) * 30;
                let w = self.ms_window(start - 5, start - 25);
                let m = median(w);
                let bit = if (m - 1100.0).abs() < VIS_TOL {
                    1
                } else if (m - 1300.0).abs() < VIS_TOL {
                    0
                } else {
                    return None;
                };
                score += (m - if bit == 1 { 1100.0 } else { 1300.0 }).abs();
                ones += bit;
                if j < 7 {
                    code |= bit << j;
                }
            }
            if ones % 2 != 0 {
                return None;
            }
            by_vis(code).map(|m| (score, m))
        })();

        if let Some((score, mode)) = matched {
            if self.cand.is_none_or(|c| score < c.0) {
                self.cand = Some((score, self.ms_count, mode));
            }
        }
        if let Some((_, end_ms, mode)) = self.cand {
            self.cand_age += 1;
            // Keep looking for a few ms for the best-aligned match.
            if self.cand_age > 20 {
                self.cand = None;
                self.cand_age = 0;
                self.start_image(mode, end_ms as f64 * self.ms_len, out);
            }
        }
    }

    fn start_image(&mut self, mode: &'static Mode, t0: f64, out: &mut Vec<Output>) {
        self.session = self.session.wrapping_add(1);
        let w = mode.width as usize;
        tracing::info!("SSTV: {} image starting", mode.name);
        self.img = Some(ImgState {
            mode,
            session: self.session,
            a0: t0 + mode.first_sync_ms * self.fs / 1000.0,
            b0: mode.line_ms * self.fs / 1000.0,
            next: 0,
            meas: Vec::new(),
            misses: 0,
            rgb: vec![0; w * mode.height as usize * 3],
            lines: 0,
            last_u: vec![128; w],
            last_v: vec![128; w],
            pending_y: None,
        });
        out.push(Output::Started { mode });
    }

    /// Sample range a period needs: (start, end) offsets from its sync, samples.
    fn period_span(&self, mode: &Mode) -> (f64, f64) {
        let ms = self.fs / 1000.0;
        let lo = mode.scans.iter().map(|s| s.0).fold(0.0f64, f64::min);
        let hi = mode
            .scans
            .iter()
            .map(|s| s.0 + s.1)
            .fold(mode.sync_ms + mode.porch_ms, f64::max);
        (lo * ms, hi * ms)
    }

    fn decode_ready(&mut self, out: &mut Vec<Output>) {
        let ms = self.fs / 1000.0;
        while let Some(st) = self.img.as_ref() {
            let mode = st.mode;
            let i = st.next;
            let pred = st.fit(i);
            let win = if st.meas.len() < 3 { 20.0 * ms } else { 6.0 * ms };
            let (lo, hi) = self.period_span(mode);
            let end = self.base + self.buf.len();
            if (pred + win + hi + 2.0) as usize >= end {
                break;
            }
            if pred + lo - win < self.base as f64 {
                // Shouldn't happen (we keep enough history); skip the period.
                self.img.as_mut().unwrap().next += 1;
                continue;
            }

            let found = self.find_sync(mode, pred, win);
            let st = self.img.as_mut().unwrap();
            match found {
                Some(q) if st.meas.len() < 4 || (q - pred).abs() < 3.0 * ms => {
                    st.meas.push((i as f64, q));
                    st.misses = 0;
                }
                _ => st.misses += 1,
            }
            let pos = st.fit(i);
            let lost = st.misses >= MAX_MISSES;
            self.decode_period(i, pos, out);
            let st = self.img.as_mut().unwrap();
            st.next += 1;
            if lost || st.next >= mode.periods() {
                self.finish(!lost, out);
            }
        }
    }

    /// Best sync start within ±`win` of `pred`, if convincing.
    fn find_sync(&self, mode: &Mode, pred: f64, win: f64) -> Option<f64> {
        let ms = self.fs / 1000.0;
        let s = (mode.sync_ms * ms).round() as usize;
        let p = ((mode.porch_ms.min(2.0)) * ms).round().max(3.0) as usize;
        let start = (pred - win).floor() as usize - self.base;
        let n = (2.0 * win).ceil() as usize;
        let span = &self.buf[start..(start + n + s + p).min(self.buf.len())];
        let mut pre = Vec::with_capacity(span.len() + 1);
        pre.push(0u32);
        // Judge "is this sync?" on a 0.5 ms centred average, so noise spikes
        // in a weak signal don't break up the pulse.
        let half = ((0.25 * ms) as usize).max(1);
        let mut fsum = Vec::with_capacity(span.len() + 1);
        fsum.push(0f64);
        for &f in span {
            fsum.push(fsum.last().unwrap() + f as f64);
        }
        for k in 0..span.len() {
            let (a, b) = (k.saturating_sub(half), (k + half + 1).min(span.len()));
            let avg = ((fsum[b] - fsum[a]) / (b - a) as f64) as f32;
            let ind = ((avg - SYNC_HZ).abs() < SYNC_TOL) as u32;
            pre.push(pre.last().unwrap() + ind);
        }
        let mut best = (f32::MIN, 0usize);
        for q in 0..=n.min(span.len().saturating_sub(s + p)) {
            let a = (pre[q + s] - pre[q]) as f32 / s as f32;
            let b = (pre[q + s + p] - pre[q + s]) as f32 / p as f32;
            if a - b > best.0 {
                best = (a - b, q);
            }
        }
        (best.0 > 0.4).then(|| (start + best.1 + self.base) as f64)
    }

    /// Average frequency over [a, b) (absolute, fractional) as a pixel value.
    fn pixel(&self, a: f64, b: f64) -> u8 {
        let i0 = (a.floor() as usize).saturating_sub(self.base);
        let i1 = ((b.ceil() as usize).saturating_sub(self.base)).max(i0 + 1).min(self.buf.len());
        let w = &self.buf[i0.min(i1 - 1)..i1];
        freq_to_value(w.iter().sum::<f32>() / w.len() as f32)
    }

    fn scan(&self, pos: f64, idx: usize, mode: &Mode) -> Vec<u8> {
        let ms = self.fs / 1000.0;
        let (off, dur) = mode.scans[idx];
        let w = mode.width as usize;
        let t0 = pos + off * ms;
        let px = dur * ms / w as f64;
        (0..w).map(|x| self.pixel(t0 + x as f64 * px, t0 + (x + 1) as f64 * px)).collect()
    }

    fn mean_freq(&self, a: f64, b: f64) -> f32 {
        let i0 = (a as usize).saturating_sub(self.base);
        let i1 = (b as usize).saturating_sub(self.base).min(self.buf.len()).max(i0 + 1);
        let w = &self.buf[i0..i1];
        w.iter().sum::<f32>() / w.len() as f32
    }

    fn decode_period(&mut self, i: usize, pos: f64, out: &mut Vec<Output>) {
        let mode = self.img.as_ref().unwrap().mode;
        let w = mode.width as usize;
        let mut rows: Vec<(usize, Vec<u8>)> = Vec::new();
        match mode.color {
            Color::Gbr => {
                let (g, b, r) = (self.scan(pos, 0, mode), self.scan(pos, 1, mode), self.scan(pos, 2, mode));
                rows.push((i, (0..w).flat_map(|x| [r[x], g[x], b[x]]).collect()));
            }
            Color::Yuv => {
                let (y, v, u) = (self.scan(pos, 0, mode), self.scan(pos, 1, mode), self.scan(pos, 2, mode));
                rows.push((i, rgb_row(&y, &u, &v)));
            }
            Color::Pd => {
                let y0 = self.scan(pos, 0, mode);
                let v = self.scan(pos, 1, mode);
                let u = self.scan(pos, 2, mode);
                let y1 = self.scan(pos, 3, mode);
                rows.push((2 * i, rgb_row(&y0, &u, &v)));
                rows.push((2 * i + 1, rgb_row(&y1, &u, &v)));
            }
            Color::Robot36 => {
                let ms = self.fs / 1000.0;
                let y = self.scan(pos, 0, mode);
                let c = self.scan(pos, 1, mode);
                // The separator before the chroma says which one it is:
                // 1500 Hz → R-Y (even line), 2300 Hz → B-Y (odd line).
                let sep = self.mean_freq(pos + 100.5 * ms, pos + 104.0 * ms);
                let is_v = if (sep - 1900.0).abs() > 200.0 { sep < 1900.0 } else { i.is_multiple_of(2) };
                let st = self.img.as_mut().unwrap();
                if is_v {
                    st.last_v = c;
                    if let Some((li, py)) = st.pending_y.take() {
                        rows.push((li, rgb_row(&py, &st.last_u, &st.last_v)));
                    }
                    st.pending_y = Some((i, y));
                } else {
                    st.last_u = c;
                    if let Some((li, py)) = st.pending_y.take() {
                        rows.push((li, rgb_row(&py, &st.last_u, &st.last_v)));
                    }
                    rows.push((i, rgb_row(&y, &st.last_u, &st.last_v)));
                }
            }
        }
        self.emit(rows, out);
    }

    fn emit(&mut self, rows: Vec<(usize, Vec<u8>)>, out: &mut Vec<Output>) {
        let st = self.img.as_mut().unwrap();
        let w3 = st.mode.width as usize * 3;
        for (line, rgb) in rows {
            if line >= st.mode.height as usize {
                continue;
            }
            st.rgb[line * w3..(line + 1) * w3].copy_from_slice(&rgb);
            st.lines = st.lines.max(line + 1);
            out.push(Output::Rows { session: st.session, mode: st.mode, first_line: line, rgb });
        }
    }

    fn finish(&mut self, complete: bool, out: &mut Vec<Output>) {
        // Flush a Robot 36 line still waiting for its partner.
        let flush = self.img.as_mut().and_then(|st| {
            st.pending_y.take().map(|(li, py)| (li, rgb_row(&py, &st.last_u, &st.last_v)))
        });
        if let Some(row) = flush {
            self.emit(vec![row], out);
        }
        let Some(st) = self.img.take() else { return };
        let lines = if complete { st.mode.height as usize } else { st.lines };
        tracing::info!("SSTV: {} image {} ({lines} lines)", st.mode.name, if complete { "complete" } else { "lost" });
        out.push(Output::Done {
            image: Image { mode: st.mode, rgb: st.rgb, lines, complete },
        });
        self.ms_hist.clear();
        self.cand = None;
        self.cand_age = 0;
        // Resume VIS detection from "now".
        let now = (self.base + self.buf.len()) as f64;
        self.ms_next = (now / self.ms_len).ceil() * self.ms_len;
        self.ms_count = (self.ms_next / self.ms_len).round() as usize - 1;
        self.ms_sum = 0.0;
        self.ms_n = 0;
    }

    fn trim(&mut self) {
        let keep_from = match self.img.as_ref() {
            Some(st) => {
                let (lo, _) = self.period_span(st.mode);
                (st.fit(st.next) + lo - 40.0 * self.fs / 1000.0).max(0.0) as usize
            }
            None => (self.base + self.buf.len()).saturating_sub(self.fs as usize),
        };
        let drop = keep_from.saturating_sub(self.base);
        // Drain in big chunks to keep this cheap.
        if drop > self.fs as usize {
            self.buf.drain(..drop);
            self.base += drop;
        }
    }
}

fn rgb_row(y: &[u8], u: &[u8], v: &[u8]) -> Vec<u8> {
    y.iter()
        .zip(u.iter().zip(v))
        .flat_map(|(&y, (&u, &v))| {
            let (r, g, b) = yuv_to_rgb(y, u, v);
            [r, g, b]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sstv::encode::{synth, test_pattern, tones};
    use crate::sstv::modes::by_name;
    use crate::sstv::tone::ToneDemod;

    /// Encode a test card, decode it, return (image, mean abs error).
    fn roundtrip(name: &str, rate_err: f64, noise: f32) -> (Image, f64) {
        let mode = by_name(name).unwrap();
        let (w, h) = (mode.width as usize, mode.height as usize);
        let img = test_pattern(w, h, 3);
        let rate = 48_000.0;
        // Transmitter clock off by `rate_err` (relative) → slant.
        let mut audio = vec![0.0f32; 24_000];
        audio.extend(synth(&tones(mode, &img), rate * (1.0 + rate_err), 0.5));
        audio.extend(vec![0.0f32; 48_000]);
        let mut rng = 0x1234_5678_9abc_def0u64;
        for s in audio.iter_mut() {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            *s += noise * ((rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5);
        }
        let mut tone = ToneDemod::new(rate);
        let mut dec = Decoder::new(tone.rate());
        let mut out = Vec::new();
        for chunk in audio.chunks(2400) {
            let mut f = Vec::new();
            tone.process(chunk, &mut f);
            dec.push(&f, &mut out);
        }
        let done = out
            .into_iter()
            .find_map(|o| match o {
                Output::Done { image, .. } => Some(image),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name}: no image decoded"));
        assert_eq!(done.mode.name, name);
        // Compare away from the left/right edges, where filter ringing and
        // the first pixel's timing make small differences.
        let mut err = 0f64;
        let mut n = 0;
        for y in 0..h {
            for x in 4..w - 4 {
                for c in 0..3 {
                    let i = (y * w + x) * 3 + c;
                    err += (img[i] as f64 - done.rgb[i] as f64).abs();
                    n += 1;
                }
            }
        }
        eprintln!("{name} err={rate_err} noise={noise}: lines={} complete={} mae={:.1}", done.lines, done.complete, err / n as f64);
        (done, err / n as f64)
    }

    #[test]
    fn decodes_every_mode_family() {
        for name in ["Robot 36", "Robot 72", "Martin M1", "Martin M2", "Scottie S1", "PD 90", "PD 120"] {
            let (img, err) = roundtrip(name, 0.0, 0.0);
            assert!(img.complete, "{name} incomplete");
            assert!(err < 12.0, "{name}: mean error {err:.1}");
        }
    }

    #[test]
    fn corrects_slant_and_survives_noise() {
        // 0.2 % clock error: ~70 ms of drift over a Robot 36 image.
        let (img, err) = roundtrip("Robot 36", 0.002, 0.0);
        assert!(img.complete);
        assert!(err < 14.0, "slanted: mean error {err:.1}");
        let (img, err) = roundtrip("Scottie S1", -0.002, 0.0);
        assert!(img.complete);
        assert!(err < 14.0, "slanted Scottie: mean error {err:.1}");
        // Noise at about the same level as the tones.
        let (img, err) = roundtrip("Martin M1", 0.0, 0.8);
        assert!(img.complete);
        assert!(err < 25.0, "noisy: mean error {err:.1}");
    }
}
