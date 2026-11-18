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
use crate::decoders::burst::{RawSignal, RawStore};
use crate::decoders::DecodedEvent;
use crate::mqtt::MqttHandle;

/// Fields that vary between repeats of the same transmission (for APRS, the
/// digipeater path differs between copies of one packet).
const LEVEL_FIELDS: &[&str] = &["rssi", "snr", "noise", "freq", "freq1", "freq2", "mod", "path", "raw"];

/// Window in which an identical message from the same device is a repeat:
/// sensors send bursts; APRS packets come back via digipeaters for a while.
fn dedupe_window(decoder: &str) -> Duration {
    if decoder == crate::decoders::aprs::NAME {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(3)
    }
}

/// Amateur radio traffic is public; everything else (TPMS, neighbours'
/// sensors) is shown to admins only unless configured otherwise.
pub fn is_public(decoder: &str) -> bool {
    decoder == crate::decoders::aprs::NAME
        || decoder == crate::decoders::sstv::NAME
        || decoder == crate::scheduler::PASSES
}

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
    /// Last reported position (kept when later packets have none).
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub pos_ts: Option<i64>,
    pub symbol: Option<String>,
    /// Position on the sensor dashboard (lowest first); `None` = not shown.
    pub dash_pos: Option<i64>,
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
    /// Receiver debug details (see `DecodedEvent::rx`), as JSON.
    #[serde(serialize_with = "raw_json_opt")]
    pub rx: Option<String>,
    /// A radio burst was saved with this event (see `get_burst`).
    pub burst: bool,
}

/// Emit a stored JSON string as JSON rather than as a quoted string.
fn raw_json<S: serde::Serializer>(s: &str, ser: S) -> Result<S::Ok, S::Error> {
    let v: Value = serde_json::from_str(s).unwrap_or(Value::Null);
    v.serialize(ser)
}

fn raw_json_opt<S: serde::Serializer>(s: &Option<String>, ser: S) -> Result<S::Ok, S::Error> {
    raw_json(s.as_deref().unwrap_or("null"), ser)
}

impl Device {
    pub fn label(&self) -> String {
        if let Some(n) = &self.name {
            return n.clone();
        }
        // APRS stations and objects are best known by their callsign/name.
        if is_public(&self.decoder) {
            if let Some(id) = &self.dev_id {
                return id.clone();
            }
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
    first_seen, last_seen, count, last_data, lat, lon, pos_ts, symbol, dash_pos";

pub async fn list_devices(db: &SqlitePool, decoder: Option<&str>, limit: i64) -> Result<Vec<Device>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {DEVICE_COLS} FROM devices WHERE (?1 IS NULL OR decoder = ?1) \
         ORDER BY last_seen DESC LIMIT ?2"
    ))
    .bind(decoder)
    .bind(limit)
    .fetch_all(db)
    .await?)
}

/// Positions per device since `since`, oldest first: (device_id, ts, lat, lon).
pub async fn tracks(db: &SqlitePool, decoder: &str, since: i64) -> Result<Vec<(i64, i64, f64, f64)>> {
    Ok(sqlx::query_as(
        "SELECT e.device_id, e.ts, json_extract(e.data, '$.lat'), json_extract(e.data, '$.lon') \
         FROM events e JOIN devices d ON d.id = e.device_id \
         WHERE d.decoder = ? AND e.ts >= ? AND json_extract(e.data, '$.lat') IS NOT NULL \
         ORDER BY e.device_id, e.ts LIMIT 50000",
    )
    .bind(decoder)
    .bind(since)
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

/// Devices on the dashboard, in display order.
pub async fn dashboard_devices(db: &SqlitePool) -> Result<Vec<Device>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {DEVICE_COLS} FROM devices WHERE dash_pos IS NOT NULL ORDER BY dash_pos, id"
    ))
    .fetch_all(db)
    .await?)
}

/// Add a device to the end of the dashboard, or take it off. Adding also
/// tracks it, so the purge can't delete a device that's on show.
pub async fn set_on_dashboard(db: &SqlitePool, id: i64, on: bool) -> Result<()> {
    if on {
        sqlx::query(
            "UPDATE devices SET tracked = 1, \
                 dash_pos = COALESCE(dash_pos, (SELECT COALESCE(MAX(dash_pos), 0) + 1 FROM devices)) \
             WHERE id = ?",
        )
        .bind(id)
        .execute(db)
        .await?;
    } else {
        sqlx::query("UPDATE devices SET dash_pos = NULL WHERE id = ?")
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(())
}

/// Renumber the dashboard in the given order. Devices not on the dashboard
/// are ignored; ones on it but missing from `ids` keep their place after them.
pub async fn reorder_dashboard(db: &SqlitePool, ids: &[i64]) -> Result<()> {
    let mut tx = db.begin().await?;
    let mut order: Vec<i64> = ids.to_vec();
    let rest: Vec<(i64,)> = sqlx::query_as("SELECT id FROM devices WHERE dash_pos IS NOT NULL ORDER BY dash_pos, id")
        .fetch_all(&mut *tx)
        .await?;
    order.extend(rest.into_iter().map(|(id,)| id).filter(|id| !ids.contains(id)));
    for (i, id) in order.iter().enumerate() {
        sqlx::query("UPDATE devices SET dash_pos = ? WHERE id = ? AND dash_pos IS NOT NULL")
            .bind(i as i64 + 1)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn delete_device(db: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM devices WHERE id = ?").bind(id).execute(db).await?;
    Ok(r.rows_affected() > 0)
}

/// Newest first, from `since` (inclusive) to before the event at
/// (`before`, `before_id`) (exclusive; a missing id means all of that time).
pub async fn device_events(
    db: &SqlitePool,
    device_id: i64,
    since: Option<i64>,
    before: Option<i64>,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<EventRow>> {
    Ok(sqlx::query_as(
        "SELECT id, device_id, ts, freq_hz, rssi, snr, data, rx, \
         EXISTS(SELECT 1 FROM bursts b WHERE b.event_id = events.id) AS burst FROM events \
         WHERE device_id = ? AND ts >= ? AND (ts, id) < (?, ?) ORDER BY ts DESC, id DESC LIMIT ?",
    )
    .bind(device_id)
    .bind(since.unwrap_or(0))
    .bind(before.unwrap_or(i64::MAX))
    .bind(before_id.unwrap_or(i64::MIN))
    .bind(limit)
    .fetch_all(db)
    .await?)
}

/// Oldest first, after the event at `after` (ts, id) and before `until`; for
/// paging through a long export.
pub async fn events_after(
    db: &SqlitePool,
    device_id: i64,
    after: (i64, i64),
    until: i64,
    limit: i64,
) -> Result<Vec<EventRow>> {
    Ok(sqlx::query_as(
        "SELECT id, device_id, ts, freq_hz, rssi, snr, data, NULL AS rx, 0 AS burst FROM events \
         WHERE device_id = ? AND (ts, id) > (?, ?) AND ts < ? ORDER BY ts, id LIMIT ?",
    )
    .bind(device_id)
    .bind(after.0)
    .bind(after.1)
    .bind(until)
    .bind(limit)
    .fetch_all(db)
    .await?)
}

/// Every data field a device reported between `since` and `until`, sorted.
pub async fn data_keys(db: &SqlitePool, device_id: i64, since: i64, until: i64) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT j.key FROM events e, json_each(e.data) j \
         WHERE e.device_id = ? AND e.ts >= ? AND e.ts < ? AND json_valid(e.data) ORDER BY j.key",
    )
    .bind(device_id)
    .bind(since)
    .bind(until)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|(k,)| k).collect())
}

/// Average of each numeric field per `step` ms between `since` and `until`,
/// for charts over long ranges: (bucket start, one average per field).
/// `fields` must already be checked to be plain names (letters, digits, _).
pub async fn device_series(
    db: &SqlitePool,
    device_id: i64,
    since: i64,
    until: i64,
    step: i64,
    fields: &[String],
) -> Result<Vec<(i64, Vec<Option<f64>>)>> {
    use sqlx::Row;
    let cols: String = fields
        .iter()
        .map(|f| {
            format!(
                ", AVG(CASE WHEN json_type(data, '$.{f}') IN ('integer', 'real') \
                 THEN json_extract(data, '$.{f}') END)"
            )
        })
        .collect();
    let sql = format!(
        "SELECT (ts - ?1) / ?2 AS b{cols} FROM events \
         WHERE device_id = ?3 AND ts >= ?1 AND ts < ?4 AND json_valid(data) GROUP BY b ORDER BY b"
    );
    let rows = sqlx::query(&sql)
        .bind(since)
        .bind(step)
        .bind(device_id)
        .bind(until)
        .fetch_all(db)
        .await?;
    rows.iter()
        .map(|r| {
            let b: i64 = r.try_get(0)?;
            let vals = (1..=fields.len())
                .map(|i| r.try_get::<Option<f64>, _>(i))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((since + b * step, vals))
        })
        .collect()
}

pub async fn recent_events(db: &SqlitePool, decoder: Option<&str>, limit: i64) -> Result<Vec<EventRow>> {
    Ok(sqlx::query_as(
        "SELECT e.id, e.device_id, e.ts, e.freq_hz, e.rssi, e.snr, e.data, e.rx, \
         EXISTS(SELECT 1 FROM bursts b WHERE b.event_id = e.id) AS burst FROM events e \
         JOIN devices d ON d.id = e.device_id WHERE (?1 IS NULL OR d.decoder = ?1) \
         ORDER BY e.ts DESC LIMIT ?2",
    )
    .bind(decoder)
    .bind(limit)
    .fetch_all(db)
    .await?)
}

/// Delete events older than `days`, and untracked devices not heard since.
/// Tracked sensors keep their readings for `sensor_days` instead, for
/// long-range charts. SSTV images and pass recordings are kept until deleted
/// by hand.
pub async fn purge(db: &SqlitePool, days: i64, sensor_days: i64) -> Result<(u64, u64)> {
    let now = db::now_ms();
    let cutoff = now - days * 86_400_000;
    let sensor_cutoff = now - sensor_days * 86_400_000;
    let e = sqlx::query(
        "DELETE FROM events WHERE ts < ?1 \
         AND device_id NOT IN (SELECT id FROM devices WHERE decoder IN ('sstv', 'passes') \
             OR (decoder = ?2 AND tracked = 1))",
    )
    .bind(cutoff)
    .bind(crate::decoders::rtl433::NAME)
    .execute(db)
    .await?;
    let s = sqlx::query(
        "DELETE FROM events WHERE ts < ?1 \
         AND device_id IN (SELECT id FROM devices WHERE decoder = ?2 AND tracked = 1)",
    )
    .bind(sensor_cutoff)
    .bind(crate::decoders::rtl433::NAME)
    .execute(db)
    .await?;
    let d = sqlx::query("DELETE FROM devices WHERE last_seen < ? AND tracked = 0 AND decoder NOT IN ('sstv', 'passes')")
        .bind(cutoff)
        .execute(db)
        .await?;
    Ok((e.rows_affected() + s.rows_affected(), d.rows_affected()))
}

/// (id, device_id, ts, freq_hz, rssi, snr, data, rx, burst, decoder)
type EventDecoderRow = (i64, i64, i64, Option<i64>, Option<f64>, Option<f64>, String, Option<String>, bool, String);

/// An event and the decoder of its device.
pub async fn get_event(db: &SqlitePool, id: i64) -> Result<Option<(EventRow, String)>> {
    let row: Option<EventDecoderRow> = sqlx::query_as(
        "SELECT e.id, e.device_id, e.ts, e.freq_hz, e.rssi, e.snr, e.data, e.rx, \
         EXISTS(SELECT 1 FROM bursts b WHERE b.event_id = e.id), d.decoder \
         FROM events e JOIN devices d ON d.id = e.device_id WHERE e.id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(id, device_id, ts, freq_hz, rssi, snr, data, rx, burst, decoder)| {
        (EventRow { id, device_id, ts, freq_hz, rssi, snr, data, rx, burst }, decoder)
    }))
}

pub async fn delete_event(db: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM events WHERE id = ?").bind(id).execute(db).await?;
    Ok(r.rows_affected() > 0)
}

/// Keep a radio burst with its event (removed with the event or device).
pub async fn save_burst(db: &SqlitePool, event_id: i64, sig: &RawSignal) -> Result<()> {
    let mut meta = serde_json::to_value(sig)?;
    meta["file_name"] = sig.file_name().into();
    let iq: Vec<u8> = sig.iq.iter().map(|b| *b as u8).collect();
    sqlx::query("INSERT OR REPLACE INTO bursts (event_id, meta, iq) VALUES (?, ?, ?)")
        .bind(event_id)
        .bind(meta.to_string())
        .bind(iq)
        .execute(db)
        .await?;
    Ok(())
}

/// A saved burst: its description (JSON) and cs8 IQ.
pub async fn get_burst(db: &SqlitePool, event_id: i64) -> Result<Option<(String, Vec<u8>)>> {
    Ok(sqlx::query_as("SELECT meta, iq FROM bursts WHERE event_id = ?")
        .bind(event_id)
        .fetch_optional(db)
        .await?)
}

pub async fn delete_burst(db: &SqlitePool, event_id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM bursts WHERE event_id = ?").bind(event_id).execute(db).await?;
    Ok(r.rows_affected() > 0)
}

async fn store(db: &SqlitePool, ev: &DecodedEvent) -> Result<(Device, EventRow)> {
    let (key, model, dev_id, channel) = identity(ev);
    let kind = field_str(&ev.data, "type");
    let data = Value::Object(ev.data.clone()).to_string();
    let lat = ev.data.get("lat").and_then(Value::as_f64);
    let lon = ev.data.get("lon").and_then(Value::as_f64);
    let (lat, lon, pos_ts) = match (lat, lon) {
        (Some(a), Some(o)) => (Some(a), Some(o), Some(ev.ts_ms)),
        _ => (None, None, None),
    };
    let symbol = field_str(&ev.data, "symbol");
    let mut tx = db.begin().await?;
    let device: Device = sqlx::query_as(&format!(
        "INSERT INTO devices (decoder, key, model, dev_id, channel, kind, first_seen, last_seen, count, last_data, \
             lat, lon, pos_ts, symbol) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?) \
         ON CONFLICT(key) DO UPDATE SET last_seen = excluded.last_seen, count = count + 1, \
             last_data = excluded.last_data, kind = COALESCE(excluded.kind, kind), \
             lat = COALESCE(excluded.lat, lat), lon = COALESCE(excluded.lon, lon), \
             pos_ts = COALESCE(excluded.pos_ts, pos_ts), symbol = COALESCE(excluded.symbol, symbol) \
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
    .bind(lat)
    .bind(lon)
    .bind(pos_ts)
    .bind(&symbol)
    .fetch_one(&mut *tx)
    .await?;
    let rssi = ev.data.get("rssi").and_then(Value::as_f64);
    let snr = ev.data.get("snr").and_then(Value::as_f64);
    let freq_hz = ev.freq_hz.map(|f| f as i64);
    let rx = ev.rx.as_ref().map(|m| Value::Object(m.clone()).to_string());
    let id = sqlx::query(
        "INSERT INTO events (device_id, ts, freq_hz, rssi, snr, data, rx) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(device.id)
    .bind(ev.ts_ms)
    .bind(freq_hz)
    .bind(rssi)
    .bind(snr)
    .bind(&data)
    .bind(&rx)
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
        rx,
        burst: false,
    };
    Ok((device, row))
}

/// A pre-serialised `{"type":"event",...}` message for WebSocket clients.
#[derive(Clone)]
pub struct FeedMsg {
    /// Viewers may see it (see `is_public`).
    pub public: bool,
    pub json: Arc<str>,
}

pub type EventFeed = broadcast::Sender<FeedMsg>;

pub fn ws_message(device: &Device, event: &EventRow) -> FeedMsg {
    FeedMsg {
        public: is_public(&device.decoder),
        json: json!({ "type": "event", "device": device, "event": event }).to_string().into(),
    }
}

pub async fn run(
    db: SqlitePool,
    mut rx: mpsc::Receiver<DecodedEvent>,
    feed: EventFeed,
    mqtt: Option<MqttHandle>,
    raw: RawStore,
) {
    // Per device: when its last event was stored, that event's fingerprint,
    // and the event itself while it still waits for a burst to be logged.
    // Sensors repeat each packet several times; a repeat is dropped as a
    // duplicate, but if the stored copy had no signal and the device is armed,
    // a repeat's burst is saved with it. Only a burst checked to be the
    // device's own (`verified`) is saved.
    let mut recent: HashMap<String, (Instant, String, Option<Box<(Device, EventRow)>>)> = HashMap::new();
    while let Some(ev) = rx.recv().await {
        let (key, ..) = identity(&ev);
        let fp = fingerprint(&ev.data);
        let now = Instant::now();
        let window = dedupe_window(ev.decoder);
        if let Some((t, last, waiting)) = recent.get_mut(&key) {
            if *last == fp && now.duration_since(*t) < window {
                if let (Some(sig), Some(w)) = (ev.signal.as_ref().filter(|s| s.verified), waiting.as_mut()) {
                    if raw.take(&key) {
                        let (device, row) = &mut **w;
                        log_burst(&db, &feed, device, row, sig).await;
                        *waiting = None;
                    }
                }
                continue;
            }
        }
        if recent.len() > 4096 {
            recent.retain(|_, (t, ..)| now.duration_since(*t) < Duration::from_secs(30));
        }

        match store(&db, &ev).await {
            Ok((device, mut row)) => {
                tracing::debug!(device = %device.key, "event");
                let mut waiting = None;
                if raw.wants(&device.key) {
                    match ev.signal.as_ref().filter(|s| s.verified) {
                        Some(sig) if raw.take(&device.key) => {
                            save_and_flag(&db, &device, &mut row, sig).await;
                        }
                        Some(_) => {}
                        None => waiting = Some(Box::new((device.clone(), row.clone()))),
                    }
                }
                recent.insert(key, (now, fp, waiting));
                let _ = feed.send(ws_message(&device, &row));
                if let Some(m) = &mqtt {
                    m.publish_event(&device, &ev);
                }
            }
            Err(e) => {
                recent.insert(key, (now, fp, None));
                tracing::warn!("storing event failed: {e}");
            }
        }
    }
}

/// Save a burst with a stored event and mark the row.
async fn save_and_flag(db: &SqlitePool, device: &Device, row: &mut EventRow, sig: &RawSignal) {
    match save_burst(db, row.id, sig).await {
        Ok(()) => {
            row.burst = true;
            tracing::info!(device = %device.key, event = row.id, "burst logged");
        }
        Err(e) => tracing::warn!("saving burst failed: {e}"),
    }
}

/// Save a repeat's burst with the already-sent event and tell browsers again
/// (same event id, now flagged), so they update the row in place.
async fn log_burst(db: &SqlitePool, feed: &EventFeed, device: &Device, row: &mut EventRow, sig: &RawSignal) {
    save_and_flag(db, device, row, sig).await;
    if row.burst {
        let _ = feed.send(ws_message(device, row));
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
        assert_eq!(device_events(&db, d1.id, None, None, None, 10).await.unwrap().len(), 2);
        assert_eq!(e2.device_id, d1.id);
        update_device(&db, d1.id, Some(Some("Car FL".into())), Some(true)).await.unwrap();
        let d = get_device(&db, d1.id).await.unwrap().unwrap();
        assert_eq!(d.label(), "Car FL");
        assert!(d.tracked);

        // Dashboard: pinning tracks, appends, and reorders.
        let (t1, _) = store(&db, &ev(json!({"model":"Nexus-TH","id":1,"temperature_C":20.0}))).await.unwrap();
        let (t2, _) = store(&db, &ev(json!({"model":"Nexus-TH","id":2,"temperature_C":21.0}))).await.unwrap();
        assert!(!t1.tracked && t1.dash_pos.is_none());
        set_on_dashboard(&db, t1.id, true).await.unwrap();
        set_on_dashboard(&db, t2.id, true).await.unwrap();
        set_on_dashboard(&db, t1.id, true).await.unwrap(); // already on: keeps its place
        let ids = |v: Vec<Device>| v.into_iter().map(|d| d.id).collect::<Vec<_>>();
        assert_eq!(ids(dashboard_devices(&db).await.unwrap()), vec![t1.id, t2.id]);
        assert!(get_device(&db, t1.id).await.unwrap().unwrap().tracked);
        reorder_dashboard(&db, &[t2.id, d1.id]).await.unwrap(); // d1 isn't on it: ignored
        assert_eq!(ids(dashboard_devices(&db).await.unwrap()), vec![t2.id, t1.id]);
        assert!(get_device(&db, d1.id).await.unwrap().unwrap().dash_pos.is_none());
        set_on_dashboard(&db, t2.id, false).await.unwrap();
        assert_eq!(ids(dashboard_devices(&db).await.unwrap()), vec![t1.id]);

        // A saved burst comes back with its event and goes with it.
        let c = num_complex::Complex32::new;
        let mut iq = vec![c(0.01, 0.0); 100];
        iq.extend(std::iter::repeat_n(c(1.0, 0.0), 500));
        iq.extend(std::iter::repeat_n(c(0.01, 0.0), 2000));
        let sig =
            crate::decoders::burst::extract(&iq, 0, 100, 250_000, 433_920_000, Some("ASK"), e2.ts).unwrap();
        save_burst(&db, e2.id, &sig).await.unwrap();
        let (meta, iq) = get_burst(&db, e2.id).await.unwrap().unwrap();
        assert_eq!(iq.len(), sig.iq_samples * 2);
        assert!(meta.contains("g001_433.92M_250k.cs8"));
        let evs = device_events(&db, d1.id, None, None, None, 10).await.unwrap();
        assert_eq!(evs.iter().filter(|e| e.burst).count(), 1);
        assert!(get_event(&db, e2.id).await.unwrap().unwrap().0.burst);

        assert!(delete_device(&db, d1.id).await.unwrap());
        assert!(get_burst(&db, e2.id).await.unwrap().is_none()); // cascaded
        for d in [t1.id, t2.id] {
            delete_device(&db, d).await.unwrap();
        }
        assert!(recent_events(&db, None, 10).await.unwrap().is_empty()); // cascaded

        // Positions survive later packets without one.
        let aprs = |v: Value| DecodedEvent::new("aprs", Some(144_800_000), v.as_object().unwrap().clone());
        let (s1, _) = store(&db, &aprs(json!({"model":"APRS","id":"M0ABC-9","lat":51.5,"lon":-0.1,"symbol":"/>"})))
            .await
            .unwrap();
        assert_eq!(s1.lat, Some(51.5));
        let (s2, _) = store(&db, &aprs(json!({"model":"APRS","id":"M0ABC-9","status":"QRV"}))).await.unwrap();
        assert_eq!((s2.lat, s2.lon, s2.symbol.as_deref()), (Some(51.5), Some(-0.1), Some("/>")));
        assert_eq!(list_devices(&db, Some("aprs"), 10).await.unwrap().len(), 1);
        assert_eq!(list_devices(&db, Some("rtl_433"), 10).await.unwrap().len(), 0);
        assert_eq!(tracks(&db, "aprs", 0).await.unwrap().len(), 1);
        assert_eq!(recent_events(&db, Some("aprs"), 10).await.unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sensor repeats its packet; if the stored copy had no signal, an
    /// armed device's burst is taken from a repeat and saved with it.
    #[tokio::test]
    async fn logs_burst_from_a_repeat() {
        let dir = std::env::temp_dir().join(format!("sdrd-burst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let db = db::open(&path).await.unwrap();

        let c = num_complex::Complex32::new;
        let mut iq = vec![c(0.01, 0.0); 100];
        iq.extend(std::iter::repeat_n(c(1.0, 0.0), 500));
        iq.extend(std::iter::repeat_n(c(0.01, 0.0), 2000));
        let mut sig = crate::decoders::burst::extract(&iq, 0, 100, 250_000, 433_920_000, Some("ASK"), 0).unwrap();
        // Another transmitter's burst (not decoded as this device) isn't logged.
        let other = Arc::new(sig.clone());
        sig.verified = true;
        let sig = Arc::new(sig);

        let raw = RawStore::default();
        let (feed, mut sub) = broadcast::channel(16);
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(run(db.clone(), rx, feed, None, raw.clone()));
        let pkt = json!({"model":"Nexus-TH","id":90,"channel":1,"temperature_C":21.3});
        raw.arm(&identity(&ev(pkt.clone())).0, true);

        tx.send(ev(pkt.clone())).await.unwrap(); // stored, no signal
        let mut wrong = ev(pkt.clone());
        wrong.signal = Some(other);
        tx.send(wrong).await.unwrap(); // duplicate, burst isn't this device's
        let mut repeat = ev(pkt.clone());
        repeat.signal = Some(sig.clone());
        tx.send(repeat).await.unwrap(); // duplicate, carries the burst
        drop(tx);
        task.await.unwrap();

        let first: Value = serde_json::from_str(&sub.recv().await.unwrap().json).unwrap();
        let again: Value = serde_json::from_str(&sub.recv().await.unwrap().json).unwrap();
        assert_eq!(first["event"]["burst"], false);
        assert_eq!(again["event"]["burst"], true);
        assert_eq!(first["event"]["id"], again["event"]["id"]);
        let id = again["event"]["id"].as_i64().unwrap();
        assert!(get_burst(&db, id).await.unwrap().is_some());
        assert!(!raw.is_armed("rtl_433:Nexus-TH:90:1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
