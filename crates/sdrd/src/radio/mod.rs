//! The radio owns the IQ source (a dongle, a test generator or a recording)
//! on a dedicated thread and broadcasts raw cu8 blocks to the DSP consumers.

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::config::{Config, SourceKind};

mod replay;
#[cfg(feature = "rtlsdr")]
mod rtlsdr;
mod synthetic;

/// Sample rates offered in the UI. All are valid for the RTL2832U; rates above
/// 2.4 MS/s tend to drop samples on USB so they are not offered.
pub const SAMPLE_RATES: &[u32] = &[
    250_000, 1_024_000, 1_536_000, 1_800_000, 1_920_000, 2_048_000, 2_400_000,
];
/// Widest span any supported tuner covers (HF direct sampling up to E4000).
/// What a particular device can reach is narrower: see [`tuner_ranges`].
pub const MIN_FREQ_HZ: u64 = 500_000;
pub const MAX_FREQ_HZ: u64 = 2_200_000_000;

/// A continuous span of frequencies a device can tune, inclusive.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct FreqRange {
    pub lo_hz: u64,
    pub hi_hz: u64,
}

impl FreqRange {
    const fn khz(lo: u64, hi: u64) -> FreqRange {
        FreqRange { lo_hz: lo * 1000, hi_hz: hi * 1000 }
    }
    pub fn contains(&self, hz: u64) -> bool {
        (self.lo_hz..=self.hi_hz).contains(&hz)
    }
}

/// HF reception by sampling the antenna directly (V3-style dongles).
pub const HF_DIRECT: FreqRange = FreqRange::khz(500, 28_800);

/// What a tuner chip can receive. `product` is the USB product string: the
/// RTL-SDR Blog V4 pairs an R828D with an HF upconverter that the
/// rtl-sdr-blog driver switches in automatically.
pub fn tuner_ranges(tuner: &str, product: &str) -> Vec<FreqRange> {
    match tuner {
        "R828D" if product.contains("V4") => vec![FreqRange::khz(500, 1_766_000)],
        "R820T/R860" | "R828D" => vec![FreqRange::khz(24_000, 1_766_000)],
        "E4000" => vec![FreqRange::khz(52_000, 1_100_000), FreqRange::khz(1_250_000, 2_200_000)],
        "FC0012" => vec![FreqRange::khz(22_000, 948_600)],
        "FC0013" => vec![FreqRange::khz(22_000, 1_100_000)],
        "FC2580" => vec![FreqRange::khz(146_000, 308_000), FreqRange::khz(438_000, 924_000)],
        // Unknown tuner: assume the common R820T range.
        _ => vec![FreqRange::khz(24_000, 1_766_000)],
    }
}

/// Whether a device with these ranges can receive `hz`. No ranges means the
/// device is unknown (not connected, or a recording) and nothing is ruled out.
pub fn covers(ranges: &[FreqRange], hz: u64) -> bool {
    ranges.is_empty() || ranges.iter().any(|r| r.contains(hz))
}

/// The closest frequency to `hz` that the device can receive.
pub fn clamp_into(ranges: &[FreqRange], hz: u64) -> u64 {
    if covers(ranges, hz) {
        return hz;
    }
    ranges
        .iter()
        .map(|r| hz.clamp(r.lo_hz, r.hi_hz))
        .min_by_key(|f| f.abs_diff(hz))
        .unwrap_or(hz)
}

/// "24–1766 MHz" or "146–308, 438–924 MHz".
pub fn describe_ranges(ranges: &[FreqRange]) -> String {
    if ranges.is_empty() {
        return "unknown".into();
    }
    let mhz = |hz: u64| {
        let v = hz as f64 / 1e6;
        if v.fract() == 0.0 {
            format!("{v:.0}")
        } else {
            format!("{v}")
        }
    };
    let parts: Vec<String> = ranges.iter().map(|r| format!("{}–{}", mhz(r.lo_hz), mhz(r.hi_hz))).collect();
    format!("{} MHz", parts.join(", "))
}

/// A receiver that can be selected.
#[derive(Clone, Debug, Serialize)]
pub struct DeviceInfo {
    /// Stable id used to select it: `sn:<serial>`, `idx:<n>` or `sim-*`.
    pub id: String,
    pub name: String,
    pub serial: Option<String>,
    pub tuner: Option<String>,
    pub ranges: Vec<FreqRange>,
    pub active: bool,
    /// Why details are missing, e.g. the dongle is busy in another program.
    pub note: Option<String>,
}

/// A block of interleaved unsigned 8-bit I/Q samples, as produced by librtlsdr.
pub struct IqBlock {
    pub center_hz: u64,
    pub sample_rate: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RadioSettings {
    pub center_hz: u64,
    pub sample_rate: u32,
    /// `None` = tuner AGC.
    pub gain_db: Option<f32>,
    pub ppm: i32,
    pub bias_tee: bool,
    /// 0 = off, 1 = I branch, 2 = Q branch (HF on V3-style dongles).
    pub direct_sampling: u8,
}

/// Partial update from the API. `auto_gain: true` wins over `gain_db`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioPatch {
    pub center_hz: Option<u64>,
    pub sample_rate: Option<u32>,
    pub gain_db: Option<f32>,
    pub auto_gain: Option<bool>,
    pub ppm: Option<i32>,
    pub bias_tee: Option<bool>,
    pub direct_sampling: Option<u8>,
}

impl RadioSettings {
    pub fn apply_patch(&self, p: &RadioPatch) -> Result<RadioSettings, String> {
        let mut s = self.clone();
        if let Some(f) = p.center_hz {
            if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&f) {
                return Err(format!(
                    "frequency must be between {} and {} Hz",
                    MIN_FREQ_HZ, MAX_FREQ_HZ
                ));
            }
            s.center_hz = f;
        }
        if let Some(r) = p.sample_rate {
            if !SAMPLE_RATES.contains(&r) {
                return Err(format!("unsupported sample rate {r}"));
            }
            s.sample_rate = r;
        }
        if let Some(g) = p.gain_db {
            if !(0.0..=60.0).contains(&g) || !g.is_finite() {
                return Err("gain must be between 0 and 60 dB".into());
            }
            s.gain_db = Some(g);
        }
        if p.auto_gain == Some(true) {
            s.gain_db = None;
        }
        if let Some(ppm) = p.ppm {
            if !(-200..=200).contains(&ppm) {
                return Err("ppm must be between -200 and 200".into());
            }
            s.ppm = ppm;
        }
        if let Some(b) = p.bias_tee {
            s.bias_tee = b;
        }
        if let Some(ds) = p.direct_sampling {
            if ds > 2 {
                return Err("direct_sampling must be 0, 1 or 2".into());
            }
            s.direct_sampling = ds;
        }
        Ok(s)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RadioState {
    pub source: &'static str,
    pub can_tune: bool,
    /// Receivers can be listed and switched between (`/api/radio/devices`).
    pub can_select: bool,
    pub connected: bool,
    /// Id of the selected device (see [`DeviceInfo::id`]).
    pub device_id: Option<String>,
    /// USB index of the open dongle, to mark it in the device list.
    #[serde(skip)]
    #[cfg_attr(not(feature = "rtlsdr"), allow(dead_code))]
    pub device_index: Option<u32>,
    pub device: Option<String>,
    pub tuner: Option<String>,
    /// What the selected device can receive; empty = unknown.
    pub ranges: Vec<FreqRange>,
    pub settings: RadioSettings,
    pub gains_db: Vec<f32>,
    pub sample_rates: &'static [u32],
    pub error: Option<String>,
}

pub(crate) enum Cmd {
    Settings(RadioSettings),
    Select(String),
}

/// Commands received but not yet acted on.
#[derive(Default)]
pub(crate) struct Pending {
    pub settings: Option<RadioSettings>,
    pub device: Option<String>,
}

/// Shared between the source thread implementations.
pub(crate) struct SourceCtx {
    cmd_rx: mpsc::Receiver<Cmd>,
    state_tx: watch::Sender<RadioState>,
    iq_tx: broadcast::Sender<Arc<IqBlock>>,
    desired: Arc<Mutex<RadioSettings>>,
    pending: Pending,
    pub initial: RadioSettings,
    /// Device to open first.
    pub device: String,
}

impl SourceCtx {
    fn stash(&mut self, c: Cmd) {
        match c {
            Cmd::Settings(s) => self.pending.settings = Some(s),
            Cmd::Select(d) => self.pending.device = Some(d),
        }
    }

    /// Commands queued since the last call; older settings are superseded.
    pub fn take(&mut self) -> Pending {
        while let Ok(c) = self.cmd_rx.try_recv() {
            self.stash(c);
        }
        std::mem::take(&mut self.pending)
    }

    /// Sleep for `d`, waking early if another device is selected.
    #[cfg_attr(not(feature = "rtlsdr"), allow(dead_code))]
    pub fn wait(&mut self, d: Duration) {
        let until = Instant::now() + d;
        while self.pending.device.is_none() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.cmd_rx.recv_timeout(left) {
                Ok(c) => self.stash(c),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(left);
                    break;
                }
            }
        }
    }

    /// After switching device: move the centre frequency inside the new
    /// device's range if needed, so it starts on something it can receive.
    pub fn fit_to(&self, s: &mut RadioSettings, ranges: &[FreqRange]) {
        let f = clamp_into(ranges, s.center_hz);
        if f != s.center_hz {
            tracing::info!(from = s.center_hz, to = f, "retuning into the new device's range");
            s.center_hz = f;
            self.desired.lock().unwrap().center_hz = f;
        }
    }

    pub fn update_state(&self, f: impl FnOnce(&mut RadioState)) {
        self.state_tx.send_modify(f);
    }

    pub fn publish(&self, block: IqBlock) {
        // No receivers is fine; DSP consumers come and go.
        let _ = self.iq_tx.send(Arc::new(block));
    }
}

#[derive(Clone)]
pub struct RadioHandle {
    cmd_tx: mpsc::Sender<Cmd>,
    state_rx: watch::Receiver<RadioState>,
    /// Desired settings (what the user last asked for).
    desired: Arc<Mutex<RadioSettings>>,
    can_tune: bool,
    source: SourceKind,
    /// Tuner types of dongles we've opened to identify (key: id + name).
    #[cfg_attr(not(feature = "rtlsdr"), allow(dead_code))]
    tuners: Arc<Mutex<HashMap<String, String>>>,
}

impl RadioHandle {
    pub fn subscribe(&self) -> watch::Receiver<RadioState> {
        self.state_rx.clone()
    }

    pub fn state(&self) -> RadioState {
        self.state_rx.borrow().clone()
    }

    pub fn configure(&self, patch: &RadioPatch) -> Result<RadioSettings, String> {
        if !self.can_tune {
            return Err("this source cannot be retuned".into());
        }
        let mut desired = self.desired.lock().unwrap();
        let next = desired.apply_patch(patch)?;
        if patch.center_hz.is_some() || patch.direct_sampling.is_some() {
            let ranges = self.state_rx.borrow().ranges.clone();
            let hf = next.direct_sampling != 0 && HF_DIRECT.contains(next.center_hz);
            if !hf && !covers(&ranges, next.center_hz) {
                return Err(format!(
                    "{:.3} MHz is outside this SDR's range ({})",
                    next.center_hz as f64 / 1e6,
                    describe_ranges(&ranges)
                ));
            }
        }
        self.cmd_tx
            .send(Cmd::Settings(next.clone()))
            .map_err(|_| "radio thread has stopped".to_string())?;
        *desired = next.clone();
        Ok(next)
    }

    /// Switch to another receiver (an id from [`RadioHandle::devices`]).
    pub fn select(&self, id: &str) -> Result<(), String> {
        if !self.state_rx.borrow().can_select {
            return Err(format!("the {} source has no other devices", self.source.name()));
        }
        if !valid_id(self.source, id) {
            return Err("unknown device".into());
        }
        self.cmd_tx
            .send(Cmd::Select(id.to_string()))
            .map_err(|_| "radio thread has stopped".to_string())
    }

    /// Receivers that can be selected, with what each can tune.
    pub async fn devices(&self) -> Vec<DeviceInfo> {
        let st = self.state();
        match self.source {
            SourceKind::Synthetic => synthetic::devices(st.device_id.as_deref()),
            SourceKind::Replay => vec![DeviceInfo {
                id: "replay".into(),
                name: st.device.unwrap_or_else(|| "Replay".into()),
                serial: None,
                tuner: st.tuner,
                ranges: st.ranges,
                active: true,
                note: None,
            }],
            #[cfg(feature = "rtlsdr")]
            SourceKind::Rtlsdr => {
                let tuners = self.tuners.clone();
                tokio::task::spawn_blocking(move || rtlsdr::enumerate(&st, &tuners))
                    .await
                    .unwrap_or_default()
            }
            #[cfg(not(feature = "rtlsdr"))]
            SourceKind::Rtlsdr => Vec::new(),
        }
    }
}

fn valid_id(source: SourceKind, id: &str) -> bool {
    match source {
        SourceKind::Synthetic => synthetic::DEVICES.iter().any(|d| d.id == id),
        SourceKind::Replay => false,
        SourceKind::Rtlsdr => {
            id.len() <= 80
                && !id.chars().any(char::is_control)
                && (id.strip_prefix("idx:").is_some_and(|n| n.parse::<u32>().is_ok())
                    || id.strip_prefix("sn:").is_some_and(|s| !s.is_empty()))
        }
    }
}

/// The device to open at start-up: the one last chosen in the UI if it's
/// still a valid id, else what the environment says.
fn initial_device(cfg: &Config, saved: Option<String>) -> String {
    if let Some(id) = saved.filter(|id| valid_id(cfg.source, id)) {
        return id;
    }
    match cfg.source {
        SourceKind::Synthetic => synthetic::DEVICES[0].id.to_string(),
        SourceKind::Replay => "replay".into(),
        SourceKind::Rtlsdr => match &cfg.device_serial {
            Some(s) => format!("sn:{s}"),
            None => format!("idx:{}", cfg.device_index),
        },
    }
}

pub fn spawn(
    cfg: &Config,
    iq_tx: broadcast::Sender<Arc<IqBlock>>,
    saved_device: Option<String>,
) -> Result<RadioHandle> {
    let settings = RadioSettings {
        center_hz: cfg.center_hz,
        sample_rate: cfg.sample_rate,
        gain_db: cfg.gain_db,
        ppm: cfg.ppm,
        bias_tee: cfg.bias_tee,
        direct_sampling: 0,
    };
    let can_tune = cfg.source != SourceKind::Replay;
    let (state_tx, state_rx) = watch::channel(RadioState {
        source: cfg.source.name(),
        can_tune,
        can_select: cfg.source != SourceKind::Replay,
        connected: false,
        device_id: None,
        device_index: None,
        device: None,
        tuner: None,
        ranges: Vec::new(),
        settings: settings.clone(),
        gains_db: Vec::new(),
        sample_rates: SAMPLE_RATES,
        error: None,
    });
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let desired = Arc::new(Mutex::new(settings.clone()));
    let ctx = SourceCtx {
        cmd_rx,
        state_tx,
        iq_tx,
        desired: desired.clone(),
        pending: Pending::default(),
        initial: settings,
        device: initial_device(cfg, saved_device),
    };

    let cfg = cfg.clone();
    std::thread::Builder::new()
        .name("radio".into())
        .spawn(move || match cfg.source {
            SourceKind::Synthetic => synthetic::run(ctx),
            SourceKind::Replay => replay::run(ctx, cfg.replay_file.clone().unwrap()),
            #[cfg(feature = "rtlsdr")]
            SourceKind::Rtlsdr => rtlsdr::run(ctx),
            #[cfg(not(feature = "rtlsdr"))]
            SourceKind::Rtlsdr => unreachable!("checked in Config::from_env"),
        })?;

    Ok(RadioHandle {
        cmd_tx,
        state_rx,
        desired,
        can_tune,
        source: cfg.source,
        tuners: Arc::new(Mutex::new(HashMap::new())),
    })
}

/// Convert a float sample (nominally -1..1) to the cu8 representation.
#[inline]
pub(crate) fn to_cu8(v: f32) -> u8 {
    (v * 127.5 + 127.5).round().clamp(0.0, 255.0) as u8
}

/// Block length in bytes for ~50 ms of samples, a multiple of 16 KiB
/// (librtlsdr requires multiples of 512; 16 KiB keeps USB transfers efficient).
pub(crate) fn block_len(sample_rate: u32) -> usize {
    let bytes = (sample_rate as usize * 2) / 20;
    bytes.div_ceil(16384).max(1) * 16384
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> RadioSettings {
        RadioSettings {
            center_hz: 145_300_000,
            sample_rate: 2_048_000,
            gain_db: Some(30.0),
            ppm: 0,
            bias_tee: false,
            direct_sampling: 0,
        }
    }

    #[test]
    fn patch_validates() {
        let s = base();
        assert!(s
            .apply_patch(&RadioPatch { center_hz: Some(10), ..Default::default() })
            .is_err());
        assert!(s
            .apply_patch(&RadioPatch { sample_rate: Some(3_000_000), ..Default::default() })
            .is_err());
        let n = s
            .apply_patch(&RadioPatch {
                center_hz: Some(433_920_000),
                auto_gain: Some(true),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(n.center_hz, 433_920_000);
        assert_eq!(n.gain_db, None);
    }

    #[test]
    fn tuner_coverage() {
        let r820 = tuner_ranges("R820T/R860", "RTL2838UHIDIR");
        assert!(covers(&r820, 145_800_000));
        assert!(!covers(&r820, 7_100_000));
        let v4 = tuner_ranges("R828D", "Blog V4");
        assert!(covers(&v4, 7_100_000));
        let fc = tuner_ranges("FC2580", "");
        assert!(!covers(&fc, 144_800_000));
        assert!(!covers(&fc, 433_920_000));
        assert!(covers(&fc, 162_000_000));
        assert_eq!(clamp_into(&fc, 145_300_000), 146_000_000);
        assert_eq!(clamp_into(&fc, 400_000_000), 438_000_000);
        assert!(covers(&[], 1));
        assert_eq!(describe_ranges(&fc), "146–308, 438–924 MHz");
        assert_eq!(describe_ranges(&tuner_ranges("FC0012", "")), "22–948.6 MHz");
    }

    #[test]
    fn device_ids() {
        assert!(valid_id(SourceKind::Rtlsdr, "idx:0"));
        assert!(valid_id(SourceKind::Rtlsdr, "sn:00000001"));
        assert!(!valid_id(SourceKind::Rtlsdr, "idx:x"));
        assert!(!valid_id(SourceKind::Rtlsdr, "sn:"));
        assert!(valid_id(SourceKind::Synthetic, "sim-r820t"));
        assert!(!valid_id(SourceKind::Synthetic, "idx:0"));
    }

    #[test]
    fn block_len_is_aligned() {
        for &r in SAMPLE_RATES {
            assert_eq!(block_len(r) % 16384, 0);
        }
    }
}
