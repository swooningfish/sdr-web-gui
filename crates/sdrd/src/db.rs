use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT    NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('admin', 'viewer')),
    created_at    INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT    PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS sessions_expiry ON sessions(expires_at);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS devices (
    id         INTEGER PRIMARY KEY,
    decoder    TEXT    NOT NULL,
    key        TEXT    NOT NULL UNIQUE,
    model      TEXT    NOT NULL,
    dev_id     TEXT,
    channel    TEXT,
    kind       TEXT,
    name       TEXT,
    tracked    INTEGER NOT NULL DEFAULT 0,
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL,
    count      INTEGER NOT NULL DEFAULT 0,
    last_data  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS devices_last_seen ON devices(last_seen);
CREATE TABLE IF NOT EXISTS events (
    id        INTEGER PRIMARY KEY,
    device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    ts        INTEGER NOT NULL,
    freq_hz   INTEGER,
    rssi      REAL,
    snr       REAL,
    data      TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS events_device_ts ON events(device_id, ts);
CREATE INDEX IF NOT EXISTS events_ts ON events(ts);
CREATE TABLE IF NOT EXISTS bookmarks (
    id         INTEGER PRIMARY KEY,
    -- NULL = the station's shared list, otherwise the user's own
    owner_id   INTEGER REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    freq_hz    INTEGER NOT NULL,
    mode       TEXT    NOT NULL,
    grp        TEXT,
    colour     TEXT,
    notes      TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS bookmarks_owner ON bookmarks(owner_id, freq_hz);
CREATE TABLE IF NOT EXISTS aircraft_db (
    icao  INTEGER PRIMARY KEY,
    reg   TEXT,
    type  TEXT,
    descr TEXT,
    flags TEXT,
    year  INTEGER
);
CREATE TABLE IF NOT EXISTS adsb_sightings (
    id       INTEGER PRIMARY KEY,
    icao     INTEGER NOT NULL,
    callsign TEXT,
    reg      TEXT,
    type     TEXT,
    category TEXT,
    first_ms INTEGER NOT NULL,
    last_ms  INTEGER NOT NULL,
    msgs     INTEGER NOT NULL,
    max_km   REAL,
    min_alt  INTEGER,
    max_alt  INTEGER
);
CREATE INDEX IF NOT EXISTS adsb_sightings_last ON adsb_sightings(last_ms)
"#;

pub async fn open(path: &Path) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await?;
    for stmt in SCHEMA.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(stmt).execute(&pool).await?;
    }
    add_columns(
        &pool,
        "devices",
        &[
            ("lat", "REAL"),
            ("lon", "REAL"),
            ("pos_ts", "INTEGER"),
            ("symbol", "TEXT"),
            // Position on the sensor dashboard; NULL = not on it.
            ("dash_pos", "INTEGER"),
        ],
    )
    .await?;
    Ok(pool)
}

/// Additive migrations: add any columns an older database doesn't have yet.
async fn add_columns(db: &SqlitePool, table: &str, cols: &[(&str, &str)]) -> Result<()> {
    let existing: Vec<(String,)> = sqlx::query_as(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .fetch_all(db)
        .await?;
    for (name, ty) in cols {
        if !existing.iter().any(|(n,)| n == name) {
            sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN {name} {ty}")).execute(db).await?;
        }
    }
    Ok(())
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// JSON value stored in the `settings` table.
pub async fn get_setting<T: DeserializeOwned>(db: &SqlitePool, key: &str) -> Result<Option<T>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await?;
    Ok(match row {
        Some((v,)) => Some(serde_json::from_str(&v)?),
        None => None,
    })
}

pub async fn set_setting<T: Serialize>(db: &SqlitePool, key: &str, value: &T) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(serde_json::to_string(value)?)
    .execute(db)
    .await?;
    Ok(())
}
