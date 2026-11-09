//! Frequency bookmarks: a shared station list (admins edit) plus each user's
//! own. Stored in SQLite; CSV import/export.
//!
//! CSV export columns: `name,frequency_mhz,mode,group,colour,notes`.
//! Import reads that, and also CHIRP exports (Name, Frequency, Mode, Comment)
//! and gqrx `bookmarks.csv` (semicolon separated, frequency in Hz).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::audio::demod::Mode;
use crate::radio::{MAX_FREQ_HZ, MIN_FREQ_HZ};

pub const MAX_PER_LIST: i64 = 5_000;
const MAX_NAME: usize = 64;
const MAX_GROUP: usize = 32;
const MAX_NOTES: usize = 500;

/// A bookmark as created or edited.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BookmarkIn {
    pub name: String,
    pub freq_hz: u64,
    pub mode: Mode,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub colour: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// In the shared station list (admins only) rather than the user's own.
    #[serde(default)]
    pub shared: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Bookmark {
    pub id: i64,
    pub name: String,
    pub freq_hz: u64,
    pub mode: Mode,
    pub group: Option<String>,
    pub colour: Option<String>,
    pub notes: Option<String>,
    pub shared: bool,
    pub updated_at: i64,
}

fn clean(s: Option<String>, max: usize) -> Option<String> {
    s.map(|v| v.trim().chars().filter(|c| !c.is_control()).take(max).collect::<String>())
        .filter(|v| !v.is_empty())
}

impl BookmarkIn {
    /// Tidy the text fields and check the rest.
    pub fn validate(mut self) -> Result<Self, String> {
        self.name = clean(Some(self.name), MAX_NAME).ok_or("give the bookmark a name")?;
        if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&self.freq_hz) {
            return Err(format!(
                "frequency must be {}–{} MHz",
                MIN_FREQ_HZ as f64 / 1e6,
                MAX_FREQ_HZ as f64 / 1e6
            ));
        }
        self.group = clean(self.group, MAX_GROUP);
        self.notes = clean(self.notes, MAX_NOTES);
        self.colour = clean(self.colour, 7).map(|c| c.to_ascii_lowercase());
        if let Some(c) = &self.colour {
            if !(c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit())) {
                return Err("colour must look like #3b82f6".into());
            }
        }
        Ok(self)
    }
}

type Row = (i64, Option<i64>, String, i64, String, Option<String>, Option<String>, Option<String>, i64);

fn from_row(r: Row) -> Bookmark {
    Bookmark {
        id: r.0,
        shared: r.1.is_none(),
        name: r.2,
        freq_hz: r.3 as u64,
        mode: serde_json::from_value(serde_json::Value::String(r.4)).unwrap_or(Mode::Nfm),
        group: r.5,
        colour: r.6,
        notes: r.7,
        updated_at: r.8,
    }
}

const COLS: &str = "id, owner_id, name, freq_hz, mode, grp, colour, notes, updated_at";

fn mode_str(m: Mode) -> String {
    serde_json::to_value(m).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "nfm".into())
}

/// The shared list plus `user_id`'s own, by frequency.
pub async fn list(db: &SqlitePool, user_id: i64) -> Result<Vec<Bookmark>> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM bookmarks WHERE owner_id IS NULL OR owner_id = ? ORDER BY freq_hz, name"
    ))
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(from_row).collect())
}

/// One bookmark and its owner (None = shared).
pub async fn get(db: &SqlitePool, id: i64) -> Result<Option<(Bookmark, Option<i64>)>> {
    let row: Option<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM bookmarks WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|r| {
        let owner = r.1;
        (from_row(r), owner)
    }))
}

pub async fn count(db: &SqlitePool, owner: Option<i64>) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM bookmarks WHERE owner_id IS ?")
        .bind(owner)
        .fetch_one(db)
        .await?;
    Ok(n)
}

pub async fn insert(db: &SqlitePool, owner: Option<i64>, b: &BookmarkIn) -> Result<i64> {
    let now = crate::db::now();
    let r = sqlx::query(
        "INSERT INTO bookmarks (owner_id, name, freq_hz, mode, grp, colour, notes, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(owner)
    .bind(&b.name)
    .bind(b.freq_hz as i64)
    .bind(mode_str(b.mode))
    .bind(&b.group)
    .bind(&b.colour)
    .bind(&b.notes)
    .bind(now)
    .bind(now)
    .execute(db)
    .await?;
    Ok(r.last_insert_rowid())
}

pub async fn update(db: &SqlitePool, id: i64, owner: Option<i64>, b: &BookmarkIn) -> Result<()> {
    sqlx::query(
        "UPDATE bookmarks SET owner_id = ?, name = ?, freq_hz = ?, mode = ?, grp = ?, colour = ?, notes = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(owner)
    .bind(&b.name)
    .bind(b.freq_hz as i64)
    .bind(mode_str(b.mode))
    .bind(&b.group)
    .bind(&b.colour)
    .bind(&b.notes)
    .bind(crate::db::now())
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete(db: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM bookmarks WHERE id = ?").bind(id).execute(db).await?;
    Ok(())
}

/// Whether a bookmark with this name and frequency is already in the list.
pub async fn exists(db: &SqlitePool, owner: Option<i64>, name: &str, freq_hz: u64) -> Result<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM bookmarks WHERE owner_id IS ? AND name = ? AND freq_hz = ? LIMIT 1")
            .bind(owner)
            .bind(name)
            .bind(freq_hz as i64)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

// --------------------------------------------------------------------- CSV

/// Spreadsheets run cells starting with these as formulas; prefix a quote.
fn defuse(s: &str) -> String {
    if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    }
}

fn csv_field(s: &str) -> String {
    let s = defuse(s);
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

pub fn to_csv(list: &[Bookmark]) -> String {
    let mut out = String::from("name,frequency_mhz,mode,group,colour,notes\r\n");
    for b in list {
        let fields = [
            csv_field(&b.name),
            format!("{:.6}", b.freq_hz as f64 / 1e6),
            mode_str(b.mode),
            csv_field(b.group.as_deref().unwrap_or("")),
            csv_field(b.colour.as_deref().unwrap_or("")),
            csv_field(b.notes.as_deref().unwrap_or("")),
        ];
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }
    out
}

/// Split CSV text into records (quoted fields may hold separators, quotes
/// and line breaks).
fn records(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut rec = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.trim().is_empty() {
            field.clear();
            quoted = true;
        } else if c == sep {
            rec.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            rec.push(std::mem::take(&mut field));
            if rec.iter().any(|f| !f.trim().is_empty()) {
                out.push(std::mem::take(&mut rec));
            }
            rec.clear();
        } else {
            field.push(c);
        }
    }
    rec.push(field);
    if rec.iter().any(|f| !f.trim().is_empty()) {
        out.push(rec);
    }
    out
}

/// Mode names as other programs write them.
fn parse_mode(s: &str) -> Mode {
    let m = s.trim().to_ascii_lowercase();
    if m.contains("wfm") || m.contains("wide") || m.contains("stereo") || m == "bfm" {
        Mode::Wfm
    } else if m.starts_with("cw") {
        Mode::Cw
    } else if m.contains("usb") {
        Mode::Usb
    } else if m.contains("lsb") {
        Mode::Lsb
    } else if m == "am" || m.starts_with("am ") || m == "nam" || m.contains("sync") {
        Mode::Am
    } else {
        Mode::Nfm
    }
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub rows: Vec<BookmarkIn>,
    /// "line 4: …" for rows that were skipped.
    pub errors: Vec<String>,
}

/// Read bookmarks from CSV text (our own export, CHIRP or gqrx).
pub fn from_csv(text: &str) -> Result<Parsed, String> {
    let text = text.trim_start_matches('\u{feff}');
    // gqrx: semicolons, a tag table first, and a "# Frequency ; Name ; …" header.
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let sep = if first.matches(';').count() > first.matches(',').count() { ';' } else { ',' };
    let recs = records(text, sep);
    let header_at = recs
        .iter()
        .position(|r| r.iter().any(|f| f.trim().trim_start_matches('#').trim().to_ascii_lowercase().starts_with("freq")))
        .ok_or("no header row with a Frequency column")?;
    let header: Vec<String> = recs[header_at]
        .iter()
        .map(|f| f.trim().trim_start_matches('#').trim().to_ascii_lowercase())
        .collect();
    let col = |names: &[&str]| header.iter().position(|h| names.contains(&h.as_str()));
    let freq_mhz = col(&["frequency", "freq", "frequency_mhz", "frequency (mhz)", "freq_mhz", "mhz"]);
    let freq_hz = col(&["frequency_hz", "freq_hz", "hz"]);
    let name = col(&["name", "label", "description", "callsign"]);
    let mode = col(&["mode", "modulation"]);
    let group = col(&["group", "category", "folder", "tags", "tag"]);
    let colour = col(&["colour", "color"]);
    let notes = col(&["notes", "note", "comment", "comments"]);
    if freq_mhz.is_none() && freq_hz.is_none() {
        return Err("no Frequency column".into());
    }
    let mut out = Parsed::default();
    for (i, r) in recs.iter().enumerate().skip(header_at + 1) {
        let line = i + 1;
        let get = |c: Option<usize>| {
            c.and_then(|c| r.get(c)).map(|s| {
                let s = s.trim();
                // Undo the formula guard of our own export.
                match s.strip_prefix('\'') {
                    Some(rest) if rest.starts_with(['=', '+', '-', '@']) => rest.to_string(),
                    _ => s.to_string(),
                }
            })
        };
        if r.first().is_some_and(|f| f.trim_start().starts_with('#')) {
            continue;
        }
        let hz = if let Some(v) = get(freq_hz).filter(|v| !v.is_empty()) {
            v.parse::<f64>().ok()
        } else {
            get(freq_mhz).and_then(|v| v.parse::<f64>().ok()).map(|v| if v > 10_000.0 { v } else { v * 1e6 })
        };
        let Some(hz) = hz.filter(|h| h.is_finite() && *h > 0.0) else {
            out.errors.push(format!("line {line}: no frequency"));
            continue;
        };
        let hz = hz.round() as u64;
        let b = BookmarkIn {
            name: get(name).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("{:.4} MHz", hz as f64 / 1e6)),
            freq_hz: hz,
            mode: get(mode).map_or(Mode::Nfm, |m| parse_mode(&m)),
            group: get(group).map(|g| g.split(',').next().unwrap_or("").trim().to_string()),
            colour: get(colour).filter(|c| !c.is_empty()),
            notes: get(notes),
            shared: false,
        };
        match b.validate() {
            Ok(b) => out.rows.push(b),
            Err(e) => out.errors.push(format!("line {line}: {e}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bm(name: &str, hz: u64, mode: Mode) -> Bookmark {
        Bookmark {
            id: 1,
            name: name.into(),
            freq_hz: hz,
            mode,
            group: Some("Repeaters".into()),
            colour: Some("#22c55e".into()),
            notes: Some("CTCSS 77.0, \"busy\"".into()),
            shared: true,
            updated_at: 0,
        }
    }

    #[test]
    fn export_then_import_round_trips() {
        let list = vec![bm("GB3XX", 145_725_000, Mode::Nfm), bm("=SUM(A1)", 7_030_000, Mode::Cw)];
        let csv = to_csv(&list);
        assert!(csv.contains("'=SUM(A1)"), "formula guard: {csv}");
        let p = from_csv(&csv).unwrap();
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].name, "GB3XX");
        assert_eq!(p.rows[0].freq_hz, 145_725_000);
        assert_eq!(p.rows[0].notes.as_deref(), Some("CTCSS 77.0, \"busy\""));
        assert_eq!(p.rows[1].name, "=SUM(A1)");
        assert_eq!(p.rows[1].mode, Mode::Cw);
        assert_eq!(p.rows[1].colour.as_deref(), Some("#22c55e"));
    }

    #[test]
    fn reads_chirp_and_gqrx() {
        let chirp = "Location,Name,Frequency,Duplex,Offset,Tone,rToneFreq,cToneFreq,DtcsCode,DtcsPolarity,Mode,TStep,Skip,Comment\n\
                     0,GB3WH,145.712500,-,0.600000,Tone,118.8,88.5,023,NN,FM,12.50,,Swindon\n\
                     1,S20,145.500000,,0.600000,,88.5,88.5,023,NN,NFM,12.50,,\n\
                     2,Bad,,,,,,,,,FM,,,\n";
        let p = from_csv(chirp).unwrap();
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].freq_hz, 145_712_500);
        assert_eq!(p.rows[0].notes.as_deref(), Some("Swindon"));
        assert_eq!(p.errors, vec!["line 4: no frequency".to_string()]);

        let gqrx = "# Tag name          ;  color\nUntagged            ; #c0c0c0\nBroadcast           ; #ff0000\n\n\
                    # Frequency ; Name                     ; Modulation          ;  Bandwidth; Tags\n\
                    93500000 ; Radio 4                  ; WFM (stereo)        ;     160000; Broadcast\n\
                    7030000 ; QRP calling              ; CW-U                ;        500; Untagged\n";
        let p = from_csv(gqrx).unwrap();
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].name, "Radio 4");
        assert_eq!(p.rows[0].freq_hz, 93_500_000);
        assert_eq!(p.rows[0].mode, Mode::Wfm);
        assert_eq!(p.rows[0].group.as_deref(), Some("Broadcast"));
        assert_eq!(p.rows[1].mode, Mode::Cw);
    }

    #[test]
    fn validation() {
        let ok = BookmarkIn {
            name: "  Air  ".into(),
            freq_hz: 118_000_000,
            mode: Mode::Am,
            group: Some(" ".into()),
            colour: Some("#ABCDEF".into()),
            notes: None,
            shared: false,
        };
        let v = ok.clone().validate().unwrap();
        assert_eq!(v.name, "Air");
        assert_eq!(v.group, None);
        assert_eq!(v.colour.as_deref(), Some("#abcdef"));
        assert!(BookmarkIn { name: "".into(), ..ok.clone() }.validate().is_err());
        assert!(BookmarkIn { freq_hz: 3, ..ok.clone() }.validate().is_err());
        assert!(BookmarkIn { colour: Some("red".into()), ..ok }.validate().is_err());
        assert!(from_csv("a,b\n1,2\n").is_err());
    }
}
