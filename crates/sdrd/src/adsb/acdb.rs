//! Aircraft database: registration and type by ICAO address, from the
//! community database Mictronics maintains (via the tar1090-db CSV export).
//! Downloaded on request (~9 MB) into SQLite; looked up once per aircraft.

use std::io::Read;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::tracker::DbInfo;
use crate::db;

pub const SOURCE_URL: &str = "https://github.com/wiedehopf/tar1090-db/raw/refs/heads/csv/aircraft.csv.gz";
const META_KEY: &str = "adsb.aircraft_db";
/// Refuse anything much bigger than the real file.
const MAX_DOWNLOAD: u64 = 64 << 20;
const BATCH: usize = 400;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DbStatus {
    pub rows: u64,
    pub updated_ms: Option<i64>,
    #[serde(default)]
    pub downloading: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub source: String,
}

pub async fn status(pool: &SqlitePool) -> DbStatus {
    db::get_setting::<DbStatus>(pool, META_KEY).await.ok().flatten().unwrap_or_default()
}

type DbRow = (Option<String>, Option<String>, Option<String>, Option<String>);

pub async fn lookup(pool: &SqlitePool, icao: u32) -> Option<DbInfo> {
    let row: Option<DbRow> =
        sqlx::query_as("SELECT reg, type, descr, flags FROM aircraft_db WHERE icao = ?")
            .bind(icao as i64)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let (reg, type_code, desc, flags) = row?;
    Some(DbInfo {
        reg,
        type_code,
        desc,
        military: flags.is_some_and(|f| f.starts_with('1')),
    })
}

struct Row {
    icao: u32,
    reg: Option<String>,
    type_code: Option<String>,
    desc: Option<String>,
    flags: Option<String>,
    year: Option<i64>,
}

fn opt(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.chars().take(64).collect())
}

/// `icao;reg;type;flags;description;year;owner;` (owner is not kept).
fn parse_line(line: &str) -> Option<Row> {
    let mut f = line.split(';');
    let icao = u32::from_str_radix(f.next()?.trim(), 16).ok().filter(|v| *v <= 0xFF_FFFF)?;
    let reg = opt(f.next().unwrap_or(""));
    let type_code = opt(f.next().unwrap_or(""));
    let flags = opt(f.next().unwrap_or(""));
    let desc = opt(f.next().unwrap_or(""));
    let year = f.next().and_then(|y| y.trim().parse().ok());
    // Rows with nothing useful in them aren't worth storing.
    if reg.is_none() && type_code.is_none() && desc.is_none() {
        return None;
    }
    Some(Row { icao, reg, type_code, desc, flags, year })
}

fn fetch() -> Result<String> {
    let resp = ureq::get(SOURCE_URL)
        .timeout(Duration::from_secs(120))
        .set("User-Agent", "sdrd (sdr-web-gui)")
        .call()
        .map_err(|e| anyhow!("download failed: {e}"))?;
    let mut gz = Vec::new();
    resp.into_reader().take(MAX_DOWNLOAD).read_to_end(&mut gz).context("download interrupted")?;
    let mut text = String::new();
    flate2::read::GzDecoder::new(&gz[..])
        .take(MAX_DOWNLOAD * 8)
        .read_to_string(&mut text)
        .context("not a gzip CSV")?;
    Ok(text)
}

async fn set_status(pool: &SqlitePool, f: impl FnOnce(&mut DbStatus)) {
    let mut s = status(pool).await;
    f(&mut s);
    if let Err(e) = db::set_setting(pool, META_KEY, &s).await {
        tracing::warn!("saving aircraft database status: {e}");
    }
}

/// Download and replace the database. Returns the number of aircraft.
pub async fn download(pool: &SqlitePool) -> Result<u64> {
    set_status(pool, |s| {
        s.downloading = true;
        s.error = None;
    })
    .await;
    let result = load(pool).await;
    set_status(pool, |s| {
        s.downloading = false;
        match &result {
            Ok(n) => {
                s.rows = *n;
                s.updated_ms = Some(db::now_ms());
                s.source = SOURCE_URL.into();
            }
            Err(e) => s.error = Some(e.to_string()),
        }
    })
    .await;
    result
}

/// After a restart mid-download, the flag would be stuck.
pub async fn clear_stale(pool: &SqlitePool) {
    if status(pool).await.downloading {
        set_status(pool, |s| s.downloading = false).await;
    }
}

async fn load(pool: &SqlitePool) -> Result<u64> {
    let text = tokio::task::spawn_blocking(fetch).await??;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM aircraft_db").execute(&mut *tx).await?;
    let mut n = 0u64;
    let mut batch: Vec<Row> = Vec::with_capacity(BATCH);
    let mut lines = text.lines().filter_map(parse_line).peekable();
    while lines.peek().is_some() {
        batch.clear();
        batch.extend(lines.by_ref().take(BATCH));
        let sql = format!(
            "INSERT OR REPLACE INTO aircraft_db (icao, reg, type, descr, flags, year) VALUES {}",
            vec!["(?, ?, ?, ?, ?, ?)"; batch.len()].join(", ")
        );
        let mut q = sqlx::query(&sql);
        for r in &batch {
            q = q
                .bind(r.icao as i64)
                .bind(r.reg.as_deref())
                .bind(r.type_code.as_deref())
                .bind(r.desc.as_deref())
                .bind(r.flags.as_deref())
                .bind(r.year);
        }
        q.execute(&mut *tx).await?;
        n += batch.len() as u64;
    }
    if n < 1000 {
        return Err(anyhow!("the download had only {n} aircraft; keeping the old database"));
    }
    tx.commit().await?;
    tracing::info!("aircraft database updated: {n} aircraft");
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows() {
        let r = parse_line("4CA129;EI-CSI;B738;00;BOEING 737-800;;;").unwrap();
        assert_eq!(r.icao, 0x4CA129);
        assert_eq!(r.reg.as_deref(), Some("EI-CSI"));
        assert_eq!(r.type_code.as_deref(), Some("B738"));
        assert_eq!(r.desc.as_deref(), Some("BOEING 737-800"));
        let r = parse_line("A127AB;N1737M;C182;00;CESSNA 182 Skylane;1975;SOME OWNER;").unwrap();
        assert_eq!(r.year, Some(1975));
        assert!(parse_line("A127B2;;;0010;;;;").is_none());
        assert!(parse_line("garbage").is_none());
    }
}
