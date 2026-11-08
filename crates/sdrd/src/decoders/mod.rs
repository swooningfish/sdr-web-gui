//! External and built-in decoders. Each one turns IQ into `DecodedEvent`s,
//! which `events` stores, streams to the browser and publishes to MQTT.

use serde_json::{Map, Value};

pub mod aprs;
pub mod rtl433;

#[derive(Debug, Clone)]
pub struct DecodedEvent {
    pub decoder: &'static str,
    /// Milliseconds since the Unix epoch, taken when we received it.
    pub ts_ms: i64,
    pub freq_hz: Option<u64>,
    pub data: Map<String, Value>,
}

impl DecodedEvent {
    pub fn new(decoder: &'static str, freq_hz: Option<u64>, mut data: Map<String, Value>) -> Self {
        // rtl_433 reports sample-offset times ("@1.23s") for piped input; we
        // timestamp events ourselves instead.
        data.remove("time");
        DecodedEvent {
            decoder,
            ts_ms: crate::db::now_ms(),
            freq_hz,
            data,
        }
    }
}
