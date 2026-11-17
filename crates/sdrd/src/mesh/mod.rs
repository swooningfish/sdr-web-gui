//! Meshtastic and MeshCore: LoRa mesh networks on 869 MHz (UK/EU).
//!
//! Whenever the main receiver's capture covers one of the presets, a
//! demodulator thread mixes each preset's channel down, resamples it to
//! [`lora::OS`] samples per chip and runs the LoRa demodulator ([`lora`]). Frames go to an
//! async task that decodes them ([`meshtastic`], [`meshcore`]) with the
//! channel keys we know, and keeps the node list, the channel messages
//! (SQLite) and a packet log.
//!
//! Messages on private channels (keys an admin added) are shown to admins
//! only. Direct messages are end-to-end encrypted: only who sent them to whom.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use num_complex::Complex32;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tokio::sync::{broadcast, mpsc, watch};

use crate::db;
use crate::dsp::ddc::{self, Ddc};
use crate::radio::{IqBlock, RadioState};

pub mod lora;
pub mod meshcore;
pub mod meshtastic;
pub mod proto;

const CFG_KEY: &str = "mesh";
/// Packets kept in the in-memory log.
const LOG_LEN: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    Meshtastic,
    Meshcore,
}

impl Proto {
    fn as_str(self) -> &'static str {
        match self {
            Proto::Meshtastic => "meshtastic",
            Proto::Meshcore => "meshcore",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub proto: Proto,
    pub name: &'static str,
    pub freq_hz: u64,
    pub sf: u8,
    pub bw: u32,
    pub cr: u8,
    pub sync: u8,
}

impl Preset {
    pub fn params(&self) -> lora::Params {
        lora::Params { sf: self.sf, bw: self.bw, cr: self.cr, sync: self.sync, preamble: 16 }
    }
}

/// UK/EU 868 presets. Meshtastic's EU_868 region has one 250 kHz slot, so
/// its presets share 869.525 MHz and differ by spreading factor.
pub const PRESETS: &[Preset] = &[
    Preset { id: "mt-longfast", proto: Proto::Meshtastic, name: "Meshtastic LongFast", freq_hz: 869_525_000, sf: 11, bw: 250_000, cr: 1, sync: 0x2B },
    Preset { id: "mt-mediumfast", proto: Proto::Meshtastic, name: "Meshtastic MediumFast", freq_hz: 869_525_000, sf: 9, bw: 250_000, cr: 1, sync: 0x2B },
    Preset { id: "mt-shortfast", proto: Proto::Meshtastic, name: "Meshtastic ShortFast", freq_hz: 869_525_000, sf: 7, bw: 250_000, cr: 1, sync: 0x2B },
    Preset { id: "mc-narrow", proto: Proto::Meshcore, name: "MeshCore UK/EU narrow", freq_hz: 869_618_000, sf: 8, bw: 62_500, cr: 4, sync: 0x12 },
];

/// Where to tune the main receiver to hear all the presets.
pub const TUNE_HZ: u64 = 869_575_000;

/// Meshtastic's default channel names (one per modem preset), all on the
/// well-known default key.
const MT_DEFAULT_CHANNELS: &[&str] =
    &["LongFast", "LongSlow", "LongModerate", "MediumFast", "MediumSlow", "ShortFast", "ShortSlow", "ShortTurbo", "VLongSlow"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChannelKey {
    pub name: String,
    /// Meshtastic: the PSK in base64 as the apps show it. MeshCore: the
    /// secret in hex or base64, or empty for a "#hashtag" channel.
    #[serde(default)]
    pub key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MeshConfig {
    pub enabled: bool,
    /// Preset ids to decode.
    pub presets: Vec<String>,
    #[serde(default)]
    pub meshtastic_channels: Vec<ChannelKey>,
    #[serde(default)]
    pub meshcore_channels: Vec<ChannelKey>,
}

impl Default for MeshConfig {
    fn default() -> Self {
        MeshConfig {
            enabled: true,
            presets: PRESETS.iter().map(|p| p.id.to_string()).collect(),
            meshtastic_channels: Vec::new(),
            meshcore_channels: Vec::new(),
        }
    }
}

fn decode_key(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.len() % 2 == 0 && s.len() >= 32 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return hex::decode(s).map_err(|e| e.to_string());
    }
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|_| "key isn't valid base64 or hex".to_string())
}

impl MeshConfig {
    /// Check the settings and build the channel lists.
    pub fn channels(&self) -> Result<(Vec<meshtastic::Channel>, Vec<meshcore::Channel>), String> {
        for p in &self.presets {
            if !PRESETS.iter().any(|x| x.id == p) {
                return Err(format!("unknown preset {p}"));
            }
        }
        let mut mt: Vec<meshtastic::Channel> = MT_DEFAULT_CHANNELS
            .iter()
            .map(|n| meshtastic::Channel { name: n.to_string(), key: meshtastic::DEFAULT_KEY.to_vec(), private: false })
            .collect();
        for c in &self.meshtastic_channels {
            let name = c.name.trim();
            if name.is_empty() || name.len() > 11 {
                return Err("a Meshtastic channel name is 1–11 characters".into());
            }
            let key = meshtastic::expand_psk(&decode_key(&c.key).map_err(|e| format!("{name}: {e}"))?)?;
            // The default key under another name is still a public channel.
            let private = key != meshtastic::DEFAULT_KEY;
            mt.push(meshtastic::Channel { name: name.to_string(), key, private });
        }
        let mut mc = vec![meshcore::Channel { name: "Public".into(), key: meshcore::PUBLIC_KEY, private: false }];
        for c in &self.meshcore_channels {
            let name = c.name.trim();
            if name.is_empty() || name.len() > 32 {
                return Err("a MeshCore channel name is 1–32 characters".into());
            }
            if c.key.trim().is_empty() {
                if !name.starts_with('#') {
                    return Err(format!("{name}: give a key, or name a hashtag channel like #uk"));
                }
                mc.push(meshcore::Channel::hashtag(name));
                continue;
            }
            let k = decode_key(&c.key).map_err(|e| format!("{name}: {e}"))?;
            let key: [u8; 16] = k
                .get(..16)
                .filter(|_| k.len() == 16 || (k.len() == 32 && k[16..].iter().all(|&b| b == 0)))
                .and_then(|s| s.try_into().ok())
                .ok_or_else(|| format!("{name}: a MeshCore channel key is 16 bytes"))?;
            mc.push(meshcore::Channel { name: name.to_string(), key, private: key != meshcore::PUBLIC_KEY });
        }
        Ok((mt, mc))
    }

    /// Without the keys (what viewers see).
    pub fn redacted(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "presets": self.presets,
            "meshtastic_channels": self.meshtastic_channels.iter().map(|c| json!({ "name": c.name })).collect::<Vec<_>>(),
            "meshcore_channels": self.meshcore_channels.iter().map(|c| json!({ "name": c.name })).collect::<Vec<_>>(),
        })
    }
}

/// Whether the main receiver's capture covers a preset.
pub fn in_band(center_hz: u64, rate: u32, p: &Preset) -> bool {
    ddc::fits(center_hz, rate, p.freq_hz, p.bw as f64 * 1.2)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Node {
    pub key: String,
    pub proto: String,
    /// "!1234abcd" (Meshtastic) or the start of the public key (MeshCore).
    pub id: String,
    pub short_name: Option<String>,
    pub long_name: Option<String>,
    pub hw: Option<String>,
    pub role: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub alt: Option<i32>,
    pub pos_ms: Option<i64>,
    pub battery: Option<u32>,
    pub voltage: Option<f32>,
    pub channel_util: Option<f32>,
    pub air_util_tx: Option<f32>,
    pub uptime_s: Option<u32>,
    pub temperature: Option<f32>,
    pub humidity: Option<f32>,
    pub pressure: Option<f32>,
    pub snr: Option<f32>,
    pub hops: Option<u8>,
    pub preset: String,
    pub first_ms: i64,
    pub last_ms: i64,
    pub packets: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    pub ts_ms: i64,
    pub preset: &'static str,
    pub proto: Proto,
    pub crc_ok: bool,
    pub snr: f32,
    pub offset_hz: f32,
    pub len: usize,
    pub raw: String,
    pub from: Option<String>,
    pub from_name: Option<String>,
    pub to: Option<String>,
    pub kind: String,
    pub summary: String,
    pub channel: Option<String>,
    pub private: bool,
    pub hops: Option<u8>,
    /// Heard before (a rebroadcast of a packet we already have).
    pub repeat: bool,
}

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct Message {
    pub id: i64,
    pub ts_ms: i64,
    pub proto: String,
    pub channel: Option<String>,
    pub private: bool,
    pub from_key: Option<String>,
    pub from_name: Option<String>,
    pub text: String,
    pub snr: Option<f32>,
    pub hops: Option<i64>,
}

#[derive(Default)]
struct Tracker {
    nodes: HashMap<String, Node>,
    dirty: HashSet<String>,
    log: VecDeque<LogEntry>,
    /// Recently seen packets (dedupe of rebroadcasts), with when.
    seen: HashMap<u64, i64>,
    counts: HashMap<&'static str, (u64, i64)>,
    crc_errors: u64,
}

impl Tracker {
    fn node(&mut self, key: &str, proto: Proto, id: &str, preset: &str, now: i64) -> &mut Node {
        self.dirty.insert(key.to_string());
        let n = self.nodes.entry(key.to_string()).or_insert_with(|| Node {
            key: key.to_string(),
            proto: proto.as_str().into(),
            id: id.to_string(),
            first_ms: now,
            ..Default::default()
        });
        n.last_ms = now;
        n.preset = preset.to_string();
        n
    }

    fn name_of(&self, key: &str) -> Option<String> {
        self.nodes.get(key).and_then(|n| n.long_name.clone().or_else(|| n.short_name.clone()))
    }

    /// MeshCore nodes whose public key starts with this byte (DM hashes).
    fn mc_by_hash(&self, h: u8) -> Option<String> {
        let prefix = format!("mc:{h:02x}");
        let mut it = self.nodes.values().filter(|n| n.key.starts_with(&prefix));
        let first = it.next()?;
        if it.next().is_some() {
            return None;
        }
        first.long_name.clone()
    }

    /// True if this packet was seen in the last 10 minutes.
    fn repeat(&mut self, h: u64, now: i64) -> bool {
        if self.seen.len() > 4096 {
            self.seen.retain(|_, t| now - *t < 600_000);
        }
        self.seen.insert(h, now).is_some_and(|t| now - t < 600_000)
    }
}

fn hash64(parts: &[&[u8]]) -> u64 {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p);
    }
    u64::from_le_bytes(h.finalize()[..8].try_into().unwrap())
}

struct Rx {
    preset: usize,
    frame: lora::Frame,
    ts_ms: i64,
}

#[derive(Clone)]
pub struct MeshHandle {
    pool: SqlitePool,
    tracker: Arc<Mutex<Tracker>>,
    cfg_tx: Arc<watch::Sender<MeshConfig>>,
    /// The receiver the capture comes from (the main one or Mesh's own).
    radio: watch::Receiver<RadioState>,
    last_iq: Arc<AtomicI64>,
}

impl MeshHandle {
    pub fn config(&self) -> MeshConfig {
        self.cfg_tx.borrow().clone()
    }

    pub async fn set_config(&self, cfg: MeshConfig) -> Result<(), String> {
        cfg.channels()?;
        db::set_setting(&self.pool, CFG_KEY, &cfg).await.map_err(|e| e.to_string())?;
        self.cfg_tx.send_replace(cfg);
        Ok(())
    }

    pub fn status(&self) -> Value {
        let now = db::now_ms();
        let st = self.radio.borrow().settings.clone();
        let cfg = self.config();
        let t = self.tracker.lock().unwrap();
        let presets: Vec<Value> = PRESETS
            .iter()
            .map(|p| {
                let (packets, last) = t.counts.get(p.id).copied().unwrap_or((0, 0));
                json!({
                    "preset": p,
                    "enabled": cfg.presets.iter().any(|x| x == p.id),
                    "in_band": in_band(st.center_hz, st.sample_rate, p),
                    "packets": packets,
                    "last_ms": (last > 0).then_some(last),
                })
            })
            .collect();
        let live = |proto: &str| t.nodes.values().filter(|n| n.proto == proto && now - n.last_ms < 86_400_000).count();
        json!({
            "enabled": cfg.enabled,
            "listening": now - self.last_iq.load(Ordering::Relaxed) < 3000,
            "presets": presets,
            "tune_hz": TUNE_HZ,
            "nodes_24h": { "meshtastic": live("meshtastic"), "meshcore": live("meshcore") },
            "crc_errors": t.crc_errors,
        })
    }

    pub fn nodes(&self) -> Vec<Node> {
        let t = self.tracker.lock().unwrap();
        let mut v: Vec<Node> = t.nodes.values().cloned().collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.last_ms));
        v
    }

    /// The packet log, newest first; private-channel content only for admins.
    pub fn packets(&self, admin: bool) -> Vec<LogEntry> {
        let t = self.tracker.lock().unwrap();
        t.log
            .iter()
            .rev()
            .map(|e| {
                let mut e = e.clone();
                if e.private && !admin {
                    e.summary = "message on a private channel".into();
                    e.channel = None;
                    e.raw.clear();
                }
                e
            })
            .collect()
    }

    pub async fn messages(&self, admin: bool, limit: i64) -> anyhow::Result<Vec<Message>> {
        Ok(sqlx::query_as(
            "SELECT id, ts_ms, proto, channel, private, from_key, from_name, text, snr, hops FROM mesh_messages \
             WHERE private = 0 OR ? ORDER BY id DESC LIMIT ?",
        )
        .bind(admin)
        .bind(limit.clamp(1, 2000))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn delete_nodes(&self, keys: &[String]) -> anyhow::Result<()> {
        {
            let mut t = self.tracker.lock().unwrap();
            for k in keys {
                t.nodes.remove(k);
                t.dirty.remove(k);
            }
        }
        for k in keys {
            sqlx::query("DELETE FROM mesh_nodes WHERE key = ?").bind(k).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// Settings of the receiver the capture comes from.
    pub fn radio_settings(&self) -> crate::radio::RadioSettings {
        self.radio.borrow().settings.clone()
    }
}

pub async fn spawn(
    pool: &SqlitePool,
    radio: watch::Receiver<RadioState>,
    iq: broadcast::Receiver<Arc<IqBlock>>,
) -> anyhow::Result<MeshHandle> {
    let mut cfg = db::get_setting::<MeshConfig>(pool, CFG_KEY).await.ok().flatten().unwrap_or_default();
    if cfg.channels().is_err() {
        tracing::warn!("mesh settings invalid; using defaults");
        cfg = MeshConfig::default();
    }
    let mut t = Tracker::default();
    let rows: Vec<(String,)> = sqlx::query_as("SELECT data FROM mesh_nodes").fetch_all(pool).await?;
    for (data,) in rows {
        if let Ok(n) = serde_json::from_str::<Node>(&data) {
            t.nodes.insert(n.key.clone(), n);
        }
    }
    let tracker = Arc::new(Mutex::new(t));
    let (cfg_tx, cfg_rx) = watch::channel(cfg);
    let last_iq = Arc::new(AtomicI64::new(0));
    let (tx, rx) = mpsc::unbounded_channel();
    {
        let (cfg_rx, last) = (cfg_rx.clone(), last_iq.clone());
        std::thread::Builder::new().name("mesh-demod".into()).spawn(move || feeder(iq, cfg_rx, last, tx))?;
    }
    let handle = MeshHandle { pool: pool.clone(), tracker, cfg_tx: Arc::new(cfg_tx), radio, last_iq };
    tokio::spawn(ingest(handle.clone(), rx, cfg_rx));
    tokio::spawn(housekeeping(handle.clone()));
    Ok(handle)
}

/// One channel of the capture: down-converter and resampler shared by the
/// presets on that frequency and bandwidth.
struct Chain {
    freq_hz: u64,
    bw: u32,
    ddc: Ddc,
    rs: lora::Resampler,
    demods: Vec<(usize, lora::Demod)>,
}

fn build_chains(center_hz: u64, rate: u32, cfg: &MeshConfig) -> Vec<Chain> {
    let mut chains: Vec<Chain> = Vec::new();
    for (i, p) in PRESETS.iter().enumerate() {
        if !cfg.presets.iter().any(|x| x == p.id) || !in_band(center_hz, rate, p) {
            continue;
        }
        let demod = lora::Demod::new(p.params(), p.freq_hz as f64);
        if let Some(c) = chains.iter_mut().find(|c| c.freq_hz == p.freq_hz && c.bw == p.bw) {
            c.demods.push((i, demod));
            continue;
        }
        // At least two samples per chip before resampling to one.
        let decim = (rate / (2 * p.bw)).max(1) as usize;
        let mid = rate as f64 / decim as f64;
        let offset = p.freq_hz as f64 - center_hz as f64;
        chains.push(Chain {
            freq_hz: p.freq_hz,
            bw: p.bw,
            ddc: Ddc::with_cutoff(rate, offset, decim, p.bw as f64 * 0.55),
            rs: lora::Resampler::new(mid, (p.bw as usize * lora::OS) as f64),
            demods: vec![(i, demod)],
        });
    }
    chains
}

/// Run the receive chain over a cu8 capture (tests).
#[cfg(test)]
pub(crate) fn decode_cu8(center_hz: u64, rate: u32, cu8: &[u8]) -> Vec<(usize, lora::Frame)> {
    let mut chains = build_chains(center_hz, rate, &MeshConfig::default());
    let mut got = Vec::new();
    for block in cu8.chunks(2 * 65_536) {
        for c in &mut chains {
            let (mut mid, mut chips) = (Vec::new(), Vec::new());
            c.ddc.process_cu8(block, &mut mid);
            c.rs.process(&mid, &mut chips);
            for (p, d) in &mut c.demods {
                let mut fr = Vec::new();
                d.feed(&chips, &mut fr);
                got.extend(fr.into_iter().map(|f| (*p, f)));
            }
        }
    }
    got
}

fn feeder(mut rx: broadcast::Receiver<Arc<IqBlock>>, cfg_rx: watch::Receiver<MeshConfig>, last: Arc<AtomicI64>, tx: mpsc::UnboundedSender<Rx>) {
    let mut chains: Vec<Chain> = Vec::new();
    let mut key: Option<(u64, u32, Vec<String>)> = None;
    let (mut mid, mut chips, mut frames) = (Vec::<Complex32>::new(), Vec::<Complex32>::new(), Vec::new());
    loop {
        let block = match rx.blocking_recv() {
            Ok(b) => b,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!("mesh demodulator lagged {n} blocks");
                key = None;
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };
        let cfg = cfg_rx.borrow().clone();
        if !cfg.enabled {
            key = None;
            chains.clear();
            continue;
        }
        let k = (block.center_hz, block.sample_rate, cfg.presets.clone());
        if key.as_ref() != Some(&k) {
            chains = build_chains(block.center_hz, block.sample_rate, &cfg);
            key = Some(k);
        }
        if chains.is_empty() {
            continue;
        }
        let now = db::now_ms();
        last.store(now, Ordering::Relaxed);
        for c in &mut chains {
            mid.clear();
            chips.clear();
            c.ddc.process_cu8(&block.data, &mut mid);
            c.rs.process(&mid, &mut chips);
            for (preset, d) in &mut c.demods {
                frames.clear();
                d.feed(&chips, &mut frames);
                for f in frames.drain(..) {
                    if tx.send(Rx { preset: *preset, frame: f, ts_ms: now }).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

async fn ingest(h: MeshHandle, mut rx: mpsc::UnboundedReceiver<Rx>, cfg_rx: watch::Receiver<MeshConfig>) {
    while let Some(r) = rx.recv().await {
        let (mt, mc) = cfg_rx.borrow().channels().unwrap_or_default();
        let (entry, msg) = {
            let mut t = h.tracker.lock().unwrap();
            handle_frame(&mut t, &r, &mt, &mc)
        };
        if let Some(m) = msg {
            let res = sqlx::query(
                "INSERT INTO mesh_messages (ts_ms, proto, channel, private, from_key, from_name, text, snr, hops) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(m.ts_ms)
            .bind(&m.proto)
            .bind(&m.channel)
            .bind(m.private)
            .bind(&m.from_key)
            .bind(&m.from_name)
            .bind(&m.text)
            .bind(m.snr)
            .bind(m.hops)
            .execute(&h.pool)
            .await;
            if let Err(e) = res {
                tracing::warn!("saving mesh message: {e}");
            }
        }
        tracing::debug!(preset = entry.preset, kind = %entry.kind, summary = %entry.summary, "mesh packet");
    }
}

/// Decode one frame into the tracker; returns its log entry and any channel
/// message to store.
fn handle_frame(t: &mut Tracker, r: &Rx, mt: &[meshtastic::Channel], mc: &[meshcore::Channel]) -> (LogEntry, Option<Message>) {
    let p = &PRESETS[r.preset];
    let f = &r.frame;
    let now = r.ts_ms;
    let mut e = LogEntry {
        ts_ms: now,
        preset: p.id,
        proto: p.proto,
        crc_ok: f.crc_ok,
        snr: (f.snr_db * 10.0).round() / 10.0,
        offset_hz: f.offset_hz.round(),
        len: f.payload.len(),
        raw: hex::encode(&f.payload),
        from: None,
        from_name: None,
        to: None,
        kind: "CRC error".into(),
        summary: String::new(),
        channel: None,
        private: false,
        hops: None,
        repeat: false,
    };
    let mut msg = None;
    if !f.crc_ok {
        t.crc_errors += 1;
    } else {
        let c = t.counts.entry(p.id).or_default();
        c.0 += 1;
        c.1 = now;
        match p.proto {
            Proto::Meshtastic => msg = meshtastic_frame(t, p, f, now, mt, &mut e),
            Proto::Meshcore => msg = meshcore_frame(t, p, f, now, mc, &mut e),
        }
    }
    t.log.push_back(e.clone());
    while t.log.len() > LOG_LEN {
        t.log.pop_front();
    }
    (e, msg)
}

fn meshtastic_frame(t: &mut Tracker, p: &Preset, f: &lora::Frame, now: i64, chans: &[meshtastic::Channel], e: &mut LogEntry) -> Option<Message> {
    use meshtastic::Content;
    let Some(pk) = meshtastic::decode(&f.payload, chans) else {
        e.kind = "too short".into();
        return None;
    };
    let h = &pk.header;
    let from = meshtastic::node_id(h.from);
    let key = format!("mt:{from}");
    let hops = (h.hop_start >= h.hop_limit && h.hop_start > 0).then(|| h.hop_start - h.hop_limit);
    e.repeat = t.repeat(hash64(&[b"mt", &h.from.to_le_bytes(), &h.id.to_le_bytes()]), now);
    e.from = Some(from.clone());
    e.to = Some(if h.to == meshtastic::BROADCAST { "all".into() } else { meshtastic::node_id(h.to) });
    e.channel = pk.channel.clone();
    e.private = pk.private;
    e.hops = hops;
    {
        let n = t.node(&key, Proto::Meshtastic, &from, p.id, now);
        n.packets += 1;
        // The copy straight from the node says most about the link to it.
        if !e.repeat || hops == Some(0) {
            n.snr = Some(e.snr);
            n.hops = hops;
        }
        // The node list is shown to everyone: what a node says on a private
        // channel (where it is, its name) stays in the admins' packet log.
        let content = if pk.private { &Content::Encrypted { pki: false } } else { &pk.content };
        match content {
            Content::Position { lat, lon, alt, .. } => {
                if (-90.0..=90.0).contains(lat) && (-180.0..=180.0).contains(lon) && (*lat != 0.0 || *lon != 0.0) {
                    (n.lat, n.lon, n.alt, n.pos_ms) = (Some(*lat), Some(*lon), *alt, Some(now));
                }
            }
            Content::NodeInfo { long_name, short_name, hw, role, .. } => {
                n.long_name = Some(long_name.clone()).filter(|s| !s.is_empty());
                n.short_name = Some(short_name.clone()).filter(|s| !s.is_empty());
                n.hw = Some(hw.clone());
                n.role = Some(role.clone());
            }
            Content::Telemetry { battery, voltage, channel_util, air_util_tx, uptime_s, temperature, humidity, pressure } => {
                n.battery = battery.or(n.battery);
                n.voltage = voltage.or(n.voltage);
                n.channel_util = channel_util.or(n.channel_util);
                n.air_util_tx = air_util_tx.or(n.air_util_tx);
                n.uptime_s = uptime_s.or(n.uptime_s);
                n.temperature = temperature.or(n.temperature);
                n.humidity = humidity.or(n.humidity);
                n.pressure = pressure.or(n.pressure);
            }
            _ => {}
        }
    }
    e.from_name = t.name_of(&key);
    let (kind, summary) = match &pk.content {
        Content::Encrypted { pki: true } => ("direct message".to_string(), "encrypted (public key)".to_string()),
        Content::Encrypted { pki: false } => ("encrypted".to_string(), format!("channel hash {:#04x}, no key", h.channel_hash)),
        Content::Text { text } => ("text".into(), text.clone()),
        Content::Position { lat, lon, alt, sats } => (
            "position".into(),
            format!("{lat:.5}, {lon:.5}{}{}", alt.map(|a| format!(", {a} m")).unwrap_or_default(), sats.map(|s| format!(", {s} sats")).unwrap_or_default()),
        ),
        Content::NodeInfo { long_name, short_name, hw, role, .. } => ("node info".into(), format!("{long_name} ({short_name}), {hw}, {role}")),
        Content::Telemetry { battery, voltage, temperature, .. } => {
            let mut s = Vec::new();
            if let Some(b) = battery {
                s.push(if *b > 100 { "powered".to_string() } else { format!("battery {b}%") });
            }
            if let Some(v) = voltage {
                s.push(format!("{v:.2} V"));
            }
            if let Some(c) = temperature {
                s.push(format!("{c:.1} °C"));
            }
            ("telemetry".into(), s.join(", "))
        }
        Content::Other { port_name, len, .. } => (port_name.clone(), format!("{len} bytes")),
    };
    e.kind = kind;
    e.summary = summary;
    match &pk.content {
        Content::Text { text } if !e.repeat && h.to == meshtastic::BROADCAST => Some(Message {
            id: 0,
            ts_ms: now,
            proto: "meshtastic".into(),
            channel: pk.channel.clone(),
            private: pk.private,
            from_key: Some(key),
            from_name: e.from_name.clone().or(Some(from)),
            text: text.clone(),
            snr: Some(e.snr),
            hops: hops.map(|h| h as i64),
        }),
        _ => None,
    }
}

fn meshcore_frame(t: &mut Tracker, p: &Preset, f: &lora::Frame, now: i64, chans: &[meshcore::Channel], e: &mut LogEntry) -> Option<Message> {
    use meshcore::Content;
    let Some(pk) = meshcore::decode(&f.payload, chans) else {
        e.kind = "too short".into();
        return None;
    };
    let hops = pk.path.len() as u8;
    e.hops = Some(hops);
    e.kind = pk.payload_type.to_string();
    e.channel = pk.channel.clone();
    e.private = pk.private;
    // The same packet repeated has a longer path; dedupe on what follows it.
    let body = pk_body(&f.payload).unwrap_or(&f.payload);
    e.repeat = t.repeat(hash64(&[b"mc", &[f.payload[0] >> 2 & 0x0F], body]), now);
    let mut msg = None;
    match &pk.content {
        Content::Advert { key, name, node_type, lat, lon, .. } => {
            let nkey = format!("mc:{key}");
            let id = key[..8].to_string();
            let n = t.node(&nkey, Proto::Meshcore, &id, p.id, now);
            n.packets += 1;
            n.long_name = name.clone();
            n.role = Some(node_type.clone());
            if !e.repeat || hops == 0 {
                n.snr = Some(e.snr);
                n.hops = Some(hops);
            }
            if let (Some(la), Some(lo)) = (lat, lon) {
                if *la != 0.0 || *lo != 0.0 {
                    (n.lat, n.lon, n.pos_ms) = (Some(*la), Some(*lo), Some(now));
                }
            }
            e.from = Some(id);
            e.from_name = name.clone();
            e.summary = format!(
                "{} ({node_type}){}",
                name.as_deref().unwrap_or("unnamed"),
                lat.zip(*lon).map(|(a, b)| format!(" at {a:.5}, {b:.5}")).unwrap_or_default()
            );
        }
        Content::GroupText { sender, text, .. } => {
            e.from_name = sender.clone();
            e.summary = text.clone();
            if !e.repeat {
                msg = Some(Message {
                    id: 0,
                    ts_ms: now,
                    proto: "meshcore".into(),
                    channel: pk.channel.clone(),
                    private: pk.private,
                    from_key: None,
                    from_name: sender.clone(),
                    text: text.clone(),
                    snr: Some(e.snr),
                    hops: Some(hops as i64),
                });
            }
        }
        Content::Encrypted { channel_hash, src_hash, dest_hash } => {
            e.summary = match (channel_hash, src_hash, dest_hash) {
                (Some(c), _, _) => format!("channel hash {c:#04x}, no key"),
                (None, s, Some(d)) => {
                    let who = |h: u8| t.mc_by_hash(h).map_or_else(|| format!("{h:02x}"), |n| format!("{n} ({h:02x})"));
                    e.from = s.map(|s| format!("{s:02x}"));
                    e.from_name = s.and_then(|s| t.mc_by_hash(s));
                    e.to = Some(format!("{d:02x}"));
                    match s {
                        Some(s) => format!("encrypted, {} → {}", who(*s), who(*d)),
                        None => format!("encrypted, to {}", who(*d)),
                    }
                }
                _ => "encrypted".into(),
            };
        }
        Content::Ack { crc } => e.summary = format!("{crc:08x}"),
        Content::Other { len } => e.summary = format!("{len} bytes"),
    }
    if !pk.path.is_empty() {
        e.summary = format!("{} · via {}", e.summary, pk.path.join(","));
    }
    msg
}

/// The payload after a MeshCore packet's header, transport codes and path.
fn pk_body(raw: &[u8]) -> Option<&[u8]> {
    let route = raw.first()? & 3;
    let mut i = if route == 0 || route == 3 { 5 } else { 1 };
    let pl = *raw.get(i)?;
    let (size, count) = if pl > 64 { ((pl >> 6) as usize + 1, (pl & 63) as usize) } else { (1, pl as usize) };
    i += 1 + size * count;
    raw.get(i..)
}

/// Save changed nodes every 30 s.
async fn housekeeping(h: MeshHandle) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let dirty: Vec<Node> = {
            let mut t = h.tracker.lock().unwrap();
            let keys: Vec<String> = t.dirty.drain().collect();
            keys.iter().filter_map(|k| t.nodes.get(k).cloned()).collect()
        };
        for n in dirty {
            let res = sqlx::query(
                "INSERT INTO mesh_nodes (key, data, last_ms) VALUES (?, ?, ?) \
                 ON CONFLICT(key) DO UPDATE SET data = excluded.data, last_ms = excluded.last_ms",
            )
            .bind(&n.key)
            .bind(serde_json::to_string(&n).unwrap_or_default())
            .bind(n.last_ms)
            .execute(&h.pool)
            .await;
            if let Err(e) = res {
                tracing::warn!("saving mesh node: {e}");
            }
        }
    }
}

/// Remove messages and nodes not heard for `days`.
pub async fn purge(pool: &SqlitePool, h: &MeshHandle, days: i64) -> anyhow::Result<()> {
    let cutoff = db::now_ms() - days * 86_400_000;
    sqlx::query("DELETE FROM mesh_messages WHERE ts_ms < ?").bind(cutoff).execute(pool).await?;
    sqlx::query("DELETE FROM mesh_nodes WHERE last_ms < ?").bind(cutoff).execute(pool).await?;
    h.tracker.lock().unwrap().nodes.retain(|_, n| n.last_ms >= cutoff);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(payload: Vec<u8>) -> lora::Frame {
        lora::Frame { payload, crc_ok: true, snr_db: 5.0, offset_hz: 120.0, sync: 0x2B }
    }

    #[test]
    fn config_channels() {
        let mut c = MeshConfig::default();
        let (mt, mc) = c.channels().unwrap();
        assert!(mt.iter().any(|c| c.name == "LongFast" && c.hash() == 8));
        assert_eq!(mc.len(), 1);
        c.meshtastic_channels.push(ChannelKey { name: "Club".into(), key: "AQ==".into() });
        c.meshcore_channels.push(ChannelKey { name: "#uk".into(), key: String::new() });
        c.meshcore_channels.push(ChannelKey { name: "Team".into(), key: hex::encode([9u8; 16]) });
        let (mt, mc) = c.channels().unwrap();
        assert!(!mt.last().unwrap().private, "default key under another name is public");
        assert_eq!(mc.len(), 3);
        assert!(mc[2].private);
        c.meshcore_channels.push(ChannelKey { name: "Nope".into(), key: String::new() });
        assert!(c.channels().is_err());
        assert!(!c.redacted().to_string().contains("AQ=="));
    }

    #[test]
    fn tracker_builds_nodes_and_messages() {
        let mut t = Tracker::default();
        let (mt, mc) = MeshConfig::default().channels().unwrap();
        let ch = mt[0].clone();
        let hdr = |id| meshtastic::Header {
            to: meshtastic::BROADCAST,
            from: 0xa1b2c3d4,
            id,
            hop_limit: 3,
            hop_start: 3,
            want_ack: false,
            via_mqtt: false,
            channel_hash: 0,
            next_hop: 0,
            relay: 0,
        };
        let user = meshtastic::encode(&hdr(1), &ch, 4, &meshtastic::build::user("!a1b2c3d4", "Base", "BS", 9, 0));
        let text = meshtastic::encode(&hdr(2), &ch, 1, b"hello");
        let rx = |payload| Rx { preset: 0, frame: frame(payload), ts_ms: 1000 };
        handle_frame(&mut t, &rx(user), &mt, &mc);
        let (e, m) = handle_frame(&mut t, &rx(text.clone()), &mt, &mc);
        assert_eq!(e.from_name.as_deref(), Some("Base"));
        let m = m.unwrap();
        assert_eq!((m.text.as_str(), m.channel.as_deref()), ("hello", Some("LongFast")));
        // A rebroadcast is logged as a repeat and not stored twice.
        let (e, m) = handle_frame(&mut t, &rx(text), &mt, &mc);
        assert!(e.repeat && m.is_none());
        assert_eq!(t.nodes["mt:!a1b2c3d4"].hw.as_deref(), Some("RAK4631"));
        // A position on a private channel stays out of the shared node list.
        let mut mt2 = mt.clone();
        mt2.push(meshtastic::Channel { name: "Club".into(), key: vec![3; 16], private: true });
        let pos = meshtastic::encode(&hdr(3), &mt2.last().unwrap().clone(), 3, &meshtastic::build::position(51.0, 0.0, 1, 1));
        let (e, _) = handle_frame(&mut t, &rx(pos), &mt2, &mc);
        assert!(e.private && e.kind == "position");
        assert_eq!(t.nodes["mt:!a1b2c3d4"].lat, None);

        let adv = meshcore::encode_advert(&[0x42; 32], 5, 1, "Carol", Some((52.0, 0.1)), &[]);
        let rxc = |payload| Rx { preset: 3, frame: frame(payload), ts_ms: 2000 };
        handle_frame(&mut t, &rxc(adv), &mt, &mc);
        let n = &t.nodes[&format!("mc:{}", hex::encode([0x42; 32]))];
        assert_eq!(n.long_name.as_deref(), Some("Carol"));
        assert_eq!(n.lat, Some(52.0));
        let dm = meshcore::encode_direct(0x42, 0x42, &[0; 18], &[]);
        let (e, _) = handle_frame(&mut t, &rxc(dm), &mt, &mc);
        assert!(e.summary.contains("Carol (42)"), "{}", e.summary);
        let gt = meshcore::encode_group_text(&mc[0], 9, "Carol", "hi all", &[7]);
        let (e, m) = handle_frame(&mut t, &rxc(gt.clone()), &mt, &mc);
        assert_eq!(e.hops, Some(1));
        assert_eq!(m.unwrap().text, "hi all");
        // Same message relayed once more: longer path, still a repeat.
        let gt2 = meshcore::encode_group_text(&mc[0], 9, "Carol", "hi all", &[7, 8]);
        let (e, m) = handle_frame(&mut t, &rxc(gt2), &mt, &mc);
        assert!(e.repeat && m.is_none());
        // Bad CRC: logged, nothing decoded.
        let mut bad = frame(vec![1, 2, 3]);
        bad.crc_ok = false;
        let (e, _) = handle_frame(&mut t, &Rx { preset: 0, frame: bad, ts_ms: 3000 }, &mt, &mc);
        assert_eq!(e.kind, "CRC error");
        assert_eq!(t.crc_errors, 1);
    }

    /// The whole receive chain from cu8 at 2.048 MS/s: modulate both kinds,
    /// put them on their carriers, and decode.
    #[test]
    fn receive_chain_from_iq() {
        let rate = 2_048_000u32;
        let center = TUNE_HZ;
        let (mt, mc) = MeshConfig::default().channels().unwrap();
        let hdr = meshtastic::Header {
            to: meshtastic::BROADCAST,
            from: 0x0102_0304,
            id: 77,
            hop_limit: 3,
            hop_start: 3,
            want_ack: false,
            via_mqtt: false,
            channel_hash: 0,
            next_hop: 0,
            relay: 0,
        };
        let mt_pkt = meshtastic::encode(&hdr, &mt[0], 1, b"over the air");
        let mc_pkt = meshcore::encode_group_text(&mc[0], 1, "Dave", "ping", &[]);
        let mut iq = vec![0f64; 0];
        let total = (rate as f64 * 1.0) as usize;
        iq.resize(total * 2, 0.0);
        for (preset, pkt, at) in [(&PRESETS[0], &mt_pkt, 0.05), (&PRESETS[3], &mc_pkt, 0.6)] {
            let w = lora::Waveform::new(&preset.params(), pkt);
            let off = preset.freq_hz as f64 - center as f64;
            let start = (at * rate as f64) as usize;
            for k in 0..total - start {
                let t = k as f64 / rate as f64;
                let Some(c) = w.phase_at(t) else { break };
                let ph = std::f64::consts::TAU * (c + off * t).fract();
                iq[2 * (start + k)] += 0.3 * ph.cos();
                iq[2 * (start + k) + 1] += 0.3 * ph.sin();
            }
        }
        let cu8: Vec<u8> = iq.iter().map(|&v| crate::radio::to_cu8(v as f32)).collect();
        assert_eq!(build_chains(center, rate, &MeshConfig::default()).len(), 2);
        let got = decode_cu8(center, rate, &cu8);
        let ok: Vec<_> = got.iter().filter(|(_, f)| f.crc_ok).collect();
        assert!(ok.iter().any(|(p, f)| *p == 0 && f.payload == mt_pkt), "{got:?}");
        assert!(ok.iter().any(|(p, f)| *p == 3 && f.payload == mc_pkt), "{got:?}");
    }
}
