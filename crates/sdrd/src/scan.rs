//! Band scan: steps the main receiver through broadcast FM or DAB Band III
//! and lists the stations it finds, to be added to the bookmarks.
//!
//! FM: a quick power sweep of 87.5–108 MHz in 1.5 MHz windows finds the
//! carriers on the 50 kHz raster (well above the noise floor and the
//! strongest within ±150 kHz), then each window with stations is captured
//! for a few seconds and every station in it is demodulated for its RDS
//! name (PS) and PI.
//!
//! DAB: each Band III channel is checked for the null symbol a multiplex
//! has every 96 ms; where there is one, a few seconds are captured and the
//! FIC decoded for the ensemble and service names (see [`crate::dab`]).
//! DAB stations are saved in DAB mode with their service id, so picking
//! one tunes the multiplex and plays that service.
//!
//! While a scan runs the scheduler's rotation waits; a satellite pass stops
//! the scan. Afterwards the receiver goes back to where it was.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use num_complex::Complex32;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::audio::demod::{Demod, Mode, Params};
use crate::audio::rds::RdsInfo;
use crate::dab;
use crate::radio::{covers, IqBlock, RadioHandle, RadioPatch};
use crate::scheduler::SchedulerHandle;

/// Every step listens at this rate (DAB needs exactly it; FM windows are
/// 1.5 MHz of it).
const RATE: u32 = dab::RATE;
const FM_LO: u64 = 87_500_000;
const FM_HI: u64 = 108_000_000;
const FM_RASTER: u64 = 50_000;
/// Half the span of each FM window (the rest of the capture is roll-off).
const FM_HALF: u64 = 750_000;
/// A station: this far above the band's noise floor (dB).
const FM_THRESHOLD_DB: f32 = 10.0;
/// Seconds of power sweep per FM window, and of audio for RDS.
const FM_SWEEP_S: f64 = 0.25;
const FM_RDS_S: f64 = 5.0;
/// Seconds to look for a DAB null, and to read a multiplex's FIC.
const DAB_PROBE_S: f64 = 0.25;
const DAB_READ_S: f64 = 3.0;
/// Null power below this share of the average: a multiplex is there.
const DAB_NULL_RATIO: f64 = 0.7;
/// Blocks dropped after each retune while the tuner settles.
const SETTLE_BLOCKS: usize = 2;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Band {
    Fm,
    Dab,
}

impl Band {
    fn label(self) -> &'static str {
        match self {
            Band::Fm => "FM",
            Band::Dab => "DAB",
        }
    }
}

/// A station found.
#[derive(Clone, Debug, Serialize)]
pub struct Found {
    /// Stable within a scan: `fm-<hz>` or `dab-<channel>-<sid>`.
    pub id: String,
    pub band: Band,
    pub name: String,
    pub freq_hz: u64,
    pub mode: Mode,
    pub sample_rate: Option<u32>,
    /// DAB: the service id, for playing it.
    pub service_id: Option<u16>,
    pub group: String,
    pub notes: String,
    /// Signal above the noise floor (FM) or signal-to-noise (DAB), dB.
    pub level_db: f32,
    /// RDS PI (FM) or service id (DAB), hex.
    pub ident: Option<String>,
    /// DAB: the multiplex and its channel.
    pub ensemble: Option<String>,
    pub channel: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScanStatus {
    pub running: bool,
    pub band: Option<Band>,
    /// What it's doing now.
    pub step: String,
    /// 0–1.
    pub progress: f32,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    /// Who started it.
    pub by: Option<String>,
    pub stopped: bool,
    pub error: Option<String>,
    /// Things worth knowing that aren't stations (e.g. an unreadable multiplex).
    pub notes: Vec<String>,
    pub found: Vec<Found>,
}

#[derive(Clone, Default)]
pub struct ScanHandle {
    status: Arc<Mutex<ScanStatus>>,
    cancel: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
}

/// Why a scan ended early.
enum Halt {
    Stopped,
    Pass,
    Failed(String),
}

impl From<String> for Halt {
    fn from(s: String) -> Self {
        Halt::Failed(s)
    }
}

impl ScanHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set while a scan has the receiver (the scheduler leaves it alone).
    pub fn active_flag(&self) -> Arc<AtomicBool> {
        self.active.clone()
    }

    pub fn status(&self) -> ScanStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn update(&self, f: impl FnOnce(&mut ScanStatus)) {
        f(&mut self.status.lock().unwrap());
    }

    pub fn start(
        &self,
        band: Band,
        radio: RadioHandle,
        iq_tx: broadcast::Sender<Arc<IqBlock>>,
        schedule: SchedulerHandle,
        by: String,
    ) -> Result<(), String> {
        let st = radio.state();
        if !st.can_tune || !st.connected {
            return Err("the receiver can't be tuned (no dongle open, or a recording)".into());
        }
        if schedule.status().active.is_some() {
            return Err("a satellite pass is using the receiver; scan after it".into());
        }
        let (lo, hi) = match band {
            Band::Fm => (FM_LO, FM_HI),
            Band::Dab => (dab::CHANNELS[0].1, dab::CHANNELS[dab::CHANNELS.len() - 1].1),
        };
        if !covers(&st.ranges, lo) && !covers(&st.ranges, hi) {
            return Err(format!("this receiver can't tune the {} band", band.label()));
        }
        if self.active.swap(true, Ordering::SeqCst) {
            return Err("a scan is already running".into());
        }
        self.cancel.store(false, Ordering::SeqCst);
        *self.status.lock().unwrap() = ScanStatus {
            running: true,
            band: Some(band),
            step: "Starting".into(),
            started_ms: Some(crate::db::now_ms()),
            by: Some(by),
            ..Default::default()
        };
        let ctx = Ctx { h: self.clone(), radio, iq_tx, schedule };
        tokio::spawn(ctx.run(band));
        Ok(())
    }
}

struct Ctx {
    h: ScanHandle,
    radio: RadioHandle,
    iq_tx: broadcast::Sender<Arc<IqBlock>>,
    schedule: SchedulerHandle,
}

impl Ctx {
    async fn run(self, band: Band) {
        let before = self.radio.state().settings;
        tracing::info!(band = band.label(), "band scan started");
        let res = match band {
            Band::Fm => self.fm().await,
            Band::Dab => self.dab().await,
        };
        // Back to where it was, unless a pass has taken the receiver.
        if !matches!(res, Err(Halt::Pass)) {
            let _ = self.radio.configure(&RadioPatch {
                center_hz: Some(before.center_hz),
                sample_rate: Some(before.sample_rate),
                ..Default::default()
            });
        }
        let found = self.h.status().found.len();
        self.h.update(|s| {
            s.running = false;
            s.finished_ms = Some(crate::db::now_ms());
            match res {
                Ok(()) => {
                    s.progress = 1.0;
                    s.step = "Done".into();
                }
                Err(Halt::Stopped) => {
                    s.stopped = true;
                    s.step = "Stopped".into();
                }
                Err(Halt::Pass) => {
                    s.stopped = true;
                    s.step = "Stopped for a satellite pass".into();
                }
                Err(Halt::Failed(e)) => {
                    s.step = "Failed".into();
                    s.error = Some(e);
                }
            }
        });
        tracing::info!(band = band.label(), found, "band scan finished");
        self.h.active.store(false, Ordering::SeqCst);
    }

    fn check(&self) -> Result<(), Halt> {
        if self.h.cancel.load(Ordering::SeqCst) {
            return Err(Halt::Stopped);
        }
        if self.schedule.status().active.is_some() {
            return Err(Halt::Pass);
        }
        Ok(())
    }

    fn step(&self, progress: f32, text: String) {
        self.h.update(|s| {
            s.progress = progress.clamp(0.0, 1.0);
            s.step = text;
        });
    }

    /// Tune to `hz` and capture `secs` of cu8 samples in one piece.
    async fn capture(&self, hz: u64, secs: f64) -> Result<Vec<u8>, Halt> {
        self.check()?;
        let mut rx = self.iq_tx.subscribe();
        self.radio
            .configure(&RadioPatch { center_hz: Some(hz), sample_rate: Some(RATE), ..Default::default() })
            .map_err(Halt::Failed)?;
        let want = (secs * RATE as f64) as usize * 2;
        let mut out = Vec::with_capacity(want);
        let mut settle = SETTLE_BLOCKS;
        let mut gaps = 0;
        while out.len() < want {
            let block = match tokio::time::timeout(Duration::from_secs(3), rx.recv()).await {
                Ok(Ok(b)) => b,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    // Samples were dropped: start this capture again.
                    gaps += 1;
                    if gaps > 5 {
                        return Err(Halt::Failed("the receiver is dropping samples".into()));
                    }
                    out.clear();
                    continue;
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return Err(Halt::Failed("the receiver stopped".into()))
                }
                Err(_) => {
                    self.check()?;
                    return Err(Halt::Failed(format!(
                        "no samples at {:.3} MHz (was the receiver retuned during the scan?)",
                        hz as f64 / 1e6
                    )));
                }
            };
            self.check()?;
            if block.center_hz != hz || block.sample_rate != RATE {
                continue;
            }
            if settle > 0 {
                settle -= 1;
                continue;
            }
            out.extend_from_slice(&block.data);
        }
        out.truncate(want);
        Ok(out)
    }

    fn push(&self, f: Found) {
        self.h.update(|s| {
            s.found.retain(|o| o.id != f.id);
            s.found.push(f);
            s.found.sort_by_key(|o| o.freq_hz);
        });
    }

    fn note(&self, n: String) {
        self.h.update(|s| s.notes.push(n));
    }

    async fn fm(&self) -> Result<(), Halt> {
        let ranges = self.radio.state().ranges;
        let windows: Vec<u64> = (0..)
            .map(|i| FM_LO - FM_RASTER + FM_HALF + i * 2 * FM_HALF)
            .take_while(|c| c - FM_HALF <= FM_HI)
            .filter(|&c| covers(&ranges, c))
            .collect();
        // Pass 1: power on the raster.
        let mut chans: Vec<(u64, f32, u64)> = Vec::new();
        for (i, &c) in windows.iter().enumerate() {
            self.step(
                0.3 * i as f32 / windows.len() as f32,
                format!("Sweeping {:.1}–{:.1} MHz", (c - FM_HALF) as f64 / 1e6, (c + FM_HALF) as f64 / 1e6),
            );
            let data = self.capture(c, FM_SWEEP_S).await?;
            let psd = tokio::task::spawn_blocking(move || spectrum(&data)).await.map_err(|e| e.to_string())?;
            let mut f = FM_LO.max(c - FM_HALF).div_ceil(FM_RASTER) * FM_RASTER;
            while f < c + FM_HALF && f <= FM_HI {
                chans.push((f, channel_db(&psd, f as f64 - c as f64, 60_000.0), c));
                f += FM_RASTER;
            }
        }
        let stations = pick_fm(&chans);
        if stations.is_empty() {
            self.note("No FM stations stood out from the noise. Is the antenna connected?".into());
            return Ok(());
        }
        // Pass 2: RDS, a window at a time.
        let mut done = 0;
        let total = stations.len();
        for &c in &windows {
            let here: Vec<(u64, f32)> = stations.iter().filter(|s| s.2 == c).map(|s| (s.0, s.1)).collect();
            if here.is_empty() {
                continue;
            }
            self.step(
                0.3 + 0.7 * done as f32 / total as f32,
                format!(
                    "Reading RDS from {} station{} near {:.1} MHz",
                    here.len(),
                    if here.len() == 1 { "" } else { "s" },
                    c as f64 / 1e6
                ),
            );
            let n = here.len();
            let data = self.capture(c, FM_RDS_S).await?;
            let freqs: Vec<u64> = here.iter().map(|h| h.0).collect();
            let infos = tokio::task::spawn_blocking(move || read_rds(&data, c, &freqs))
                .await
                .map_err(|e| e.to_string())?;
            for ((hz, level), info) in here.into_iter().zip(infos) {
                self.push(fm_found(hz, level, info));
            }
            done += n;
        }
        Ok(())
    }

    async fn dab(&self) -> Result<(), Halt> {
        let ranges = self.radio.state().ranges;
        let chans: Vec<(&str, u64)> = dab::CHANNELS.iter().copied().filter(|c| covers(&ranges, c.1)).collect();
        let mut seen = 0;
        for (i, &(name, hz)) in chans.iter().enumerate() {
            let p = i as f32 / chans.len() as f32;
            self.step(p, format!("Channel {name} ({:.3} MHz)", hz as f64 / 1e6));
            let probe = self.capture(hz, DAB_PROBE_S).await?;
            let here = tokio::task::spawn_blocking(move || dab_here(&probe)).await.map_err(|e| e.to_string())?;
            if !here {
                continue;
            }
            seen += 1;
            self.step(p, format!("Reading the multiplex on {name} ({:.3} MHz)", hz as f64 / 1e6));
            let data = self.capture(hz, DAB_READ_S).await?;
            let (dec, depth) = tokio::task::spawn_blocking(move || (dab::decode(&data), dab::null_depth(&data)))
                .await
                .map_err(|e| e.to_string())?;
            let snr = depth.map_or(0.0, |d| d.1) as f32;
            let e = &dec.ensemble;
            if e.services.is_empty() {
                self.note(if let Some(l) = &e.label {
                    format!("{name}: {l} found, but no station names were read in time.")
                } else {
                    format!("{name}: a DAB-like signal, but its station list couldn't be read (weak, or a neighbouring multiplex).")
                });
                continue;
            }
            let ens = e.label.clone().unwrap_or_else(|| format!("Multiplex {name}"));
            for (sid, label) in &e.services {
                self.push(Found {
                    id: format!("dab-{name}-{sid:04X}"),
                    band: Band::Dab,
                    name: label.clone(),
                    freq_hz: hz,
                    mode: Mode::Dab,
                    sample_rate: Some(RATE),
                    service_id: Some(*sid),
                    group: "DAB".into(),
                    notes: format!("DAB {name} · {ens} · SId {sid:04X}"),
                    level_db: snr,
                    ident: Some(format!("{sid:04X}")),
                    ensemble: Some(ens.clone()),
                    channel: Some(name.to_string()),
                });
            }
        }
        if seen == 0 {
            self.note("No DAB multiplexes found. DAB needs a Band III antenna (a vertical about 35 cm long).".into());
        }
        Ok(())
    }
}

/// Whether a capture tuned to a DAB channel holds a multiplex centred on
/// it: null symbols, and more power in the middle than at the edges (a
/// strong multiplex on the next channel shows its nulls through its edge,
/// but leaves the middle empty).
fn dab_here(probe: &[u8]) -> bool {
    let Some((ratio, _)) = dab::null_depth(probe) else { return false };
    if ratio > DAB_NULL_RATIO {
        return false;
    }
    let psd = spectrum(probe);
    let inner = channel_db(&psd, 0.0, 500_000.0);
    let edge = channel_db(&psd, -900_000.0, 100_000.0).max(channel_db(&psd, 900_000.0, 100_000.0));
    inner > edge - 3.0
}

fn fm_found(hz: u64, level: f32, info: Option<RdsInfo>) -> Found {
    let mhz = hz as f64 / 1e6;
    let ps = info.as_ref().and_then(|i| i.ps.as_deref()).map(str::trim).filter(|s| !s.is_empty());
    let pi = info.as_ref().and_then(|i| i.pi.clone());
    let notes = match &pi {
        Some(pi) => format!("FM {mhz:.2} MHz · RDS PI {pi}"),
        None => format!("FM {mhz:.2} MHz · no RDS heard"),
    };
    Found {
        id: format!("fm-{hz}"),
        band: Band::Fm,
        name: ps.map_or_else(|| format!("FM {mhz:.2}"), str::to_string),
        freq_hz: hz,
        mode: Mode::Wfm,
        sample_rate: None,
        service_id: None,
        group: "FM".into(),
        notes,
        level_db: level,
        ident: pi,
        ensemble: None,
        channel: None,
    }
}

/// Averaged power spectrum of a cu8 capture: 2048 bins of 1 kHz at 2.048
/// MS/s, DC in the middle (index 1024).
fn spectrum(cu8: &[u8]) -> Vec<f32> {
    const N: usize = 2048;
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
    let win: Vec<f32> =
        (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
    let mut acc = vec![0f32; N];
    let mut buf = vec![Complex32::default(); N];
    let mut count = 0;
    for chunk in cu8.chunks_exact(2 * N) {
        for (i, (b, w)) in buf.iter_mut().zip(&win).enumerate() {
            *b = Complex32::new(chunk[2 * i] as f32 - 127.5, chunk[2 * i + 1] as f32 - 127.5) * *w;
        }
        fft.process(&mut buf);
        for (a, b) in acc.iter_mut().zip(&buf) {
            *a += b.norm_sqr();
        }
        count += 1;
    }
    let mut out = vec![0f32; N];
    for (i, a) in acc.iter().enumerate() {
        out[(i + N / 2) % N] = a / count.max(1) as f32;
    }
    out
}

/// Mean power (dB) within ±`half` Hz of `offset` Hz from the centre.
fn channel_db(psd: &[f32], offset: f64, half: f64) -> f32 {
    let bin_hz = RATE as f64 / psd.len() as f64;
    let mid = psd.len() as f64 / 2.0;
    let lo = ((offset - half) / bin_hz + mid).round().max(0.0) as usize;
    let hi = (((offset + half) / bin_hz + mid).round() as usize).min(psd.len() - 1);
    let sum: f32 = psd[lo..=hi].iter().sum();
    10.0 * (sum / (hi - lo + 1) as f32 + 1e-12).log10()
}

/// Stations from raster powers (frequency, dB, window): clear of the noise
/// floor, a local peak, and the strongest within ±150 kHz. Returns
/// (frequency, dB above the floor, window).
fn pick_fm(chans: &[(u64, f32, u64)]) -> Vec<(u64, f32, u64)> {
    if chans.is_empty() {
        return Vec::new();
    }
    let mut levels: Vec<f32> = chans.iter().map(|c| c.1).collect();
    levels.sort_by(f32::total_cmp);
    let floor = levels[levels.len() / 4];
    let at = |hz: u64| chans.iter().find(|c| c.0 == hz).map(|c| c.1);
    let mut cand: Vec<(u64, f32, u64)> = chans
        .iter()
        .filter(|c| c.1 >= floor + FM_THRESHOLD_DB)
        .filter(|c| {
            [c.0.wrapping_sub(FM_RASTER), c.0 + FM_RASTER]
                .iter()
                .all(|&n| at(n).is_none_or(|v| v <= c.1))
        })
        .map(|c| (c.0, c.1 - floor, c.2))
        .collect();
    cand.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut out: Vec<(u64, f32, u64)> = Vec::new();
    for c in cand {
        if out.iter().all(|o| o.0.abs_diff(c.0) > 150_000) {
            out.push(c);
        }
    }
    out.sort_by_key(|o| o.0);
    out
}

/// RDS from each station in a capture, in parallel (stops early once the
/// station name is complete).
fn read_rds(data: &[u8], center_hz: u64, freqs: &[u64]) -> Vec<Option<RdsInfo>> {
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
    let mut out: Vec<Option<RdsInfo>> = vec![None; freqs.len()];
    for (chunk_f, chunk_o) in freqs.chunks(threads).zip(out.chunks_mut(threads)) {
        std::thread::scope(|s| {
            for (&hz, slot) in chunk_f.iter().zip(chunk_o.iter_mut()) {
                s.spawn(move || *slot = rds_of(data, center_hz, hz));
            }
        });
    }
    out
}

fn rds_of(data: &[u8], center_hz: u64, hz: u64) -> Option<RdsInfo> {
    let p = Params { freq_hz: hz, mode: Mode::Wfm, squelch_db: None, nr: false, cw: false, afc: false, digital: false, service: None };
    let mut d = Demod::new(&p, center_hz, RATE);
    let mut audio = Vec::new();
    let mut last: Option<RdsInfo> = None;
    // 50 ms at a time, like the live receiver.
    for chunk in data.chunks(2 * RATE as usize / 20) {
        audio.clear();
        d.process(chunk, &mut audio);
        if let Some(info) = d.take_rds() {
            let done = info.ps.is_some();
            last = Some(info);
            if done {
                break;
            }
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_peaks_above_the_floor() {
        // Noise at -50 dB with stations at 88.6 (spilling into its
        // neighbours), 96.1 and a weak 100.0 just under the threshold.
        let mut chans = Vec::new();
        let mut f = FM_LO;
        while f <= FM_HI {
            let v = match f {
                88_600_000 => -20.0,
                88_550_000 | 88_650_000 => -28.0,
                96_100_000 => -30.0,
                96_150_000 => -35.0,
                100_000_000 => -42.0,
                _ => -50.0,
            };
            chans.push((f, v, 0));
            f += FM_RASTER;
        }
        let got: Vec<(u64, i32)> = pick_fm(&chans).iter().map(|c| (c.0, c.1.round() as i32)).collect();
        assert_eq!(got, [(88_600_000, 30), (96_100_000, 20)]);
    }

    #[test]
    fn dab_from_the_simulator() {
        let mut gen = crate::radio::synth_for_tests();
        assert!(!dab_here(&gen(dab::CHANNELS[0].1, RATE, DAB_PROBE_S)), "empty channel");
        assert!(dab_here(&gen(222_064_000, RATE, DAB_PROBE_S)), "11D");
        // 11C and 12A sit either side of 11D, 12A also next to 12B.
        assert!(!dab_here(&gen(220_352_000, RATE, DAB_PROBE_S)), "11C");
        assert!(!dab_here(&gen(223_936_000, RATE, DAB_PROBE_S)), "12A");
        let d = dab::decode(&gen(222_064_000, RATE, 1.0));
        assert_eq!(d.ensemble.label.as_deref(), Some("SIM DAB"));
        assert_eq!(d.ensemble.services.get(&0xC223).map(String::as_str), Some("Sim News 24"));
        assert_eq!(d.ensemble.services.len(), 5);
    }

    #[test]
    fn fm_windows_cover_the_band() {
        let windows: Vec<u64> = (0..)
            .map(|i| FM_LO - FM_RASTER + FM_HALF + i * 2 * FM_HALF)
            .take_while(|c| c - FM_HALF <= FM_HI)
            .collect();
        assert!(windows[0] - FM_HALF <= FM_LO);
        assert!(windows.last().unwrap() + FM_HALF > FM_HI);
        assert!(windows.windows(2).all(|w| w[1] - w[0] == 2 * FM_HALF));
    }

    #[test]
    fn rds_from_a_simulated_station() {
        // The simulator's 96.1 MHz station, heard from a window centred 400 kHz away.
        let mut gen = crate::radio::synth_for_tests();
        let data = gen(96_500_000, RATE, 3.0);
        let psd = spectrum(&data);
        let on = channel_db(&psd, -400_000.0, 60_000.0);
        let off = channel_db(&psd, 300_000.0, 60_000.0);
        assert!(on - off > 15.0, "station {on} dB, empty {off} dB");
        let info = read_rds(&data, 96_500_000, &[96_100_000]).remove(0).expect("RDS");
        assert_eq!(info.ps.as_deref().map(str::trim), Some("SIM FM"));
        let f = fm_found(96_100_000, 20.0, Some(info));
        assert_eq!(f.name, "SIM FM");
        assert_eq!(f.notes, "FM 96.10 MHz · RDS PI C0DE");
    }
}
