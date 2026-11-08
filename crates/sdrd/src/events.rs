//! Decoded events: group them into devices, store them in SQLite, push them to
//! WebSocket clients and MQTT.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Map, Value};
use sqlx::SqlitePool;
use tokio::sync::{broadcast, mpsc};

use crate::db;
use crate::decoders::DecodedEvent;
use crate::mqtt::MqttHandle;

/// Fields that vary between repeats of the same transmission.
const LEVEL_FIELDS: &[&str] = &["rssi", "snr", "noise", "freq", "freq1", "freq2", "mod"];
/// Many sensors repeat a message several times per transmission.
const DEDUPE_WINDOW: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct Device {
    pub id: i64,
    pub decoder: String,
    pub key: String,
    pub model: String,
    pub dev_id: Option<String>,
    pub channel: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub tracked: bool,
    pub first_seen: i64,
    pub last_seen: i64,
    pub count: i64,
    #[serde(serialize_with = "raw_json")]
    pub last_data: String,
}

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct EventRow {
    pub id: i64,
    pub device_id: i64,
    pub ts: i64,
    pub freq_hz: Option<i64>,
    pub rssi: Option<f64>,
    pub snr: Option<f64>,
    #[serde(serialize_with = "raw_json")]
    pub data: String,
}

/// Emit a stored JSON string as JSON rather than as a quoted string.
fn raw_json<S: serde::Serializer>(s: &str, ser: S) -> Result<S::Ok, S::Error> {
    let v: Value = serde_json::from_str(s).unwrap_or(Value::Null);
    v.serialize(ser)
}

impl Device {
    pub fn label(&self) -> String {
        if let Some(n) = &self.name {
            return n.clone();
        }
        let mut s = self.model.clone();
        if let Some(id) = &self.dev_id {
            s.push(' ');
            s.push_str(id);
        }
        if let Some(c) = &self.channel {
            s.push_str(" ch");
            s.push_str(c);
        }
        s
    }

    pub fn data(&self) -> Map<String, Value> {
        serde_json::from_str(&self.last_data).unwrap_or_default()
    }
}

fn field_str(data: &Map<String, Value>, k: &str) -> Option<String> {
    match data.get(k)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Stable identity for the transmitter that sent an event.
pub fn identity(ev: &DecodedEvent) -> (String, String, Option<String>, Option<String>) {
    let model = field_str(&ev.data, "model").unwrap_or_else(|| "unknown".into());
    let id = field_str(&ev.data, "id");
    let channel = field_str(&ev.data, "channel");
    let key = format!(
        "{}:{}:{}:{}",
        ev.decoder,
        model,
        id.as_deref().unwrap_or(""),
        channel.as_deref().unwrap_or("")
    );
    (key, model, id, channel)
}

fn fingerprint(data: &Map<String, Value>) -> String {
    let mut d = data.clone();
    for f in LEVEL_FIELDS {
        d.remove(*f);
    }
    Value::Object(d).to_string()
}

const DEVICE_COLS: &str = "id, decoder, key, model, dev_id, channel, kind, name, tracked, \
    first_seen, last_seen, count, last_data";

pub async fn list_devices(db: &SqlitePool, limit: i64) -> Result<Vec<Device>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {DEVICE_COLS} FROM devices ORDER BY last_seen DESC LIMIT ?"
    ))
    .bind(limit)
    .fetch_all(db)
    .await?)
}

pub async fn get_device(db: &SqlitePool, id: i64) -> Result<Option<Device>> {
    Ok(sqlx::query_as(&format!("SELECT {DEVICE_COLS} FROM devices WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await?)
}

pub async fn tracked_devices(db: &SqlitePool) -> Result<Vec<Device>> {
    Ok(sqlx::query_as(&format!("SELECT {DEVICE_COLS} FROM devices WHERE tracked = 1"))
        .fetch_all(db)
        .await?)
}

pub async fn update_device(db: &SqlitePool, id: i64, name: Option<Option<String>>, tracked: Option<bool>) -> Result<()> {
    if let Some(name) = name {
        sqlx::query("UPDATE devices SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(db)
            .await?;
    }
    if let Some(t) = tracked {
        sqlx::query("UPDATE devices SET tracked = ? WHERE id = ?")
            .bind(t)
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(())
}

pub async fn delete_device(db: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM devices WHERE id = ?").bind(id).execute(db).await?;
    Ok(r.rows_affected() > 0)
}

/// Newest first, between `since` (inclusive) and `before` (exclusive).
pub async fn device_events(
    db: &SqlitePool,
    device_id: i64,
    since: Option<i64>,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<EventRow>> {
    Ok(sqlx::query_as(
        "SELECT id, device_id, ts, freq_hz, rssi, snr, data FROM events \
         WHERE device_id = ? AND ts >= ? AND ts < ? ORDER BY ts DESC LIMIT ?",
    )
    .bind(device_id)
    .bind(since.unwrap_or(0))
    .bind(before.unwrap_or(i64::MAX))
    .bind(limit)
    .fetch_all(db)
    .await?)
}

pub async fn recent_events(db: &SqlitePool, limit: i64) -> Result<Vec<EventRow>> {
    Ok(sqlx::query_as(
        "SELECT id, device_id, ts, freq_hz, rssi, snr, data FROM events ORDER BY ts DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(db)
    .await?)
}

/// Delete events older than `days`, and untracked devices not heard since.
pub async fn purge(db: &SqlitePool, days: i64) -> Result<(u64, u64)> {
    let cutoff = db::now_ms() - days * 86_400_000;
    let e = sqlx::query("DELETE FROM events WHERE ts < ?").bind(cutoff).execute(db).await?;
    let d = sqlx::query("DELETE FROM devices WHERE last_seen < ? AND tracked = 0")
        .bind(cutoff)
        .execute(db)
        .await?;
    Ok((e.rows_affected(), d.rows_affected()))
}

async fn store(db: &SqlitePool, ev: &DecodedEvent) -> Result<(Device, EventRow)> {
    let (key, model, dev_id, channel) = identity(ev);
    let kind = field_str(&ev.data, "type");
    let data = Value::Object(ev.data.clone()).to_string();
    let mut tx = db.begin().await?;
    let device: Device = sqlx::query_as(&format!(
        "INSERT INTO devices (decoder, key, model, dev_id, channel, kind, first_seen, last_seen, count, last_data) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?) \
         ON CONFLICT(key) DO UPDATE SET last_seen = excluded.last_seen, count = count + 1, \
             last_data = excluded.last_data, kind = COALESCE(excluded.kind, kind) \
         RETURNING {DEVICE_COLS}"
    ))
    .bind(ev.decoder)
    .bind(&key)
    .bind(&model)
    .bind(&dev_id)
    .bind(&channel)
    .bind(&kind)
    .bind(ev.ts_ms)
    .bind(ev.ts_ms)
    .bind(&data)
    .fetch_one(&mut *tx)
    .await?;
    let rssi = ev.data.get("rssi").and_then(Value::as_f64);
    let snr = ev.data.get("snr").and_then(Value::as_f64);
    let freq_hz = ev.freq_hz.map(|f| f as i64);
    let id = sqlx::query(
        "INSERT INTO events (device_id, ts, freq_hz, rssi, snr, data) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(device.id)
    .bind(ev.ts_ms)
    .bind(freq_hz)
    .bind(rssi)
    .bind(snr)
    .bind(&data)
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    tx.commit().await?;
    let row = EventRow {
        id,
        device_id: device.id,
        ts: ev.ts_ms,
        freq_hz,
        rssi,
        snr,
        data,
    };
    Ok((device, row))
}

/// Pre-serialised `{"type":"event",...}` messages for WebSocket clients.
pub type EventFeed = broadcast::Sender<Arc<str>>;

pub fn ws_message(device: &Device, event: &EventRow) -> Arc<str> {
    json!({ "type": "event", "device": device, "event": event }).to_string().into()
}

pub async fn run(db: SqlitePool, mut rx: mpsc::Receiver<DecodedEvent>, feed: EventFeed, mqtt: Option<MqttHandle>) {
    let mut recent: HashMap<String, (Instant, String)> = HashMap::new();
    while let Some(ev) = rx.recv().await {
        let (key, ..) = identity(&ev);
        let fp = fingerprint(&ev.data);
        let now = Instant::now();
        if let Some((t, last)) = recent.get(&key) {
            if *last == fp && now.duration_since(*t) < DEDUPE_WINDOW {
                continue;
            }
        }
        recent.insert(key, (now, fp));
        if recent.len() > 4096 {
            recent.retain(|_, (t, _)| now.duration_since(*t) < DEDUPE_WINDOW);
        }

        match store(&db, &ev).await {
            Ok((device, row)) => {
                tracing::debug!(device = %device.key, "event");
                let _ = feed.send(ws_message(&device, &row));
                if let Some(m) = &mqtt {
                    m.publish_event(&device, &ev);
                }
            }
            Err(e) => tracing::warn!("storing event failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(v: Value) -> DecodedEvent {
        DecodedEvent::new("rtl_433", Some(433_920_000), v.as_object().unwrap().clone())
    }

    #[test]
    fn identity_and_fingerprint() {
        let a = ev(json!({"time":"@0.1s","model":"Nexus-TH","id":90,"channel":1,"temperature_C":21.3,"rssi":-9.7}));
        let b = ev(json!({"model":"Nexus-TH","id":90,"channel":1,"temperature_C":21.3,"rssi":-12.0}));
        assert_eq!(identity(&a).0, "rtl_433:Nexus-TH:90:1");
        assert!(!a.data.contains_key("time"));
        assert_eq!(fingerprint(&a.data), fingerprint(&b.data));
        let flex = ev(json!({"model":"TPMS","count":1,"num_rows":1}));
        assert_eq!(identity(&flex).0, "rtl_433:TPMS::");
    }

    #[tokio::test]
    async fn stores_and_groups_events() {
        let dir = std::env::temp_dir().join(format!("sdrd-ev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let db = db::open(&path).await.unwrap();
        let (d1, _) = store(&db, &ev(json!({"model":"Toyota","type":"TPMS","id":"abc","pressure_kPa":230.0}))).await.unwrap();
        let (d2, e2) = store(&db, &ev(json!({"model":"Toyota","type":"TPMS","id":"abc","pressure_kPa":231.0}))).await.unwrap();
        assert_eq!(d1.id, d2.id);
        assert_eq!(d2.count, 2);
        assert_eq!(d2.kind.as_deref(), Some("TPMS"));
        assert_eq!(device_events(&db, d1.id, None, None, 10).await.unwrap().len(), 2);
        assert_eq!(e2.device_id, d1.id);
        update_device(&db, d1.id, Some(Some("Car FL".into())), Some(true)).await.unwrap();
        let d = get_device(&db, d1.id).await.unwrap().unwrap();
        assert_eq!(d.label(), "Car FL");
        assert!(d.tracked);
        assert!(delete_device(&db, d1.id).await.unwrap());
        assert!(recent_events(&db, 10).await.unwrap().is_empty()); // cascaded
        let _ = std::fs::remove_dir_all(&dir);
    }
}
