//! MeshCore observer: sends the MeshCore packets we hear to the LetsMesh
//! analyzer (analyzer.letsmesh.net), as a MeshCore "observer" node does.
//!
//! The observer has a MeshCore identity of its own: an Ed25519 key pair made
//! on first use and kept in the settings table. The broker login is
//! `v1_<PUBLIC KEY>` with a JWT-style token signed by that key (header and
//! claims in base64url, the signature in hex, as meshcore-decoder makes
//! them). Packets go to `meshcore/<IATA>/<PUBLIC KEY>/packets` in the JSON of
//! meshcore-packet-capture, and the status (retained, with an offline last
//! will) to `.../status`.
//!
//! Only packets that passed their CRC are sent, and never while the Mesh
//! receiver is simulated or replayed: made-up packets would pollute a shared
//! map. Nothing we decrypted is sent: `raw` is the packet as it was on air.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use rumqttc::{AsyncClient, ConnectionError, Event, LastWill, MqttOptions, Packet, QoS, Transport};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::{mpsc, watch};

use super::{Proto, PRESETS};
use crate::db;
use crate::radio::RadioState;

const CFG_KEY: &str = "mesh.observer";
const KEY_KEY: &str = "mesh.observer.key";
/// Lifetime of a login token.
const TOKEN_TTL_S: i64 = 86_400;
/// Log in again with a fresh token this long before the old one runs out.
const RENEW_BEFORE_S: i64 = 600;
const STATUS_EVERY: Duration = Duration::from_secs(300);
const MAX_BACKOFF: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Broker {
    pub id: &'static str,
    pub name: &'static str,
    pub host: &'static str,
}

pub const BROKERS: &[Broker] = &[
    Broker { id: "letsmesh-eu", name: "LetsMesh EU", host: "mqtt-eu-v1.letsmesh.net" },
    Broker { id: "letsmesh-us", name: "LetsMesh US", host: "mqtt-us-v1.letsmesh.net" },
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObserverConfig {
    pub enabled: bool,
    /// One of [`BROKERS`].
    pub broker: String,
    /// IATA code of the nearest airport, naming the area ("LHR").
    pub iata: String,
    /// Name the analyzer shows for this observer.
    pub name: String,
    /// Optional: the owner's MeshCore public key, linking the observer to them.
    #[serde(default)]
    pub owner: String,
    /// Optional contact email, sent in the login token.
    #[serde(default)]
    pub email: String,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        ObserverConfig {
            enabled: false,
            broker: BROKERS[0].id.into(),
            iata: String::new(),
            name: "SDR observer".into(),
            owner: String::new(),
            email: String::new(),
        }
    }
}

impl ObserverConfig {
    /// Tidied and checked.
    pub fn checked(mut self) -> Result<Self, String> {
        self.iata = self.iata.trim().to_ascii_uppercase();
        self.name = self.name.trim().to_string();
        self.owner = self.owner.trim().to_ascii_uppercase();
        self.email = self.email.trim().to_string();
        if !BROKERS.iter().any(|b| b.id == self.broker) {
            return Err(format!("unknown server {}", self.broker));
        }
        if !self.iata.is_empty() && !(self.iata.len() == 3 && self.iata.chars().all(|c| c.is_ascii_uppercase())) {
            return Err("the IATA code is three letters, like LHR".into());
        }
        if self.enabled && self.iata.is_empty() {
            return Err("give the IATA code of the nearest airport first".into());
        }
        let n = self.name.chars().count();
        if self.name.chars().any(char::is_control) || n > 32 || (self.enabled && n == 0) {
            return Err("the observer name is 1–32 characters".into());
        }
        if !self.owner.is_empty() && !(self.owner.len() == 64 && self.owner.chars().all(|c| c.is_ascii_hexdigit())) {
            return Err("the owner key is a MeshCore public key: 64 hex characters".into());
        }
        if !self.email.is_empty() && (self.email.len() > 120 || !self.email.contains('@') || self.email.chars().any(char::is_whitespace)) {
            return Err("that email address doesn't look right".into());
        }
        Ok(self)
    }

    fn broker(&self) -> Broker {
        *BROKERS.iter().find(|b| b.id == self.broker).unwrap_or(&BROKERS[0])
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ObserverStatus {
    pub connected: bool,
    pub server: Option<String>,
    pub error: Option<String>,
    pub connected_ms: Option<i64>,
    /// Packets sent since sdrd started.
    pub sent: u64,
    pub last_sent_ms: Option<i64>,
}

/// One packet heard, offered to the observer.
struct Heard {
    ts_ms: i64,
    raw: Vec<u8>,
    snr: f32,
}

#[derive(Clone)]
pub struct ObserverHandle {
    pool: SqlitePool,
    cfg_tx: Arc<watch::Sender<ObserverConfig>>,
    status: Arc<Mutex<ObserverStatus>>,
    public_key: String,
    tx: mpsc::Sender<Heard>,
    radio: watch::Receiver<RadioState>,
}

/// Only real receivers feed the observer.
fn real_source(source: &str) -> bool {
    matches!(source, "rtlsdr" | "rtl_tcp")
}

impl ObserverHandle {
    pub fn config(&self) -> ObserverConfig {
        self.cfg_tx.borrow().clone()
    }

    pub async fn set_config(&self, cfg: ObserverConfig) -> Result<ObserverConfig, String> {
        let cfg = cfg.checked()?;
        db::set_setting(&self.pool, CFG_KEY, &cfg).await.map_err(|e| e.to_string())?;
        self.cfg_tx.send_replace(cfg.clone());
        Ok(cfg)
    }

    /// Settings, identity and connection state for the admin page.
    pub fn info(&self) -> Value {
        let cfg = self.config();
        let source = self.radio.borrow().source;
        let paused = (cfg.enabled && !real_source(source))
            .then_some("The Mesh receiver is simulated or replayed, so the observer stays offline until a real one is in use.");
        json!({
            "config": cfg,
            "public_key": self.public_key,
            "brokers": BROKERS,
            "status": self.status.lock().unwrap().clone(),
            "paused": paused,
        })
    }

    /// A MeshCore packet that passed its CRC. Never blocks: dropped when the
    /// observer is off, the receiver isn't real, or the queue is full.
    pub fn offer(&self, ts_ms: i64, raw: &[u8], snr: f32) {
        if !self.cfg_tx.borrow().enabled || !real_source(self.radio.borrow().source) {
            return;
        }
        let _ = self.tx.try_send(Heard { ts_ms, raw: raw.to_vec(), snr });
    }
}

pub async fn spawn(pool: &SqlitePool, radio: watch::Receiver<RadioState>) -> anyhow::Result<ObserverHandle> {
    let key = load_or_make_key(pool).await?;
    let mut cfg = db::get_setting::<ObserverConfig>(pool, CFG_KEY).await.ok().flatten().unwrap_or_default();
    if let Err(e) = cfg.clone().checked() {
        tracing::warn!("mesh observer settings invalid ({e}); switched off");
        cfg = ObserverConfig::default();
    }
    let (cfg_tx, cfg_rx) = watch::channel(cfg);
    let (tx, rx) = mpsc::channel(256);
    let h = ObserverHandle {
        pool: pool.clone(),
        cfg_tx: Arc::new(cfg_tx),
        status: Arc::default(),
        public_key: public_key(&key),
        tx,
        radio,
    };
    tokio::spawn(run(h.clone(), key, cfg_rx, rx));
    Ok(h)
}

#[derive(Serialize, Deserialize)]
struct StoredKey {
    /// Ed25519 seed, hex. Never sent to browsers.
    seed: String,
}

async fn load_or_make_key(pool: &SqlitePool) -> anyhow::Result<SigningKey> {
    if let Some(k) = db::get_setting::<StoredKey>(pool, KEY_KEY).await? {
        if let Some(seed) = hex::decode(&k.seed).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()) {
            return Ok(SigningKey::from_bytes(&seed));
        }
        tracing::warn!("stored mesh observer key unreadable; making a new one");
    }
    let seed: [u8; 32] = rand::random();
    db::set_setting(pool, KEY_KEY, &StoredKey { seed: hex::encode(seed) }).await?;
    let key = SigningKey::from_bytes(&seed);
    tracing::info!(public_key = %public_key(&key), "made a MeshCore observer identity");
    Ok(key)
}

fn public_key(key: &SigningKey) -> String {
    hex::encode_upper(key.verifying_key().to_bytes())
}

/// Login token for the broker `aud`: `header.claims.signature`, header and
/// claims base64url JSON, the Ed25519 signature of `header.claims` in hex.
pub fn auth_token(key: &SigningKey, aud: &str, now_s: i64, cfg: &ObserverConfig) -> String {
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut claims = Map::new();
    claims.insert("publicKey".into(), json!(public_key(key)));
    claims.insert("iat".into(), json!(now_s));
    claims.insert("exp".into(), json!(now_s + TOKEN_TTL_S));
    claims.insert("aud".into(), json!(aud));
    if !cfg.owner.is_empty() {
        claims.insert("owner".into(), json!(cfg.owner));
    }
    if !cfg.email.is_empty() {
        claims.insert("email".into(), json!(cfg.email));
    }
    claims.insert("client".into(), json!(client_version()));
    let input = format!(
        "{}.{}",
        b64.encode(br#"{"alg":"Ed25519","typ":"JWT"}"#),
        b64.encode(Value::Object(claims).to_string())
    );
    let sig = key.sign(input.as_bytes());
    format!("{input}.{}", hex::encode(sig.to_bytes()))
}

fn client_version() -> String {
    format!("sdrd/{}", env!("CARGO_PKG_VERSION"))
}

/// What the analyzers want from a packet's header.
#[derive(Debug, PartialEq)]
struct Header {
    payload_type: u8,
    /// F (flood), D (direct), T (transport direct).
    route: char,
    payload_len: usize,
    /// Repeater hashes, hex.
    path: Vec<String>,
    /// MeshCore's packet hash: SHA-256 of the payload type (and, for traces,
    /// the path length) and the payload; first 8 bytes in hex.
    hash: String,
}

fn header(raw: &[u8]) -> Option<Header> {
    let hdr = *raw.first()?;
    let route = hdr & 3;
    let payload_type = (hdr >> 2) & 0x0F;
    let mut i = 1;
    if route == 0 || route == 3 {
        i += 4; // transport codes
    }
    let path_len = *raw.get(i)?;
    i += 1;
    // Low 6 bits: hops; top 2: bytes per hop less one (4 is reserved and read
    // the old way, as a byte count).
    let (size, count) = match path_len >> 6 {
        3 => (1, path_len as usize),
        s => (s as usize + 1, (path_len & 63) as usize),
    };
    let path = raw.get(i..i + size * count)?.chunks(size).map(hex::encode_upper).collect();
    i += size * count;
    let payload = raw.get(i..)?;
    let mut h = Sha256::new();
    h.update([payload_type]);
    if payload_type == 9 {
        h.update((path_len as u16).to_le_bytes());
    }
    h.update(payload);
    Some(Header {
        payload_type,
        route: match route {
            2 => 'D',
            3 => 'T',
            _ => 'F',
        },
        payload_len: payload.len(),
        path,
        hash: hex::encode_upper(&h.finalize()[..8]),
    })
}

/// UTC date and time of a Unix time in ms: (year, month, day, h, m, s, ms).
fn utc(ms: i64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    let s = rem / 1000;
    (y, m, d, (s / 3600) as u32, (s / 60 % 60) as u32, (s % 60) as u32, (rem % 1000) as u32)
}

/// ISO 8601 as Python's `datetime.isoformat()` writes it in UTC.
fn iso(ms: i64) -> String {
    let (y, mo, d, h, mi, s, ms) = utc(ms);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{:06}+00:00", ms * 1000)
}

/// The packet message, or `None` if the header doesn't parse.
fn packet_message(name: &str, public_key: &str, p: &Heard) -> Option<Value> {
    let h = header(&p.raw)?;
    let (y, mo, d, hh, mi, s, _) = utc(p.ts_ms);
    let mut v = json!({
        "origin": name,
        "origin_id": public_key,
        "timestamp": iso(p.ts_ms),
        "type": "PACKET",
        "direction": "rx",
        "time": format!("{hh:02}:{mi:02}:{s:02}"),
        "date": format!("{d:02}/{mo:02}/{y:04}"),
        "len": p.raw.len().to_string(),
        "packet_type": h.payload_type.to_string(),
        "route": h.route.to_string(),
        "payload_len": h.payload_len.to_string(),
        "raw": hex::encode_upper(&p.raw),
        "SNR": format!("{:.1}", p.snr),
        // We measure SNR only; an SDR has no calibrated signal level in dBm.
        "RSSI": "Unknown",
        "hash": h.hash,
    });
    if h.route == 'D' {
        v["path"] = json!(h.path.join(","));
    }
    Some(v)
}

fn status_message(status: &str, cfg: &ObserverConfig, public_key: &str) -> Value {
    let radio = PRESETS
        .iter()
        .find(|p| p.proto == Proto::Meshcore)
        .map(|p| format!("{},{},{},{}", p.freq_hz as f64 / 1e6, p.bw as f64 / 1e3, p.sf, p.cr + 4))
        .unwrap_or_default();
    json!({
        "status": status,
        "timestamp": iso(db::now_ms()),
        "origin": cfg.name,
        "origin_id": public_key,
        "model": "sdrd (RTL-SDR receiver)",
        "firmware_version": env!("CARGO_PKG_VERSION"),
        "radio": radio,
        "client_version": client_version(),
    })
}

enum Ended {
    /// Settings changed or the token is due for renewal: connect again now.
    Again,
    Failed(String),
    Shutdown,
}

async fn run(h: ObserverHandle, key: SigningKey, mut cfg_rx: watch::Receiver<ObserverConfig>, mut rx: mpsc::Receiver<Heard>) {
    let mut backoff = Duration::from_secs(10);
    let mut radio = h.radio.clone();
    loop {
        let cfg = cfg_rx.borrow_and_update().clone();
        // Not even logged in while the receiver is simulated: a made-up
        // observer would show on the map.
        if !cfg.enabled || !real_source(radio.borrow_and_update().source) {
            {
                let mut s = h.status.lock().unwrap();
                *s = ObserverStatus { sent: s.sent, last_sent_ms: s.last_sent_ms, ..Default::default() };
            }
            if !idle(&mut cfg_rx, &mut radio, &mut rx, None).await {
                return;
            }
            continue;
        }
        match session(&h, &key, &cfg, &mut cfg_rx, &mut radio, &mut rx, &mut backoff).await {
            Ended::Again => {}
            Ended::Shutdown => return,
            Ended::Failed(e) => {
                tracing::warn!(server = cfg.broker().host, retry_s = backoff.as_secs(), "mesh observer: {e}");
                {
                    let mut s = h.status.lock().unwrap();
                    s.connected = false;
                    s.connected_ms = None;
                    s.error = Some(e);
                }
                if !idle(&mut cfg_rx, &mut radio, &mut rx, Some(backoff)).await {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// Drop packets until the settings change, the receiver changes (when not
/// waiting out a failure) or `wait` passes. False when sdrd is shutting down.
async fn idle(
    cfg_rx: &mut watch::Receiver<ObserverConfig>,
    radio: &mut watch::Receiver<RadioState>,
    rx: &mut mpsc::Receiver<Heard>,
    wait: Option<Duration>,
) -> bool {
    let until = tokio::time::Instant::now() + wait.unwrap_or(Duration::from_secs(365 * 86_400));
    loop {
        tokio::select! {
            r = cfg_rx.changed() => return r.is_ok(),
            r = radio.changed(), if wait.is_none() => return r.is_ok(),
            p = rx.recv() => if p.is_none() { return false },
            _ = tokio::time::sleep_until(until) => return true,
        }
    }
}

fn describe(e: &ConnectionError) -> String {
    match e {
        ConnectionError::ConnectionRefused(code) => format!("the server refused the login ({code:?})"),
        e => e.to_string(),
    }
}

async fn session(
    h: &ObserverHandle,
    key: &SigningKey,
    cfg: &ObserverConfig,
    cfg_rx: &mut watch::Receiver<ObserverConfig>,
    radio: &mut watch::Receiver<RadioState>,
    rx: &mut mpsc::Receiver<Heard>,
    backoff: &mut Duration,
) -> Ended {
    let broker = cfg.broker();
    let pk = &h.public_key;
    let base = format!("meshcore/{}/{pk}", cfg.iata);
    let now_s = db::now_ms() / 1000;
    let mut opts = MqttOptions::new(format!("sdrd_{}", &pk[..16]), format!("wss://{}:443/", broker.host), 443);
    opts.set_transport(Transport::wss_with_default_config());
    opts.set_keep_alive(Duration::from_secs(60));
    opts.set_credentials(format!("v1_{pk}"), auth_token(key, broker.host, now_s, cfg));
    opts.set_last_will(LastWill::new(
        format!("{base}/status"),
        status_message("offline", cfg, pk).to_string(),
        QoS::AtMostOnce,
        true,
    ));
    let (client, mut eventloop) = AsyncClient::new(opts, 256);
    h.status.lock().unwrap().server = Some(broker.host.to_string());

    // The event loop in a task of its own: cancelling `poll` mid-read isn't safe.
    let (ev_tx, mut ev_rx) = mpsc::channel::<Result<(), String>>(4);
    let poller = tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    if ev_tx.send(Ok(())).await.is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = ev_tx.send(Err(describe(&e))).await;
                    return;
                }
            }
        }
    });

    let renew_at = tokio::time::Instant::now() + Duration::from_secs((TOKEN_TTL_S - RENEW_BEFORE_S) as u64);
    let mut status_tick = tokio::time::interval(STATUS_EVERY);
    status_tick.tick().await;
    let mut connected = false;
    let publish_status = |status: &str| {
        let _ = client.try_publish(format!("{base}/status"), QoS::AtMostOnce, true, status_message(status, cfg, pk).to_string());
    };
    let ended = loop {
        tokio::select! {
            ev = ev_rx.recv() => match ev {
                Some(Ok(())) => {
                    connected = true;
                    *backoff = Duration::from_secs(10);
                    tracing::info!(server = broker.host, iata = %cfg.iata, "mesh observer connected");
                    {
                        let mut s = h.status.lock().unwrap();
                        s.connected = true;
                        s.error = None;
                        s.connected_ms = Some(db::now_ms());
                    }
                    publish_status("online");
                }
                Some(Err(e)) => break Ended::Failed(e),
                None => break Ended::Failed("connection closed".into()),
            },
            p = rx.recv() => {
                let Some(p) = p else { break Ended::Shutdown };
                if !connected {
                    continue;
                }
                let Some(msg) = packet_message(&cfg.name, pk, &p) else { continue };
                if client.try_publish(format!("{base}/packets"), QoS::AtMostOnce, false, msg.to_string()).is_ok() {
                    let mut s = h.status.lock().unwrap();
                    s.sent += 1;
                    s.last_sent_ms = Some(p.ts_ms);
                }
            }
            _ = status_tick.tick() => if connected { publish_status("online") },
            r = cfg_rx.changed() => {
                if r.is_err() {
                    break Ended::Shutdown;
                }
                // Off, or moved elsewhere: say so where we were.
                if connected {
                    publish_status("offline");
                }
                break Ended::Again;
            }
            r = radio.changed() => {
                if r.is_err() {
                    break Ended::Shutdown;
                }
                if !real_source(radio.borrow_and_update().source) {
                    if connected {
                        publish_status("offline");
                    }
                    break Ended::Again;
                }
            }
            _ = tokio::time::sleep_until(renew_at) => break Ended::Again,
        }
    };
    // Let the last messages and a clean disconnect go out, briefly.
    if connected && !matches!(ended, Ended::Failed(_)) {
        let _ = client.try_disconnect();
        let _ = tokio::time::timeout(Duration::from_secs(3), async { while ev_rx.recv().await.is_some() {} }).await;
    }
    poller.abort();
    {
        let mut s = h.status.lock().unwrap();
        s.connected = false;
        s.connected_ms = None;
    }
    ended
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::meshcore;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    fn cfg() -> ObserverConfig {
        ObserverConfig { enabled: true, iata: "lhr".into(), ..Default::default() }.checked().unwrap()
    }

    #[test]
    fn token_verifies() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let c = ObserverConfig { owner: "ab".repeat(32), ..cfg() }.checked().unwrap();
        let t = auth_token(&key, "mqtt-eu-v1.letsmesh.net", 1_700_000_000, &c);
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 3);
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header: Value = serde_json::from_slice(&b64.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header, json!({ "alg": "Ed25519", "typ": "JWT" }));
        let claims: Value = serde_json::from_slice(&b64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["publicKey"], json!(public_key(&key)));
        assert_eq!(claims["aud"], "mqtt-eu-v1.letsmesh.net");
        assert_eq!(claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(), TOKEN_TTL_S);
        assert_eq!(claims["owner"], json!("AB".repeat(32)));
        assert!(claims.get("email").is_none());
        let sig = Signature::from_slice(&hex::decode(parts[2]).unwrap()).unwrap();
        let vk = VerifyingKey::from_bytes(&hex::decode(public_key(&key)).unwrap().try_into().unwrap()).unwrap();
        vk.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).unwrap();
    }

    #[test]
    fn signs_like_rfc8032() {
        // RFC 8032 test 1: MeshCore's orlp/ed25519 signs the same way.
        let seed: [u8; 32] = hex::decode("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60").unwrap().try_into().unwrap();
        let key = SigningKey::from_bytes(&seed);
        assert_eq!(public_key(&key), "D75A980182B10AB7D54BFED3C964073A0EE172F3DAA62325AF021A68F707511A");
        assert_eq!(
            hex::encode(key.sign(b"").to_bytes()),
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        );
    }

    #[test]
    fn advert_message() {
        let raw = meshcore::encode_advert(&[0x42; 32], 1_700_000_000, 2, "Hilltop RPT", Some((51.75, -1.25)), &[0xab]);
        let h = header(&raw).unwrap();
        assert_eq!((h.payload_type, h.route, h.path.clone()), (4, 'F', vec!["AB".to_string()]));
        assert_eq!(h.payload_len, raw.len() - 3);
        let mut sha = Sha256::new();
        sha.update([4u8]);
        sha.update(&raw[3..]);
        assert_eq!(h.hash, hex::encode_upper(&sha.finalize()[..8]));
        let m = packet_message("Obs", "PK", &Heard { ts_ms: 1_700_000_000_123, raw: raw.clone(), snr: 7.26 }).unwrap();
        assert_eq!(m["timestamp"], "2023-11-14T22:13:20.123000+00:00");
        assert_eq!(m["time"], "22:13:20");
        assert_eq!(m["date"], "14/11/2023");
        assert_eq!(m["len"], raw.len().to_string());
        assert_eq!(m["packet_type"], "4");
        assert_eq!(m["SNR"], "7.3");
        assert_eq!(m["raw"], hex::encode_upper(&raw));
        assert!(m.get("path").is_none());
    }

    #[test]
    fn direct_has_path() {
        let raw = meshcore::encode_direct(0x12, 0x34, &[0u8; 18], &[0x01, 0x02]);
        let m = packet_message("Obs", "PK", &Heard { ts_ms: 0, raw, snr: -3.0 }).unwrap();
        assert_eq!((m["route"].as_str(), m["path"].as_str()), (Some("D"), Some("01,02")));
        assert_eq!(m["date"], "01/01/1970");
    }

    #[test]
    fn transport_codes_and_wide_hashes() {
        // Transport flood, two 2-byte hops.
        let mut raw = vec![(5 << 2), 1, 2, 3, 4, 0x40 | 2, 0xaa, 0xbb, 0xcc, 0xdd];
        raw.extend_from_slice(&[9u8; 20]);
        let h = header(&raw).unwrap();
        assert_eq!((h.route, h.payload_len), ('F', 20));
        assert_eq!(h.path, vec!["AABB", "CCDD"]);
        assert!(header(&[1, 5, 0]).is_none());
        assert!(header(&[]).is_none());
    }

    #[test]
    fn config_checks() {
        assert_eq!(cfg().iata, "LHR");
        assert!(ObserverConfig { enabled: true, ..Default::default() }.checked().is_err());
        assert!(ObserverConfig { iata: "LH".into(), ..Default::default() }.checked().is_err());
        assert!(ObserverConfig { owner: "xyz".into(), ..cfg() }.checked().is_err());
        assert!(ObserverConfig { broker: "nope".into(), ..cfg() }.checked().is_err());
        assert!(ObserverConfig { name: "".into(), ..cfg() }.checked().is_err());
        assert!(ObserverConfig { email: "a@b.example".into(), ..cfg() }.checked().is_ok());
        assert!(ObserverConfig::default().checked().is_ok());
    }

    #[test]
    fn only_real_receivers() {
        assert!(real_source("rtlsdr") && real_source("rtl_tcp"));
        assert!(!real_source("synthetic") && !real_source("replay") && !real_source("off"));
    }

    #[test]
    fn status_radio() {
        let s = status_message("online", &cfg(), "PK");
        assert_eq!(s["radio"], "869.618,62.5,8,8");
        assert_eq!(s["origin"], "SDR observer");
    }
}
