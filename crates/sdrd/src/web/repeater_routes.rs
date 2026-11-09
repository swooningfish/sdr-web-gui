//! Local repeaters from RepeaterBook, for the bookmarks list.
//!
//! RepeaterBook only serves approved clients: the admin pastes their own
//! API token (and a contact email for the User-Agent, which RepeaterBook
//! requires). We fetch the country export at most once a day, keep only the
//! repeaters within the chosen radius of the station, and serve those.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::SqlitePool;

use super::{AdminUser, ApiError, AppState, AuthUser};
use crate::db;
use crate::orbit::Observer;

const CFG_KEY: &str = "repeaters.config";
const CACHE_KEY: &str = "repeaters.cache";
const MAX_AGE_MS: i64 = 24 * 3_600_000;
/// Don't ask RepeaterBook more often than this, even by hand.
const MIN_GAP_MS: i64 = 60_000;
/// After an error, wait this long before trying again on our own.
const ERROR_BACKOFF_MS: i64 = 6 * 3_600_000;

static FETCHING: AtomicBool = AtomicBool::new(false);
static LAST_TRY_MS: AtomicI64 = AtomicI64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepeaterConfig {
    pub enabled: bool,
    /// RepeaterBook API token (never sent back to browsers).
    #[serde(default)]
    pub token: String,
    /// Contact email for the User-Agent, as RepeaterBook asks.
    #[serde(default)]
    pub email: String,
    pub country: String,
    pub radius_miles: f64,
}

impl Default for RepeaterConfig {
    fn default() -> Self {
        RepeaterConfig {
            enabled: false,
            token: String::new(),
            email: String::new(),
            country: "United Kingdom".into(),
            radius_miles: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Repeater {
    pub callsign: String,
    pub freq_hz: u64,
    pub input_hz: Option<u64>,
    /// CTCSS uplink tone, e.g. "77.0".
    pub tone: Option<String>,
    pub city: String,
    /// "FM", "DMR", "D-Star", "Fusion", …
    pub mode: String,
    pub status: Option<String>,
    pub distance_mi: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Cache {
    fetched_ms: i64,
    /// What the list was fetched for, so a change triggers a new fetch.
    key: String,
    error: Option<String>,
    repeaters: Vec<Repeater>,
}

async fn load_cfg(db: &SqlitePool) -> RepeaterConfig {
    db::get_setting(db, CFG_KEY).await.ok().flatten().unwrap_or_default()
}

async fn load_cache(db: &SqlitePool) -> Cache {
    db::get_setting(db, CACHE_KEY).await.ok().flatten().unwrap_or_default()
}

fn cache_key(cfg: &RepeaterConfig, loc: &Observer) -> String {
    format!("{}|{:.3}|{:.3}|{}", cfg.country, loc.lat_deg, loc.lon_deg, cfg.radius_miles)
}

fn public_cfg(cfg: &RepeaterConfig) -> Value {
    json!({
        "enabled": cfg.enabled,
        "has_token": !cfg.token.is_empty(),
        "email": cfg.email,
        "country": cfg.country,
        "radius_miles": cfg.radius_miles,
    })
}

/// Great-circle distance in miles.
fn miles(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 3958.8 * h.sqrt().asin()
}

/// A field that may be a JSON string or number.
fn text(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn num(v: &Value, k: &str) -> Option<f64> {
    text(v, k)?.parse().ok()
}

fn mhz(v: &Value, k: &str) -> Option<u64> {
    num(v, k).filter(|f| *f > 0.0).map(|f| (f * 1e6).round() as u64)
}

fn yes(v: &Value, k: &str) -> bool {
    text(v, k).is_some_and(|s| s.eq_ignore_ascii_case("yes") || s == "1")
}

/// Keep RepeaterBook rows within `radius` miles of `home`, nearest first.
fn parse(body: &str, home: (f64, f64), radius: f64) -> Result<Vec<Repeater>, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "RepeaterBook sent something that isn't JSON".to_string())?;
    if let Some(e) = v.get("error_code").or_else(|| v.get("error")).and_then(Value::as_str) {
        return Err(format!("RepeaterBook: {e}"));
    }
    let rows = v.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out: Vec<Repeater> = rows
        .iter()
        .filter_map(|r| {
            let lat = num(r, "Lat")?;
            let lon = num(r, "Long").or_else(|| num(r, "Lng"))?;
            let d = miles(home, (lat, lon));
            if d > radius {
                return None;
            }
            let mode = if yes(r, "FM Analog") || yes(r, "Analog Capable") {
                "FM"
            } else if yes(r, "DMR") {
                "DMR"
            } else if yes(r, "D-Star") {
                "D-Star"
            } else if yes(r, "System Fusion") {
                "Fusion"
            } else if yes(r, "NXDN") {
                "NXDN"
            } else if yes(r, "M17") {
                "M17"
            } else if yes(r, "APCO P-25") || yes(r, "P-25") {
                "P25"
            } else if yes(r, "Tetra") {
                "TETRA"
            } else {
                "FM"
            };
            Some(Repeater {
                callsign: text(r, "Callsign")?,
                freq_hz: mhz(r, "Frequency")?,
                input_hz: mhz(r, "Input Freq"),
                tone: text(r, "PL").filter(|t| t != "0" && !t.eq_ignore_ascii_case("csq")),
                city: text(r, "Nearest City").or_else(|| text(r, "Landmark")).unwrap_or_default(),
                mode: mode.into(),
                status: text(r, "Operational Status"),
                distance_mi: (d * 10.0).round() / 10.0,
            })
        })
        .collect();
    out.sort_by(|a, b| a.distance_mi.total_cmp(&b.distance_mi));
    out.truncate(200);
    Ok(out)
}

fn fetch(cfg: &RepeaterConfig) -> Result<String, String> {
    let na = ["united states", "canada", "mexico"].contains(&cfg.country.to_lowercase().as_str());
    let url = if na {
        "https://www.repeaterbook.com/api/export.php"
    } else {
        "https://www.repeaterbook.com/api/exportROW.php"
    };
    // RepeaterBook: no redirects with credentials; a descriptive User-Agent
    // with a contact address.
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(30)).build();
    let ua = format!("sdrd/{} (SDR monitor; {})", env!("CARGO_PKG_VERSION"), cfg.email);
    match agent
        .get(url)
        .query("country", &cfg.country)
        .set("X-RB-App-Token", &cfg.token)
        .set("User-Agent", &ua)
        .set("Accept", "application/json")
        .call()
    {
        Ok(r) if r.status() >= 300 => Err(format!("RepeaterBook answered {} (redirect not followed)", r.status())),
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            let retry = r.header("Retry-After").map(str::to_string);
            let body = r.into_string().unwrap_or_default();
            let why = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| text(&v, "error_code").or_else(|| text(&v, "error")).or_else(|| text(&v, "message")))
                .unwrap_or_else(|| format!("HTTP {code}"));
            Err(match (code, retry) {
                (429, Some(s)) => format!("RepeaterBook rate limit, retry after {s} s"),
                _ => format!("RepeaterBook refused the request: {why}"),
            })
        }
        Err(e) => Err(format!("couldn't reach RepeaterBook: {e}")),
    }
}

/// Fetch and store a fresh list (one at a time, never more than once a minute).
async fn refresh(db: SqlitePool, cfg: RepeaterConfig, loc: Observer) {
    let now = db::now_ms();
    if now - LAST_TRY_MS.load(Ordering::Relaxed) < MIN_GAP_MS || FETCHING.swap(true, Ordering::AcqRel) {
        return;
    }
    LAST_TRY_MS.store(now, Ordering::Relaxed);
    let key = cache_key(&cfg, &loc);
    let home = (loc.lat_deg, loc.lon_deg);
    let radius = cfg.radius_miles;
    let res = tokio::task::spawn_blocking(move || fetch(&cfg).and_then(|b| parse(&b, home, radius)))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    let mut cache = load_cache(&db).await;
    match res {
        Ok(list) => {
            tracing::info!(count = list.len(), "repeaters fetched from RepeaterBook");
            cache = Cache { fetched_ms: now, key, error: None, repeaters: list };
        }
        Err(e) => {
            tracing::warn!("repeaters: {e}");
            cache.error = Some(e);
            // Back off: count the failure as a fetch, but keep the old list.
            cache.fetched_ms = now - MAX_AGE_MS + ERROR_BACKOFF_MS;
            if cache.key != key {
                cache.repeaters.clear();
                cache.key = key;
            }
        }
    }
    if let Err(e) = db::set_setting(&db, CACHE_KEY, &cache).await {
        tracing::warn!("repeaters: can't store list: {e}");
    }
    FETCHING.store(false, Ordering::Release);
}

/// Nearby repeaters (any signed-in user). Starts a background refresh when
/// the list is stale or the settings changed.
pub async fn list(State(state): State<AppState>, _user: AuthUser) -> Json<Value> {
    let cfg = load_cfg(&state.db).await;
    let loc = state.schedule.status().location;
    let cache = load_cache(&state.db).await;
    let mut note = None;
    if !cfg.enabled {
        return Json(json!({ "enabled": false, "repeaters": [] }));
    }
    match loc {
        None => note = Some("Set the station location (Settings → Passes) to find nearby repeaters.".to_string()),
        Some(_) if cfg.token.is_empty() => note = Some("Add a RepeaterBook API token in Settings → Integrations.".to_string()),
        Some(l) => {
            let stale = cache.key != cache_key(&cfg, &l) || db::now_ms() - cache.fetched_ms > MAX_AGE_MS;
            if stale {
                tokio::spawn(refresh(state.db.clone(), cfg.clone(), l));
            }
        }
    }
    let current = loc.is_some_and(|l| cache.key == cache_key(&cfg, &l));
    Json(json!({
        "enabled": true,
        "radius_miles": cfg.radius_miles,
        "fetched_ms": current.then_some(cache.fetched_ms).filter(|&t| t > 0 && cache.error.is_none()),
        "fetching": FETCHING.load(Ordering::Relaxed),
        "error": note.or(if current { cache.error.clone() } else { None }),
        "repeaters": if current { cache.repeaters } else { Vec::new() },
    }))
}

pub async fn get_config(State(state): State<AppState>, _admin: AdminUser) -> Json<Value> {
    Json(public_cfg(&load_cfg(&state.db).await))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigReq {
    enabled: bool,
    /// None: keep the stored token; "": forget it.
    token: Option<String>,
    email: String,
    country: String,
    radius_miles: f64,
}

pub async fn set_config(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(r): Json<ConfigReq>,
) -> Result<Json<Value>, ApiError> {
    let mut cfg = load_cfg(&state.db).await;
    if !(0.1..=200.0).contains(&r.radius_miles) {
        return Err(ApiError::bad_request("radius must be 0.1–200 miles"));
    }
    let country = r.country.trim();
    if country.is_empty() || country.len() > 64 || country.chars().any(char::is_control) {
        return Err(ApiError::bad_request("enter a country, e.g. United Kingdom"));
    }
    let email = r.email.trim();
    if email.len() > 120 || email.chars().any(|c| c.is_control() || c == '(' || c == ')') {
        return Err(ApiError::bad_request("that email address isn't valid"));
    }
    if let Some(t) = r.token {
        let t = t.trim();
        if t.len() > 200 || !t.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) {
            return Err(ApiError::bad_request("that doesn't look like a RepeaterBook token"));
        }
        cfg.token = t.to_string();
    }
    if r.enabled && (cfg.token.is_empty() || !email.contains('@')) {
        return Err(ApiError::bad_request(
            "RepeaterBook needs your API token and a contact email before it can be switched on",
        ));
    }
    cfg.enabled = r.enabled;
    cfg.email = email.to_string();
    cfg.country = country.to_string();
    cfg.radius_miles = r.radius_miles;
    db::set_setting(&state.db, CFG_KEY, &cfg).await.map_err(ApiError::internal)?;
    tracing::info!(by = %admin.user.username, enabled = cfg.enabled, "repeater lookup configured");
    Ok(Json(public_cfg(&cfg)))
}

/// Fetch now (admins), e.g. after changing the token.
pub async fn refresh_now(State(state): State<AppState>, _admin: AdminUser) -> Result<Json<Value>, ApiError> {
    let cfg = load_cfg(&state.db).await;
    let Some(loc) = state.schedule.status().location else {
        return Err(ApiError::bad_request("set the station location first"));
    };
    if !cfg.enabled {
        return Err(ApiError::bad_request("switch the repeater lookup on first"));
    }
    if db::now_ms() - LAST_TRY_MS.load(Ordering::Relaxed) < MIN_GAP_MS {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "fetched less than a minute ago"));
    }
    refresh(state.db.clone(), cfg, loc).await;
    let cache = load_cache(&state.db).await;
    match cache.error {
        Some(e) => Err(ApiError::new(StatusCode::BAD_GATEWAY, e)),
        None => Ok(Json(json!({ "count": cache.repeaters.len() }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_filters_by_distance() {
        let body = r#"{"count":3,"results":[
            {"Callsign":"GB3AA","Frequency":"145.6625","Input Freq":"145.0625","PL":"77.0","Lat":"51.50","Long":"-0.12","Nearest City":"London","FM Analog":"Yes","Operational Status":"On-air"},
            {"Callsign":"GB7BB","Frequency":439.5,"Input Freq":430.5,"PL":"","Lat":51.51,"Long":-0.13,"Nearest City":"London","FM Analog":"No","DMR":"Yes"},
            {"Callsign":"GB3FAR","Frequency":"145.7","Lat":"53.0","Long":"-2.0","FM Analog":"Yes"}
        ]}"#;
        let list = parse(body, (51.5, -0.12), 5.0).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].callsign, "GB3AA");
        assert_eq!(list[0].freq_hz, 145_662_500);
        assert_eq!(list[0].input_hz, Some(145_062_500));
        assert_eq!(list[0].tone.as_deref(), Some("77.0"));
        assert_eq!(list[1].mode, "DMR");
        assert!(list[1].tone.is_none());
    }

    #[test]
    fn reports_api_errors() {
        assert!(parse(r#"{"error_code":"auth_missing"}"#, (0.0, 0.0), 1.0).unwrap_err().contains("auth_missing"));
    }

    #[test]
    fn distance() {
        // London → Manchester is about 163 miles.
        let d = miles((51.507, -0.128), (53.48, -2.242));
        assert!((d - 163.0).abs() < 3.0, "{d}");
    }
}
