//! Function keys: each user's programmable F1–F8 on the receiver's front
//! panel. A key recalls a frequency and mode, usually set from a bookmark;
//! it keeps its own copy so it still works if the bookmark is deleted.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::audio::demod::Mode;
use crate::bookmarks::mode_str;
use crate::radio::{MAX_FREQ_HZ, MIN_FREQ_HZ};

pub const SLOTS: u8 = 8;
const MAX_NAME: usize = 64;

/// A key as set by the user.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FKeyIn {
    pub name: String,
    pub freq_hz: u64,
    pub mode: Mode,
    #[serde(default)]
    pub bookmark_id: Option<i64>,
    /// DAB service id (DAB mode).
    #[serde(default)]
    pub service_id: Option<u16>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct FKey {
    pub slot: u8,
    pub name: String,
    pub freq_hz: u64,
    pub mode: Mode,
    pub bookmark_id: Option<i64>,
    pub service_id: Option<u16>,
}

impl FKeyIn {
    pub fn validate(mut self) -> Result<Self, String> {
        self.name = self
            .name
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_NAME)
            .collect::<String>();
        if self.name.is_empty() {
            return Err("give the key a name".into());
        }
        if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&self.freq_hz) {
            return Err(format!(
                "frequency must be {}–{} MHz",
                MIN_FREQ_HZ as f64 / 1e6,
                MAX_FREQ_HZ as f64 / 1e6
            ));
        }
        // Calling frequencies and repeaters aren't stored bookmarks.
        if self.bookmark_id.is_some_and(|id| id <= 0) {
            self.bookmark_id = None;
        }
        if self.mode != Mode::Dab {
            self.service_id = None;
        }
        Ok(self)
    }
}

pub fn check_slot(slot: u8) -> Result<(), String> {
    if (1..=SLOTS).contains(&slot) {
        Ok(())
    } else {
        Err(format!("slot must be 1–{SLOTS}"))
    }
}

type Row = (i64, String, i64, String, Option<i64>, Option<i64>);

pub async fn list(db: &SqlitePool, user_id: i64) -> Result<Vec<FKey>> {
    let rows: Vec<Row> =
        sqlx::query_as("SELECT slot, name, freq_hz, mode, bookmark_id, service_id FROM fkeys WHERE user_id = ? ORDER BY slot")
            .bind(user_id)
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|r| FKey {
            slot: r.0 as u8,
            name: r.1,
            freq_hz: r.2 as u64,
            mode: serde_json::from_value(serde_json::Value::String(r.3)).unwrap_or(Mode::Nfm),
            bookmark_id: r.4,
            service_id: r.5.and_then(|v| u16::try_from(v).ok()),
        })
        .collect())
}

pub async fn set(db: &SqlitePool, user_id: i64, slot: u8, k: &FKeyIn) -> Result<()> {
    sqlx::query(
        "INSERT INTO fkeys (user_id, slot, name, freq_hz, mode, bookmark_id, service_id) VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(user_id, slot) DO UPDATE SET name = excluded.name, freq_hz = excluded.freq_hz, \
         mode = excluded.mode, bookmark_id = excluded.bookmark_id, service_id = excluded.service_id",
    )
    .bind(user_id)
    .bind(slot as i64)
    .bind(&k.name)
    .bind(k.freq_hz as i64)
    .bind(mode_str(k.mode))
    .bind(k.bookmark_id)
    .bind(k.service_id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn clear(db: &SqlitePool, user_id: i64, slot: u8) -> Result<()> {
    sqlx::query("DELETE FROM fkeys WHERE user_id = ? AND slot = ?")
        .bind(user_id)
        .bind(slot as i64)
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, freq_hz: u64) -> FKeyIn {
        FKeyIn { name: name.into(), freq_hz, mode: Mode::Nfm, bookmark_id: Some(-3), service_id: None }
    }

    #[test]
    fn validates() {
        let k = key("  GB3XX\n ", 145_600_000).validate().unwrap();
        assert_eq!(k.name, "GB3XX");
        assert_eq!(k.bookmark_id, None, "built-in rows aren't bookmarks");
        assert!(key("  ", 145_600_000).validate().is_err());
        assert!(key("x", 1).validate().is_err());
        assert!(check_slot(0).is_err());
        assert!(check_slot(1).is_ok());
        assert!(check_slot(SLOTS).is_ok());
        assert!(check_slot(SLOTS + 1).is_err());
    }

    #[tokio::test]
    async fn stores_per_user() {
        let dir = std::env::temp_dir().join(format!("fkeys-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let db = crate::db::open(&path).await.unwrap();
        for (id, name) in [(1, "a"), (2, "b")] {
            sqlx::query("INSERT INTO users (id, username, password_hash, role, created_at) VALUES (?, ?, 'x', 'viewer', 0)")
                .bind(id)
                .bind(name)
                .execute(&db)
                .await
                .unwrap();
        }
        let k = key("Calling", 145_500_000).validate().unwrap();
        set(&db, 1, 3, &k).await.unwrap();
        set(&db, 1, 3, &FKeyIn { name: "S20".into(), ..k.clone() }).await.unwrap();
        let got = list(&db, 1).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].slot, 3);
        assert_eq!(got[0].name, "S20");
        assert_eq!(got[0].service_id, None);
        assert!(list(&db, 2).await.unwrap().is_empty());
        // A DAB key keeps its station; setting it again replaces it.
        let dab = FKeyIn { name: "Jazz".into(), freq_hz: 222_064_000, mode: Mode::Dab, bookmark_id: None, service_id: Some(0xC222) };
        set(&db, 1, 3, &dab).await.unwrap();
        let got = list(&db, 1).await.unwrap();
        assert_eq!((got[0].mode, got[0].service_id), (Mode::Dab, Some(0xC222)));
        set(&db, 1, 3, &FKeyIn { service_id: Some(0xC223), ..dab }).await.unwrap();
        assert_eq!(list(&db, 1).await.unwrap()[0].service_id, Some(0xC223));
        clear(&db, 1, 3).await.unwrap();
        assert!(list(&db, 1).await.unwrap().is_empty());
        db.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
