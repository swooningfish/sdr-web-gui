//! The raw radio signal behind a decoded rtl_433 packet, for debugging.
//!
//! The rtl_433 feeder keeps the last couple of seconds of the IQ stream it
//! hands to rtl_433 in a [`Ring`]. rtl_433 timestamps piped input by sample
//! offset ("@5.550031s"), so when a packet is decoded we cut the burst at that
//! point out of the ring and keep a compact picture of it: the envelope, the
//! instantaneous frequency (for FSK), the pulse/gap timings, and the IQ itself
//! as a `.cs8` file that rtl_433 can replay (`rtl_433 -r file.cs8 -A`).
//! Only the latest burst per device is kept, in memory.

use std::collections::{HashMap, HashSet};
use std::f32::consts::PI;
use std::sync::{Arc, Mutex};

use num_complex::Complex32;
use serde::Serialize;

/// Seconds of stream kept for cutting bursts out.
const RING_SECS: f64 = 2.0;
/// Longest burst kept.
const MAX_BURST_SECS: f64 = 1.0;
/// Silence that ends a burst (gaps inside a packet train are shorter).
const MAX_GAP_SECS: f64 = 0.02;
/// Margin kept either side of the burst.
const PAD_SECS: f64 = 0.002;
/// Points in the envelope / frequency traces.
const TRACE_POINTS: usize = 1000;
/// Length of the zoomed-in envelope of the burst's start.
const DETAIL_SECS: f64 = 0.025;
/// Most pulse/gap timings kept.
const MAX_PULSES: usize = 4000;
/// Devices whose last burst is remembered.
const MAX_DEVICES: usize = 128;

/// The most recent samples written to rtl_433, indexed by their position in
/// the stream since the process started (which is what rtl_433's time counts).
pub struct Ring {
    buf: Vec<Complex32>,
    /// Samples written since the stream started.
    written: u64,
    pub rate: u32,
}

impl Ring {
    pub fn new() -> Self {
        Ring { buf: Vec::new(), written: 0, rate: 0 }
    }

    /// A new rtl_433 process: its clock starts again at 0.
    pub fn reset(&mut self, rate: u32) {
        let cap = (rate as f64 * RING_SECS) as usize;
        self.buf.clear();
        self.buf.resize(cap.max(1), Complex32::new(0.0, 0.0));
        self.written = 0;
        self.rate = rate;
    }

    pub fn push(&mut self, iq: &[Complex32]) {
        let cap = self.buf.len();
        if cap == 0 {
            return;
        }
        for c in iq {
            self.buf[(self.written % cap as u64) as usize] = *c;
            self.written += 1;
        }
    }

    /// Samples `from..to` (stream positions), clipped to what is still held.
    /// Returns the clipped start and the samples.
    pub fn slice(&self, from: u64, to: u64) -> (u64, Vec<Complex32>) {
        let cap = self.buf.len() as u64;
        let from = from.max(self.written.saturating_sub(cap));
        let to = to.min(self.written);
        if cap == 0 || from >= to {
            return (from, Vec::new());
        }
        (from, (from..to).map(|i| self.buf[(i % cap) as usize]).collect())
    }
}

impl Default for Ring {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse rtl_433's piped-input time, e.g. "@5.550031s", into seconds.
pub fn stream_secs(t: &str) -> Option<f64> {
    t.strip_prefix('@')?.strip_suffix('s')?.parse().ok().filter(|s: &f64| s.is_finite() && *s >= 0.0)
}

#[derive(Clone, Debug, Serialize)]
pub struct RawSignal {
    /// Time of the event this burst belongs to (matches `EventRow::ts`).
    pub ts: i64,
    pub rate: u32,
    /// Centre frequency of the stream (the decoder frequency).
    pub freq_hz: u64,
    pub demod: Option<String>,
    /// Start of the cut relative to rtl_433's packet time, in µs.
    pub start_us: f64,
    pub duration_us: f64,
    /// Peak amplitude per trace point, 0–1000 relative to the burst's peak.
    pub envelope: Vec<u16>,
    /// Mean frequency offset per trace point in kHz, `None` where it's quiet.
    pub freq_khz: Vec<Option<f64>>,
    /// The envelope of just the first `detail_us`, finely enough to see
    /// single pulses.
    pub detail: Vec<u16>,
    pub detail_us: f64,
    /// Signal to noise of the burst, dB.
    pub snr_db: f32,
    /// Durations in µs: positive = pulse (carrier on / FSK mark), negative =
    /// gap (carrier off / FSK space), in order.
    pub pulses: Vec<i32>,
    pub pulses_truncated: bool,
    /// What `pulses` were sliced on.
    pub slicer: &'static str,
    pub iq_samples: usize,
    /// Interleaved I/Q, signed 8-bit (rtl_433's cs8 format).
    #[serde(skip)]
    pub iq: Vec<i8>,
}

impl RawSignal {
    /// rtl_433 reads the frequency and rate from the file name.
    pub fn file_name(&self) -> String {
        let mhz = format!("{:.3}", self.freq_hz as f64 / 1e6);
        let mhz = mhz.trim_end_matches('0').trim_end_matches('.');
        format!("g001_{mhz}M_{}k.cs8", self.rate as f64 / 1e3)
    }
}

/// Find the burst nearest `at` (a stream position) in `window` (which starts
/// at stream position `start`) and describe it.
pub fn extract(
    window: &[Complex32],
    start: u64,
    at: u64,
    rate: u32,
    freq_hz: u64,
    demod: Option<&str>,
    ts: i64,
) -> Option<RawSignal> {
    if window.is_empty() || rate == 0 {
        return None;
    }
    let mag: Vec<f32> = window.iter().map(|c| c.norm()).collect();

    // Activity per block of ~0.25 ms; noise = median block peak (most of the
    // window is quiet), active = 12 dB above it.
    let block = ((rate as f64 * 0.00025) as usize).max(4);
    let peaks: Vec<f32> = mag.chunks(block).map(|c| c.iter().cloned().fold(0.0, f32::max)).collect();
    let mut sorted = peaks.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let noise = sorted[sorted.len() / 2].max(1e-6);
    let thr = noise * 4.0;

    // Group active blocks into bursts, bridging short gaps.
    let max_gap = ((MAX_GAP_SECS * rate as f64) as usize / block).max(1);
    let mut bursts: Vec<(usize, usize)> = Vec::new();
    for (i, p) in peaks.iter().enumerate() {
        if *p <= thr {
            continue;
        }
        match bursts.last_mut() {
            Some((_, e)) if i - *e <= max_gap => *e = i,
            _ => bursts.push((i, i)),
        }
    }
    let at_block = (at.saturating_sub(start) as usize / block) as i64;
    let (b0, b1) = *bursts.iter().min_by_key(|(s, e)| {
        if (*s as i64..=*e as i64).contains(&at_block) {
            0
        } else {
            (*s as i64 - at_block).abs().min((*e as i64 - at_block).abs())
        }
    })?;

    let pad = (PAD_SECS * rate as f64) as usize;
    let s0 = (b0 * block).saturating_sub(pad);
    let s1 = ((b1 + 1) * block + pad).min(mag.len());
    let s1 = s1.min(s0 + (MAX_BURST_SECS * rate as f64) as usize);
    let iq = &window[s0..s1];
    let m = &mag[s0..s1];
    let peak = m.iter().cloned().fold(0.0, f32::max).max(1e-6);
    let us = 1e6 / rate as f64;

    // Instantaneous frequency (Hz) where the signal is up.
    let on_thr = (noise * peak).sqrt().max(thr);
    let mut inst = vec![None; iq.len()];
    for i in 1..iq.len() {
        if m[i] > on_thr && m[i - 1] > on_thr {
            let d = iq[i] * iq[i - 1].conj();
            inst[i] = Some(d.arg() * rate as f32 / (2.0 * PI));
        }
    }

    // Traces: TRACE_POINTS buckets (or one per sample for short bursts).
    let n = iq.len().min(TRACE_POINTS).max(1);
    let envelope = peaks_of(m, n, peak);
    let mut freq_khz = Vec::with_capacity(n);
    for k in 0..n {
        let a = k * iq.len() / n;
        let b = ((k + 1) * iq.len() / n).max(a + 1).min(iq.len());
        let (sum, cnt) = inst[a..b].iter().flatten().fold((0.0f64, 0u32), |(s, c), f| (s + *f as f64, c + 1));
        freq_khz.push((cnt > 0).then(|| ((sum / cnt as f64) / 100.0).round() / 10.0));
    }

    let dn = iq.len().min((DETAIL_SECS * rate as f64) as usize).max(1);
    let detail = peaks_of(&m[..dn], TRACE_POINTS, peak);

    // Slice into pulses: on carrier level for OOK, on frequency for FSK.
    let fsk = demod == Some("FSK");
    let levels: Vec<bool> = if fsk {
        let on: Vec<f32> = inst.iter().flatten().cloned().collect();
        let mid = if on.is_empty() { 0.0 } else { on.iter().sum::<f32>() / on.len() as f32 };
        // Gaps in carrier keep the previous tone.
        let mut last = false;
        inst.iter()
            .map(|f| {
                if let Some(f) = f {
                    last = *f > mid;
                }
                last
            })
            .collect()
    } else {
        m.iter().map(|v| *v > on_thr).collect()
    };
    let (pulses, truncated) = runs(&levels, us, 2);

    let scale = 127.0 / peak;
    let mut out = Vec::with_capacity(iq.len() * 2);
    for c in iq {
        out.push((c.re * scale).round().clamp(-127.0, 127.0) as i8);
        out.push((c.im * scale).round().clamp(-127.0, 127.0) as i8);
    }

    Some(RawSignal {
        ts,
        rate,
        freq_hz,
        demod: demod.map(str::to_string),
        start_us: (start as f64 + s0 as f64 - at as f64) * us,
        duration_us: iq.len() as f64 * us,
        envelope,
        freq_khz,
        detail,
        detail_us: dn as f64 * us,
        snr_db: 20.0 * (peak / noise).log10(),
        pulses,
        pulses_truncated: truncated,
        slicer: if fsk { "frequency" } else { "level" },
        iq_samples: iq.len(),
        iq: out,
    })
}

/// Peak of each of (at most) `n` buckets of `m`, 0–1000 relative to `peak`.
fn peaks_of(m: &[f32], n: usize, peak: f32) -> Vec<u16> {
    let n = m.len().min(n).max(1);
    (0..n)
        .map(|k| {
            let a = k * m.len() / n;
            let b = ((k + 1) * m.len() / n).max(a + 1).min(m.len());
            let pk = m[a..b].iter().cloned().fold(0.0, f32::max);
            (pk / peak * 1000.0).round() as u16
        })
        .collect()
}

/// Run lengths of `levels` in µs (positive = high), ignoring the leading and
/// trailing lows and merging glitches shorter than `min` samples.
fn runs(levels: &[bool], us: f64, min: usize) -> (Vec<i32>, bool) {
    let mut raw: Vec<(bool, usize)> = Vec::new();
    for &l in levels {
        match raw.last_mut() {
            Some((v, n)) if *v == l => *n += 1,
            _ => raw.push((l, 1)),
        }
    }
    // Merge glitches into the run before them.
    let mut merged: Vec<(bool, usize)> = Vec::new();
    for (v, n) in raw {
        match merged.last_mut() {
            Some((pv, pn)) if n < min || *pv == v => *pn += n,
            _ => merged.push((v, n)),
        }
    }
    while merged.first().is_some_and(|(v, _)| !v) {
        merged.remove(0);
    }
    while merged.last().is_some_and(|(v, _)| !v) {
        merged.pop();
    }
    let truncated = merged.len() > MAX_PULSES;
    merged.truncate(MAX_PULSES);
    let out = merged
        .into_iter()
        .map(|(v, n)| {
            let d = (n as f64 * us).round() as i32;
            if v { d } else { -d }
        })
        .collect();
    (out, truncated)
}

/// The latest burst of each device, keyed by device key, and the devices
/// whose next burst should be saved with its event ("log next burst").
#[derive(Clone, Default)]
pub struct RawStore {
    latest: Arc<Mutex<HashMap<String, Arc<RawSignal>>>>,
    armed: Arc<Mutex<HashSet<String>>>,
}

impl RawStore {
    pub fn insert(&self, key: String, sig: Arc<RawSignal>) {
        let mut m = self.latest.lock().unwrap();
        if m.len() >= MAX_DEVICES && !m.contains_key(&key) {
            if let Some(old) = m.iter().min_by_key(|(_, s)| s.ts).map(|(k, _)| k.clone()) {
                m.remove(&old);
            }
        }
        m.insert(key, sig);
    }

    pub fn get(&self, key: &str) -> Option<Arc<RawSignal>> {
        self.latest.lock().unwrap().get(key).cloned()
    }

    /// Save (or stop waiting to save) the device's next burst.
    pub fn arm(&self, key: &str, on: bool) {
        let mut a = self.armed.lock().unwrap();
        if on {
            a.insert(key.to_string());
        } else {
            a.remove(key);
        }
    }

    pub fn is_armed(&self, key: &str) -> bool {
        self.armed.lock().unwrap().contains(key)
    }

    /// True (once) if the device's next burst was to be saved.
    pub fn take_armed(&self, key: &str) -> bool {
        self.armed.lock().unwrap().remove(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OOK: 500 µs pulses with 1000/2000 µs gaps, in noise, at 250 kS/s.
    fn ook(rate: u32) -> (Vec<Complex32>, usize) {
        let s = |us: f64| (us * rate as f64 / 1e6) as usize;
        let mut v = vec![Complex32::new(0.01, 0.0); s(300_000.0)];
        let start = s(100_000.0);
        let mut i = start;
        for gap in [1000.0, 2000.0, 1000.0, 2000.0, 2000.0] {
            for k in 0..s(500.0) {
                let ph = k as f32 * 0.3;
                v[i + k] = Complex32::new(ph.cos(), ph.sin());
            }
            i += s(500.0) + s(gap);
        }
        (v, start)
    }

    #[test]
    fn ring_keeps_latest_and_slices_by_position() {
        let mut r = Ring::new();
        r.reset(10); // holds 20 samples
        let data: Vec<Complex32> = (0..50).map(|i| Complex32::new(i as f32, 0.0)).collect();
        r.push(&data);
        let (from, s) = r.slice(0, 50);
        assert_eq!(from, 30);
        assert_eq!(s.len(), 20);
        assert_eq!(s[0].re, 30.0);
        let (_, s) = r.slice(45, 48);
        assert_eq!(s.iter().map(|c| c.re).collect::<Vec<_>>(), vec![45.0, 46.0, 47.0]);
    }

    #[test]
    fn parses_stream_time() {
        assert_eq!(stream_secs("@5.550031s"), Some(5.550031));
        assert_eq!(stream_secs("2026-01-01"), None);
    }

    #[test]
    fn finds_burst_and_pulses() {
        let rate = 250_000;
        let (v, start) = ook(rate);
        let sig = extract(&v, 1000, 1000 + start as u64, rate, 433_920_000, Some("ASK"), 7).unwrap();
        assert_eq!(sig.slicer, "level");
        assert_eq!(sig.pulses, vec![500, -1000, 500, -2000, 500, -1000, 500, -2000, 500]);
        assert!(sig.start_us < 0.0 && sig.start_us > -3000.0, "{}", sig.start_us);
        assert!(sig.snr_db > 30.0);
        assert_eq!(sig.iq.len(), sig.iq_samples * 2);
        assert_eq!(sig.file_name(), "g001_433.92M_250k.cs8");
        assert_eq!(*sig.envelope.iter().max().unwrap(), 1000);
        // The zoomed envelope covers the first 25 ms (or the whole burst).
        assert!(sig.detail_us <= 25_000.0 && sig.detail_us > 10_000.0, "{}", sig.detail_us);
        assert!(sig.detail.iter().any(|v| *v > 900) && sig.detail.iter().any(|v| *v < 100));
    }

    #[test]
    fn fsk_slices_on_frequency() {
        let rate = 250_000u32;
        let mut v = vec![Complex32::new(0.001, 0.0); 50_000];
        let mut ph = 0.0f32;
        // mark +40 kHz / space -40 kHz, 100 µs (25 samples) each: M M S M S S
        let bits = [true, true, false, true, false, false];
        let mut i = 20_000;
        for b in bits {
            for _ in 0..25 {
                ph += 2.0 * PI * if b { 40_000.0 } else { -40_000.0 } / rate as f32;
                v[i] = Complex32::new(ph.cos(), ph.sin());
                i += 1;
            }
        }
        let sig = extract(&v, 0, 20_000, rate, 868_300_000, Some("FSK"), 1).unwrap();
        assert_eq!(sig.slicer, "frequency");
        // Leading/trailing space is trimmed; the inner runs are mark 200, space 100, mark 100
        // (the first mark loses a sample: frequency needs two samples of carrier).
        assert_eq!(sig.pulses.len(), 3, "{:?}", sig.pulses);
        for (got, want) in sig.pulses.iter().zip([200, -100, 100]) {
            assert!((got - want).abs() <= 8, "{:?}", sig.pulses);
        }
        let f: Vec<f64> = sig.freq_khz.iter().flatten().cloned().collect();
        assert!(f.iter().any(|k| (*k - 40.0).abs() < 1.0));
        assert!(f.iter().any(|k| (*k + 40.0).abs() < 1.0));
    }

    #[test]
    fn arming_fires_once() {
        let s = RawStore::default();
        assert!(!s.take_armed("k"));
        s.arm("k", true);
        assert!(s.is_armed("k"));
        assert!(s.take_armed("k"));
        assert!(!s.take_armed("k"));
        s.arm("k", true);
        s.arm("k", false);
        assert!(!s.is_armed("k"));
    }

    #[test]
    fn nothing_in_silence() {
        let v = vec![Complex32::new(0.01, 0.0); 10_000];
        assert!(extract(&v, 0, 5000, 250_000, 433_920_000, None, 0).is_none());
    }
}
