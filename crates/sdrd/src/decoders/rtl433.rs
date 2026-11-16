//! rtl_433 runs as a child process fed from our own IQ stream: the decoder
//! thread cuts ~256 kHz around the configured frequency out of the capture
//! (when it is in the tuned band), and pipes it to `rtl_433 -r cs16:-`. JSON
//! lines from its stdout become events; anything else goes to the status log.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::{broadcast, mpsc, watch};

use super::burst::{self, RawSignal, RawStore, Ring};
use super::DecodedEvent;
use crate::db;
use crate::dsp::ddc::{decimation_for, fits, Ddc};
use crate::radio::{IqBlock, RadioState, MAX_FREQ_HZ, MIN_FREQ_HZ};

pub const NAME: &str = "rtl_433";
/// rtl_433's own default rate; we never go below it.
const MIN_RATE: u32 = 250_000;
const LOG_LINES: usize = 40;
/// Longest raw rtl_433 line kept with an event for debugging.
const RAW_MAX: usize = 4096;
/// Flex decoder older versions shipped by default. It has no preamble or match
/// and an 8 µs bit (2 samples at 250 kS/s), so it reports any FSK burst as raw
/// bits under the name "TPMS". Real TPMS comes from rtl_433's built-in decoders.
const LEGACY_TPMS_FLEX: &str = "n=TPMS,m=FSK_PCM,s=8,l=8,r=3000";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FlexDecoder {
    pub spec: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rtl433Config {
    pub enabled: bool,
    pub frequency_hz: u64,
    /// Passed as `-R n` each: positive enables only those protocols, negative
    /// disables one, 0 disables all built-in decoders.
    #[serde(default)]
    pub protocols: Vec<i32>,
    /// General-purpose decoders, passed as `-X spec`.
    #[serde(default)]
    pub flex: Vec<FlexDecoder>,
}

impl Default for Rtl433Config {
    fn default() -> Self {
        Rtl433Config {
            enabled: true,
            frequency_hz: 433_920_000,
            protocols: Vec::new(),
            flex: Vec::new(),
        }
    }
}

pub fn validate_spec(spec: &str) -> Result<(), String> {
    if spec.is_empty() || spec.len() > 1024 {
        return Err("flex spec must be 1–1024 characters".into());
    }
    if spec.starts_with('-') {
        return Err("flex spec can't start with '-'".into());
    }
    if let Some(c) = spec
        .chars()
        .find(|c| !c.is_ascii_graphic() || matches!(c, '"' | '\'' | '\\' | '`'))
    {
        return Err(format!("flex spec contains an invalid character {c:?}"));
    }
    if !spec.split(',').any(|kv| kv.starts_with("m=") || kv.starts_with("modulation=")) {
        return Err("flex spec needs a modulation (m=...)".into());
    }
    Ok(())
}

impl Rtl433Config {
    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&self.frequency_hz) {
            return Err("frequency out of range".into());
        }
        if self.protocols.len() > 64 || self.protocols.iter().any(|p| p.abs() > 9999) {
            return Err("protocols must be at most 64 numbers".into());
        }
        if self.flex.len() > 32 {
            return Err("at most 32 flex decoders".into());
        }
        for f in &self.flex {
            validate_spec(&f.spec)?;
        }
        Ok(())
    }

    fn args(&self, rate: u32) -> Vec<String> {
        // -f only labels the reported frequency; the stream is already centred on it.
        let mut a: Vec<String> = [
            "-r", "cs16:-", "-s", &rate.to_string(), "-f", &self.frequency_hz.to_string(),
            "-F", "json", "-F", "log", "-M", "level", "-M", "protocol", "-C", "si",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for p in &self.protocols {
            a.push("-R".into());
            a.push(p.to_string());
        }
        for f in self.flex.iter().filter(|f| f.enabled) {
            a.push("-X".into());
            a.push(f.spec.clone());
        }
        a
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Rtl433Status {
    pub enabled: bool,
    /// The child process is up.
    pub running: bool,
    /// The configured frequency is inside the band the radio is tuned to.
    pub in_band: bool,
    pub frequency_hz: u64,
    /// Rate of the stream handed to rtl_433.
    pub sample_rate: u32,
    pub restarts: u32,
    pub error: Option<String>,
    pub log: VecDeque<String>,
}

#[derive(Clone)]
pub struct Rtl433Handle {
    path: Arc<str>,
    cfg_tx: Arc<watch::Sender<Rtl433Config>>,
    status_rx: watch::Receiver<Rtl433Status>,
    raw: RawStore,
}

const SETTINGS_KEY: &str = "decoder.rtl_433";

impl Rtl433Handle {
    pub fn config(&self) -> Rtl433Config {
        self.cfg_tx.borrow().clone()
    }

    pub fn status(&self) -> Rtl433Status {
        self.status_rx.borrow().clone()
    }

    /// The radio signal of a device's latest packet (by device key).
    pub fn raw_signal(&self, key: &str) -> Option<Arc<RawSignal>> {
        self.raw.get(key)
    }

    pub async fn set_config(&self, db: &sqlx::SqlitePool, cfg: Rtl433Config) -> anyhow::Result<()> {
        db::set_setting(db, SETTINGS_KEY, &cfg).await?;
        self.cfg_tx.send_replace(cfg);
        Ok(())
    }

    /// Ask rtl_433 itself whether it accepts a flex spec.
    pub async fn check_spec(&self, spec: &str) -> Result<(), SpecError> {
        validate_spec(spec).map_err(SpecError::Invalid)?;
        let run = tokio::process::Command::new(&*self.path)
            .args(["-X", spec, "-r", "cs16:/dev/null", "-s", "250k"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .map_err(|_| SpecError::Unavailable("rtl_433 did not answer".into()))?
            .map_err(|e| SpecError::Unavailable(format!("can't run {}: {e}", self.path)))?;
        if out.status.success() {
            return Ok(());
        }
        let msg: Vec<&str> = std::str::from_utf8(&out.stderr)
            .unwrap_or("")
            .lines()
            .filter(|l| !l.starts_with("rtl_433 version") && !l.starts_with("Use -X") && !l.trim().is_empty())
            .collect();
        Err(SpecError::Invalid(if msg.is_empty() {
            "rejected by rtl_433".into()
        } else {
            msg.join(" ")
        }))
    }
}

pub enum SpecError {
    /// rtl_433 (or our own checks) rejected it.
    Invalid(String),
    /// Couldn't ask rtl_433.
    Unavailable(String),
}

/// Remove [`LEGACY_TPMS_FLEX`] from a saved config; true if it was there.
fn drop_legacy_flex(cfg: &mut Rtl433Config) -> bool {
    let before = cfg.flex.len();
    cfg.flex.retain(|f| f.spec != LEGACY_TPMS_FLEX);
    cfg.flex.len() != before
}

pub async fn spawn(
    db: &sqlx::SqlitePool,
    path: String,
    iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    radio: watch::Receiver<RadioState>,
    events: mpsc::Sender<DecodedEvent>,
) -> anyhow::Result<Rtl433Handle> {
    let cfg = match db::get_setting::<Rtl433Config>(db, SETTINGS_KEY).await {
        Ok(Some(mut c)) if c.validate().is_ok() => {
            if drop_legacy_flex(&mut c) {
                tracing::info!("removed the old default flex decoder {LEGACY_TPMS_FLEX}");
                if let Err(e) = db::set_setting(db, SETTINGS_KEY, &c).await {
                    tracing::warn!("saving rtl_433 config: {e}");
                }
            }
            c
        }
        Ok(_) => Rtl433Config::default(),
        Err(e) => {
            tracing::warn!("ignoring stored rtl_433 config: {e}");
            Rtl433Config::default()
        }
    };
    let (cfg_tx, cfg_rx) = watch::channel(cfg);
    let (status_tx, status_rx) = watch::channel(Rtl433Status::default());
    let path: Arc<str> = path.into();
    let worker = Worker {
        path: path.clone(),
        iq_rx,
        cfg_rx,
        status: Arc::new(status_tx),
        events,
        radio,
        tuning: Arc::new(Mutex::new(Tuning::default())),
        ring: Arc::new(Mutex::new(Ring::new())),
        raw: RawStore::default(),
    };
    let raw = worker.raw.clone();
    std::thread::Builder::new()
        .name("rtl_433".into())
        .spawn(move || worker.run())?;
    Ok(Rtl433Handle {
        path,
        cfg_tx: Arc::new(cfg_tx),
        status_rx,
        raw,
    })
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    rate: u32,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Worker {
    path: Arc<str>,
    iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    cfg_rx: watch::Receiver<Rtl433Config>,
    status: Arc<watch::Sender<Rtl433Status>>,
    events: mpsc::Sender<DecodedEvent>,
    radio: watch::Receiver<RadioState>,
    /// How the stream now being fed to rtl_433 was cut from the capture.
    tuning: Arc<Mutex<Tuning>>,
    /// The last seconds of that stream, to cut packet bursts out of.
    ring: Arc<Mutex<Ring>>,
    raw: RawStore,
}

/// The receiver side of the stream handed to rtl_433, for packet debugging.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Tuning {
    /// Radio centre frequency.
    center_hz: u64,
    /// Radio (capture) sample rate.
    sdr_rate: u32,
    decimation: u32,
    /// Rate of the stream rtl_433 sees.
    stream_rate: u32,
}

/// The burst behind a decoded packet, from the stream ring, if it can be found.
fn cut_burst(ring: &Mutex<Ring>, data: &Map<String, Value>, rate: u32, freq_hz: u64) -> Option<RawSignal> {
    let secs = burst::stream_secs(data.get("time")?.as_str()?)?;
    let at = (secs * rate as f64).round() as u64;
    let (start, window) = {
        let r = ring.lock().unwrap();
        if r.rate != rate {
            return None; // a newer rtl_433 process has taken over
        }
        r.slice(at.saturating_sub(rate as u64), at + rate as u64)
    };
    let demod = data.get("mod").and_then(Value::as_str);
    burst::extract(&window, start, at, rate, freq_hz, demod, 0)
}

fn mhz_to_hz(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_f64).map(|m| (m * 1e6).round() as i64)
}

/// Debug details for one decoded packet: the raw line, the demodulation and
/// frequencies rtl_433 measured, and where that sits in our tuning (the IF is
/// the signal's offset from the radio's centre frequency).
fn rx_info(line: &str, data: &Map<String, Value>, nominal_hz: u64, t: Tuning, radio: Option<&RadioState>) -> Map<String, Value> {
    use serde_json::json;
    let mut rx = Map::new();
    let mut raw = line.to_string();
    if raw.len() > RAW_MAX {
        let mut cut = RAW_MAX;
        while !raw.is_char_boundary(cut) {
            cut -= 1;
        }
        raw.truncate(cut);
        raw.push('…');
    }
    rx.insert("raw".into(), raw.into());
    // rtl_433's own time for piped input: offset into the stream it was fed.
    if let Some(v) = data.get("time") {
        rx.insert("stream_time".into(), v.clone());
    }
    if let Some(v) = data.get("mod") {
        rx.insert("demod".into(), v.clone());
    }
    if let Some(v) = data.get("protocol") {
        rx.insert("protocol".into(), v.clone());
    }
    // FSK reports both tones (freq1/freq2); OOK/ASK reports one (freq).
    let f1 = mhz_to_hz(data.get("freq1"));
    let f2 = mhz_to_hz(data.get("freq2"));
    let signal = f1.or_else(|| mhz_to_hz(data.get("freq")));
    rx.insert("nominal_hz".into(), json!(nominal_hz));
    if let Some(sig) = signal {
        let sig = match (f1, f2) {
            (Some(a), Some(b)) => (a + b) / 2,
            _ => sig,
        };
        rx.insert("signal_hz".into(), json!(sig));
        rx.insert("offset_hz".into(), json!(sig - nominal_hz as i64));
        if t.center_hz > 0 {
            rx.insert("if_hz".into(), json!(sig - t.center_hz as i64));
        }
    }
    if let (Some(a), Some(b)) = (f1, f2) {
        rx.insert("fsk_shift_hz".into(), json!((b - a).abs()));
    }
    if t.center_hz > 0 {
        rx.insert("center_hz".into(), json!(t.center_hz));
        rx.insert("ddc_offset_hz".into(), json!(nominal_hz as i64 - t.center_hz as i64));
        rx.insert("sdr_rate".into(), json!(t.sdr_rate));
        rx.insert("decimation".into(), json!(t.decimation));
        rx.insert("stream_rate".into(), json!(t.stream_rate));
    }
    if let Some(radio) = radio {
        // None = tuner AGC.
        rx.insert("gain_db".into(), json!(radio.settings.gain_db));
        rx.insert("ppm".into(), json!(radio.settings.ppm));
        if let Some(d) = &radio.device {
            rx.insert("device".into(), json!(d));
        }
        if let Some(d) = &radio.tuner {
            rx.insert("tuner".into(), json!(d));
        }
    }
    rx
}

fn push_log(status: &watch::Sender<Rtl433Status>, line: String) {
    status.send_modify(|s| {
        if s.log.len() >= LOG_LINES {
            s.log.pop_front();
        }
        s.log.push_back(line);
    });
}

impl Worker {
    fn run(mut self) {
        let mut cfg = self.cfg_rx.borrow_and_update().clone();
        let mut proc: Option<Proc> = None;
        // (centre, rate, target) the DDC was built for.
        let mut ddc: Option<((u64, u32, u64), Ddc)> = None;
        let mut retry_at = Instant::now();
        let mut backoff = Duration::from_secs(2);
        let mut iq = Vec::new();
        let mut bytes = Vec::new();

        loop {
            let block = match self.iq_rx.blocking_recv() {
                Ok(b) => b,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("rtl_433 feeder lagged {n} blocks");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if self.cfg_rx.has_changed().unwrap_or(false) {
                cfg = self.cfg_rx.borrow_and_update().clone();
                proc = None;
                ddc = None;
                retry_at = Instant::now();
                backoff = Duration::from_secs(2);
                self.status.send_modify(|s| s.error = None);
            }

            let decim = decimation_for(block.sample_rate, MIN_RATE);
            let rate = (block.sample_rate as f64 / decim as f64).round() as u32;
            let in_band = fits(block.center_hz, block.sample_rate, cfg.frequency_hz, 0.8 * rate as f64);
            self.status.send_if_modified(|s| {
                let changed = s.enabled != cfg.enabled
                    || s.in_band != in_band
                    || s.frequency_hz != cfg.frequency_hz
                    || s.sample_rate != rate;
                s.enabled = cfg.enabled;
                s.in_band = in_band;
                s.frequency_hz = cfg.frequency_hz;
                s.sample_rate = rate;
                changed
            });

            if !cfg.enabled {
                if proc.take().is_some() {
                    self.set_running(false);
                }
                continue;
            }

            // Notice a crashed child even while it's idle (out of band).
            if let Some(p) = proc.as_mut() {
                if let Ok(Some(st)) = p.child.try_wait() {
                    proc = None;
                    self.failed(format!("rtl_433 exited ({st})"), &mut retry_at, &mut backoff);
                }
            }
            if !in_band {
                ddc = None;
                continue;
            }

            if proc.as_ref().is_some_and(|p| p.rate != rate) {
                proc = None;
            }
            if proc.is_none() {
                if Instant::now() < retry_at {
                    continue;
                }
                match self.start(&cfg, rate) {
                    Ok(p) => {
                        proc = Some(p);
                        self.set_running(true);
                    }
                    Err(e) => {
                        self.failed(e, &mut retry_at, &mut backoff);
                        continue;
                    }
                }
            }

            let key = (block.center_hz, block.sample_rate, cfg.frequency_hz);
            if ddc.as_ref().map(|(k, _)| *k) != Some(key) {
                let offset = cfg.frequency_hz as f64 - block.center_hz as f64;
                ddc = Some((key, Ddc::new(block.sample_rate, offset, decim)));
            }
            let t = Tuning {
                center_hz: block.center_hz,
                sdr_rate: block.sample_rate,
                decimation: decim as u32,
                stream_rate: rate,
            };
            *self.tuning.lock().unwrap() = t;
            let (_, d) = ddc.as_mut().unwrap();
            iq.clear();
            d.process_cu8(&block.data, &mut iq);
            bytes.clear();
            for c in &iq {
                bytes.extend_from_slice(&((c.re * 32767.0).clamp(-32767.0, 32767.0) as i16).to_le_bytes());
                bytes.extend_from_slice(&((c.im * 32767.0).clamp(-32767.0, 32767.0) as i16).to_le_bytes());
            }
            let p = proc.as_mut().unwrap();
            if let Err(e) = p.stdin.write_all(&bytes) {
                proc = None;
                self.failed(format!("writing to rtl_433: {e}"), &mut retry_at, &mut backoff);
            } else {
                backoff = Duration::from_secs(2);
                self.ring.lock().unwrap().push(&iq);
            }
        }
    }

    fn set_running(&self, running: bool) {
        self.status.send_if_modified(|s| {
            let changed = s.running != running;
            s.running = running;
            if running {
                s.error = None;
            }
            changed
        });
    }

    fn failed(&self, msg: String, retry_at: &mut Instant, backoff: &mut Duration) {
        tracing::warn!("{msg}; retrying in {} s", backoff.as_secs());
        self.status.send_modify(|s| {
            s.running = false;
            s.restarts += 1;
            s.error = Some(msg);
        });
        *retry_at = Instant::now() + *backoff;
        *backoff = (*backoff * 2).min(Duration::from_secs(60));
    }

    fn start(&self, cfg: &Rtl433Config, rate: u32) -> Result<Proc, String> {
        let args = cfg.args(rate);
        tracing::info!("starting {} {}", self.path, args.join(" "));
        let mut child = Command::new(&*self.path)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("can't start {}: {e}", self.path))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        // rtl_433's clock starts at the first sample of this process.
        self.ring.lock().unwrap().reset(rate);
        let stderr = child.stderr.take().unwrap();

        let status = self.status.clone();
        let events = self.events.clone();
        let freq_hz = cfg.frequency_hz;
        let tuning = self.tuning.clone();
        let radio = self.radio.clone();
        let ring = self.ring.clone();
        let raw = self.raw.clone();
        std::thread::Builder::new()
            .name("rtl_433-out".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.starts_with('{') {
                        match serde_json::from_str::<Map<String, Value>>(&line) {
                            Ok(data) => {
                                let t = *tuning.lock().unwrap();
                                let mut rx = rx_info(&line, &data, freq_hz, t, Some(&radio.borrow()));
                                let sig = cut_burst(&ring, &data, rate, freq_hz);
                                let mut ev = DecodedEvent::new(NAME, Some(freq_hz), data);
                                if let Some(mut sig) = sig {
                                    sig.ts = ev.ts_ms;
                                    rx.insert("signal".into(), true.into());
                                    raw.insert(crate::events::identity(&ev).0, sig);
                                }
                                ev = ev.with_rx(rx);
                                if events.try_send(ev).is_err() {
                                    tracing::warn!("event queue full, dropping rtl_433 event");
                                }
                            }
                            Err(e) => tracing::debug!("bad JSON from rtl_433: {e}"),
                        }
                    } else if !line.trim().is_empty() {
                        push_log(&status, line);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let status = self.status.clone();
        std::thread::Builder::new()
            .name("rtl_433-err".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    if !line.trim().is_empty() {
                        push_log(&status, line);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Proc { child, stdin, rate })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_validation() {
        assert!(validate_spec("n=TPMS,m=FSK_PCM,s=8,l=8,r=3000").is_ok());
        assert!(validate_spec("n=x,m=OOK_PWM,s=400,l=800,r=2000,match={24}0xabcdef,repeats>=3").is_ok());
        assert!(validate_spec("n=TPMS s=8,m=FSK_PCM").is_err()); // whitespace
        assert!(validate_spec("-w,m=OOK_PWM").is_err());
        assert!(validate_spec("n=x,s=1").is_err()); // no modulation
        assert!(validate_spec("n=\"x\",m=OOK_PWM").is_err());
    }

    #[test]
    fn args_include_enabled_flex_only() {
        let mut c = Rtl433Config { protocols: vec![19, -59], ..Default::default() };
        c.flex.push(FlexDecoder { spec: "n=TPMS,m=FSK_PCM,s=52,l=52,r=1000".into(), enabled: true });
        c.flex.push(FlexDecoder { spec: "n=off,m=OOK_PWM,s=1,l=2,r=3".into(), enabled: false });
        let a = c.args(256_000);
        assert!(a.windows(2).any(|w| w == ["-s", "256000"]));
        assert!(a.windows(2).any(|w| w == ["-f", "433920000"]));
        assert!(a.windows(2).any(|w| w == ["-R", "-59"]));
        assert!(a.windows(2).any(|w| w[0] == "-X" && w[1].starts_with("n=TPMS")));
        assert!(!a.iter().any(|x| x.starts_with("n=off")));
    }

    #[test]
    fn rx_info_works_out_if_and_offset() {
        let line = r#"{"time":"@1.5s","model":"Toyota","id":"abc","mod":"FSK","freq1":433.9,"freq2":433.96,"protocol":88}"#;
        let data: Map<String, Value> = serde_json::from_str(line).unwrap();
        let t = Tuning { center_hz: 434_000_000, sdr_rate: 1_024_000, decimation: 4, stream_rate: 256_000 };
        let rx = rx_info(line, &data, 433_920_000, t, None);
        assert_eq!(rx["raw"], line);
        assert_eq!(rx["demod"], "FSK");
        assert_eq!(rx["stream_time"], "@1.5s");
        assert_eq!(rx["signal_hz"], 433_930_000);
        assert_eq!(rx["fsk_shift_hz"], 60_000);
        assert_eq!(rx["offset_hz"], 10_000);
        assert_eq!(rx["if_hz"], -70_000);
        assert_eq!(rx["ddc_offset_hz"], -80_000);
        assert_eq!(rx["stream_rate"], 256_000);
    }

    #[test]
    fn default_has_no_flex_and_legacy_one_is_dropped() {
        assert!(Rtl433Config::default().flex.is_empty());
        let mut c = Rtl433Config::default();
        c.flex.push(FlexDecoder { spec: LEGACY_TPMS_FLEX.into(), enabled: true });
        c.flex.push(FlexDecoder { spec: "n=mine,m=OOK_PWM,s=1,l=2,r=3".into(), enabled: true });
        assert!(drop_legacy_flex(&mut c));
        assert_eq!(c.flex.len(), 1);
        assert_eq!(c.flex[0].spec, "n=mine,m=OOK_PWM,s=1,l=2,r=3");
        assert!(!drop_legacy_flex(&mut c));
    }
}
