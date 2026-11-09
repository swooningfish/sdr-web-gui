//! ADS-B: aircraft from Mode S replies and extended squitters on 1090 MHz.
//!
//! IQ comes from a second dongle kept on 1090 MHz (the ADS-B receiver), or,
//! when there isn't one, from the main receiver whenever it is tuned there
//! (e.g. an "ADS-B" rotation profile: aircraft then drop off between
//! visits). Each source has its own demodulator thread; both feed one
//! [`tracker::Tracker`].

use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tokio::sync::{broadcast, watch};

use crate::db;
use crate::mqtt::MqttHandle;
use crate::radio::{IqBlock, RadioHandle, RadioSettings, RadioState};
use crate::scheduler::SchedulerHandle;

pub mod acdb;
pub mod cpr;
pub mod crc;
pub mod demod;
pub mod encode;
pub mod message;
pub mod tracker;

use tracker::{RangeStats, Sighting, Tracker};

pub const ADSB_HZ: u64 = 1_090_000_000;
/// The ADS-B receiver's sample rate.
pub const ADSB_RATE: u32 = 2_400_000;
/// Settings keys: chosen receiver, its gain/ppm/bias-T, decoder config, range.
pub const DEVICE_KEY: &str = "adsb.device";
pub const RADIO_KEY: &str = "adsb.radio";
const CFG_KEY: &str = "adsb";
const RANGE_KEY: &str = "adsb.range";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdsbConfig {
    /// Decode at all (it costs some CPU while there's 1090 MHz IQ).
    pub enabled: bool,
}

impl Default for AdsbConfig {
    fn default() -> Self {
        AdsbConfig { enabled: true }
    }
}

/// Gain, ppm and bias-T of the ADS-B receiver, as saved.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReceiverSettings {
    pub gain_db: Option<f32>,
    pub ppm: i32,
    pub bias_tee: bool,
}

impl Default for ReceiverSettings {
    fn default() -> Self {
        // Weak signals: maximum gain suits ADS-B better than the tuner's AGC.
        ReceiverSettings { gain_db: Some(49.6), ppm: 0, bias_tee: false }
    }
}

impl ReceiverSettings {
    pub fn radio(&self) -> RadioSettings {
        RadioSettings {
            center_hz: ADSB_HZ,
            sample_rate: ADSB_RATE,
            gain_db: self.gain_db,
            ppm: self.ppm,
            bias_tee: self.bias_tee,
            direct_sampling: 0,
        }
    }
}

/// Whether a block from the main receiver covers 1090 MHz well enough.
pub fn main_in_band(center_hz: u64, rate: u32) -> bool {
    rate >= 2_000_000 && (center_hz as f64 - ADSB_HZ as f64).abs() <= rate as f64 / 2.0 - 900e3
}

#[derive(Clone, Debug, Serialize)]
pub struct AdsbStatus {
    pub enabled: bool,
    /// Where IQ is coming from: "receiver" (second dongle), "main" (the main
    /// receiver tuned to 1090 MHz), or "none".
    pub source: &'static str,
    pub receiver: RadioState,
    pub msg_rate: f32,
    pub messages: u64,
    pub aircraft: usize,
    pub with_position: usize,
    pub home: Option<(f64, f64)>,
    pub max_range_km: Option<f64>,
    pub db: acdb::DbStatus,
}

#[derive(Clone)]
pub struct AdsbHandle {
    pool: SqlitePool,
    tracker: Arc<Mutex<Tracker>>,
    cfg_tx: Arc<watch::Sender<AdsbConfig>>,
    /// The ADS-B receiver (second dongle); "off" when there isn't one.
    pub radio: RadioHandle,
    main: RadioHandle,
    /// When we last decoded IQ from the main receiver / the ADS-B receiver.
    last_main: Arc<AtomicI64>,
    last_own: Arc<AtomicI64>,
}

impl AdsbHandle {
    pub fn config(&self) -> AdsbConfig {
        self.cfg_tx.borrow().clone()
    }

    pub async fn set_config(&self, cfg: AdsbConfig) -> anyhow::Result<()> {
        db::set_setting(&self.pool, CFG_KEY, &cfg).await?;
        self.cfg_tx.send_replace(cfg);
        Ok(())
    }

    pub async fn status(&self) -> AdsbStatus {
        let now = db::now_ms();
        let receiver = self.radio.state();
        let source = if receiver.connected && now - self.last_own.load(Ordering::Relaxed) < 3000 {
            "receiver"
        } else if now - self.last_main.load(Ordering::Relaxed) < 3000 {
            "main"
        } else {
            "none"
        };
        let (msg_rate, messages, aircraft, with_position, home, max_range_km) = {
            let t = self.tracker.lock().unwrap();
            let live: Vec<_> = t.aircraft.values().filter(|a| a.reliable && now - a.seen_ms < 60_000).collect();
            (
                t.msg_rate(now),
                t.total,
                live.len(),
                live.iter().filter(|a| a.lat.is_some()).count(),
                t.home,
                live.iter().filter_map(|a| a.distance_km).reduce(f64::max),
            )
        };
        AdsbStatus {
            enabled: self.config().enabled,
            source,
            receiver,
            msg_rate,
            messages,
            aircraft,
            with_position,
            home,
            max_range_km,
            db: acdb::status(&self.pool).await,
        }
    }

    /// Aircraft heard recently, each with the trail points newer than `since`.
    pub fn aircraft(&self, since: i64) -> Value {
        let now = db::now_ms();
        let t = self.tracker.lock().unwrap();
        let list: Vec<Value> = t
            .aircraft
            .values()
            .filter(|a| a.reliable)
            .map(|a| {
                let mut v = serde_json::to_value(a).unwrap_or(Value::Null);
                if let Value::Object(m) = &mut v {
                    m.insert("trail".into(), json!(a.trail_since(since)));
                }
                v
            })
            .collect();
        json!({ "now": now, "aircraft": list })
    }

    pub fn range(&self) -> Value {
        let now = db::now_ms();
        let t = self.tracker.lock().unwrap();
        json!({
            "sectors": tracker::SECTORS,
            "since_ms": t.range.since_ms,
            "all_time": t.range.all_time,
            "last_24h": t.range.last_24h(now),
            "home": t.home,
        })
    }

    pub async fn reset_range(&self) -> anyhow::Result<()> {
        let r = {
            let mut t = self.tracker.lock().unwrap();
            t.range = RangeStats::new(db::now_ms());
            t.range.clone()
        };
        db::set_setting(&self.pool, RANGE_KEY, &r).await
    }

    /// Aircraft logged in the last 24 hours (gone ones plus those still here).
    pub async fn seen_24h(&self) -> u64 {
        let since = db::now_ms() - 86_400_000;
        let logged: Vec<(i64,)> = sqlx::query_as("SELECT DISTINCT icao FROM adsb_sightings WHERE last_ms >= ?")
            .bind(since)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();
        let mut set: HashSet<u32> = logged.into_iter().map(|r| r.0 as u32).collect();
        let t = self.tracker.lock().unwrap();
        set.extend(t.aircraft.values().filter(|a| a.reliable).map(|a| a.icao));
        set.len() as u64
    }

    /// Look every aircraft up again (after the database was downloaded).
    pub fn refresh_db(&self) {
        let mut t = self.tracker.lock().unwrap();
        let all: Vec<u32> = t.aircraft.keys().copied().collect();
        t.lookups.extend(all);
    }

    pub fn main_radio(&self) -> &RadioHandle {
        &self.main
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn spawn(
    pool: &SqlitePool,
    radio: RadioHandle,
    main: RadioHandle,
    own_iq: broadcast::Receiver<Arc<IqBlock>>,
    main_iq: broadcast::Receiver<Arc<IqBlock>>,
    schedule: SchedulerHandle,
    mqtt: Option<MqttHandle>,
) -> anyhow::Result<AdsbHandle> {
    let cfg = db::get_setting::<AdsbConfig>(pool, CFG_KEY).await.ok().flatten().unwrap_or_default();
    let now = db::now_ms();
    let mut t = Tracker::new(now);
    if let Ok(Some(r)) = db::get_setting::<RangeStats>(pool, RANGE_KEY).await {
        if r.valid() {
            t.range = r;
        }
    }
    acdb::clear_stale(pool).await;
    let tracker = Arc::new(Mutex::new(t));
    let (cfg_tx, cfg_rx) = watch::channel(cfg);
    let last_main = Arc::new(AtomicI64::new(0));
    let last_own = Arc::new(AtomicI64::new(0));

    {
        let (tracker, cfg_rx, last) = (tracker.clone(), cfg_rx.clone(), last_own.clone());
        std::thread::Builder::new()
            .name("adsb-receiver".into())
            .spawn(move || feeder(own_iq, tracker, cfg_rx, last, |_| true))?;
    }
    {
        let (tracker, cfg_rx, last) = (tracker.clone(), cfg_rx.clone(), last_main.clone());
        let own = radio.clone();
        std::thread::Builder::new().name("adsb-main".into()).spawn(move || {
            // Only while the ADS-B receiver isn't doing the job.
            feeder(main_iq, tracker, cfg_rx, last, move |b| {
                main_in_band(b.center_hz, b.sample_rate) && !own.state().connected
            })
        })?;
    }

    let handle = AdsbHandle {
        pool: pool.clone(),
        tracker: tracker.clone(),
        cfg_tx: Arc::new(cfg_tx),
        radio,
        main,
        last_main,
        last_own,
    };
    tokio::spawn(housekeeping(handle.clone(), schedule, mqtt));
    Ok(handle)
}

/// Demodulate one IQ stream into the tracker.
fn feeder(
    mut rx: broadcast::Receiver<Arc<IqBlock>>,
    tracker: Arc<Mutex<Tracker>>,
    cfg_rx: watch::Receiver<AdsbConfig>,
    last: Arc<AtomicI64>,
    wanted: impl Fn(&IqBlock) -> bool,
) {
    let mut demod: Option<demod::Demod> = None;
    let mut frames = Vec::new();
    let mut known = HashSet::new();
    let mut known_at = 0i64;
    loop {
        let block = match rx.blocking_recv() {
            Ok(b) => b,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!("ADS-B demodulator lagged {n} blocks");
                demod = None;
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };
        if !cfg_rx.borrow().enabled || !wanted(&block) {
            demod = None;
            continue;
        }
        let now = db::now_ms();
        if demod.as_ref().is_none_or(|d| d.rate() != block.sample_rate) {
            demod = Some(demod::Demod::new(block.sample_rate));
        }
        if now - known_at > 1000 {
            known = tracker.lock().unwrap().known(now);
            known_at = now;
        }
        frames.clear();
        demod.as_mut().unwrap().feed(&block.data, &known, &mut frames);
        last.store(now, Ordering::Relaxed);
        if !frames.is_empty() {
            let mut t = tracker.lock().unwrap();
            for f in &frames {
                t.ingest(f, now);
            }
        }
    }
}

/// Once a second: station location, database lookups, expiry and the
/// sightings log; every few minutes, range stats and MQTT.
async fn housekeeping(h: AdsbHandle, schedule: SchedulerHandle, mqtt: Option<MqttHandle>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut n: u64 = 0;
    let mut announced = false;
    loop {
        tick.tick().await;
        n += 1;
        let now = db::now_ms();
        let home = schedule.status().location.map(|o| (o.lat_deg, o.lon_deg));
        let (lookups, gone) = {
            let mut t = h.tracker.lock().unwrap();
            t.home = home;
            (std::mem::take(&mut t.lookups), t.expire(now))
        };
        for icao in lookups {
            if let Some(info) = acdb::lookup(&h.pool, icao).await {
                h.tracker.lock().unwrap().set_db(icao, info);
            }
        }
        for s in gone {
            if let Err(e) = log_sighting(&h.pool, &s).await {
                tracing::warn!("logging aircraft sighting: {e}");
            }
        }
        if n.is_multiple_of(300) {
            let r = h.tracker.lock().unwrap().range.clone();
            if let Err(e) = db::set_setting(&h.pool, RANGE_KEY, &r).await {
                tracing::warn!("saving ADS-B range: {e}");
            }
        }
        if let Some(m) = &mqtt {
            let st = h.status().await;
            // Only once there's something to report, and again now and then
            // in case Home Assistant restarted.
            if st.source != "none" || st.aircraft > 0 {
                let first = !announced;
                if first || n.is_multiple_of(600) {
                    m.announce_adsb();
                    announced = true;
                }
                if first || n.is_multiple_of(30) {
                    m.publish_adsb(adsb_state(&h, &st, now));
                }
            }
        }
    }
}

/// MQTT state: counts and the closest aircraft.
fn adsb_state(h: &AdsbHandle, st: &AdsbStatus, now: i64) -> Value {
    let t = h.tracker.lock().unwrap();
    let closest = t
        .aircraft
        .values()
        .filter(|a| a.reliable && now - a.seen_ms < 60_000 && a.distance_km.is_some())
        .min_by(|a, b| a.distance_km.unwrap().total_cmp(&b.distance_km.unwrap()));
    json!({
        "aircraft": st.aircraft,
        "with_position": st.with_position,
        "messages_per_s": (st.msg_rate as f64 * 10.0).round() / 10.0,
        "max_range_km": st.max_range_km.map(|k| (k * 10.0).round() / 10.0),
        "source": st.source,
        "closest": closest.map(|a| a.callsign.clone().or(a.db.reg.clone()).unwrap_or_else(|| format!("{:06x}", a.icao))),
        "closest_icao": closest.map(|a| format!("{:06x}", a.icao)),
        "closest_km": closest.and_then(|a| a.distance_km).map(|k| (k * 10.0).round() / 10.0),
        "closest_alt_ft": closest.and_then(|a| a.alt()),
        "closest_type": closest.and_then(|a| a.db.type_code.clone()),
    })
}

async fn log_sighting(pool: &SqlitePool, s: &Sighting) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO adsb_sightings (icao, callsign, reg, type, category, first_ms, last_ms, msgs, max_km, min_alt, max_alt) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(s.icao as i64)
    .bind(&s.callsign)
    .bind(&s.reg)
    .bind(&s.type_code)
    .bind(&s.category)
    .bind(s.first_ms)
    .bind(s.last_ms)
    .bind(s.msgs as i64)
    .bind(s.max_km)
    .bind(s.min_alt)
    .bind(s.max_alt)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove sightings older than `days`.
pub async fn purge(pool: &SqlitePool, days: i64) -> anyhow::Result<u64> {
    let cutoff = db::now_ms() - days * 86_400_000;
    Ok(sqlx::query("DELETE FROM adsb_sightings WHERE last_ms < ?").bind(cutoff).execute(pool).await?.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_band() {
        assert!(main_in_band(1_090_000_000, 2_400_000));
        assert!(main_in_band(1_090_200_000, 2_400_000));
        assert!(!main_in_band(1_090_500_000, 2_400_000));
        assert!(main_in_band(1_090_000_000, 2_048_000));
        assert!(!main_in_band(1_090_000_000, 1_024_000));
        assert!(!main_in_band(145_000_000, 2_400_000));
    }
}
