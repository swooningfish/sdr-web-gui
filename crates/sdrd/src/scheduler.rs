//! Scheduler: decides what the single dongle listens to.
//!
//! - **Rotation** (optional): cycle through profiles (e.g. VHF block 60 s,
//!   ISM 30 s). Never hops away while an SSTV image is coming in.
//! - **Passes**: for each enabled satellite, passes are predicted from its
//!   TLE (fetched from CelesTrak every 12 h) and the station location. From
//!   shortly before AOS until LOS the radio is tuned to the satellite's
//!   profile, the Doppler shift is published to the decoders, and the
//!   downlink is recorded to WAV. Afterwards rotation resumes, or the
//!   previous manual tuning is restored.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map};
use sqlx::SqlitePool;
use tokio::sync::{mpsc, watch};

use crate::db;
use crate::decoders::sstv::SstvHandle;
use crate::decoders::DecodedEvent;
use crate::orbit::{self, doppler_hz, Look, Observer, Pass, Satellite};
use crate::radio::{covers, RadioHandle, RadioPatch, RadioSettings, MAX_FREQ_HZ, MIN_FREQ_HZ, SAMPLE_RATES};
use crate::recorder::{RecordDone, RecordSpec, RecorderHandle};

pub const PASSES: &str = "passes";
const CFG_KEY: &str = "schedule";
const LOCATION_KEY: &str = "station.location";
const TLE_MAX_AGE_MS: i64 = 12 * 3_600_000;
const PREDICT_HOURS: i64 = 48;
/// Longest we'll hold a rotation switch for an SSTV image in progress.
const MAX_HOLD_MS: i64 = 5 * 60_000;

/// The satellite currently being tracked, for decoders to Doppler-correct
/// their satellite channels.
#[derive(Clone, Debug)]
pub struct DopplerNow {
    /// km/s, positive when moving away.
    pub range_rate_km_s: f64,
}

pub type DopplerRx = watch::Receiver<Option<DopplerNow>>;

/// Current Doppler shift (Hz) for a satellite downlink on `freq_hz`, or 0
/// when no satellite is being tracked.
pub fn shift_hz(rx: &DopplerRx, freq_hz: u64) -> f64 {
    rx.borrow().as_ref().map_or(0.0, |d| doppler_hz(freq_hz as f64, d.range_rate_km_s))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub center_hz: u64,
    pub sample_rate: u32,
    pub dwell_s: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SatConfig {
    pub name: String,
    pub norad_id: u32,
    pub enabled: bool,
    pub min_elevation_deg: f64,
    /// Where to tune during a pass.
    pub center_hz: u64,
    pub sample_rate: u32,
    /// Record the downlink at `record_hz` (FM) to WAV.
    pub record: bool,
    pub record_hz: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScheduleConfig {
    pub rotation: bool,
    pub profiles: Vec<Profile>,
    pub satellites: Vec<SatConfig>,
    /// Tune this many seconds before AOS.
    pub lead_s: u32,
    /// Retune (and record) automatically for passes, and have browsers
    /// switch to the Passes page. Off: passes are only predicted and shown.
    #[serde(default = "default_true")]
    pub auto_switch: bool,
    /// Warn browsers this many seconds before the radio is retuned for a pass.
    #[serde(default = "default_notice_s")]
    pub notice_s: u32,
}

fn default_true() -> bool {
    true
}

fn default_notice_s() -> u32 {
    120
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        ScheduleConfig {
            rotation: false,
            profiles: vec![
                Profile { name: "VHF block".into(), center_hz: 145_300_000, sample_rate: 2_048_000, dwell_s: 60 },
                Profile { name: "ISM 433.92".into(), center_hz: 434_000_000, sample_rate: 1_024_000, dwell_s: 30 },
            ],
            satellites: vec![SatConfig {
                name: "ISS".into(),
                norad_id: 25544,
                enabled: true,
                min_elevation_deg: 10.0,
                // Covers SSTV 145.800 and APRS 145.825 with Doppler to spare.
                center_hz: 145_300_000,
                sample_rate: 2_048_000,
                record: true,
                record_hz: 145_800_000,
            }],
            lead_s: 30,
            auto_switch: true,
            notice_s: default_notice_s(),
        }
    }
}

fn check_tuning(center: u64, rate: u32) -> Result<(), String> {
    if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&center) {
        return Err("frequency out of range".into());
    }
    if !SAMPLE_RATES.contains(&rate) {
        return Err(format!("unsupported sample rate {rate}"));
    }
    Ok(())
}

fn check_name(n: &str) -> Result<(), String> {
    let len = n.chars().count();
    if len == 0 || len > 32 || n.chars().any(char::is_control) {
        return Err("names must be 1–32 characters".into());
    }
    Ok(())
}

impl ScheduleConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.profiles.len() > 8 || self.satellites.len() > 4 {
            return Err("at most 8 profiles and 4 satellites".into());
        }
        if self.lead_s > 300 {
            return Err("lead time must be at most 300 s".into());
        }
        if !(10..=1800).contains(&self.notice_s) {
            return Err("pass notice must be 10–1800 s".into());
        }
        for p in &self.profiles {
            check_name(&p.name)?;
            check_tuning(p.center_hz, p.sample_rate)?;
            if !(10..=3600).contains(&p.dwell_s) {
                return Err("dwell must be 10–3600 s".into());
            }
        }
        for s in &self.satellites {
            check_name(&s.name)?;
            check_tuning(s.center_hz, s.sample_rate)?;
            if !(0.0..=90.0).contains(&s.min_elevation_deg) {
                return Err("minimum elevation must be 0–90°".into());
            }
            if s.norad_id == 0 || s.norad_id > 999_999 {
                return Err("NORAD id must be 1–999999".into());
            }
            if s.record && !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&s.record_hz) {
                return Err("recording frequency out of range".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TleRecord {
    pub name: String,
    pub line1: String,
    pub line2: String,
    pub fetched_ms: i64,
    /// "celestrak" or "manual".
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct TleInfo {
    pub norad_id: u32,
    pub name: String,
    pub epoch_ms: Option<i64>,
    pub fetched_ms: Option<i64>,
    pub source: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActiveStatus {
    pub satellite: String,
    pub aos_ms: i64,
    pub los_ms: i64,
    pub max_el_deg: f64,
    pub test: bool,
    pub recording: bool,
    pub look: Option<Look>,
    /// Current shift at the recording frequency.
    pub doppler_hz: Option<f64>,
    pub record_hz: u64,
    pub track: Vec<(i64, f32, f32)>,
}

/// The next pass the scheduler would tune for.
#[derive(Clone, Debug, Serialize)]
pub struct Upcoming {
    pub satellite: String,
    pub aos_ms: i64,
    pub los_ms: i64,
    pub max_el_deg: f64,
    /// When the radio will be retuned (AOS minus the lead time).
    pub tune_ms: i64,
    pub center_hz: u64,
    pub sample_rate: u32,
    /// An admin chose to stay on the current frequency for this pass.
    pub skipped: bool,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct ScheduleStatus {
    /// "manual", "rotation" or "pass".
    pub mode: &'static str,
    pub profile: Option<String>,
    pub until_ms: Option<i64>,
    /// Rotation is waiting for an SSTV image to finish.
    pub holding: bool,
    pub active: Option<ActiveStatus>,
    pub upcoming: Option<Upcoming>,
    pub location: Option<Observer>,
    pub passes: Vec<Pass>,
    pub tles: Vec<TleInfo>,
}

enum Cmd {
    RefreshTle,
    TestPass(u32),
    /// End the pass in progress (test or real) now.
    Stop,
    /// Stay on the current tuning for (or un-skip) the pass with this AOS.
    Skip(i64, bool),
    Reload,
}

#[derive(Clone)]
pub struct SchedulerHandle {
    cfg_tx: Arc<watch::Sender<ScheduleConfig>>,
    loc_tx: Arc<watch::Sender<Option<Observer>>>,
    status_rx: watch::Receiver<ScheduleStatus>,
    cmd_tx: mpsc::Sender<Cmd>,
    pub passes_dir: Arc<std::path::Path>,
}

impl SchedulerHandle {
    pub fn config(&self) -> ScheduleConfig {
        self.cfg_tx.borrow().clone()
    }

    pub fn status(&self) -> ScheduleStatus {
        self.status_rx.borrow().clone()
    }

    pub async fn set_config(&self, db: &SqlitePool, cfg: ScheduleConfig) -> Result<()> {
        db::set_setting(db, CFG_KEY, &cfg).await?;
        self.cfg_tx.send_replace(cfg);
        let _ = self.cmd_tx.send(Cmd::Reload).await;
        Ok(())
    }

    pub async fn set_location(&self, db: &SqlitePool, obs: Observer) -> Result<()> {
        db::set_setting(db, LOCATION_KEY, &obs).await?;
        self.loc_tx.send_replace(Some(obs));
        let _ = self.cmd_tx.send(Cmd::Reload).await;
        Ok(())
    }

    pub async fn set_manual_tle(&self, db: &SqlitePool, name: &str, line1: &str, line2: &str) -> Result<u32> {
        let sat = Satellite::from_tle(name, line1, line2)?;
        let rec = TleRecord {
            name: name.to_string(),
            line1: line1.trim().to_string(),
            line2: line2.trim().to_string(),
            fetched_ms: db::now_ms(),
            source: "manual".into(),
        };
        db::set_setting(db, &format!("tle.{}", sat.norad_id), &rec).await?;
        let _ = self.cmd_tx.send(Cmd::Reload).await;
        Ok(sat.norad_id)
    }

    pub async fn refresh_tle(&self) {
        let _ = self.cmd_tx.send(Cmd::RefreshTle).await;
    }

    /// Stop the pass in progress; a real pass is then skipped, not restarted.
    pub async fn stop_pass(&self) {
        let _ = self.cmd_tx.send(Cmd::Stop).await;
    }

    pub async fn skip_pass(&self, aos_ms: i64, skip: bool) {
        let _ = self.cmd_tx.send(Cmd::Skip(aos_ms, skip)).await;
    }

    pub async fn test_pass(&self, seconds: u32) {
        let _ = self.cmd_tx.send(Cmd::TestPass(seconds)).await;
    }
}

async fn fetch_tle(norad: u32) -> Result<(String, String, String)> {
    tokio::task::spawn_blocking(move || -> Result<(String, String, String)> {
        let url = format!("https://celestrak.org/NORAD/elements/gp.php?CATNR={norad}&FORMAT=TLE");
        let body = ureq::get(&url)
            .timeout(Duration::from_secs(20))
            .set("User-Agent", "sdrd (sdr-web-gui)")
            .call()
            .map_err(|e| anyhow!("CelesTrak: {e}"))?
            .into_string()?;
        orbit::parse_tle_text(&body)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("CelesTrak returned no TLE for {norad}"))
    })
    .await?
}

#[allow(clippy::too_many_arguments)]
pub async fn spawn(
    db: SqlitePool,
    data_dir: PathBuf,
    radio: RadioHandle,
    sstv: SstvHandle,
    recorder: RecorderHandle,
    doppler_tx: watch::Sender<Option<DopplerNow>>,
    events: mpsc::Sender<DecodedEvent>,
) -> Result<SchedulerHandle> {
    let cfg = match db::get_setting::<ScheduleConfig>(&db, CFG_KEY).await {
        Ok(Some(c)) if c.validate().is_ok() => c,
        _ => ScheduleConfig::default(),
    };
    let loc = db::get_setting::<Observer>(&db, LOCATION_KEY).await.ok().flatten();
    let (cfg_tx, cfg_rx) = watch::channel(cfg);
    let (loc_tx, loc_rx) = watch::channel(loc);
    let (status_tx, status_rx) = watch::channel(ScheduleStatus { mode: "manual", ..Default::default() });
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let passes_dir: Arc<std::path::Path> = data_dir.join("passes").into();
    std::fs::create_dir_all(&passes_dir)?;
    let run = Run {
        db,
        radio,
        sstv,
        record_ctl: recorder.control,
        doppler_tx,
        events,
        status_tx,
        cfg_rx,
        loc_rx,
        passes_dir: passes_dir.clone(),
        sats: HashMap::new(),
        tles: Vec::new(),
        passes: Vec::new(),
        predicted_at: 0,
        rot_idx: None,
        rot_until: 0,
        hold_since: None,
        active: None,
        before_pass: None,
        pending: HashMap::new(),
        skipped: Vec::new(),
        tune_error: false,
    };
    tokio::spawn(run.main(cmd_rx, recorder.done));
    Ok(SchedulerHandle {
        cfg_tx: Arc::new(cfg_tx),
        loc_tx: Arc::new(loc_tx),
        status_rx,
        cmd_tx,
        passes_dir,
    })
}

struct Active {
    sat: SatConfig,
    pass: Pass,
    test: bool,
    recording: Option<PathBuf>,
}

struct Run {
    db: SqlitePool,
    radio: RadioHandle,
    sstv: SstvHandle,
    record_ctl: watch::Sender<Option<RecordSpec>>,
    doppler_tx: watch::Sender<Option<DopplerNow>>,
    events: mpsc::Sender<DecodedEvent>,
    status_tx: watch::Sender<ScheduleStatus>,
    cfg_rx: watch::Receiver<ScheduleConfig>,
    loc_rx: watch::Receiver<Option<Observer>>,
    passes_dir: Arc<std::path::Path>,
    sats: HashMap<u32, Satellite>,
    tles: Vec<TleInfo>,
    passes: Vec<(u32, Pass)>,
    predicted_at: i64,
    rot_idx: Option<usize>,
    rot_until: i64,
    hold_since: Option<i64>,
    active: Option<Active>,
    before_pass: Option<RadioSettings>,
    /// Recordings being finalised → the pass they belong to.
    pending: HashMap<PathBuf, (SatConfig, Pass, bool)>,
    tune_error: bool,
    /// AOS times of real passes stopped or skipped by hand (so they aren't
    /// started or restarted).
    skipped: Vec<i64>,
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

impl Run {
    async fn main(mut self, mut cmd_rx: mpsc::Receiver<Cmd>, mut done_rx: mpsc::UnboundedReceiver<RecordDone>) {
        self.refresh_tles(false).await;
        self.predict();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut tle_check = tokio::time::interval(Duration::from_secs(30 * 60));
        tle_check.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => self.step().await,
                _ = tle_check.tick() => {
                    if self.refresh_tles(false).await {
                        self.predict();
                    }
                }
                Some(cmd) = cmd_rx.recv() => match cmd {
                    Cmd::RefreshTle => {
                        self.refresh_tles(true).await;
                        self.predict();
                    }
                    Cmd::Reload => {
                        self.load_tles().await;
                        self.predict();
                        // Rotation settings may have changed: start over.
                        self.rot_idx = None;
                        self.rot_until = 0;
                    }
                    Cmd::TestPass(secs) => self.start_test(secs),
                    Cmd::Skip(aos, skip) => {
                        self.skipped.retain(|&a| a != aos);
                        if skip {
                            self.skipped.push(aos);
                            tracing::info!(aos_ms = aos, "upcoming pass skipped by hand");
                        }
                        self.step().await;
                    }
                    Cmd::Stop => {
                        if let Some(a) = self.active.as_ref() {
                            if !a.test {
                                self.skipped.push(a.pass.aos_ms);
                            }
                            tracing::info!("{} pass stopped by hand", a.sat.name);
                            self.end().await;
                        }
                    }
                },
                Some(done) = done_rx.recv() => self.recorded(done).await,
            }
        }
    }

    /// Load stored TLEs for all configured satellites.
    async fn load_tles(&mut self) {
        let cfg = self.cfg_rx.borrow().clone();
        self.sats.clear();
        let mut infos = Vec::new();
        for s in &cfg.satellites {
            let mut info = TleInfo { norad_id: s.norad_id, name: s.name.clone(), ..Default::default() };
            if let Ok(Some(rec)) = db::get_setting::<TleRecord>(&self.db, &format!("tle.{}", s.norad_id)).await {
                match Satellite::from_tle(&s.name, &rec.line1, &rec.line2) {
                    Ok(sat) => {
                        info.epoch_ms = Some(sat.epoch_ms);
                        info.fetched_ms = Some(rec.fetched_ms);
                        info.source = Some(rec.source);
                        self.sats.insert(s.norad_id, sat);
                    }
                    Err(e) => info.error = Some(e.to_string()),
                }
            }
            // Keep the last fetch error until a fetch succeeds.
            if let Some(old) = self.tles.iter().find(|t| t.norad_id == s.norad_id) {
                if info.error.is_none() && old.error.is_some() && info.fetched_ms == old.fetched_ms {
                    info.error = old.error.clone();
                }
            }
            infos.push(info);
        }
        self.tles = infos;
    }

    /// Fetch TLEs that are missing or older than 12 h (or all, if `force`).
    /// Returns whether anything changed.
    async fn refresh_tles(&mut self, force: bool) -> bool {
        self.load_tles().await;
        let cfg = self.cfg_rx.borrow().clone();
        let now = db::now_ms();
        let mut changed = false;
        for s in &cfg.satellites {
            let stale = self
                .tles
                .iter()
                .find(|t| t.norad_id == s.norad_id)
                .and_then(|t| t.fetched_ms)
                .is_none_or(|f| now - f > TLE_MAX_AGE_MS);
            if !(force || stale) {
                continue;
            }
            match fetch_tle(s.norad_id).await {
                Ok((name, l1, l2)) => {
                    let rec = TleRecord {
                        name: if name.is_empty() { s.name.clone() } else { name },
                        line1: l1,
                        line2: l2,
                        fetched_ms: now,
                        source: "celestrak".into(),
                    };
                    if let Err(e) = Satellite::from_tle(&s.name, &rec.line1, &rec.line2) {
                        self.set_tle_error(s.norad_id, e.to_string());
                        continue;
                    }
                    if let Err(e) = db::set_setting(&self.db, &format!("tle.{}", s.norad_id), &rec).await {
                        tracing::warn!("storing TLE: {e}");
                    }
                    tracing::info!("TLE for {} updated from CelesTrak", s.name);
                    changed = true;
                }
                Err(e) => {
                    tracing::warn!("TLE for {}: {e}", s.name);
                    self.set_tle_error(s.norad_id, e.to_string());
                }
            }
        }
        if changed {
            self.load_tles().await;
        }
        changed
    }

    fn set_tle_error(&mut self, norad: u32, e: String) {
        if let Some(t) = self.tles.iter_mut().find(|t| t.norad_id == norad) {
            t.error = Some(e);
        }
    }

    fn predict(&mut self) {
        let now = db::now_ms();
        self.predicted_at = now;
        self.passes.clear();
        let Some(obs) = *self.loc_rx.borrow() else { return };
        let cfg = self.cfg_rx.borrow().clone();
        for s in cfg.satellites.iter().filter(|s| s.enabled) {
            let Some(sat) = self.sats.get(&s.norad_id) else { continue };
            for mut p in orbit::passes(sat, &obs, now - 20 * 60_000, now + PREDICT_HOURS * 3_600_000, s.min_elevation_deg)
            {
                if p.los_ms > now {
                    p.satellite = s.name.clone();
                    self.passes.push((s.norad_id, p));
                }
            }
        }
        self.passes.sort_by_key(|(_, p)| p.aos_ms);
    }

    fn tune(&mut self, center_hz: u64, sample_rate: u32, why: &str) {
        let cur = self.radio.state().settings;
        if cur.center_hz == center_hz && cur.sample_rate == sample_rate {
            return;
        }
        let patch = RadioPatch { center_hz: Some(center_hz), sample_rate: Some(sample_rate), ..Default::default() };
        match self.radio.configure(&patch) {
            Ok(_) => {
                self.tune_error = false;
                tracing::info!(center_hz, sample_rate, "scheduler: {why}");
            }
            Err(e) => {
                if !self.tune_error {
                    tracing::warn!("scheduler can't tune: {e}");
                }
                self.tune_error = true;
            }
        }
    }

    fn start(&mut self, sat: SatConfig, pass: Pass, test: bool) {
        let cfg = self.cfg_rx.borrow().clone();
        if !cfg.rotation {
            self.before_pass = Some(self.radio.state().settings);
        }
        tracing::info!("{} pass{}: tuning for AOS", sat.name, if test { " (test)" } else { "" });
        self.tune(sat.center_hz, sat.sample_rate, &format!("{} pass", sat.name));
        let recording = sat.record.then(|| {
            let path = self.passes_dir.join(format!(
                "{}-{}{}.wav",
                pass.aos_ms,
                slug(&sat.name),
                if test { "-test" } else { "" }
            ));
            self.record_ctl.send_replace(Some(RecordSpec { path: path.clone(), freq_hz: sat.record_hz, doppler: !test }));
            path
        });
        self.active = Some(Active { sat, pass, test, recording });
    }

    fn start_test(&mut self, secs: u32) {
        if self.active.is_some() {
            return;
        }
        let cfg = self.cfg_rx.borrow().clone();
        let sat = cfg
            .satellites
            .first()
            .cloned()
            .unwrap_or_else(|| ScheduleConfig::default().satellites[0].clone());
        let now = db::now_ms();
        let pass = Pass {
            satellite: sat.name.clone(),
            aos_ms: now,
            los_ms: now + secs as i64 * 1000,
            max_ms: now,
            max_el_deg: 0.0,
            aos_az_deg: 0.0,
            los_az_deg: 0.0,
            track: Vec::new(),
        };
        self.start(sat, pass, true);
    }

    async fn end(&mut self) {
        let Some(a) = self.active.take() else { return };
        tracing::info!("{} pass ended", a.sat.name);
        self.doppler_tx.send_replace(None);
        match a.recording {
            Some(path) => {
                self.record_ctl.send_replace(None);
                self.pending.insert(path, (a.sat, a.pass, a.test));
            }
            None => self.log_pass(&a.sat, &a.pass, a.test, None).await,
        }
        if let Some(prev) = self.before_pass.take() {
            self.tune(prev.center_hz, prev.sample_rate, "restoring tuning after pass");
        }
        // Rotation picks up straight away.
        self.rot_until = 0;
    }

    async fn recorded(&mut self, done: RecordDone) {
        let Some((sat, pass, test)) = self.pending.remove(&done.path) else { return };
        self.log_pass(&sat, &pass, test, Some(done)).await;
    }

    async fn log_pass(&self, sat: &SatConfig, pass: &Pass, test: bool, rec: Option<RecordDone>) {
        let mut data = Map::new();
        data.insert("model".into(), json!("Pass"));
        data.insert("id".into(), json!(sat.name));
        data.insert("aos_ms".into(), json!(pass.aos_ms));
        data.insert("los_ms".into(), json!(pass.los_ms));
        data.insert("max_el_deg".into(), json!((pass.max_el_deg * 10.0).round() / 10.0));
        data.insert("test".into(), json!(test));
        if let Some(r) = rec {
            let name = r.path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            data.insert("audio".into(), json!(name));
            data.insert("seconds".into(), json!((r.seconds * 10.0).round() / 10.0));
        }
        let freq = sat.record.then_some(sat.record_hz);
        if self.events.send(DecodedEvent::new(PASSES, freq, data)).await.is_err() {
            tracing::warn!("event queue closed");
        }
    }

    async fn step(&mut self) {
        let now = db::now_ms();
        if now - self.predicted_at > 10 * 60_000 {
            self.predict();
        }
        let cfg = self.cfg_rx.borrow().clone();
        // Satellites and profiles the selected SDR can't receive are skipped.
        let ranges = self.radio.state().ranges;

        if self.active.as_ref().is_some_and(|a| now > a.pass.los_ms + 5_000) {
            self.end().await;
        }
        let lead = cfg.lead_s as i64 * 1000;
        self.skipped.retain(|aos| now - aos < 3 * 3_600_000);
        let eligible = |id: u32| {
            cfg.satellites
                .iter()
                .any(|s| s.norad_id == id && s.enabled && covers(&ranges, s.center_hz))
        };
        if self.active.is_none() && cfg.auto_switch {
            let next = self.passes.iter().find(|(id, p)| {
                now >= p.aos_ms - lead && now < p.los_ms && !self.skipped.contains(&p.aos_ms) && eligible(*id)
            });
            if let Some((id, p)) = next.cloned() {
                let sat = cfg.satellites.iter().find(|s| s.norad_id == id).cloned().unwrap();
                self.start(sat, p, false);
            }
        }
        // The next pass we'd tune for, so browsers can warn before it happens.
        let active_aos = self.active.as_ref().map(|a| a.pass.aos_ms);
        let upcoming = self
            .passes
            .iter()
            .find(|(id, p)| now < p.los_ms && Some(p.aos_ms) != active_aos && eligible(*id))
            .and_then(|(id, p)| {
                let s = cfg.satellites.iter().find(|s| s.norad_id == *id)?;
                Some(Upcoming {
                    satellite: s.name.clone(),
                    aos_ms: p.aos_ms,
                    los_ms: p.los_ms,
                    max_el_deg: p.max_el_deg,
                    tune_ms: p.aos_ms - lead,
                    center_hz: s.center_hz,
                    sample_rate: s.sample_rate,
                    skipped: self.skipped.contains(&p.aos_ms),
                })
            });

        // Doppler and live look angles for the pass in progress.
        let mut active_status = None;
        if let Some(a) = self.active.as_ref() {
            let obs = *self.loc_rx.borrow();
            let look = match (a.test, obs, self.sats.get(&a.sat.norad_id)) {
                (false, Some(o), Some(sat)) => orbit::look(sat, &o, now),
                _ => None,
            };
            self.doppler_tx
                .send_replace(look.map(|l| DopplerNow { range_rate_km_s: l.range_rate_km_s }));
            active_status = Some(ActiveStatus {
                satellite: a.sat.name.clone(),
                aos_ms: a.pass.aos_ms,
                los_ms: a.pass.los_ms,
                max_el_deg: a.pass.max_el_deg,
                test: a.test,
                recording: a.recording.is_some(),
                look,
                doppler_hz: look.map(|l| doppler_hz(a.sat.record_hz as f64, l.range_rate_km_s)),
                record_hz: a.sat.record_hz,
                track: a.pass.track.clone(),
            });
        }

        // Rotation.
        let mut holding = false;
        if self.active.is_none() && cfg.rotation && !cfg.profiles.is_empty() {
            let receiving = self.sstv.status().channels.iter().any(|c| c.receiving.is_some());
            if now >= self.rot_until && receiving && self.rot_idx.is_some() {
                let since = *self.hold_since.get_or_insert(now);
                holding = now - since < MAX_HOLD_MS;
            }
            if now >= self.rot_until && !holding {
                self.hold_since = None;
                let n = cfg.profiles.len();
                let first = self.rot_idx.map_or(0, |i| i + 1);
                let next = (0..n).map(|k| (first + k) % n).find(|&i| covers(&ranges, cfg.profiles[i].center_hz));
                match next {
                    Some(idx) => {
                        let p = cfg.profiles[idx].clone();
                        self.rot_idx = Some(idx);
                        self.rot_until = now + p.dwell_s as i64 * 1000;
                        self.tune(p.center_hz, p.sample_rate, &format!("rotation → {}", p.name));
                    }
                    // Nothing this device can receive; look again shortly.
                    None => self.rot_until = now + 10_000,
                }
            }
        }

        let (mode, profile, until) = if let Some(a) = self.active.as_ref() {
            ("pass", Some(a.sat.name.clone()), Some(a.pass.los_ms))
        } else if cfg.rotation && !cfg.profiles.is_empty() {
            let p = self.rot_idx.and_then(|i| cfg.profiles.get(i)).map(|p| p.name.clone());
            ("rotation", p, Some(self.rot_until))
        } else {
            ("manual", None, None)
        };
        let status = ScheduleStatus {
            mode,
            profile,
            until_ms: until,
            holding,
            active: active_status,
            upcoming,
            location: *self.loc_rx.borrow(),
            passes: self.passes.iter().map(|(_, p)| p.clone()).collect(),
            tles: self.tles.clone(),
        };
        self.status_tx.send_replace(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        assert!(ScheduleConfig::default().validate().is_ok());
        let mut c = ScheduleConfig::default();
        c.profiles[0].dwell_s = 1;
        assert!(c.validate().is_err());
        let mut c = ScheduleConfig::default();
        c.satellites[0].sample_rate = 123;
        assert!(c.validate().is_err());
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("ISS (ZARYA)"), "iss-zarya");
    }
}
