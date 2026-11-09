//! HTTP + WebSocket API and static file serving for the SPA.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::auth::{self, RateLimiter, Role, Session};
use crate::config::Config;
use crate::decoders::aprs::AprsHandle;
use crate::decoders::rtl433::Rtl433Handle;
use crate::decoders::sstv::SstvHandle;
use crate::events::EventFeed;
use crate::mqtt::MqttHandle;
use crate::radio::{IqBlock, RadioHandle};
use crate::scheduler::SchedulerHandle;

mod aprs_routes;
mod auth_routes;
mod radio_routes;
mod schedule_routes;
mod sensors;
mod sstv_routes;
mod system_routes;
mod users;
mod ws;

pub use radio_routes::{DEVICE_KEY, TCP_SERVERS_KEY};

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub cfg: Arc<Config>,
    pub radio: RadioHandle,
    pub waterfall_tx: broadcast::Sender<Bytes>,
    pub limiter: Arc<Mutex<RateLimiter>>,
    pub rtl433: Rtl433Handle,
    pub aprs: AprsHandle,
    pub sstv: SstvHandle,
    /// Live SSTV rows (binary frames) for WebSocket clients.
    pub sstv_rows_tx: broadcast::Sender<Bytes>,
    /// Raw IQ, for live-audio listeners to subscribe to.
    pub iq_tx: broadcast::Sender<Arc<IqBlock>>,
    pub audio_listeners: Arc<std::sync::atomic::AtomicUsize>,
    pub schedule: SchedulerHandle,
    pub event_feed: EventFeed,
    pub mqtt: Option<MqttHandle>,
}

/// Content-Security-Policy; map tiles may come from the configured tile server.
fn csp(cfg: &Config) -> String {
    format!(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob: {}; connect-src 'self'; media-src 'self' blob:; worker-src 'self' blob:; \
         frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'",
        tile_origin(&cfg.map_tile_url)
    )
}

/// `https://{s}.tile.example.org/{z}/{x}/{y}.png` → `https://*.tile.example.org`.
fn tile_origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return String::new();
    };
    let host = rest.split('/').next().unwrap_or("").replace("{s}", "*");
    format!("{scheme}://{host}")
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(|| async { Json(json!({ "ok": true })) }))
        .route("/auth/login", post(auth_routes::login))
        .route("/auth/logout", post(auth_routes::logout))
        .route("/auth/me", get(auth_routes::me))
        .route("/auth/password", post(auth_routes::change_password))
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", axum::routing::delete(users::remove))
        .route("/radio", get(radio_routes::get_state).post(radio_routes::configure))
        .route("/radio/devices", get(radio_routes::devices))
        .route("/radio/device", post(radio_routes::select_device))
        .route("/radio/rtl_tcp", axum::routing::delete(radio_routes::forget_tcp_server))
        .route("/devices", get(sensors::list_devices))
        .route(
            "/devices/{id}",
            axum::routing::patch(sensors::update_device).delete(sensors::delete_device),
        )
        .route("/devices/{id}/events", get(sensors::device_events))
        .route("/dashboard", get(sensors::dashboard))
        .route("/dashboard/order", axum::routing::put(sensors::set_dashboard_order))
        .route("/events", get(sensors::recent_events))
        .route("/decoders/rtl_433", get(sensors::get_rtl433).put(sensors::set_rtl433))
        .route("/decoders/rtl_433/check", post(sensors::check_spec))
        .route("/aprs", get(aprs_routes::get_config).put(aprs_routes::set_config))
        .route("/aprs/tracks", get(aprs_routes::tracks))
        .route("/sstv", get(sstv_routes::get_config).put(sstv_routes::set_config))
        .route("/schedule", get(schedule_routes::get).put(schedule_routes::set_config))
        .route("/schedule/rotation", axum::routing::put(schedule_routes::set_rotation))
        .route("/schedule/location", axum::routing::put(schedule_routes::set_location))
        .route("/schedule/tle", axum::routing::put(schedule_routes::set_tle))
        .route("/schedule/tle/refresh", post(schedule_routes::refresh_tle))
        .route("/schedule/test", post(schedule_routes::test_pass))
        .route("/schedule/stop", post(schedule_routes::stop_pass))
        .route("/system", get(system_routes::get))
        .route("/schedule/geocode", get(schedule_routes::geocode))
        .route("/passes/{id}/audio", get(schedule_routes::audio))
        .route("/passes/{id}", axum::routing::delete(schedule_routes::delete_pass))
        .route("/sstv/image/{id}", get(sstv_routes::image).delete(sstv_routes::delete_image))
        .route("/sstv/images/delete", post(sstv_routes::delete_images))
        .route("/sstv/images.zip", get(sstv_routes::zip))
        .route("/devices/delete", post(sensors::delete_devices))
        .route("/ws", get(ws::handler))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "not found") })
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    let web_dir = &state.cfg.web_dir;
    let spa = ServeDir::new(web_dir).fallback(ServeFile::new(web_dir.join("index.html")));
    let csp = HeaderValue::from_str(&csp(&state.cfg)).expect("CSP is a valid header value");

    Router::new()
        .nest("/api", api)
        .fallback_service(spa)
        .layer(SetResponseHeaderLayer::if_not_present(header::CONTENT_SECURITY_POLICY, csp))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// ------------------------------------------------------------------- errors

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        ApiError {
            status,
            message: message.into(),
            retry_after: None,
        }
    }
    pub fn bad_request(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, m)
    }
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "not logged in")
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, m)
    }
    pub fn too_many(secs: u64) -> Self {
        ApiError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: format!("too many failed attempts, try again in {secs} s"),
            retry_after: Some(secs),
        }
    }
    pub fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!("internal error: {e}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut resp = (self.status, Json(json!({ "error": self.message }))).into_response();
        if let Some(s) = self.retry_after {
            if let Ok(v) = HeaderValue::from_str(&s.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
        }
        resp
    }
}

// ------------------------------------------------------------- extractors

pub fn session_cookie_name(cfg: &Config) -> &'static str {
    // The __Host- prefix makes browsers refuse the cookie unless it is Secure,
    // host-only and Path=/ – only usable over HTTPS.
    if cfg.cookie_secure {
        "__Host-sdr_session"
    } else {
        "sdr_session"
    }
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// Any logged-in user. Mutating requests must carry a matching X-CSRF-Token.
pub struct AuthUser(pub Session);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = cookie_value(&parts.headers, session_cookie_name(&state.cfg))
            .ok_or_else(ApiError::unauthorized)?;
        let session = auth::lookup_session(&state.db, &token)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::unauthorized)?;
        if !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
            let sent = parts
                .headers
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !auth::ct_eq(sent, &session.csrf) {
                return Err(ApiError::forbidden("missing or invalid CSRF token"));
            }
        }
        Ok(AuthUser(session))
    }
}

/// A logged-in admin.
pub struct AdminUser(pub Session);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let AuthUser(s) = AuthUser::from_request_parts(parts, state).await?;
        if s.user.role != Role::Admin {
            return Err(ApiError::forbidden("admin only"));
        }
        Ok(AdminUser(s))
    }
}

// ---------------------------------------------------------------- helpers

/// Client IP, honouring X-Forwarded-For only behind a trusted proxy. We take
/// the right-most entry: the address the proxy itself saw.
pub fn client_ip(cfg: &Config, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    if cfg.trust_proxy {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse().ok())
        {
            return ip;
        }
    }
    peer.ip()
}

/// Reject cross-site requests where the browser tells us the origin
/// (login and WebSocket upgrades, which cookies alone can't protect).
pub fn check_origin(cfg: &Config, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return Ok(()); // non-browser client
    };
    let host = if cfg.trust_proxy {
        headers.get("x-forwarded-host").or_else(|| headers.get(header::HOST))
    } else {
        headers.get(header::HOST)
    }
    .and_then(|v| v.to_str().ok())
    .unwrap_or("");
    let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or("");
    if !host.is_empty() && origin_host.eq_ignore_ascii_case(host) {
        Ok(())
    } else {
        Err(ApiError::forbidden("cross-origin request rejected"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_origins() {
        assert_eq!(
            tile_origin("https://tile.openstreetmap.org/{z}/{x}/{y}.png"),
            "https://tile.openstreetmap.org"
        );
        assert_eq!(tile_origin("https://{s}.tile.example.org/{z}/{x}/{y}.png"), "https://*.tile.example.org");
    }

    #[test]
    fn parses_cookie() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; sdr_session=abc; b=2"));
        assert_eq!(cookie_value(&h, "sdr_session").as_deref(), Some("abc"));
        assert_eq!(cookie_value(&h, "missing"), None);
    }
}
