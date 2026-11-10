//! APRS information-field parsing (APRS 1.0.1 spec + common extensions) into
//! the JSON event data used throughout sdrd.
//!
//! Event fields: `model` ("APRS", "APRS object" or "APRS item") and `id` (the
//! station or object name) identify the device; plus `from`, `to`, `path`,
//! `packet` (kind), `raw` (TNC2), and when present `lat`, `lon`, `symbol`,
//! `course_deg`, `speed_kmh`, `altitude_m`, `comment`, `status`, `message`,
//! and weather fields in the same names rtl_433 uses (`temperature_C`, ...).

use serde_json::{json, Map, Value};

use super::ax25::{printable, Frame};

const KNOT_KMH: f64 = 1.852;
const MPH_KMH: f64 = 1.609_344;

pub fn parse(f: &Frame) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("model".into(), json!("APRS"));
    m.insert("id".into(), json!(f.src));
    m.insert("from".into(), json!(f.src));
    m.insert("to".into(), json!(f.dst));
    m.insert("path".into(), json!(f.path.iter().map(|(c, h)| if *h { format!("{c}*") } else { c.clone() }).collect::<Vec<_>>()));
    m.insert("raw".into(), json!(f.tnc2()));

    let info = &f.info[..];
    let kind = match info.first() {
        Some(b'!') | Some(b'=') => position(&info[1..], &mut m).map(|_| "position"),
        Some(b'/') | Some(b'@') => info.get(8..).and_then(|p| position(p, &mut m)).map(|_| "position"),
        Some(b';') => object(info, &mut m),
        Some(b')') => item(info, &mut m),
        Some(b'`') | Some(b'\'') | Some(0x1c) | Some(0x1d) => mic_e(&f.dst, info, &mut m).map(|_| "mic-e"),
        Some(b'>') => {
            put_text(&mut m, "status", &info[1..]);
            Some("status")
        }
        Some(b':') => message(info, &mut m),
        Some(b'_') => {
            // Positionless weather: MDHM timestamp, then data.
            let rest = info.get(9..).map(printable).unwrap_or_default();
            let rest = weather(&rest, None, &mut m);
            put_comment(&mut m, &rest);
            Some("weather")
        }
        Some(b'T') => Some("telemetry"),
        Some(b'$') => Some("nmea"),
        _ => None,
    };
    m.insert("packet".into(), json!(kind.unwrap_or("other")));
    m
}

fn put_text(m: &mut Map<String, Value>, key: &str, b: &[u8]) {
    let s = printable(b).trim().to_string();
    if !s.is_empty() {
        m.insert(key.into(), json!(s));
    }
}

fn put_comment(m: &mut Map<String, Value>, s: &str) {
    let s = s.trim();
    if !s.is_empty() {
        m.insert("comment".into(), json!(s));
    }
}

fn round(v: f64, places: i32) -> f64 {
    let p = 10f64.powi(places);
    (v * p).round() / p
}

fn num(s: &str) -> Option<f64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit() || c == b'.' || c == b'-') {
        return None;
    }
    s.parse().ok()
}

/// Position (uncompressed or compressed) followed by extensions and comment.
fn position(p: &[u8], m: &mut Map<String, Value>) -> Option<()> {
    let first = *p.first()?;
    if first.is_ascii_digit() || first == b' ' {
        uncompressed(p, m)
    } else {
        compressed(p, m)
    }
}

fn set_pos(m: &mut Map<String, Value>, lat: f64, lon: f64, table: u8, code: u8) -> Option<()> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    m.insert("lat".into(), json!(round(lat, 5)));
    m.insert("lon".into(), json!(round(lon, 5)));
    m.insert("symbol".into(), json!(format!("{}{}", table as char, code as char)));
    Some(())
}

/// `DDMM.mmN` / `DDDMM.mmW`, ambiguity spaces read as zeros.
fn dm(s: &[u8], deg_len: usize, pos: u8, neg: u8) -> Option<f64> {
    // Packets come off the air: a non-ASCII byte would become a multi-byte
    // char and the byte-indexed splits below would panic.
    if !s.is_ascii() || s.len() <= deg_len {
        return None;
    }
    let txt: String = s.iter().map(|&c| if c == b' ' { '0' } else { c as char }).collect();
    let (d, rest) = txt.split_at(deg_len);
    let deg: f64 = d.parse().ok()?;
    let min: f64 = rest[..rest.len() - 1].parse().ok()?;
    if min >= 60.0 {
        return None;
    }
    let v = deg + min / 60.0;
    match *s.last()? {
        c if c == pos => Some(v),
        c if c == neg => Some(-v),
        _ => None,
    }
}

fn uncompressed(p: &[u8], m: &mut Map<String, Value>) -> Option<()> {
    if p.len() < 19 || p[4] != b'.' || p[14] != b'.' {
        return None;
    }
    let lat = dm(&p[0..8], 2, b'N', b'S')?;
    let lon = dm(&p[9..18], 3, b'E', b'W')?;
    set_pos(m, lat, lon, p[8], p[18])?;
    let mut rest = printable(&p[19..]);
    // Course/speed "CSE/SPD" (for weather stations: wind direction/speed).
    let mut cse_spd = None;
    if rest.len() >= 7 && rest.as_bytes()[3] == b'/' {
        if let (Some(c), Some(s)) = (num(&rest[0..3]), num(&rest[4..7])) {
            cse_spd = Some((c, s));
            rest = rest[7..].to_string();
        }
    }
    if p[18] == b'_' {
        rest = weather(&rest, cse_spd, m);
    } else if let Some((c, s)) = cse_spd {
        if c > 0.0 && c <= 360.0 {
            m.insert("course_deg".into(), json!(c));
        }
        m.insert("speed_kmh".into(), json!(round(s * KNOT_KMH, 1)));
    }
    rest = altitude(&rest, m);
    put_comment(m, &rest);
    Some(())
}

fn b91(s: &[u8]) -> Option<f64> {
    s.iter().try_fold(0f64, |acc, &c| (33..=124).contains(&c).then(|| acc * 91.0 + (c - 33) as f64))
}

fn compressed(p: &[u8], m: &mut Map<String, Value>) -> Option<()> {
    if p.len() < 13 {
        return None;
    }
    let table = p[0];
    if !(table == b'/' || table == b'\\' || table.is_ascii_uppercase() || (b'a'..=b'j').contains(&table)) {
        return None;
    }
    let lat = 90.0 - b91(&p[1..5])? / 380_926.0;
    let lon = -180.0 + b91(&p[5..9])? / 190_463.0;
    set_pos(m, lat, lon, table, p[9])?;
    let (c, s, t) = (p[10], p[11], p[12]);
    if c != b' ' {
        let t = t.wrapping_sub(33);
        if (t >> 3) & 3 == 2 {
            // cs carries altitude in feet.
            let feet = 1.002f64.powf(((c - 33) as f64) * 91.0 + (s - 33) as f64);
            m.insert("altitude_m".into(), json!(round(feet * 0.3048, 0)));
        } else if (b'!'..=b'z').contains(&c) {
            let course = (c - 33) as f64 * 4.0;
            if course > 0.0 {
                m.insert("course_deg".into(), json!(course));
            }
            let knots = 1.08f64.powi((s as i32) - 33) - 1.0;
            m.insert("speed_kmh".into(), json!(round(knots * KNOT_KMH, 1)));
        }
    }
    let mut rest = printable(&p[13..]);
    if p[9] == b'_' {
        rest = weather(&rest, None, m);
    }
    rest = altitude(&rest, m);
    put_comment(m, &rest);
    Some(())
}

/// Pull "/A=001234" (feet) out of a comment.
fn altitude(s: &str, m: &mut Map<String, Value>) -> String {
    if let Some(i) = s.find("/A=") {
        if let Some(feet) = s.get(i + 3..i + 9).and_then(num) {
            m.insert("altitude_m".into(), json!(round(feet * 0.3048, 0)));
            return format!("{}{}", &s[..i], &s[i + 9..]);
        }
    }
    s.to_string()
}

/// Weather report fields (`gGGG tTTT rRRR pPPP PPPP hHH bBBBBB ...`) after an
/// optional wind direction/speed. Returns whatever text is left over.
fn weather(s: &str, wind: Option<(f64, f64)>, m: &mut Map<String, Value>) -> String {
    let mut s = s;
    let mut wind = wind;
    // Positionless reports start with cDDDsSSS instead of DDD/SSS.
    if wind.is_none() && s.len() >= 8 && s.starts_with('c') && s.as_bytes()[4] == b's' {
        wind = num(&s[1..4]).zip(num(&s[5..8]));
        s = &s[8..];
    }
    if let Some((dir, spd)) = wind {
        m.insert("wind_dir_deg".into(), json!(dir));
        m.insert("wind_avg_km_h".into(), json!(round(spd * MPH_KMH, 1)));
    }
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let width = match b[i] {
            b'g' | b't' | b'r' | b'p' | b'P' | b'L' | b'l' => 3,
            b'h' => 2,
            b'b' => 5,
            _ => break,
        };
        let Some(v) = s.get(i + 1..i + 1 + width) else { break };
        let key = b[i];
        i += 1 + width;
        let Some(v) = num(v) else { continue }; // "..." = no data
        let (k, val) = match key {
            b'g' => ("wind_max_km_h", round(v * MPH_KMH, 1)),
            b't' => ("temperature_C", round((v - 32.0) * 5.0 / 9.0, 1)),
            b'r' => ("rain_1h_mm", round(v * 0.254, 1)),
            b'p' => ("rain_24h_mm", round(v * 0.254, 1)),
            b'P' => ("rain_mm", round(v * 0.254, 1)),
            b'h' => ("humidity", if v == 0.0 { 100.0 } else { v }),
            b'b' => ("pressure_hPa", round(v / 10.0, 1)),
            b'L' => ("light_wm2", v),
            _ => ("light_wm2", v + 1000.0),
        };
        m.insert(k.into(), json!(val));
    }
    s[i..].to_string()
}

fn object(info: &[u8], m: &mut Map<String, Value>) -> Option<&'static str> {
    if info.len() < 18 {
        return None;
    }
    let name = printable(&info[1..10]).trim().to_string();
    m.insert("model".into(), json!("APRS object"));
    m.insert("id".into(), json!(name));
    m.insert("killed".into(), json!(info[10] == b'_'));
    position(&info[18..], m).map(|_| "object")
}

fn item(info: &[u8], m: &mut Map<String, Value>) -> Option<&'static str> {
    let end = info.iter().take(11).position(|&c| c == b'!' || c == b'_')?;
    if end < 4 {
        return None;
    }
    m.insert("model".into(), json!("APRS item"));
    m.insert("id".into(), json!(printable(&info[1..end]).trim()));
    m.insert("killed".into(), json!(info[end] == b'_'));
    position(&info[end + 1..], m).map(|_| "item")
}

fn message(info: &[u8], m: &mut Map<String, Value>) -> Option<&'static str> {
    if info.len() < 11 || info[10] != b':' {
        return None;
    }
    let to = printable(&info[1..10]).trim().to_string();
    let body = printable(&info[11..]);
    let (text, id) = match body.rsplit_once('{') {
        Some((t, id)) if id.len() <= 5 => (t.to_string(), Some(id.trim_end_matches('}').to_string())),
        _ => (body.clone(), None),
    };
    let kind = if to.starts_with("BLN") {
        "bulletin"
    } else if text.starts_with("ack") || text.starts_with("rej") {
        "ack"
    } else {
        "message"
    };
    m.insert("message".into(), json!({ "to": to, "text": text, "id": id }));
    Some(kind)
}

/// Mic-E: latitude and status in the destination callsign, longitude, speed
/// and course packed into the info field.
fn mic_e(dst: &str, info: &[u8], m: &mut Map<String, Value>) -> Option<()> {
    let d: Vec<u8> = dst.split('-').next()?.bytes().collect();
    if d.len() != 6 || info.len() < 9 {
        return None;
    }
    let mut digits = [0u8; 6];
    let (mut std_bits, mut custom_bits) = (0u8, 0u8);
    for (i, &c) in d.iter().enumerate() {
        digits[i] = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'J' => c - b'A',
            b'P'..=b'Y' => c - b'P',
            b'K' | b'L' | b'Z' => 0, // ambiguity
            _ => return None,
        };
        if i < 3 {
            if (b'P'..=b'Z').contains(&c) {
                std_bits |= 4 >> i;
            } else if (b'A'..=b'K').contains(&c) {
                custom_bits |= 4 >> i;
            }
        }
    }
    let north = (b'P'..=b'Z').contains(&d[3]);
    let lon_offset = (b'P'..=b'Z').contains(&d[4]);
    let west = (b'P'..=b'Z').contains(&d[5]);
    let lat = (digits[0] * 10 + digits[1]) as f64
        + ((digits[2] * 10 + digits[3]) as f64 + (digits[4] * 10 + digits[5]) as f64 / 100.0) / 60.0;
    let lat = if north { lat } else { -lat };

    let v = |i: usize| info[i] as i32 - 28;
    let mut deg = v(1) + if lon_offset { 100 } else { 0 };
    if (180..=189).contains(&deg) {
        deg -= 80;
    } else if (190..=199).contains(&deg) {
        deg -= 190;
    }
    let mut min = v(2);
    if min >= 60 {
        min -= 60;
    }
    let lon = deg as f64 + (min as f64 + v(3) as f64 / 100.0) / 60.0;
    let lon = if west { -lon } else { lon };
    set_pos(m, lat, lon, info[8], info[7])?;

    let mut speed = v(4) * 10 + v(5) / 10;
    if speed >= 800 {
        speed -= 800;
    }
    let mut course = (v(5) % 10) * 100 + v(6);
    if course >= 400 {
        course -= 400;
    }
    if (1..=360).contains(&course) {
        m.insert("course_deg".into(), json!(course));
    }
    if (0..800).contains(&speed) {
        m.insert("speed_kmh".into(), json!(round(speed as f64 * KNOT_KMH, 1)));
    }

    const STATUS: [&str; 8] = [
        "Emergency", "Priority", "Special", "Committed", "Returning", "In Service", "En Route", "Off Duty",
    ];
    let status = if custom_bits != 0 {
        format!("Custom-{custom_bits}")
    } else {
        STATUS[std_bits as usize].to_string()
    };
    m.insert("mic_e_status".into(), json!(status));

    let mut rest = &info[9..];
    // Optional altitude: three base-91 chars then '}' (metres above -10 km).
    let alt_at = if rest.get(3) == Some(&b'}') { Some(0) } else if rest.get(4) == Some(&b'}') { Some(1) } else { None };
    if let Some(a) = alt_at {
        if let Some(v) = b91(&rest[a..a + 3]) {
            m.insert("altitude_m".into(), json!(v - 10_000.0));
            rest = &rest[a + 4..];
        }
    }
    let text = printable(rest);
    let text = altitude(&text, m);
    put_comment(m, &text);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aprs::ax25::Frame;

    fn frame(src: &str, dst: &str, info: &[u8]) -> Frame {
        Frame {
            src: src.into(),
            dst: dst.into(),
            path: vec![("WIDE2-1".into(), true)],
            info: info.to_vec(),
        }
    }

    fn f(m: &Map<String, Value>, k: &str) -> f64 {
        m.get(k).and_then(Value::as_f64).unwrap_or_else(|| panic!("{k} missing in {m:?}"))
    }

    #[test]
    fn non_ascii_coordinates_are_ignored_not_fatal() {
        // A high byte in either coordinate used to panic in `dm`.
        for info in [
            &b"!1\xff30.50N/00007.25W>"[..],
            b"!5130.5\xffN/00007.25W>",
            b"!5130.50N/0\xff007.25W>",
            b"!5130.50N/00007.2\xffW>",
        ] {
            let m = parse(&frame("M0ABC-9", "APRS", info));
            assert_eq!(m["packet"], "other");
            assert!(m.get("lat").is_none());
        }
    }

    #[test]
    fn uncompressed_with_course_speed_altitude() {
        let m = parse(&frame("M0ABC-9", "APRS", b"!5130.50N/00007.25W>088/036/A=001234 Hello"));
        assert_eq!(m["packet"], "position");
        assert!((f(&m, "lat") - 51.50833).abs() < 1e-4);
        assert!((f(&m, "lon") + 0.12083).abs() < 1e-4);
        assert_eq!(m["symbol"], "/>");
        assert_eq!(f(&m, "course_deg"), 88.0);
        assert!((f(&m, "speed_kmh") - 66.7).abs() < 0.1);
        assert_eq!(f(&m, "altitude_m"), 376.0);
        assert_eq!(m["comment"], "Hello");
        assert_eq!(m["raw"], "M0ABC-9>APRS,WIDE2-1*:!5130.50N/00007.25W>088/036/A=001234 Hello");
    }

    #[test]
    fn timestamped_position_and_weather() {
        let m = parse(&frame(
            "G0WX",
            "APRS",
            b"@092345z4903.50N/07201.75W_220/004g005t077r000p000P000h50b09900wRSW",
        ));
        assert!((f(&m, "lat") - 49.05833).abs() < 1e-4);
        assert_eq!(f(&m, "wind_dir_deg"), 220.0);
        assert!((f(&m, "wind_avg_km_h") - 6.4).abs() < 0.1);
        assert_eq!(f(&m, "temperature_C"), 25.0);
        assert_eq!(f(&m, "humidity"), 50.0);
        assert_eq!(f(&m, "pressure_hPa"), 990.0);
        assert_eq!(m["comment"], "wRSW");
        assert!(m.get("speed_kmh").is_none());
    }

    #[test]
    fn compressed_position() {
        // Example from the APRS spec: 49°30' N, 72°45' W.
        let m = parse(&frame("N0CALL", "APRS", b"=/5L!!<*e7>7P[ test"));
        assert!((f(&m, "lat") - 49.5).abs() < 1e-3, "{m:?}");
        assert!((f(&m, "lon") + 72.75).abs() < 1e-3);
        assert_eq!(m["symbol"], "/>");
        assert_eq!(f(&m, "course_deg"), 88.0);
        assert!((f(&m, "speed_kmh") - 67.0).abs() < 1.0);
        assert_eq!(m["comment"], "test");
    }

    #[test]
    fn mic_e() {
        // Destination S32UVT: 33°25.64' N, +100° longitude offset, west,
        // message bits 1-0-0 ("Returning"). Info from the spec's example.
        let m = parse(&frame("M0XYZ-7", "S32UVT", b"`(_fn\"Oj/>Comment"));
        assert_eq!(m["packet"], "mic-e");
        assert!((f(&m, "lat") - 33.42733).abs() < 1e-4, "{m:?}");
        assert!((f(&m, "lon") + 112.129).abs() < 1e-4, "{m:?}");
        assert_eq!(m["symbol"], "/j");
        assert_eq!(m["mic_e_status"], "Returning");
        assert_eq!(f(&m, "speed_kmh"), round(20.0 * KNOT_KMH, 1));
        assert_eq!(f(&m, "course_deg"), 251.0);
    }

    #[test]
    fn object_message_status() {
        let o = parse(&frame("M0ABC", "APRS", b";LEADER   *092345z4903.50N/07201.75W>088/036"));
        assert_eq!(o["packet"], "object");
        assert_eq!(o["model"], "APRS object");
        assert_eq!(o["id"], "LEADER");
        let msg = parse(&frame("M0ABC", "APRS", b":G4XYZ-7  :Hello there{42"));
        assert_eq!(msg["packet"], "message");
        assert_eq!(msg["message"]["to"], "G4XYZ-7");
        assert_eq!(msg["message"]["text"], "Hello there");
        assert_eq!(msg["message"]["id"], "42");
        let st = parse(&frame("M0ABC", "APRS", b">On the air"));
        assert_eq!(st["packet"], "status");
        assert_eq!(st["status"], "On the air");
        let other = parse(&frame("M0ABC", "APRS", b"garbage"));
        assert_eq!(other["packet"], "other");
    }
}
