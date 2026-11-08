//! MQTT output and Home Assistant discovery.
//!
//! Topics (prefix defaults to `sdrd`):
//! - `<prefix>/status`                     `online` / `offline` (retained, last will)
//! - `<prefix>/events/<decoder>`           every decoded event (optional)
//! - `<prefix>/devices/<slug>/state`       latest data of *tracked* devices (retained)
//! - `<discovery>/sensor/sdrd_<slug>_<field>/config`  HA discovery for tracked devices
//!
//! Only devices an admin marks as tracked appear in Home Assistant, so passing
//! cars' TPMS sensors don't each create a new HA device.

use std::sync::Arc;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS};
use serde::Serialize;
use serde_json::{json, Map, Value};
use sqlx::SqlitePool;
use tokio::sync::watch;

use crate::config::MqttConfig;
use crate::decoders::DecodedEvent;
use crate::events::{self, Device};

#[derive(Clone, Debug, Default, Serialize)]
pub struct MqttStatus {
    pub server: String,
    pub connected: bool,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct MqttHandle {
    client: AsyncClient,
    cfg: Arc<MqttConfig>,
    status_rx: watch::Receiver<MqttStatus>,
}

/// One Home Assistant sensor per known rtl_433 field.
struct Field {
    key: &'static str,
    label: &'static str,
    device_class: Option<&'static str>,
    unit: Option<&'static str>,
    template: Option<&'static str>,
    diagnostic: bool,
}

const fn f(key: &'static str, label: &'static str, device_class: Option<&'static str>, unit: Option<&'static str>) -> Field {
    Field { key, label, device_class, unit, template: None, diagnostic: false }
}

const FIELDS: &[Field] = &[
    f("temperature_C", "Temperature", Some("temperature"), Some("°C")),
    f("temperature_F", "Temperature", Some("temperature"), Some("°F")),
    f("humidity", "Humidity", Some("humidity"), Some("%")),
    f("moisture", "Moisture", Some("moisture"), Some("%")),
    f("pressure_kPa", "Pressure", Some("pressure"), Some("kPa")),
    f("pressure_hPa", "Pressure", Some("pressure"), Some("hPa")),
    f("pressure_PSI", "Pressure", Some("pressure"), Some("psi")),
    f("wind_avg_km_h", "Wind speed", Some("wind_speed"), Some("km/h")),
    f("wind_max_km_h", "Wind gust", Some("wind_speed"), Some("km/h")),
    f("wind_avg_m_s", "Wind speed", Some("wind_speed"), Some("m/s")),
    f("wind_max_m_s", "Wind gust", Some("wind_speed"), Some("m/s")),
    f("wind_dir_deg", "Wind direction", None, Some("°")),
    f("rain_mm", "Rain", Some("precipitation"), Some("mm")),
    f("light_lux", "Light", Some("illuminance"), Some("lx")),
    f("uv", "UV index", None, None),
    Field {
        key: "battery_ok",
        label: "Battery",
        device_class: Some("battery"),
        unit: Some("%"),
        template: Some("{{ ((value_json.battery_ok | float(0)) * 100) | round(0) }}"),
        diagnostic: true,
    },
    Field {
        key: "rssi",
        label: "Signal",
        device_class: Some("signal_strength"),
        unit: Some("dB"),
        template: None,
        diagnostic: true,
    },
];

/// Keep the last known position in the state even when this packet had none,
/// so the HA tracker doesn't lose it on a status-only packet.
fn with_position(data: &mut Map<String, Value>, d: &Device) {
    if let (Some(lat), Some(lon), false) = (d.lat, d.lon, data.contains_key("lat")) {
        data.insert("lat".into(), json!(lat));
        data.insert("lon".into(), json!(lon));
    }
}

/// Topic-safe identifier derived from the device key.
pub fn slug(key: &str) -> String {
    let mut s: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    s.trim_matches('_').to_string()
}

impl MqttHandle {
    pub fn status(&self) -> MqttStatus {
        self.status_rx.borrow().clone()
    }

    fn publish(&self, topic: String, retain: bool, payload: Vec<u8>) {
        if let Err(e) = self.client.try_publish(topic, QoS::AtLeastOnce, retain, payload) {
            tracing::debug!("mqtt publish dropped: {e}");
        }
    }

    fn state_topic(&self, d: &Device) -> String {
        format!("{}/devices/{}/state", self.cfg.prefix, slug(&d.key))
    }

    pub fn publish_event(&self, device: &Device, ev: &DecodedEvent) {
        let mut data = ev.data.clone();
        data.insert("device".into(), json!(device.label()));
        data.insert("ts".into(), json!(ev.ts_ms));
        if self.cfg.events {
            let topic = format!("{}/events/{}", self.cfg.prefix, ev.decoder);
            self.publish(topic, false, Value::Object(data.clone()).to_string().into_bytes());
        }
        if device.tracked {
            with_position(&mut data, device);
            self.publish(self.state_topic(device), true, Value::Object(data).to_string().into_bytes());
        }
    }

    /// Publish HA discovery configs (and the latest state) for a tracked device.
    pub fn announce(&self, d: &Device) {
        let data = d.data();
        let s = slug(&d.key);
        let device = json!({
            "identifiers": [format!("sdrd_{s}")],
            "name": d.label(),
            "model": d.model,
            "manufacturer": format!("{} via sdrd", d.decoder),
        });
        for field in FIELDS.iter().filter(|f| data.contains_key(f.key)) {
            let mut c = Map::new();
            c.insert("name".into(), json!(field.label));
            c.insert("unique_id".into(), json!(format!("sdrd_{s}_{}", field.key)));
            c.insert("state_topic".into(), json!(self.state_topic(d)));
            c.insert(
                "value_template".into(),
                json!(field.template.map(String::from).unwrap_or_else(|| format!("{{{{ value_json.{} }}}}", field.key))),
            );
            c.insert("availability_topic".into(), json!(format!("{}/status", self.cfg.prefix)));
            c.insert("state_class".into(), json!("measurement"));
            if let Some(dc) = field.device_class {
                c.insert("device_class".into(), json!(dc));
            }
            if let Some(u) = field.unit {
                c.insert("unit_of_measurement".into(), json!(u));
            }
            if field.diagnostic {
                c.insert("entity_category".into(), json!("diagnostic"));
            }
            c.insert("device".into(), device.clone());
            self.publish(self.discovery_topic(&s, field.key), true, Value::Object(c).to_string().into_bytes());
        }
        // Stations with a position also become an HA device tracker on the map.
        if d.lat.is_some() {
            let c = json!({
                "name": "Location",
                "unique_id": format!("sdrd_{s}_location"),
                "json_attributes_topic": self.state_topic(d),
                "json_attributes_template":
                    "{{ {'latitude': value_json.lat, 'longitude': value_json.lon, 'gps_accuracy': 30} | tojson }}",
                "availability_topic": format!("{}/status", self.cfg.prefix),
                "source_type": "gps",
                "device": device,
            });
            self.publish(self.tracker_topic(&s), true, c.to_string().into_bytes());
        }
        let mut state = data;
        state.insert("device".into(), json!(d.label()));
        with_position(&mut state, d);
        self.publish(self.state_topic(d), true, Value::Object(state).to_string().into_bytes());
    }

    fn tracker_topic(&self, slug: &str) -> String {
        format!("{}/device_tracker/sdrd_{slug}/config", self.cfg.discovery_prefix)
    }

    /// Remove a device from Home Assistant.
    pub fn retract(&self, d: &Device) {
        let s = slug(&d.key);
        for field in FIELDS {
            self.publish(self.discovery_topic(&s, field.key), true, Vec::new());
        }
        self.publish(self.tracker_topic(&s), true, Vec::new());
        self.publish(self.state_topic(d), true, Vec::new());
    }

    fn discovery_topic(&self, slug: &str, field: &str) -> String {
        format!("{}/sensor/sdrd_{slug}_{}/config", self.cfg.discovery_prefix, field.to_ascii_lowercase())
    }

    async fn announce_all(&self, db: &SqlitePool) {
        match events::tracked_devices(db).await {
            Ok(list) => list.iter().for_each(|d| self.announce(d)),
            Err(e) => tracing::warn!("loading tracked devices: {e}"),
        }
    }
}

pub fn spawn(cfg: MqttConfig, db: SqlitePool) -> MqttHandle {
    let cfg = Arc::new(cfg);
    let mut opts = MqttOptions::new(&cfg.client_id, &cfg.host, cfg.port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_last_will(LastWill::new(format!("{}/status", cfg.prefix), "offline", QoS::AtLeastOnce, true));
    if let Some(user) = &cfg.user {
        opts.set_credentials(user, cfg.password.clone().unwrap_or_default());
    }
    let (client, mut eventloop) = AsyncClient::new(opts, 512);
    let (status_tx, status_rx) = watch::channel(MqttStatus {
        server: format!("{}:{}", cfg.host, cfg.port),
        ..Default::default()
    });
    let handle = MqttHandle { client, cfg: cfg.clone(), status_rx };

    let h = handle.clone();
    tokio::spawn(async move {
        let ha_status = format!("{}/status", h.cfg.discovery_prefix);
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    tracing::info!(server = %status_tx.borrow().server, "mqtt connected");
                    status_tx.send_modify(|s| {
                        s.connected = true;
                        s.error = None;
                    });
                    h.publish(format!("{}/status", h.cfg.prefix), true, b"online".to_vec());
                    let _ = h.client.try_subscribe(&ha_status, QoS::AtLeastOnce);
                    h.announce_all(&db).await;
                }
                // Home Assistant restarted: send discovery again.
                Ok(Event::Incoming(Packet::Publish(p))) if p.topic == ha_status && &p.payload[..] == b"online" => {
                    h.announce_all(&db).await;
                }
                Ok(_) => {}
                Err(e) => {
                    let msg = e.to_string();
                    if status_tx.borrow().error.as_deref() != Some(&msg) {
                        tracing::warn!("mqtt: {msg}");
                    }
                    status_tx.send_modify(|s| {
                        s.connected = false;
                        s.error = Some(msg);
                    });
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
    handle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("rtl_433:Nexus-TH:90:1"), "rtl_433_nexus_th_90_1");
        assert_eq!(slug("rtl_433:TPMS::"), "rtl_433_tpms");
    }
}
