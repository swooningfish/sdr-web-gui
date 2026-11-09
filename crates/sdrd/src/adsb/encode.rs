//! Building ADS-B squitters, for the simulated receiver and the tests.

use super::{cpr, crc};

const CHARS: &[u8; 64] = b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

fn with_parity(mut m: [u8; 14]) -> [u8; 14] {
    let p = crc::crc(&m[..11]);
    m[11] = (p >> 16) as u8;
    m[12] = (p >> 8) as u8;
    m[13] = p as u8;
    m
}

fn es(icao: u32, me: u64) -> [u8; 14] {
    let mut m = [0u8; 14];
    m[0] = 17 << 3 | 5; // DF17, CA=5 (airborne)
    m[1] = (icao >> 16) as u8;
    m[2] = (icao >> 8) as u8;
    m[3] = icao as u8;
    for k in 0..7 {
        m[4 + k] = (me >> (48 - 8 * k)) as u8;
    }
    with_parity(m)
}

/// Identification and category, e.g. category "A3".
pub fn ident(icao: u32, callsign: &str, category: &str) -> [u8; 14] {
    let cat = category.as_bytes();
    let tc = match cat.first() {
        Some(b'A') => 4,
        Some(b'B') => 3,
        Some(b'C') => 2,
        _ => 1,
    };
    let ca = cat.get(1).map_or(0, |c| (c - b'0') as u64 & 7);
    let mut me = (tc as u64) << 51 | ca << 48;
    let cs = format!("{callsign:<8}");
    for (k, c) in cs.bytes().take(8).enumerate() {
        let code = CHARS.iter().position(|&x| x == c.to_ascii_uppercase()).unwrap_or(32) as u64;
        me |= code << (42 - 6 * k);
    }
    es(icao, me)
}

/// Airborne position with barometric altitude (25 ft steps).
pub fn position(icao: u32, lat: f64, lon: f64, alt_ft: i32, odd: bool) -> [u8; 14] {
    let (y, x) = cpr::encode(lat, lon, odd, false);
    let n = ((alt_ft + 1000) / 25).clamp(0, 2047) as u64;
    let ac = ((n & 0x7F0) << 1) | 0x10 | (n & 0x0F);
    let me = 11u64 << 51 | ac << 36 | (odd as u64) << 34 | (y as u64) << 17 | x as u64;
    es(icao, me)
}

/// Surface position: ground speed (kt) and track.
pub fn surface(icao: u32, lat: f64, lon: f64, gs_kt: f64, track: f64, odd: bool) -> [u8; 14] {
    let (y, x) = cpr::encode(lat, lon, odd, true);
    let mov: u64 = match gs_kt {
        g if g < 0.125 => 1,
        g if g < 1.0 => 2 + ((g - 0.125) / 0.125) as u64,
        g if g < 2.0 => 9 + ((g - 1.0) / 0.25) as u64,
        g if g < 15.0 => 13 + ((g - 2.0) / 0.5) as u64,
        g if g < 70.0 => 39 + (g - 15.0) as u64,
        g if g < 100.0 => 94 + ((g - 70.0) / 2.0) as u64,
        g if g < 175.0 => 109 + ((g - 100.0) / 5.0) as u64,
        _ => 124,
    };
    let trk = ((track.rem_euclid(360.0) * 128.0 / 360.0).round() as u64) & 0x7F;
    let me = 7u64 << 51 | mov << 44 | 1 << 43 | trk << 36 | (odd as u64) << 34 | (y as u64) << 17 | x as u64;
    let mut m = es(icao, me);
    m[0] = 17 << 3 | 4; // on the ground
    with_parity(m)
}

/// Ground speed, track and vertical rate.
pub fn velocity(icao: u32, gs_kt: f64, track: f64, vrate_fpm: i32) -> [u8; 14] {
    let (ew, ns) = (gs_kt * track.to_radians().sin(), gs_kt * track.to_radians().cos());
    let v = |x: f64| ((x.abs().round() as u64) + 1).min(1023);
    let vr = ((vrate_fpm.unsigned_abs() as u64 + 32) / 64 + 1).min(511);
    let me = 19u64 << 51
        | 1 << 48
        | ((ew < 0.0) as u64) << 42
        | v(ew) << 32
        | ((ns < 0.0) as u64) << 31
        | v(ns) << 21
        | 1 << 20 // baro vertical rate
        | ((vrate_fpm < 0) as u64) << 19
        | vr << 10;
    es(icao, me)
}

/// Aircraft status (TC28): squawk, no emergency. `squawk` is four octal digits.
pub fn status(icao: u32, squawk: &str) -> [u8; 14] {
    let hex = u32::from_str_radix(squawk, 16).unwrap_or(0);
    // 0xABCD layout → ID13 bit order (the inverse of the decoder's mapping).
    let mut id = 0u64;
    for (to, from) in [
        (0x1000, 0x0010),
        (0x0800, 0x1000),
        (0x0400, 0x0020),
        (0x0200, 0x2000),
        (0x0100, 0x0040),
        (0x0080, 0x4000),
        (0x0020, 0x0100),
        (0x0010, 0x0001),
        (0x0008, 0x0200),
        (0x0004, 0x0002),
        (0x0002, 0x0400),
        (0x0001, 0x0004),
    ] {
        if hex & from != 0 {
            id |= to;
        }
    }
    let me = 28u64 << 51 | 1 << 48 | id << 32;
    es(icao, me)
}

/// DF11 all-call reply (address announced, interrogator 0).
pub fn all_call(icao: u32) -> [u8; 7] {
    let mut m = [11 << 3 | 5, (icao >> 16) as u8, (icao >> 8) as u8, icao as u8, 0, 0, 0];
    let p = crc::crc(&m[..4]);
    m[4] = (p >> 16) as u8;
    m[5] = (p >> 8) as u8;
    m[6] = p as u8;
    m
}

/// Pulse on-intervals (µs from the start) for a message: preamble pulses at
/// 0, 1, 3.5 and 4.5 µs, then pulse-position bits of 1 µs from 8 µs.
pub fn pulses(msg: &[u8]) -> Vec<(f64, f64)> {
    let mut out = vec![(0.0, 0.5), (1.0, 1.5), (3.5, 4.0), (4.5, 5.0)];
    for k in 0..msg.len() * 8 {
        let one = msg[k / 8] & (0x80 >> (k % 8)) != 0;
        let t = 8.0 + k as f64 + if one { 0.0 } else { 0.5 };
        // Merge with the previous pulse when they touch (a 1 after a 0).
        match out.last_mut() {
            Some(last) if (last.1 - t).abs() < 1e-9 => last.1 = t + 0.5,
            _ => out.push((t, t + 0.5)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::message::{decode, Cpr};
    use super::*;

    #[test]
    fn round_trip() {
        let id = ident(0x40ABCD, "SIM123", "A3");
        assert_eq!(crc::residual(&id), 0);
        let d = decode(&id, 0x40ABCD);
        assert_eq!(d.callsign.as_deref(), Some("SIM123"));
        assert_eq!(d.category.as_deref(), Some("A3"));

        let p = position(0x40ABCD, 51.47, -0.45, 36000, true);
        assert_eq!(crc::residual(&p), 0);
        let d = decode(&p, 0x40ABCD);
        assert_eq!(d.alt_baro_ft, Some(36000));
        assert!(matches!(d.cpr, Some(Cpr { odd: true, surface: false, .. })));

        let v = velocity(0x40ABCD, 420.0, 290.0, -1500);
        let d = decode(&v, 0x40ABCD).velocity.unwrap();
        assert!((d.gs_kt.unwrap() - 420.0).abs() < 1.5, "{d:?}");
        assert!((d.track_deg.unwrap() - 290.0).abs() < 0.5, "{d:?}");
        assert!((d.vrate_fpm.unwrap() + 1500).abs() <= 64, "{d:?}");

        let s = surface(0x40ABCD, 51.47, -0.45, 12.0, 90.0, false);
        let d = decode(&s, 0x40ABCD);
        assert_eq!(d.on_ground, Some(true));
        let (gs, trk) = d.surface.unwrap();
        assert!((gs.unwrap() - 12.0).abs() < 0.6 && (trk.unwrap() - 90.0).abs() < 3.0, "{gs:?} {trk:?}");

        assert_eq!(crc::residual(&all_call(0x40ABCD)), 0);
        for sq in ["7700", "1200", "4417", "0020"] {
            assert_eq!(decode(&status(0x40ABCD, sq), 0x40ABCD).squawk.as_deref(), Some(sq));
        }
    }
}
