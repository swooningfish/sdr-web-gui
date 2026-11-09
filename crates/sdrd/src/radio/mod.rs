//! The radio owns the IQ source (a dongle, a test generator or a recording)
//! on a dedicated thread and broadcasts raw cu8 blocks to the DSP consumers.
//!
//! The kind of source follows from the selected device id, so the UI can move
//! between a USB dongle, an rtl_tcp server and the simulator without a
//! restart. `SDRD_SOURCE` only picks what to open when nothing was chosen yet.
//!
//! sdrd can run two receivers: the main one (spectrum, audio, decoders) and
//! an optional second dongle kept on 1090 MHz for ADS-B. They share a
//! [`Registry`], so the same dongle can't be opened by both.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::config::{Config, SourceKind};

mod replay;
mod sim_aircraft;
#[cfg(feature = "rtlsdr")]
mod rtlsdr;
mod rtltcp;
mod synthetic;

/// Most rtl_tcp servers remembered in the device list.
pub const MAX_TCP_SERVERS: usize = 8;
/// Device id for "no receiver" (the ADS-B receiver only).
pub const OFF: &str = "off";

/// Which of sdrd's receivers this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Main,
    Adsb,
}

impl Role {
    /// "in use by …"
    pub fn label(self) -> &'static str {
        match self {
            Role::Main => "the main receiver",
            Role::Adsb => "ADS-B",
        }
    }
    fn thread(self) -> &'static str {
        match self {
            Role::Main => "radio",
            Role::Adsb => "radio-adsb",
        }
    }
}

/// What the receivers share: each other's state (so one can't open a dongle
/// the other has), the rtl_tcp servers and the tuner types seen.
pub struct Registry {
    radios: Mutex<Vec<(Role, watch::Receiver<RadioState>)>>,
    /// rtl_tcp servers offered in the device list (`host:port`).
    tcp_servers: Mutex<Vec<String>>,
    /// Tuner types of devices we've opened (key: id + name, or the tcp id).
    tuners: Arc<Mutex<HashMap<String, String>>>,
}

impl Registry {
    pub fn new(cfg: &Config, saved_tcp: Vec<String>) -> Arc<Registry> {
        let mut servers: Vec<String> = cfg.rtltcp_addr.iter().cloned().collect();
        for a in saved_tcp {
            if parse_tcp_addr(&a).is_some() && !servers.contains(&a) {
                servers.push(a);
            }
        }
        servers.truncate(MAX_TCP_SERVERS);
        Arc::new(Registry {
            radios: Mutex::new(Vec::new()),
            tcp_servers: Mutex::new(servers),
            tuners: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// The other receivers' states.
    fn others(&self, me: Role) -> Vec<(Role, RadioState)> {
        self.radios
            .lock()
            .unwrap()
            .iter()
            .filter(|(r, _)| *r != me)
            .map(|(r, rx)| (*r, rx.borrow().clone()))
            .collect()
    }

    /// Which other receiver has this (exclusive) device selected.
    fn holder(&self, me: Role, id: &str) -> Option<Role> {
        if !matches!(kind_of(id), Some(SourceKind::Rtlsdr | SourceKind::RtlTcp)) {
            // Simulated receivers and the recording can be shared.
            return None;
        }
        self.others(me).into_iter().find(|(_, st)| st.device_id.as_deref() == Some(id)).map(|(r, _)| r)
    }
}

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

/// librtlsdr's `rtlsdr_tuner` enum, also sent in the rtl_tcp greeting.
pub(crate) fn tuner_name(t: u32) -> &'static str {
    match t {
        1 => "E4000",
        2 => "FC0012",
        3 => "FC0013",
        4 => "FC2580",
        5 => "R820T/R860",
        6 => "R828D",
        _ => "unknown",
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
    /// Stable id used to select it: `sn:<serial>`, `idx:<n>`, `tcp:<host:port>`,
    /// `replay` or `sim-*`.
    pub id: String,
    /// Which kind of source it is ([`SourceKind::name`]).
    pub source: &'static str,
    pub name: String,
    pub serial: Option<String>,
    pub tuner: Option<String>,
    pub ranges: Vec<FreqRange>,
    pub active: bool,
    /// Why details are missing, e.g. the dongle is busy in another program.
    pub note: Option<String>,
    /// The other receiver using it ("ADS-B", "the main receiver").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_by: Option<&'static str>,
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
    /// Device to open next.
    pub device: String,
    /// Tuner types seen per device (shared with [`RadioHandle`]).
    pub tuners: Arc<Mutex<HashMap<String, String>>>,
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

    /// The settings the user last asked for.
    pub fn current(&self) -> RadioSettings {
        self.desired.lock().unwrap().clone()
    }

    /// Sleep for `d`, waking early if another device is selected.
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

    /// Wait until a device is selected and return its id (for a source that
    /// can't continue, e.g. a missing recording).
    pub fn park(&mut self) -> String {
        loop {
            if let Some(d) = self.take().device {
                return d;
            }
            self.wait(Duration::from_secs(3600));
        }
    }

    /// No receiver selected.
    fn begin_off(&self) {
        self.state_tx.send_modify(|st| {
            st.source = OFF;
            st.can_tune = false;
            st.connected = false;
            st.device_id = Some(OFF.into());
            st.device_index = None;
            st.device = None;
            st.tuner = None;
            st.ranges = Vec::new();
            st.gains_db = Vec::new();
            st.error = None;
        });
    }

    /// Reset the state for a newly started source.
    fn begin(&self, kind: SourceKind) {
        self.state_tx.send_modify(|st| {
            st.source = kind.name();
            st.can_tune = kind != SourceKind::Replay;
            st.connected = false;
            st.device_id = Some(self.device.clone());
            st.device_index = None;
            st.device = None;
            st.tuner = None;
            st.ranges = Vec::new();
            st.gains_db = Vec::new();
            st.error = None;
        });
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
    role: Role,
    cmd_tx: mpsc::Sender<Cmd>,
    state_rx: watch::Receiver<RadioState>,
    /// Desired settings (what the user last asked for).
    desired: Arc<Mutex<RadioSettings>>,
    replay_file: Option<PathBuf>,
    registry: Arc<Registry>,
}

impl RadioHandle {
    pub fn subscribe(&self) -> watch::Receiver<RadioState> {
        self.state_rx.clone()
    }

    pub fn state(&self) -> RadioState {
        self.state_rx.borrow().clone()
    }

    pub fn configure(&self, patch: &RadioPatch) -> Result<RadioSettings, String> {
        if !self.state_rx.borrow().can_tune {
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

    /// Replace the settings outright (the ADS-B receiver, whose tuning is
    /// fixed): kept even while no device is open, used when one is.
    pub fn force_settings(&self, s: RadioSettings) -> Result<(), String> {
        let mut desired = self.desired.lock().unwrap();
        // Validate through the same checks as a patch.
        let s = desired.apply_patch(&RadioPatch {
            center_hz: Some(s.center_hz),
            sample_rate: Some(s.sample_rate),
            gain_db: s.gain_db,
            auto_gain: Some(s.gain_db.is_none()),
            ppm: Some(s.ppm),
            bias_tee: Some(s.bias_tee),
            direct_sampling: Some(s.direct_sampling),
        })?;
        *desired = s.clone();
        let _ = self.cmd_tx.send(Cmd::Settings(s));
        Ok(())
    }

    /// Switch to another receiver (an id from [`RadioHandle::devices`]). The
    /// source kind follows from the id.
    pub fn select(&self, id: &str) -> Result<(), String> {
        if id == OFF && self.role == Role::Adsb {
            return self.cmd_tx.send(Cmd::Select(id.into())).map_err(|_| "radio thread has stopped".to_string());
        }
        if let Some(r) = self.registry.holder(self.role, id) {
            return Err(format!("that receiver is in use by {}", r.label()));
        }
        match kind_of(id) {
            Some(SourceKind::Rtlsdr) if !cfg!(feature = "rtlsdr") => {
                return Err("sdrd was built without USB RTL-SDR support".into())
            }
            Some(SourceKind::Replay) if self.replay_file.is_none() => {
                return Err("no recording configured (SDRD_REPLAY_FILE)".into())
            }
            Some(_) => {}
            None => return Err("unknown device".into()),
        }
        self.cmd_tx
            .send(Cmd::Select(id.to_string()))
            .map_err(|_| "radio thread has stopped".to_string())
    }

    /// Remember an rtl_tcp server (`host:port`) in the device list. Returns
    /// the new list, to be saved.
    pub fn add_tcp_server(&self, addr: &str) -> Vec<String> {
        let mut l = self.registry.tcp_servers.lock().unwrap();
        if !l.iter().any(|a| a == addr) {
            l.push(addr.to_string());
            let excess = l.len().saturating_sub(MAX_TCP_SERVERS);
            l.drain(..excess);
        }
        l.clone()
    }

    pub fn remove_tcp_server(&self, addr: &str) -> Vec<String> {
        let mut l = self.registry.tcp_servers.lock().unwrap();
        l.retain(|a| a != addr);
        l.clone()
    }

    /// Receivers that can be selected, with what each can tune: USB dongles,
    /// remembered rtl_tcp servers, the recording if configured, and the
    /// simulated dongles.
    pub async fn devices(&self) -> Vec<DeviceInfo> {
        let st = self.state();
        let others = self.registry.others(self.role);
        let mut out: Vec<DeviceInfo> = Vec::new();

        #[cfg(feature = "rtlsdr")]
        {
            let tuners = self.registry.tuners.clone();
            let st = st.clone();
            let others: Vec<RadioState> = others.iter().map(|o| o.1.clone()).collect();
            out.extend(
                tokio::task::spawn_blocking(move || rtlsdr::enumerate(&st, &tuners, &others))
                    .await
                    .unwrap_or_default(),
            );
        }

        let mut servers = self.registry.tcp_servers.lock().unwrap().clone();
        if let Some(addr) = st.device_id.as_deref().and_then(|id| id.strip_prefix("tcp:")) {
            if !servers.iter().any(|a| a == addr) {
                servers.push(addr.to_string());
            }
        }
        for (_, o) in &others {
            if let Some(addr) = o.device_id.as_deref().and_then(|id| id.strip_prefix("tcp:")) {
                if !servers.iter().any(|a| a == addr) {
                    servers.push(addr.to_string());
                }
            }
        }
        let tuners = self.registry.tuners.lock().unwrap().clone();
        out.extend(servers.iter().map(|addr| rtltcp::device_info(addr, &st, &tuners)));

        if let Some(path) = &self.replay_file {
            let active = st.device_id.as_deref() == Some("replay");
            out.push(DeviceInfo {
                id: "replay".into(),
                source: SourceKind::Replay.name(),
                name: format!("Recording: {}", path.display()),
                serial: None,
                tuner: active.then(|| "file".to_string()),
                ranges: Vec::new(),
                active,
                note: None,
                used_by: None,
            });
        }

        out.extend(synthetic::devices(st.device_id.as_deref()));
        // Mark what the other receiver has open.
        for d in &mut out {
            if let Some(r) = self.registry.holder(self.role, &d.id) {
                d.used_by = Some(r.label());
                d.active = false;
            }
        }
        out
    }
}

/// `host:port` with a plain host name, IPv4 or `[IPv6]` address.
pub fn parse_tcp_addr(addr: &str) -> Option<(&str, u16)> {
    if addr.len() > 80 {
        return None;
    }
    let (host, port) = addr.rsplit_once(':')?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    let ok = if let Some(v6) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        !v6.is_empty() && v6.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
    } else {
        !host.is_empty()
            && !host.starts_with(['-', '.'])
            && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
    };
    ok.then_some((host, port))
}

/// Which source a device id belongs to; `None` for an invalid id.
pub fn kind_of(id: &str) -> Option<SourceKind> {
    if id.len() > 90 || id.chars().any(char::is_control) {
        return None;
    }
    if synthetic::DEVICES.iter().any(|d| d.id == id) {
        Some(SourceKind::Synthetic)
    } else if id == "replay" {
        Some(SourceKind::Replay)
    } else if let Some(addr) = id.strip_prefix("tcp:") {
        parse_tcp_addr(addr).map(|_| SourceKind::RtlTcp)
    } else if id.strip_prefix("idx:").is_some_and(|n| n.parse::<u32>().is_ok())
        || id.strip_prefix("sn:").is_some_and(|s| !s.is_empty())
    {
        Some(SourceKind::Rtlsdr)
    } else {
        None
    }
}

/// Whether `id` can be opened by this build and configuration.
fn usable(cfg: &Config, id: &str) -> bool {
    match kind_of(id) {
        Some(SourceKind::Rtlsdr) => cfg!(feature = "rtlsdr"),
        Some(SourceKind::Replay) => cfg.replay_file.is_some(),
        Some(_) => true,
        None => false,
    }
}

/// The device to open at start-up: the one last chosen in the UI if it can
/// still be opened, else what the environment says.
fn initial_device(cfg: &Config, role: Role, saved: Option<String>) -> String {
    if role == Role::Adsb {
        return saved
            .or_else(|| cfg.adsb_device.clone())
            .filter(|id| id == OFF || usable(cfg, id))
            .unwrap_or_else(|| OFF.into());
    }
    if let Some(id) = saved.filter(|id| usable(cfg, id)) {
        return id;
    }
    match cfg.source {
        SourceKind::Synthetic => synthetic::DEVICES[0].id.to_string(),
        SourceKind::Replay => "replay".into(),
        SourceKind::RtlTcp => format!("tcp:{}", cfg.rtltcp_addr.as_deref().unwrap_or_default()),
        SourceKind::Rtlsdr => match &cfg.device_serial {
            Some(s) => format!("sn:{s}"),
            None => format!("idx:{}", cfg.device_index),
        },
    }
}

/// Settings the main receiver starts with.
pub fn main_settings(cfg: &Config) -> RadioSettings {
    RadioSettings {
        center_hz: cfg.center_hz,
        sample_rate: cfg.sample_rate,
        gain_db: cfg.gain_db,
        ppm: cfg.ppm,
        bias_tee: cfg.bias_tee,
        direct_sampling: 0,
    }
}

pub fn spawn(
    cfg: &Config,
    role: Role,
    registry: Arc<Registry>,
    iq_tx: broadcast::Sender<Arc<IqBlock>>,
    saved_device: Option<String>,
    settings: RadioSettings,
) -> Result<RadioHandle> {
    let mut device = initial_device(cfg, role, saved_device);
    if let Some(r) = registry.holder(role, &device) {
        tracing::warn!(device = %device, "receiver is already used by {}; not opening it", r.label());
        device = if role == Role::Adsb { OFF.into() } else { synthetic::DEVICES[0].id.into() };
    }
    let (state_tx, state_rx) = watch::channel(RadioState {
        source: cfg.source.name(),
        can_tune: false,
        can_select: true,
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
    let desired = Arc::new(Mutex::new(settings));
    // Registered before the thread starts, so the other receiver sees it.
    state_tx.send_modify(|st| st.device_id = Some(device.clone()));
    registry.radios.lock().unwrap().push((role, state_rx.clone()));
    let mut ctx = SourceCtx {
        cmd_rx,
        state_tx,
        iq_tx,
        desired: desired.clone(),
        pending: Pending::default(),
        device,
        tuners: registry.tuners.clone(),
    };

    let replay_file = cfg.replay_file.clone();
    let file = replay_file.clone();
    std::thread::Builder::new().name(role.thread().into()).spawn(move || loop {
        if ctx.device == OFF {
            ctx.begin_off();
            ctx.device = ctx.park();
            continue;
        }
        // Each source runs until a device of another kind is selected, and
        // returns its id.
        let kind = kind_of(&ctx.device).unwrap_or(SourceKind::Synthetic);
        ctx.begin(kind);
        ctx.device = match kind {
            SourceKind::Synthetic => synthetic::run(&mut ctx),
            SourceKind::Replay => replay::run(&mut ctx, file.as_deref()),
            SourceKind::RtlTcp => rtltcp::run(&mut ctx),
            #[cfg(feature = "rtlsdr")]
            SourceKind::Rtlsdr => rtlsdr::run(&mut ctx),
            #[cfg(not(feature = "rtlsdr"))]
            SourceKind::Rtlsdr => {
                ctx.update_state(|st| st.error = Some("sdrd was built without USB RTL-SDR support".into()));
                ctx.park()
            }
        };
    })?;

    Ok(RadioHandle {
        role,
        cmd_tx,
        state_rx,
        desired,
        replay_file,
        registry,
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

/// The simulator's signals at `center_hz` (cu8, `secs` long), for other
/// modules' tests.
#[cfg(test)]
pub(crate) fn synth_for_tests() -> impl FnMut(u64, u32, f64) -> Vec<u8> {
    let mut gen = synthetic::SynthGen::new();
    move |center_hz, rate, secs| gen.generate(center_hz, rate, Some(30.0), (rate as f64 * secs) as usize)
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
        assert_eq!(kind_of("idx:0"), Some(SourceKind::Rtlsdr));
        assert_eq!(kind_of("sn:00000001"), Some(SourceKind::Rtlsdr));
        assert_eq!(kind_of("idx:x"), None);
        assert_eq!(kind_of("sn:"), None);
        assert_eq!(kind_of("sim-r820t"), Some(SourceKind::Synthetic));
        assert_eq!(kind_of("sim-nope"), None);
        assert_eq!(kind_of("replay"), Some(SourceKind::Replay));
        assert_eq!(kind_of("tcp:host.docker.internal:1234"), Some(SourceKind::RtlTcp));
        assert_eq!(kind_of("tcp:192.168.0.20:1234"), Some(SourceKind::RtlTcp));
        assert_eq!(kind_of("tcp:[::1]:1234"), Some(SourceKind::RtlTcp));
        assert_eq!(kind_of("tcp:host:0"), None);
        assert_eq!(kind_of("tcp:host"), None);
        assert_eq!(kind_of("tcp:a b:1234"), None);
        assert_eq!(kind_of("tcp:-x:1234"), None);
    }

    #[test]
    fn block_len_is_aligned() {
        for &r in SAMPLE_RATES {
            assert_eq!(block_len(r) % 16384, 0);
        }
    }
}
