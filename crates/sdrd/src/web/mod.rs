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
use crate::decoders::rtl433::Rtl433Handle;
use crate::events::EventFeed;
use crate::mqtt::MqttHandle;
use crate::radio::RadioHandle;

mod auth_routes;
mod radio_routes;
mod sensors;
mod users;
mod ws;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub cfg: Arc<Config>,
    pub radio: RadioHandle,
    pub waterfall_tx: broadcast::Sender<Bytes>,
    pub limiter: Arc<Mutex<RateLimiter>>,
    pub rtl433: Rtl433Handle,
    pub event_feed: EventFeed,
    pub mqtt: Option<MqttHandle>,
}

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; connect-src 'self'; media-src 'self' blob:; worker-src 'self' blob:; \
frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";

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
        .route("/devices", get(sensors::list_devices))
        .route(
            "/devices/{id}",
            axum::routing::patch(sensors::update_device).delete(sensors::delete_device),
        )
        .route("/devices/{id}/events", get(sensors::device_events))
        .route("/events", get(sensors::recent_events))
        .route("/decoders/rtl_433", get(sensors::get_rtl433).put(sensors::set_rtl433))
        .route("/decoders/rtl_433/check", post(sensors::check_spec))
        .route("/ws", get(ws::handler))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "not found") })
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    let web_dir = &state.cfg.web_dir;
    let spa = ServeDir::new(web_dir).fallback(ServeFile::new(web_dir.join("index.html")));

    Router::new()
        .nest("/api", api)
        .fallback_service(spa)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
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
    fn parses_cookie() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; sdr_session=abc; b=2"));
        assert_eq!(cookie_value(&h, "sdr_session").as_deref(), Some("abc"));
        assert_eq!(cookie_value(&h, "missing"), None);
    }
}
