use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::{check_origin, client_ip, session_cookie_name, ApiError, AppState, AuthUser};
use crate::auth;

/// Failure budgets within the limiter window: per username from one address,
/// and per address overall.
const MAX_FAILS_PER_USER: u32 = 5;
const MAX_FAILS_PER_IP: u32 = 20;

#[derive(Deserialize)]
pub struct LoginReq {
    username: String,
    password: String,
}

fn session_cookie(state: &AppState, value: &str, max_age: i64) -> HeaderValue {
    let secure = if state.cfg.cookie_secure { "; Secure" } else { "" };
    let c = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
        session_cookie_name(&state.cfg),
        value,
        max_age,
        secure
    );
    HeaderValue::from_str(&c).expect("cookie is ASCII")
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> Result<Response, ApiError> {
    check_origin(&state.cfg, &headers)?;
    if req.username.len() > 64 || req.password.len() > auth::MAX_PASSWORD_LEN * 4 {
        return Err(ApiError::bad_request("invalid credentials"));
    }
    let ip = client_ip(&state.cfg, peer, &headers);
    let ip_key = format!("ip:{ip}");
    // Per username *and* address: wrong guesses from elsewhere must not lock
    // the real user out of their own account.
    let user_key = format!("user:{}|{ip}", req.username.to_ascii_lowercase());
    {
        // Attempts are counted here, before the password is checked, so a
        // burst of parallel requests can't all get past the limit; a good
        // login gives its attempt back below.
        let mut rl = state.limiter.lock().unwrap();
        let wait = rl
            .blocked_for(&user_key, MAX_FAILS_PER_USER)
            .or_else(|| rl.attempt(&ip_key, MAX_FAILS_PER_IP));
        if let Some(secs) = wait {
            return Err(ApiError::too_many(secs));
        }
        rl.record_failure(&user_key);
    }

    let user = auth::authenticate(&state.db, &req.username, &req.password)
        .await
        .map_err(ApiError::internal)?;
    let Some(user) = user else {
        tracing::warn!(user = %req.username, ip = %ip_key, "failed login");
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid username or password",
        ));
    };
    {
        let mut rl = state.limiter.lock().unwrap();
        rl.forgive(&ip_key);
        rl.clear(&user_key);
    }

    let (token, csrf) = auth::create_session(&state.db, user.id, state.cfg.session_hours)
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(user = %user.username, "login");
    let mut resp = Json(json!({ "user": user, "csrf": csrf })).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(&state, &token, state.cfg.session_hours * 3600),
    );
    Ok(resp)
}

pub async fn logout(State(state): State<AppState>, AuthUser(s): AuthUser) -> Result<Response, ApiError> {
    auth::delete_session(&state.db, &s.token_hash)
        .await
        .map_err(ApiError::internal)?;
    let mut resp = Json(json!({ "ok": true })).into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, session_cookie(&state, "", 0));
    Ok(resp)
}

pub async fn me(AuthUser(s): AuthUser) -> Json<serde_json::Value> {
    Json(json!({ "user": s.user, "csrf": s.csrf }))
}

#[derive(Deserialize)]
pub struct PasswordReq {
    current_password: String,
    new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    AuthUser(s): AuthUser,
    Json(req): Json<PasswordReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ok = auth::verify_user_password(&state.db, s.user.id, &req.current_password)
        .await
        .map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::forbidden("current password is wrong"));
    }
    auth::validate_password(&req.new_password).map_err(|e| ApiError::bad_request(e.to_string()))?;
    auth::set_password(&state.db, s.user.id, &req.new_password)
        .await
        .map_err(ApiError::internal)?;
    // Sign out every other session for this user.
    auth::delete_other_sessions(&state.db, s.user.id, &s.token_hash)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(json!({ "ok": true })))
}
