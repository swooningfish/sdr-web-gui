//! Users, password hashing (Argon2id), sessions and login rate limiting.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::db::now;

pub const MIN_PASSWORD_LEN: usize = 10;
pub const MAX_PASSWORD_LEN: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Viewer => "viewer",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "admin" => Some(Role::Admin),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: Role,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub user: User,
    pub csrf: String,
    pub token_hash: String,
}

// ---------------------------------------------------------------- passwords

fn hash_sync(pw: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow!("password hash failed: {e}"))
}

fn verify_sync(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

/// Hash used when the username doesn't exist, so a failed login costs the
/// same either way (no username enumeration by timing).
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_sync("not-a-real-password").expect("argon2"))
}

pub async fn hash_password(pw: String) -> Result<String> {
    tokio::task::spawn_blocking(move || hash_sync(&pw)).await?
}

pub fn validate_password(pw: &str) -> Result<()> {
    let n = pw.chars().count();
    if n < MIN_PASSWORD_LEN {
        bail!("password must be at least {MIN_PASSWORD_LEN} characters");
    }
    if n > MAX_PASSWORD_LEN {
        bail!("password is too long");
    }
    Ok(())
}

pub fn validate_username(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !ok {
        bail!("username must be 1-32 characters: letters, digits, _ - .");
    }
    Ok(())
}

// -------------------------------------------------------------------- users

pub async fn user_count(db: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(db).await?)
}

pub async fn admin_count(db: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin'")
        .fetch_one(db)
        .await?)
}

pub async fn create_user(db: &SqlitePool, username: &str, password: &str, role: Role) -> Result<i64> {
    validate_username(username)?;
    validate_password(password)?;
    let hash = hash_password(password.to_string()).await?;
    let res = sqlx::query(
        "INSERT INTO users (username, password_hash, role, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(username)
    .bind(hash)
    .bind(role.as_str())
    .bind(now())
    .execute(db)
    .await;
    match res {
        Ok(r) => Ok(r.last_insert_rowid()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            bail!("user {username:?} already exists")
        }
        Err(e) => Err(e.into()),
    }
}

#[derive(Serialize)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub role: Role,
    pub created_at: i64,
}

pub async fn list_users(db: &SqlitePool) -> Result<Vec<UserRow>> {
    let rows: Vec<(i64, String, String, i64)> =
        sqlx::query_as("SELECT id, username, role, created_at FROM users ORDER BY username")
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, username, role, created_at)| UserRow {
            id,
            username,
            role: Role::parse(&role).unwrap_or(Role::Viewer),
            created_at,
        })
        .collect())
}

pub async fn get_user(db: &SqlitePool, id: i64) -> Result<Option<User>> {
    let row: Option<(i64, String, String)> =
        sqlx::query_as("SELECT id, username, role FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(id, username, role)| User {
        id,
        username,
        role: Role::parse(&role).unwrap_or(Role::Viewer),
    }))
}

pub async fn delete_user(db: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM users WHERE id = ?").bind(id).execute(db).await?;
    Ok(r.rows_affected() > 0)
}

pub async fn set_password(db: &SqlitePool, user_id: i64, password: &str) -> Result<()> {
    validate_password(password)?;
    let hash = hash_password(password.to_string()).await?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn set_password_by_name(db: &SqlitePool, username: &str, password: &str) -> Result<()> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(db)
        .await?;
    let id = id.ok_or_else(|| anyhow!("no such user {username:?}"))?;
    set_password(db, id, password).await?;
    // Force re-login everywhere.
    sqlx::query("DELETE FROM sessions WHERE user_id = ?").bind(id).execute(db).await?;
    Ok(())
}

/// Check credentials. Always performs one Argon2 verification.
pub async fn authenticate(db: &SqlitePool, username: &str, password: &str) -> Result<Option<User>> {
    let row: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT id, username, role, password_hash FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(db)
    .await?;
    let (user, hash) = match row {
        Some((id, username, role, hash)) => (
            Some(User {
                id,
                username,
                role: Role::parse(&role).unwrap_or(Role::Viewer),
            }),
            hash,
        ),
        None => (None, dummy_hash().to_string()),
    };
    let pw = password.to_string();
    let ok = tokio::task::spawn_blocking(move || verify_sync(&pw, &hash)).await?;
    Ok(if ok { user } else { None })
}

pub async fn verify_user_password(db: &SqlitePool, user_id: i64, password: &str) -> Result<bool> {
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    let Some(hash) = hash else { return Ok(false) };
    let pw = password.to_string();
    Ok(tokio::task::spawn_blocking(move || verify_sync(&pw, &hash)).await?)
}

/// Create the first admin if the database has no users.
pub async fn bootstrap_admin(db: &SqlitePool, username: &str, password: Option<&str>) -> Result<()> {
    // Warm the dummy hash so the first failed login isn't measurably slower.
    tokio::task::spawn_blocking(|| {
        dummy_hash();
    })
    .await?;
    if user_count(db).await? > 0 {
        return Ok(());
    }
    match password {
        Some(pw) => {
            create_user(db, username, pw, Role::Admin).await?;
            tracing::info!("created admin user {username:?} from SDRD_ADMIN_PASSWORD");
        }
        None => {
            let pw = hex::encode(rand::random::<[u8; 12]>());
            create_user(db, username, &pw, Role::Admin).await?;
            tracing::warn!(
                "created admin user {username:?} with generated password {pw:?} — \
                 log in and change it (this is only shown once)"
            );
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- sessions

pub fn new_token() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Returns (session token for the cookie, CSRF token for the client).
pub async fn create_session(db: &SqlitePool, user_id: i64, hours: i64) -> Result<(String, String)> {
    let token = new_token();
    let csrf = new_token();
    let t = now();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, csrf, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(token_hash(&token))
    .bind(user_id)
    .bind(&csrf)
    .bind(t)
    .bind(t + hours * 3600)
    .execute(db)
    .await?;
    Ok((token, csrf))
}

pub async fn lookup_session(db: &SqlitePool, token: &str) -> Result<Option<Session>> {
    let th = token_hash(token);
    let row: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT u.id, u.username, u.role, s.csrf FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(&th)
    .bind(now())
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(id, username, role, csrf)| Session {
        user: User {
            id,
            username,
            role: Role::parse(&role).unwrap_or(Role::Viewer),
        },
        csrf,
        token_hash: th,
    }))
}

pub async fn session_valid(db: &SqlitePool, token_hash: &str) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sessions WHERE token_hash = ? AND expires_at > ?",
    )
    .bind(token_hash)
    .bind(now())
    .fetch_one(db)
    .await
    .map(|n| n > 0)
    .unwrap_or(false)
}

pub async fn delete_session(db: &SqlitePool, token_hash: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete_other_sessions(db: &SqlitePool, user_id: i64, keep_hash: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ? AND token_hash != ?")
        .bind(user_id)
        .bind(keep_hash)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn purge_expired(db: &SqlitePool) -> Result<u64> {
    let r = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now())
        .execute(db)
        .await?;
    Ok(r.rows_affected())
}

/// Constant-time string comparison for tokens.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ------------------------------------------------------------ rate limiting

/// Fixed-window failure counter keyed by e.g. `ip:1.2.3.4` or `user:bob`.
pub struct RateLimiter {
    entries: HashMap<String, (u32, Instant)>,
    window: Duration,
}

impl RateLimiter {
    pub fn new(window: Duration) -> Self {
        RateLimiter {
            entries: HashMap::new(),
            window,
        }
    }

    /// Seconds until the key is allowed again, or `None` if allowed now.
    pub fn blocked_for(&mut self, key: &str, max_failures: u32) -> Option<u64> {
        let now = Instant::now();
        match self.entries.get(key) {
            Some(&(_, start)) if now.duration_since(start) >= self.window => {
                self.entries.remove(key);
                None
            }
            Some(&(count, start)) if count >= max_failures => {
                Some((self.window - now.duration_since(start)).as_secs().max(1))
            }
            _ => None,
        }
    }

    pub fn record_failure(&mut self, key: &str) {
        let now = Instant::now();
        if self.entries.len() > 10_000 {
            let w = self.window;
            self.entries.retain(|_, (_, s)| now.duration_since(*s) < w);
        }
        let e = self.entries.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(e.1) >= self.window {
            *e = (0, now);
        }
        e.0 += 1;
    }

    pub fn clear(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_roundtrip() {
        let h = hash_sync("correct horse battery").unwrap();
        assert!(verify_sync("correct horse battery", &h));
        assert!(!verify_sync("wrong", &h));
        assert!(!verify_sync("x", "not a hash"));
    }

    #[test]
    fn limiter_blocks_after_max() {
        let mut rl = RateLimiter::new(Duration::from_secs(60));
        for _ in 0..3 {
            assert!(rl.blocked_for("user:a", 3).is_none());
            rl.record_failure("user:a");
        }
        assert!(rl.blocked_for("user:a", 3).is_some());
        assert!(rl.blocked_for("user:b", 3).is_none());
        rl.clear("user:a");
        assert!(rl.blocked_for("user:a", 3).is_none());
    }

    #[test]
    fn usernames() {
        assert!(validate_username("greg.c-1").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username("bad name").is_err());
        assert!(validate_username("<script>").is_err());
    }

    #[test]
    fn ct_eq_works() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
    }
}
