//! ARISS SSTV events: a few times a year the ISS sends a series of SSTV
//! pictures for several days, usually on 437.550 MHz rather than the usual
//! 145.800. The schedule is read from ariss.org's "Upcoming SSTV events"
//! page, which is free text, so the parser is forgiving and keeps only
//! entries where it found both a start and an end.

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

pub const URL: &str = "https://www.ariss.org/upcoming-sstv-events.html";
pub const ISS_NORAD: u32 = 25544;
/// Events that ended longer ago than this are dropped.
const KEEP_MS: i64 = 60 * 86_400_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArissEvent {
    pub series: u32,
    /// The theme, e.g. "Space Memories" (or the headline if it has none).
    pub name: String,
    /// The headline as ARISS wrote it.
    pub title: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub freq_hz: Option<u64>,
    pub mode: Option<String>,
    /// e.g. "36 s on / 2 min off".
    pub cadence: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArissState {
    pub checked_ms: Option<i64>,
    pub error: Option<String>,
    pub events: Vec<ArissEvent>,
}

impl ArissState {
    /// The event in progress, or else the next one to start.
    pub fn current_or_next(&self, now: i64) -> Option<&ArissEvent> {
        self.events
            .iter()
            .filter(|e| e.end_ms > now)
            .min_by_key(|e| e.start_ms)
    }

    /// The event a pass of this satellite falls in.
    pub fn during(&self, norad: u32, aos_ms: i64, los_ms: i64) -> Option<&ArissEvent> {
        (norad == ISS_NORAD)
            .then(|| self.events.iter().find(|e| los_ms > e.start_ms && aos_ms < e.end_ms))
            .flatten()
    }
}

pub async fn fetch(now_ms: i64) -> Result<Vec<ArissEvent>> {
    tokio::task::spawn_blocking(move || -> Result<Vec<ArissEvent>> {
        let body = ureq::get(URL)
            .timeout(Duration::from_secs(20))
            .set("User-Agent", "sdrd (sdr-web-gui)")
            .call()
            .map_err(|e| anyhow!("ariss.org: {e}"))?
            .into_string()?;
        let events = parse(&body, now_ms);
        if events.is_empty() && !body.contains("SSTV") {
            return Err(anyhow!("ariss.org: the events page wasn't recognised"));
        }
        Ok(events)
    })
    .await?
}

/// HTML → text lines: `<br>` and block ends become newlines, tags go,
/// the few entities the page uses are decoded.
fn text_lines(html: &str) -> Vec<String> {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('>') else { break };
        let tag = rest[i + 1..i + j].trim_start_matches('/').to_ascii_lowercase();
        let name: String = tag.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if matches!(name.as_str(), "br" | "p" | "div" | "h1" | "h2" | "h3" | "li" | "tr") {
            out.push('\n');
        }
        rest = &rest[i + j + 1..];
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&ldquo;", "\"")
        .replace("&rdquo;", "\"")
        .replace("&#8220;", "\"")
        .replace("&#8221;", "\"")
        .replace(['\u{201c}', '\u{201d}'], "\"")
        .replace("&rsquo;", "'")
        .replace("&mdash;", "-")
        .replace("&ndash;", "-")
        .replace('\u{a0}', " ");
    out.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect()
}

/// A leading `2026-11-05` (the date ARISS posted the entry).
fn posted(line: &str) -> Option<(i32, u32, u32)> {
    let d = line.split(' ').next()?;
    let mut it = d.split('-');
    let (a, b, c) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    let n = |s: &str| s.parse::<u32>().ok();
    match (a.len(), c.len()) {
        (4, 2) => Some((n(a)? as i32, n(b)?, n(c)?)),
        // "12-04-2025"
        (2, 4) => Some((n(c)? as i32, n(a)?, n(b)?)),
        _ => None,
    }
}

fn series_of(line: &str) -> Option<u32> {
    let i = line.find("Series ")?;
    let digits: String = line[i + 7..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// Days since 1970-01-01 (proleptic Gregorian).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "Begins Monday, November 9, 2026 17:45 UTC" → ms. Without a year, the
/// one that puts it soonest after the entry was posted.
fn when(line: &str, posted: (i32, u32, u32)) -> Option<i64> {
    let words: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    let (mi, month) = words.iter().enumerate().find_map(|(i, w)| {
        let w = w.to_ascii_lowercase();
        (w.len() >= 3)
            .then(|| MONTHS.iter().position(|m| w.starts_with(m)))
            .flatten()
            .map(|m| (i, m as u32 + 1))
    })?;
    let after = &words[mi + 1..];
    let day: u32 = after
        .first()?
        .trim_end_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .ok()
        .filter(|d| (1..=31).contains(d))?;
    let year = after
        .iter()
        .find_map(|w| w.parse::<i32>().ok().filter(|y| (2000..2100).contains(y)));
    let (h, min) = words.iter().find_map(|w| {
        let w = w.trim_matches(|c: char| !c.is_ascii_digit());
        let (h, m) = w.split_once(':')?;
        let (h, m) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
        (h < 24 && m < 60).then_some((h, m))
    })?;
    let year = year.unwrap_or_else(|| {
        let p = days_from_civil(posted.0, posted.1, posted.2);
        // Posted in December about an event in January.
        if days_from_civil(posted.0, month, day) < p - 30 {
            posted.0 + 1
        } else {
            posted.0
        }
    });
    Some((days_from_civil(year, month, day) * 1440 + h * 60 + min) * 60_000)
}

fn freq_hz(text: &str) -> Option<u64> {
    let i = text.find("MHz")?;
    let num = text[..i].trim_end().rsplit(' ').next()?;
    let mhz: f64 = num.parse().ok()?;
    (100.0..1000.0).contains(&mhz).then(|| (mhz * 1e6).round() as u64)
}

fn mode(text: &str) -> Option<String> {
    let t = text.to_ascii_lowercase().replace(' ', "");
    ["Robot 36", "Robot 72", "PD 120", "PD 180", "Martin M1", "Scottie 1"]
        .iter()
        .find(|m| t.contains(&m.to_ascii_lowercase().replace(' ', "")))
        .map(|m| m.replace("PD ", "PD"))
}

fn cadence(text: &str) -> Option<String> {
    let t = text.to_ascii_lowercase();
    let on = t.contains("36 seconds");
    let off = ["two minutes off", "2 minutes off", "2 minutes between", "two minutes between"]
        .iter()
        .any(|s| t.contains(s));
    match (on, off) {
        (true, true) => Some("36 s on / 2 min off".into()),
        (false, true) => Some("2 min between pictures".into()),
        _ => None,
    }
}

/// Events on the page that are in progress, still to come, or recently over.
pub fn parse(html: &str, now_ms: i64) -> Vec<ArissEvent> {
    let lines = text_lines(html);
    let from = lines
        .iter()
        .position(|l| l.to_ascii_lowercase().contains("upcoming sstv events") && l.len() < 40)
        .map_or(0, |i| i + 1);
    let lines = &lines[from..];
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&i| posted(&lines[i]).is_some() || lines[i].starts_with("SSTV Series "))
        .collect();
    let mut out: Vec<ArissEvent> = Vec::new();
    for (k, &i) in starts.iter().enumerate() {
        let head = &lines[i];
        let (Some(date), Some(series)) = (posted(head), series_of(head)) else { continue };
        let body = &lines[i + 1..starts.get(k + 1).copied().unwrap_or(lines.len())];
        let find = |keys: &[&str]| {
            body.iter()
                .filter(|l| {
                    let l = l.to_ascii_lowercase();
                    keys.iter().any(|k| l.contains(k))
                })
                .find_map(|l| when(l, date))
        };
        let (Some(start_ms), Some(end_ms)) = (find(&["begin", "start"]), find(&["conclude", "end:"])) else {
            continue;
        };
        if end_ms <= start_ms || end_ms - start_ms > 30 * 86_400_000 || end_ms < now_ms - KEEP_MS {
            continue;
        }
        // A later post about the same series replaces an earlier one (the page is newest first).
        if out.iter().any(|e| e.series == series) {
            continue;
        }
        let title = head.split_once(' ').map_or(head.as_str(), |(_, t)| t).trim().to_string();
        let name = title
            .split('"')
            .nth(1)
            .filter(|n| !n.is_empty())
            .map_or_else(|| format!("Series {series}"), str::to_string);
        let text = body.join(" ");
        out.push(ArissEvent {
            series,
            name: name.chars().take(80).collect(),
            title: title.chars().take(160).collect(),
            start_ms,
            end_ms,
            freq_hz: freq_hz(&text),
            mode: mode(&text),
            cadence: cadence(&text),
        });
    }
    out.sort_by_key(|e| e.start_ms);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As served by ariss.org on 2026-11-09 (trimmed).
    const PAGE: &str = r#"<li><a href="/keps.html">KEPS</a></li>
<h2 class="wsite-content-title" style="text-align:center;"><font size="5">Upcoming SSTV events</font></h2>
<div class="paragraph"><strong>2026-11-05 Expedition 75 Series 35 "Space Memories"</strong><br />Begins Monday, November 9, 2026&nbsp; 17:45 UTC<br />Concludes Monday, November 16, 2026 8:15 UTC<br />Images will be transmitted at 437.550 MHz (+/- Doppler shift) using Robot 36 SSTV mode for 36 seconds transmission, two minutes off.<br />Submit your original decoded images to the SSTV gallery for event certificate.<br /><br /><strong>2026-10-19 Expedition 75 Series 34 "ARISS Hardware"</strong><br />Begins Thursday October 22&nbsp;15:30 UTC<br />Concludes Monday October 26 time 17:45 UTC<br />Images will be transmitted at 437.55 MHz (+/- Doppler shift), decode with Robot 36 mode, transmission 36 seconds on, two minutes off.<br />&nbsp;<br /><br /><strong>2026-09-23 SSTV Expedition 75 Series 33 featuring "Student Education"</strong><br />Begins Friday October 2, 2026 9:00 UTC.<br />Concludes Tuesday October 6,&nbsp; 2026 at 15:55 UTC&nbsp;.&nbsp; &nbsp;<br />Images will be transmitted at 437.550 MHz (+/- Doppler shift), decode with Robot 36 mode.<br />Submit your images to the SSTV gallery for event certificate<strong>.</strong><br />Transmission 36 seconds on&nbsp; 2 minutes off<br /><br /><strong>2026-03-24&nbsp; SSTV Series 31 commemorates several space achievements</strong><br /><span>Start:&nbsp; April 10 (start time TBD)</span><br /><span>End:&nbsp; April 14 (end time TBD)</span><br /><span>Frequency:&nbsp; 437.55 MHz</span><br /><br /><strong>12-04-2025 SSTV Series 30 Update:</strong><br />The RS-38S SSTV retread is scheduled to start at 00:01 UTC December 5.<br /><strong>SSTV Series 30 From Satellite RS-38S</strong><br />Frequency:&nbsp; 437.825 MHz<br /></div>"#;

    const NOV_9_2026: i64 = 1_794_182_400_000; // 2026-11-09 00:00 UTC

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 11, 9) * 86_400_000, NOV_9_2026);
        assert_eq!(days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 28), 2);
    }

    #[test]
    fn parses_the_events_page() {
        let ev = parse(PAGE, NOV_9_2026);
        let series: Vec<u32> = ev.iter().map(|e| e.series).collect();
        // 31 has no times, 30 isn't the ISS; oldest first.
        assert_eq!(series, [33, 34, 35]);
        let e = &ev[2];
        assert_eq!(e.name, "Space Memories");
        assert_eq!(e.title, "Expedition 75 Series 35 \"Space Memories\"");
        assert_eq!(e.start_ms, NOV_9_2026 + (17 * 60 + 45) * 60_000);
        assert_eq!(e.end_ms, NOV_9_2026 + 7 * 86_400_000 + (8 * 60 + 15) * 60_000);
        assert_eq!(e.freq_hz, Some(437_550_000));
        assert_eq!(e.mode.as_deref(), Some("Robot 36"));
        assert_eq!(e.cadence.as_deref(), Some("36 s on / 2 min off"));
        // No year on the line: taken from the date it was posted.
        assert_eq!(ev[1].start_ms, (days_from_civil(2026, 10, 22) * 1440 + 15 * 60 + 30) * 60_000);
        assert_eq!(ev[1].freq_hz, Some(437_550_000));
        assert_eq!(ev[0].cadence.as_deref(), Some("36 s on / 2 min off"));
    }

    #[test]
    fn old_events_are_dropped_and_rubbish_is_ignored() {
        assert!(parse(PAGE, NOV_9_2026 + 200 * 86_400_000).is_empty());
        assert!(parse("<html><body>Service unavailable</body></html>", NOV_9_2026).is_empty());
    }

    #[test]
    fn year_rolls_over() {
        let t = when("Begins January 3 10:00 UTC", (2026, 12, 20)).unwrap();
        assert_eq!(t, (days_from_civil(2027, 1, 3) * 1440 + 600) * 60_000);
    }

    #[test]
    fn finds_the_event_for_a_pass() {
        let st = ArissState { events: parse(PAGE, NOV_9_2026), ..Default::default() };
        let t = NOV_9_2026 + 20 * 3_600_000;
        assert_eq!(st.during(ISS_NORAD, t, t + 600_000).map(|e| e.series), Some(35));
        assert!(st.during(57189, t, t + 600_000).is_none());
        assert!(st.during(ISS_NORAD, NOV_9_2026, NOV_9_2026 + 600_000).is_none());
        assert_eq!(st.current_or_next(NOV_9_2026).map(|e| e.series), Some(35));
    }
}
