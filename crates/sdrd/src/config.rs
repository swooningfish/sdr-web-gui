//! Runtime configuration, read from `SDRD_*` environment variables.

use std::{env, net::SocketAddr, path::PathBuf, str::FromStr};

use anyhow::{anyhow, bail, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A real RTL-SDR dongle via librtlsdr.
    Rtlsdr,
    /// Generated test signals (no hardware needed).
    Synthetic,
    /// Loop a recorded cu8 IQ file.
    Replay,
}

impl SourceKind {
    pub fn name(self) -> &'static str {
        match self {
            SourceKind::Rtlsdr => "rtlsdr",
            SourceKind::Synthetic => "synthetic",
            SourceKind::Replay => "replay",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub web_dir: PathBuf,

    pub source: SourceKind,
    #[cfg_attr(not(feature = "rtlsdr"), allow(dead_code))]
    pub device_index: u32,
    #[cfg_attr(not(feature = "rtlsdr"), allow(dead_code))]
    pub device_serial: Option<String>,
    pub replay_file: Option<PathBuf>,

    pub center_hz: u64,
    pub sample_rate: u32,
    /// `None` = tuner AGC.
    pub gain_db: Option<f32>,
    pub ppm: i32,
    pub bias_tee: bool,

    pub fft_size: usize,
    pub fps: u32,

    pub admin_user: String,
    pub admin_password: Option<String>,
    pub cookie_secure: bool,
    /// Trust `X-Forwarded-For` / `X-Forwarded-Host` from the reverse proxy.
    pub trust_proxy: bool,
    pub session_hours: i64,

    /// Path to the rtl_433 binary.
    pub rtl433_path: String,
    /// Days to keep decoded events (and untracked devices).
    pub event_retention_days: i64,
    /// Let viewer accounts see ISM/TPMS devices (other people's data).
    pub ism_viewers: bool,
    pub mqtt: Option<MqttConfig>,
}

#[derive(Clone, Debug)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub password: Option<String>,
    pub client_id: String,
    /// Topic prefix for our own topics.
    pub prefix: String,
    /// Home Assistant discovery prefix.
    pub discovery_prefix: String,
    /// Publish every decoded event to `<prefix>/events/<decoder>`.
    pub events: bool,
}

fn var(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse<T: FromStr>(name: &str, default: T) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    match var(name) {
        None => Ok(default),
        Some(v) => v.parse::<T>().map_err(|e| anyhow!("{name}={v:?}: {e}")),
    }
}

fn parse_bool(name: &str, default: bool) -> Result<bool> {
    match var(name).map(|v| v.to_ascii_lowercase()) {
        None => Ok(default),
        Some(v) => match v.as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => bail!("{name}={v:?}: expected true/false"),
        },
    }
}

fn topic_prefix(name: &str, default: &str) -> Result<String> {
    let v = var(name).unwrap_or_else(|| default.into());
    let v = v.trim_matches('/').to_string();
    if v.is_empty() || v.contains(['+', '#']) {
        bail!("{name}={v:?}: must be a plain MQTT topic prefix");
    }
    Ok(v)
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let default_source = if cfg!(feature = "rtlsdr") {
            SourceKind::Rtlsdr
        } else {
            SourceKind::Synthetic
        };
        let source = match var("SDRD_SOURCE").map(|v| v.to_ascii_lowercase()) {
            None => default_source,
            Some(v) => match v.as_str() {
                "rtlsdr" => SourceKind::Rtlsdr,
                "synthetic" => SourceKind::Synthetic,
                "replay" => SourceKind::Replay,
                _ => bail!("SDRD_SOURCE={v:?}: expected rtlsdr, synthetic or replay"),
            },
        };
        if source == SourceKind::Rtlsdr && !cfg!(feature = "rtlsdr") {
            bail!("SDRD_SOURCE=rtlsdr but sdrd was built without the `rtlsdr` feature");
        }

        let gain_db = match var("SDRD_GAIN").map(|v| v.to_ascii_lowercase()) {
            None => None,
            Some(v) if v == "auto" => None,
            Some(v) => Some(
                v.parse::<f32>()
                    .map_err(|e| anyhow!("SDRD_GAIN={v:?}: {e}"))?,
            ),
        };

        let fft_size: usize = parse("SDRD_FFT_SIZE", 2048)?;
        if !fft_size.is_power_of_two() || !(256..=16384).contains(&fft_size) {
            bail!("SDRD_FFT_SIZE must be a power of two between 256 and 16384");
        }
        let fps: u32 = parse("SDRD_FPS", 20)?;
        if !(1..=60).contains(&fps) {
            bail!("SDRD_FPS must be between 1 and 60");
        }

        let cfg = Config {
            bind: parse("SDRD_BIND", "0.0.0.0:8080".parse().unwrap())?,
            data_dir: PathBuf::from(var("SDRD_DATA_DIR").unwrap_or_else(|| "./data".into())),
            web_dir: PathBuf::from(var("SDRD_WEB_DIR").unwrap_or_else(|| "./web/dist".into())),
            source,
            device_index: parse("SDRD_DEVICE_INDEX", 0)?,
            device_serial: var("SDRD_DEVICE_SERIAL"),
            replay_file: var("SDRD_REPLAY_FILE").map(PathBuf::from),
            center_hz: parse("SDRD_CENTER_HZ", 145_300_000)?,
            sample_rate: parse("SDRD_SAMPLE_RATE", 2_048_000)?,
            gain_db,
            ppm: parse("SDRD_PPM", 0)?,
            bias_tee: parse_bool("SDRD_BIAS_TEE", false)?,
            fft_size,
            fps,
            admin_user: var("SDRD_ADMIN_USER").unwrap_or_else(|| "admin".into()),
            admin_password: var("SDRD_ADMIN_PASSWORD"),
            cookie_secure: parse_bool("SDRD_COOKIE_SECURE", true)?,
            trust_proxy: parse_bool("SDRD_TRUST_PROXY", false)?,
            session_hours: parse("SDRD_SESSION_HOURS", 24 * 7)?,
            rtl433_path: var("SDRD_RTL433_PATH").unwrap_or_else(|| "rtl_433".into()),
            event_retention_days: parse("SDRD_EVENT_RETENTION_DAYS", 30)?,
            ism_viewers: parse_bool("SDRD_ISM_VIEWERS", false)?,
            mqtt: match var("SDRD_MQTT_HOST") {
                None => None,
                Some(host) => Some(MqttConfig {
                    host,
                    port: parse("SDRD_MQTT_PORT", 1883)?,
                    user: var("SDRD_MQTT_USER"),
                    password: var("SDRD_MQTT_PASSWORD"),
                    client_id: var("SDRD_MQTT_CLIENT_ID").unwrap_or_else(|| "sdrd".into()),
                    prefix: topic_prefix("SDRD_MQTT_PREFIX", "sdrd")?,
                    discovery_prefix: topic_prefix("SDRD_MQTT_DISCOVERY_PREFIX", "homeassistant")?,
                    events: parse_bool("SDRD_MQTT_EVENTS", true)?,
                }),
            },
        };
        if !(1..=3650).contains(&cfg.event_retention_days) {
            bail!("SDRD_EVENT_RETENTION_DAYS must be between 1 and 3650");
        }
        if cfg.source == SourceKind::Replay && cfg.replay_file.is_none() {
            bail!("SDRD_SOURCE=replay requires SDRD_REPLAY_FILE (cu8 IQ)");
        }
        Ok(cfg)
    }
}
