//! APRS via Direwolf. For each configured channel inside the tuned band, the
//! decoder thread cuts a 16 kHz slice out of the capture, FM-demodulates it to
//! ~48 kHz audio and pipes it to its own `direwolf` process. Decoded frames
//! come back over Direwolf's KISS TCP port and are parsed into events.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, watch};

use super::DecodedEvent;
use crate::aprs::{ax25, kiss, parse};
use crate::db;
use crate::dsp::ddc::{audio_decimation, fits, Ddc};
use crate::dsp::fm::FmDemod;
use crate::radio::{IqBlock, MAX_FREQ_HZ, MIN_FREQ_HZ};
use crate::scheduler::{shift_hz, DopplerRx};

pub const NAME: &str = "aprs";
const SETTINGS_KEY: &str = "decoder.aprs";
/// 12.5 kHz NFM channel plus margin for frequency error.
const CHANNEL_BW: f64 = 16_000.0;
/// Deviation that maps to full-scale audio (APRS uses ~3 kHz).
const FULL_SCALE_DEV: f32 = 6_000.0;
const LOG_LINES: usize = 60;
const MAX_CHANNELS: usize = 4;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AprsChannel {
    pub label: String,
    pub frequency_hz: u64,
    pub enabled: bool,
    /// Satellite downlink: follow the Doppler shift during passes.
    #[serde(default)]
    pub doppler: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AprsConfig {
    pub enabled: bool,
    pub channels: Vec<AprsChannel>,
}

impl Default for AprsConfig {
    fn default() -> Self {
        let ch = |label: &str, frequency_hz, doppler| AprsChannel {
            label: label.into(),
            frequency_hz,
            enabled: true,
            doppler,
        };
        AprsConfig {
            enabled: true,
            channels: vec![ch("APRS", 144_800_000, false), ch("ISS APRS", 145_825_000, true)],
        }
    }
}

impl AprsConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.channels.len() > MAX_CHANNELS {
            return Err(format!("at most {MAX_CHANNELS} APRS channels"));
        }
        for c in &self.channels {
            if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&c.frequency_hz) {
                return Err("frequency out of range".into());
            }
            let n = c.label.chars().count();
            if n == 0 || n > 24 || c.label.chars().any(char::is_control) {
                return Err("channel labels must be 1–24 characters".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ChannelStatus {
    pub label: String,
    pub frequency_hz: u64,
    pub enabled: bool,
    pub in_band: bool,
    pub running: bool,
    pub packets: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct AprsStatus {
    pub enabled: bool,
    pub audio_rate: u32,
    pub channels: Vec<ChannelStatus>,
    pub log: VecDeque<String>,
}

#[derive(Clone)]
pub struct AprsHandle {
    cfg_tx: Arc<watch::Sender<AprsConfig>>,
    status_rx: watch::Receiver<AprsStatus>,
}

impl AprsHandle {
    pub fn config(&self) -> AprsConfig {
        self.cfg_tx.borrow().clone()
    }

    pub fn status(&self) -> AprsStatus {
        self.status_rx.borrow().clone()
    }

    pub async fn set_config(&self, db: &sqlx::SqlitePool, cfg: AprsConfig) -> anyhow::Result<()> {
        db::set_setting(db, SETTINGS_KEY, &cfg).await?;
        self.cfg_tx.send_replace(cfg);
        Ok(())
    }
}

pub async fn spawn(
    db: &sqlx::SqlitePool,
    path: String,
    iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    events: mpsc::Sender<DecodedEvent>,
    doppler: DopplerRx,
) -> anyhow::Result<AprsHandle> {
    let cfg = match db::get_setting::<AprsConfig>(db, SETTINGS_KEY).await {
        Ok(Some(c)) if c.validate().is_ok() => c,
        Ok(_) => AprsConfig::default(),
        Err(e) => {
            tracing::warn!("ignoring stored APRS config: {e}");
            AprsConfig::default()
        }
    };
    let (cfg_tx, cfg_rx) = watch::channel(cfg);
    let (status_tx, status_rx) = watch::channel(AprsStatus::default());
    let worker = Worker {
        path: path.into(),
        iq_rx,
        cfg_rx,
        status: Arc::new(status_tx),
        events,
        doppler,
    };
    std::thread::Builder::new()
        .name("aprs".into())
        .spawn(move || worker.run())?;
    Ok(AprsHandle {
        cfg_tx: Arc::new(cfg_tx),
        status_rx,
    })
}

type Status = Arc<watch::Sender<AprsStatus>>;

fn push_log(status: &Status, line: String) {
    status.send_modify(|s| {
        if s.log.len() >= LOG_LINES {
            s.log.pop_front();
        }
        s.log.push_back(line);
    });
}

/// Each Direwolf gets a fresh KISS port so a restart never races the old
/// process for the socket.
fn next_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    8101 + NEXT.fetch_add(1, Ordering::Relaxed) % 100
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    rate: u32,
    stop: Arc<AtomicBool>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Chan {
    cfg: AprsChannel,
    proc: Option<Proc>,
    /// (centre, rate) it was built for, the filter, the demod, applied Doppler shift.
    dsp: Option<((u64, u32), Ddc, FmDemod, f64)>,
    retry_at: Instant,
    backoff: Duration,
}

struct Worker {
    path: Arc<str>,
    iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    cfg_rx: watch::Receiver<AprsConfig>,
    status: Status,
    events: mpsc::Sender<DecodedEvent>,
    doppler: DopplerRx,
}

impl Worker {
    fn channels(&self, cfg: &AprsConfig) -> Vec<Chan> {
        self.status.send_modify(|s| {
            s.enabled = cfg.enabled;
            s.channels = cfg
                .channels
                .iter()
                .map(|c| ChannelStatus {
                    label: c.label.clone(),
                    frequency_hz: c.frequency_hz,
                    enabled: c.enabled,
                    ..Default::default()
                })
                .collect();
        });
        cfg.channels
            .iter()
            .map(|c| Chan {
                cfg: c.clone(),
                proc: None,
                dsp: None,
                retry_at: Instant::now(),
                backoff: Duration::from_secs(2),
            })
            .collect()
    }

    fn run(mut self) {
        let mut cfg = self.cfg_rx.borrow_and_update().clone();
        let mut chans = self.channels(&cfg);
        let mut iq = Vec::new();
        let mut audio = Vec::new();

        loop {
            let block = match self.iq_rx.blocking_recv() {
                Ok(b) => b,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("APRS feeder lagged {n} blocks");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if self.cfg_rx.has_changed().unwrap_or(false) {
                cfg = self.cfg_rx.borrow_and_update().clone();
                chans = self.channels(&cfg); // drops (and kills) the old processes
            }
            let (decim, rate) = audio_decimation(block.sample_rate);
            self.status.send_if_modified(|s| {
                let changed = s.audio_rate != rate;
                s.audio_rate = rate;
                changed
            });

            for (i, ch) in chans.iter_mut().enumerate() {
                let active = cfg.enabled && ch.cfg.enabled;
                let in_band = fits(block.center_hz, block.sample_rate, ch.cfg.frequency_hz, CHANNEL_BW);
                self.status.send_if_modified(|s| match s.channels.get_mut(i) {
                    Some(c) if c.in_band != in_band => {
                        c.in_band = in_band;
                        true
                    }
                    _ => false,
                });
                if !active {
                    if ch.proc.take().is_some() {
                        self.set_running(i, false, None);
                    }
                    continue;
                }
                if let Some(p) = ch.proc.as_mut() {
                    if let Ok(Some(st)) = p.child.try_wait() {
                        ch.proc = None;
                        self.failed(i, ch, format!("direwolf exited ({st})"));
                    }
                }
                if !in_band {
                    ch.dsp = None;
                    continue;
                }
                if ch.proc.as_ref().is_some_and(|p| p.rate != rate) {
                    ch.proc = None;
                }
                if ch.proc.is_none() {
                    if Instant::now() < ch.retry_at {
                        continue;
                    }
                    match self.start(i, &ch.cfg, rate) {
                        Ok(p) => {
                            ch.proc = Some(p);
                            self.set_running(i, true, None);
                        }
                        Err(e) => {
                            self.failed(i, ch, e);
                            continue;
                        }
                    }
                }

                let key = (block.center_hz, block.sample_rate);
                let shift = if ch.cfg.doppler { shift_hz(&self.doppler, ch.cfg.frequency_hz) } else { 0.0 };
                let offset = ch.cfg.frequency_hz as f64 + shift - block.center_hz as f64;
                if ch.dsp.as_ref().map(|(k, ..)| *k) != Some(key) {
                    ch.dsp = Some((
                        key,
                        Ddc::with_cutoff(block.sample_rate, offset, decim, CHANNEL_BW / 2.0),
                        FmDemod::new(rate, FULL_SCALE_DEV),
                        shift,
                    ));
                }
                let (_, ddc, fm, applied) = ch.dsp.as_mut().unwrap();
                if (*applied - shift).abs() > 1.0 {
                    ddc.set_offset(block.sample_rate, offset);
                    *applied = shift;
                }
                iq.clear();
                ddc.process_cu8(&block.data, &mut iq);
                audio.clear();
                fm.process_s16le(&iq, &mut audio);
                if let Err(e) = ch.proc.as_mut().unwrap().stdin.write_all(&audio) {
                    ch.proc = None;
                    self.failed(i, ch, format!("writing to direwolf: {e}"));
                } else {
                    ch.backoff = Duration::from_secs(2);
                }
            }
        }
    }

    fn set_running(&self, i: usize, running: bool, error: Option<String>) {
        self.status.send_if_modified(|s| match s.channels.get_mut(i) {
            Some(c) if c.running != running || c.error != error => {
                c.running = running;
                c.error = error;
                true
            }
            _ => false,
        });
    }

    fn failed(&self, i: usize, ch: &mut Chan, msg: String) {
        tracing::warn!("APRS {}: {msg}; retrying in {} s", ch.cfg.label, ch.backoff.as_secs());
        self.set_running(i, false, Some(msg));
        ch.retry_at = Instant::now() + ch.backoff;
        ch.backoff = (ch.backoff * 2).min(Duration::from_secs(60));
    }

    fn start(&self, idx: usize, ch: &AprsChannel, rate: u32) -> Result<Proc, String> {
        let port = next_port();
        let conf = std::env::temp_dir().join(format!("sdrd-direwolf-{port}.conf"));
        std::fs::write(
            &conf,
            format!(
                "ADEVICE stdin null\nACHANNELS 1\nCHANNEL 0\nMYCALL N0CALL\nMODEM 1200\n\
                 KISSPORT {port}\nAGWPORT 0\n"
            ),
        )
        .map_err(|e| format!("writing {}: {e}", conf.display()))?;
        let rate_s = rate.to_string();
        let args = ["-c", conf.to_str().unwrap_or_default(), "-r", &rate_s, "-t", "0", "-q", "hd"];
        tracing::info!("starting {} {} for {} ({} Hz)", self.path, args.join(" "), ch.label, ch.frequency_hz);
        let mut child = Command::new(&*self.path)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("can't start {}: {e}", self.path))?;
        let stdin = child.stdin.take().unwrap();
        let stop = Arc::new(AtomicBool::new(false));

        for pipe in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let status = self.status.clone();
            let label = ch.label.clone();
            std::thread::Builder::new()
                .name("direwolf-log".into())
                .spawn(move || {
                    for line in BufReader::new(pipe).lines() {
                        let Ok(line) = line else { break };
                        let line = line.trim();
                        // "[0.3] CALL>..." echoes of decoded packets: shown as events instead.
                        if !line.is_empty() && !line.starts_with('[') {
                            push_log(&status, format!("{label}: {line}"));
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }

        let reader = KissReader {
            port,
            stop: stop.clone(),
            events: self.events.clone(),
            status: self.status.clone(),
            idx,
            freq_hz: ch.frequency_hz,
        };
        std::thread::Builder::new()
            .name("direwolf-kiss".into())
            .spawn(move || reader.run())
            .map_err(|e| e.to_string())?;
        Ok(Proc { child, stdin, rate, stop })
    }
}

struct KissReader {
    port: u16,
    stop: Arc<AtomicBool>,
    events: mpsc::Sender<DecodedEvent>,
    status: Status,
    idx: usize,
    freq_hz: u64,
}

impl KissReader {
    fn run(self) {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));
        let mut buf = [0u8; 4096];
        while !self.stop.load(Ordering::Relaxed) {
            // Direwolf takes a moment to open its port.
            let Ok(mut sock) = TcpStream::connect_timeout(&addr, Duration::from_secs(1)) else {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            };
            let _ = sock.set_read_timeout(Some(Duration::from_secs(1)));
            let mut dec = kiss::Decoder::default();
            while !self.stop.load(Ordering::Relaxed) {
                match sock.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        for frame in dec.push(&buf[..n]) {
                            self.handle(&frame);
                        }
                    }
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    Err(_) => break,
                }
            }
        }
    }

    fn handle(&self, frame: &[u8]) {
        let Some(f) = ax25::decode(frame) else {
            tracing::debug!("undecodable AX.25 frame ({} bytes)", frame.len());
            return;
        };
        let data = parse::parse(&f);
        tracing::debug!("APRS {}", f.tnc2());
        if self.events.try_send(DecodedEvent::new(NAME, Some(self.freq_hz), data)).is_err() {
            tracing::warn!("event queue full, dropping APRS packet");
        }
        self.status.send_modify(|s| {
            if let Some(c) = s.channels.get_mut(self.idx).filter(|c| c.frequency_hz == self.freq_hz) {
                c.packets += 1;
            }
        });
    }
}
