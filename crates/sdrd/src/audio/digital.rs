//! Digital voice decoding for a listener (NFM with `Params::digital`).
//!
//! The raw FM discriminator is resampled to 48 kHz and piped to the
//! listener's own `dsd-neo` process, which finds the mode by itself: DMR,
//! D-STAR, System Fusion, NXDN, P25, dPMR, M17 and more. What it decodes
//! comes back two ways:
//!
//! - its JSON telemetry API on a loopback port (calls on air and the event
//!   history: callsigns, radio IDs, talkgroups, colour codes, text);
//! - its console log on stderr (sync lines and frame details), shown as is.
//!
//! dsd-neo runs with `-o null`: nothing is played. The listener keeps
//! hearing the FM audio (the digital "buzz").
//!
//! Messages to the browser (`{"type":"digital", …}`), one of:
//!
//! - `"calls":[{slot, protocol, src, dst, name, enc, emergency}]`: what is
//!   on air now (empty when nothing is), sent when it changes;
//! - `"event":{id, update, time, protocol, src, dst, code, alias, text, gps,
//!   summary, enc, emergency}`: a row of dsd-neo's event history (`update`
//!   when a row already sent changed, e.g. a talker alias arrived late);
//! - `"log":["…"]`: new console lines;
//! - `"error":"…"`: dsd-neo is missing or stopped.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Sample rate dsd-neo is fed (its native rate: 10 samples per 4800 Bd symbol).
pub const DSD_RATE: f64 = 48_000.0;
/// Discriminator ±1.0 (±5 kHz) maps to this many counts.
const SCALE: f32 = 16_000.0;
/// Console lines kept for one message (dsd-neo can be chatty).
const LOG_BATCH: usize = 40;

/// Linear-interpolating resampler from the demodulator's rate to 48 kHz. The
/// signal is band-limited by the channel filter well below either Nyquist
/// rate, so nothing fancier is needed.
struct Resampler {
    step: f64,
    pos: f64,
    prev: f32,
}

impl Resampler {
    fn new(in_rate: f64) -> Self {
        Resampler { step: in_rate / DSD_RATE, pos: 1.0, prev: 0.0 }
    }

    fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        // Index 0 is the previous block's last sample, index k is x[k - 1].
        let n = x.len() as f64;
        while self.pos < n {
            let i = self.pos as usize;
            let f = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.prev } else { x[i - 1] };
            out.push(a + (x[i] - a) * f);
            self.pos += self.step;
        }
        self.pos -= n;
        if let Some(&last) = x.last() {
            self.prev = last;
        }
    }
}

/// Each dsd-neo gets a fresh API port, so a restart never races the old
/// process for it.
fn next_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    9901 + NEXT.fetch_add(1, Ordering::Relaxed) % 100
}

/// From the reader threads to the listener.
enum Msg {
    /// A JSON message ready for the browser.
    Json(String),
    Log(String),
}

pub struct Digital {
    child: Child,
    stdin: SyncSender<Vec<u8>>,
    rx: Receiver<Msg>,
    stop: Arc<AtomicBool>,
    resampler: Option<(f64, Resampler)>,
    resampled: Vec<f32>,
    /// When the process was last checked for having exited.
    checked: Instant,
}

impl Drop for Digital {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Digital {
    /// Start dsd-neo at `path`.
    pub fn start(path: &str) -> Result<Self, String> {
        let port = next_port();
        let port_s = port.to_string();
        let args = ["-fa", "-i", "-", "-s", "48000", "-o", "null", "--api", &port_s];
        tracing::debug!("starting {path} {}", args.join(" "));
        let mut child = Command::new(path)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    format!("{path} isn't installed on the server (set SDRD_DSD_PATH)")
                }
                _ => format!("can't start {path}: {e}"),
            })?;
        let stop = Arc::new(AtomicBool::new(false));
        let (msg_tx, rx) = mpsc::channel();

        // Audio in: a second of slack, then blocks are dropped.
        let (stdin, audio_rx) = mpsc::sync_channel::<Vec<u8>>(20);
        let mut pipe = child.stdin.take().unwrap();
        spawn("dsd-in", move || {
            for chunk in audio_rx {
                if pipe.write_all(&chunk).is_err() {
                    break;
                }
            }
        })?;

        let stderr = child.stderr.take().unwrap();
        let tx = msg_tx.clone();
        spawn("dsd-log", move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Some(l) = console_line(&line) {
                    if tx.send(Msg::Log(l)).is_err() {
                        break;
                    }
                }
            }
        })?;

        let st = stop.clone();
        spawn("dsd-api", move || api_client(port, st, msg_tx))?;

        Ok(Digital {
            child,
            stdin,
            rx,
            stop,
            resampler: None,
            resampled: Vec::new(),
            checked: Instant::now(),
        })
    }

    /// Feed one block of discriminator output at `rate`. With the squelch
    /// closed (`open` false) it gets silence instead, so it doesn't sync to
    /// noise.
    pub fn feed(&mut self, disc: &[f32], rate: f64, open: bool) {
        if self.resampler.as_ref().is_none_or(|(r, _)| *r != rate) {
            self.resampler = Some((rate, Resampler::new(rate)));
        }
        let (_, rs) = self.resampler.as_mut().unwrap();
        self.resampled.clear();
        rs.process(disc, &mut self.resampled);
        let mut bytes = Vec::with_capacity(self.resampled.len() * 2);
        if !open {
            self.resampled.iter_mut().for_each(|v| *v = 0.0);
        }
        for v in &self.resampled {
            bytes.extend_from_slice(&((v * SCALE).clamp(-32_000.0, 32_000.0) as i16).to_le_bytes());
        }
        // A stalled decoder just loses audio; it resyncs on its own.
        if let Err(TrySendError::Disconnected(_)) = self.stdin.try_send(bytes) {
            self.checked = Instant::now() - Duration::from_secs(10);
        }
    }

    /// Messages for the browser since the last call. `Err` once dsd-neo has
    /// stopped (with what to tell the user).
    pub fn poll(&mut self, out: &mut Vec<String>) -> Result<(), String> {
        let mut log = Vec::new();
        for m in self.rx.try_iter() {
            match m {
                Msg::Json(j) => out.push(j),
                Msg::Log(l) if log.len() < LOG_BATCH => log.push(l),
                Msg::Log(_) => {}
            }
        }
        if !log.is_empty() {
            out.push(json!({ "type": "digital", "log": log }).to_string());
        }
        if self.checked.elapsed() >= Duration::from_secs(1) {
            self.checked = Instant::now();
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(format!("the digital decoder stopped ({status})"));
            }
        }
        Ok(())
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), String> {
    std::thread::Builder::new().name(name.into()).spawn(f).map(|_| ()).map_err(|e| e.to_string())
}

/// A console line worth showing, without dsd-neo's start-up notices.
fn console_line(line: &str) -> Option<String> {
    let l = line.trim_end();
    let t = l.trim_start();
    let noise = [
        "NOTICE:",
        "WARNING: Input Level",
        "Audio In Device",
        "API:",
        "Connection to STDIN",
        "Closing DSD",
        // Its symbol-rate search, a few lines a second on noise.
        "SPS hunt",
    ];
    (!t.is_empty() && !noise.iter().any(|n| t.starts_with(n))).then(|| l.to_string())
}

/// Talk to dsd-neo's telemetry API until it goes away.
fn api_client(port: u16, stop: Arc<AtomicBool>, tx: mpsc::Sender<Msg>) {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let t0 = Instant::now();
    let stream = loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
            Ok(s) => break s,
            Err(_) if t0.elapsed() < Duration::from_secs(10) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                tracing::warn!("dsd-neo API on port {port}: {e}");
                let msg = json!({ "type": "digital", "error": "couldn't reach the digital decoder's API" });
                let _ = tx.send(Msg::Json(msg.to_string()));
                return;
            }
        }
    };
    let Ok(mut w) = stream.try_clone() else { return };
    let sub = json!({ "id": 1, "cmd": "subscribe", "topics": ["call", "event"] });
    if writeln!(w, "{sub}").is_err() {
        return;
    }
    let mut calls_sent = String::new();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let msg = match v["type"].as_str() {
            Some("call") => {
                let calls = calls_msg(&v);
                if calls == calls_sent {
                    continue;
                }
                calls_sent = calls.clone();
                calls
            }
            Some("event") => event_msg(&v),
            _ => continue,
        };
        if tx.send(Msg::Json(msg)).is_err() {
            break;
        }
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("").trim()
}

/// A party as text: its name if it has one, else its number (`None` for 0).
fn party(text: &Value, id: &Value) -> Value {
    match (s(text), id.as_u64().unwrap_or(0)) {
        ("", 0) => Value::Null,
        ("", n) => json!(n.to_string()),
        (t, _) => json!(t),
    }
}

/// Calls on air now, from a `call` telemetry line.
fn calls_msg(v: &Value) -> String {
    let calls: Vec<Value> = v["slots"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|c| c["state"].as_u64() == Some(2))
        .map(|c| {
            json!({
                "slot": c["slot"],
                "protocol": s(&c["protocol_label"]),
                "src": party(&c["src_text"], &Value::Null),
                "dst": party(&c["tg_text"], &Value::Null),
                "name": s(&c["name"]),
                "enc": c["enc"].as_bool().unwrap_or(false),
                "emergency": c["emergency"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    json!({ "type": "digital", "calls": calls }).to_string()
}

/// The protocol of an event row: the prefix of its system id (`M17_CAN_5`,
/// `DMR_CC_1`, `P25_293`), else the first word of its text after the date.
fn protocol_of(row: &Value) -> String {
    let sys = s(&row["sysid_string"]);
    let word = match sys.split('_').next() {
        Some(p) if !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => p,
        _ => strip_time(s(&row["event"])).split_whitespace().next().unwrap_or(""),
    };
    match word {
        "DSTAR" => "D-STAR".into(),
        w => w.trim_end_matches(':').into(),
    }
}

/// Drop a leading `YYYY-MM-DD HH:MM:SS ` from an event's text.
fn strip_time(t: &str) -> &str {
    let b = t.as_bytes();
    if b.len() > 20 && b[4] == b'-' && b[7] == b'-' && b[13] == b':' && b[16] == b':' {
        t[19..].trim_start()
    } else {
        t
    }
}

/// An event history row, from an `event` telemetry line.
fn event_msg(v: &Value) -> String {
    let r = &v["row"];
    let code = r["access_code"].as_u64().unwrap_or(0);
    let code = match r["access_code_kind"].as_u64() {
        Some(1) => format!("CC {code}"),
        Some(2) => format!("NAC {code:03X}"),
        Some(3) => format!("RAN {code}"),
        Some(4) => format!("CAN {code}"),
        _ => String::new(),
    };
    let id = format!("{}:{}:{}", s(&r["ring"]), r["slot"], r["push"]);
    json!({
        "type": "digital",
        "event": {
            "id": id,
            "update": v["update"].as_bool().unwrap_or(false),
            "time": r["event_start_time"],
            "protocol": protocol_of(r),
            "src": party(&r["src_str"], &r["source_id"]),
            "dst": party(&r["tgt_str"], &r["target_id"]),
            "code": code,
            "alias": s(&r["alias"]),
            "text": s(&r["text"]),
            "gps": s(&r["gps"]),
            "summary": strip_time(s(&r["event"])).trim_end_matches(';'),
            "enc": r["enc"].as_u64().unwrap_or(0) != 0,
            "emergency": r["emergency"].as_u64().unwrap_or(0) != 0,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_a_tone_and_the_rate() {
        let in_rate = 51_200.0;
        let mut rs = Resampler::new(in_rate);
        let tone: Vec<f32> = (0..51_200).map(|i| (std::f64::consts::TAU * 1000.0 * i as f64 / in_rate).sin() as f32).collect();
        let mut out = Vec::new();
        for chunk in tone.chunks(2560) {
            rs.process(chunk, &mut out);
        }
        assert!((out.len() as i64 - 48_000).abs() <= 1, "{}", out.len());
        let zc = out.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!((zc as f64 / 2.0 - 1000.0).abs() < 2.0, "{zc}");
    }

    #[test]
    fn reads_an_m17_event_row() {
        // As dsd-neo 2.12 sends it (fields trimmed).
        let line = r#"{"type":"event","seq":6,"update":false,"row":{"slot":0,"push":3,"ring":"175978","sysid_string":"M17_CAN_5","source_id":2096993087,"target_id":0,"src_str":"G0TEST","tgt_str":"BROADCAST","access_code_kind":4,"access_code":5,"event_start_time":1794256319,"alias":"","gps":"","text":"","enc":0,"emergency":0,"event":"2026-11-09 20:31:59 M17 TGT: BROADCAST SRC: G0TEST CAN: 05;"}}"#;
        let m: Value = serde_json::from_str(&event_msg(&serde_json::from_str(line).unwrap())).unwrap();
        let e = &m["event"];
        assert_eq!(e["protocol"], "M17");
        assert_eq!(e["src"], "G0TEST");
        assert_eq!(e["dst"], "BROADCAST");
        assert_eq!(e["code"], "CAN 5");
        assert_eq!(e["id"], "175978:0:3");
        assert_eq!(e["summary"], "M17 TGT: BROADCAST SRC: G0TEST CAN: 05");
    }

    #[test]
    fn numbers_stand_in_for_missing_names() {
        let row = json!({"sysid_string":"DMR_CC_1","source_id":2345678,"target_id":91,"src_str":"","tgt_str":"","access_code_kind":1,"access_code":1,"event":"x"});
        let m: Value = serde_json::from_str(&event_msg(&json!({"row": row}))).unwrap();
        assert_eq!(m["event"]["protocol"], "DMR");
        assert_eq!(m["event"]["src"], "2345678");
        assert_eq!(m["event"]["dst"], "91");
        assert_eq!(m["event"]["code"], "CC 1");
    }

    #[test]
    fn only_active_calls_are_on_air() {
        let v = json!({"type":"call","slots":[
            {"slot":0,"state":1,"protocol_label":"","src_text":"","tg_text":""},
            {"slot":1,"state":2,"protocol_label":"DMR","src_text":"2345678","tg_text":"91","name":"","enc":false,"emergency":false}]});
        let m: Value = serde_json::from_str(&calls_msg(&v)).unwrap();
        let calls = m["calls"].as_array().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["protocol"], "DMR");
        assert_eq!(calls[0]["dst"], "91");
    }

    #[test]
    fn console_drops_start_up_noise() {
        assert_eq!(console_line("NOTICE: Decoding AUTO"), None);
        assert_eq!(console_line("   "), None);
        assert_eq!(console_line("SPS hunt: trying 5 sps (sym=9600)"), None);
        assert_eq!(console_line("20:31:59 Sync: +M17 LSF "), Some("20:31:59 Sync: +M17 LSF".into()));
    }
}
