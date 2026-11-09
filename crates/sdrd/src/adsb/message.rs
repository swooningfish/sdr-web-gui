//! Decoding Mode S replies and ADS-B extended squitters into fields.
//!
//! Formats handled: DF0/16 (altitude, air–air), DF4/20 (altitude), DF5/21
//! (squawk), DF11 (all-call: address), DF17 and DF18 with CF=0 (ADS-B:
//! identification and category, airborne and surface position, velocity,
//! emergency/squawk).

/// One received message.
#[derive(Clone, Debug)]
pub struct Frame {
    /// 7 or 14 bytes.
    pub bytes: Vec<u8>,
    /// Signal level, dBFS.
    pub rssi: f32,
    /// A bit was flipped back by error correction.
    pub corrected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cpr {
    pub odd: bool,
    pub lat: u32,
    pub lon: u32,
    pub surface: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Velocity {
    /// Ground speed (kt) and track (° true), or air speed and heading.
    pub gs_kt: Option<f32>,
    pub track_deg: Option<f32>,
    pub airspeed_kt: Option<u32>,
    /// True airspeed (else indicated).
    pub tas: bool,
    pub heading_deg: Option<f32>,
    /// ft/min, positive climbing.
    pub vrate_fpm: Option<i32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decoded {
    pub df: u8,
    pub icao: u32,
    pub callsign: Option<String>,
    /// Emitter category, e.g. "A3".
    pub category: Option<String>,
    pub alt_baro_ft: Option<i32>,
    pub alt_gnss_ft: Option<i32>,
    pub on_ground: Option<bool>,
    /// Four octal digits, e.g. "7700".
    pub squawk: Option<String>,
    pub cpr: Option<Cpr>,
    pub velocity: Option<Velocity>,
    /// Surface movement: ground speed (kt) and track.
    pub surface: Option<(Option<f32>, Option<f32>)>,
    /// Emergency / priority status from TC28 (0 = none).
    pub emergency: Option<u8>,
}

pub fn df(bytes: &[u8]) -> u8 {
    bytes[0] >> 3
}

/// Message length in bytes for a downlink format.
pub fn len_for(df: u8) -> usize {
    if df >= 16 {
        14
    } else {
        7
    }
}

/// `n` bits of the 56-bit ME field starting at 1-based bit `start`.
fn me_bits(me: u64, start: u32, n: u32) -> u32 {
    ((me >> (56 - (start + n - 1))) & ((1u64 << n) - 1)) as u32
}

/// Mode A code bits (ID13 layout) → the 0xABCD nibble layout.
fn id13_to_hex(id: u32) -> u32 {
    let mut h = 0;
    for (from, to) in [
        (0x1000, 0x0010), // C1
        (0x0800, 0x1000), // A1
        (0x0400, 0x0020), // C2
        (0x0200, 0x2000), // A2
        (0x0100, 0x0040), // C4
        (0x0080, 0x4000), // A4
        (0x0020, 0x0100), // B1
        (0x0010, 0x0001), // D1
        (0x0008, 0x0200), // B2
        (0x0004, 0x0002), // D2
        (0x0002, 0x0400), // B4
        (0x0001, 0x0004), // D4
    ] {
        if id & from != 0 {
            h |= to;
        }
    }
    h
}

/// Gillham (Mode C) code in the 0xABCD layout → altitude in hundreds of feet.
fn gillham_to_100ft(a: u32) -> Option<i32> {
    if a & 0xFFFF_8889 != 0 || a & 0x00F0 == 0 {
        return None;
    }
    let mut hundreds = 0u32;
    if a & 0x0010 != 0 {
        hundreds ^= 7;
    }
    if a & 0x0020 != 0 {
        hundreds ^= 3;
    }
    if a & 0x0040 != 0 {
        hundreds ^= 1;
    }
    if hundreds & 5 == 5 {
        hundreds ^= 2;
    }
    if hundreds > 5 {
        return None;
    }
    let mut fives = 0u32;
    for (bit, x) in [
        (0x0002, 0xFF),
        (0x0004, 0x7F),
        (0x1000, 0x3F),
        (0x2000, 0x1F),
        (0x4000, 0x0F),
        (0x0100, 0x07),
        (0x0200, 0x03),
        (0x0400, 0x01),
    ] {
        if a & bit != 0 {
            fives ^= x;
        }
    }
    if fives & 1 != 0 {
        hundreds = 6 - hundreds;
    }
    Some(fives as i32 * 5 + hundreds as i32 - 13)
}

/// 13-bit altitude field of DF0/4/16/20 (feet).
fn ac13(f: u32) -> Option<i32> {
    if f == 0 || f & 0x40 != 0 {
        // Unknown, or metric (not used in practice).
        return None;
    }
    if f & 0x10 != 0 {
        let n = ((f & 0x1F80) >> 2) | ((f & 0x20) >> 1) | (f & 0x0F);
        Some(n as i32 * 25 - 1000)
    } else {
        gillham_to_100ft(id13_to_hex(f)).filter(|n| *n >= -12).map(|n| n * 100)
    }
}

/// 12-bit altitude field of an airborne position (feet).
pub fn ac12(f: u32) -> Option<i32> {
    if f == 0 {
        return None;
    }
    if f & 0x10 != 0 {
        let n = ((f & 0x0FE0) >> 1) | (f & 0x0F);
        Some(n as i32 * 25 - 1000)
    } else {
        let n = ((f & 0x0FC0) << 1) | (f & 0x3F);
        gillham_to_100ft(id13_to_hex(n)).filter(|n| *n >= -12).map(|n| n * 100)
    }
}

fn squawk(id13: u32) -> String {
    format!("{:04x}", id13_to_hex(id13))
}

const CHARS: &[u8; 64] = b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

/// Ground speed (kt) for a surface movement code.
fn movement_kt(m: u32) -> Option<f32> {
    let m = m as f32;
    Some(match m as u32 {
        0 | 125..=127 => return None,
        1 => 0.0,
        2..=8 => 0.125 + (m - 2.0) * 0.125,
        9..=12 => 1.0 + (m - 9.0) * 0.25,
        13..=38 => 2.0 + (m - 13.0) * 0.5,
        39..=93 => 15.0 + (m - 39.0),
        94..=108 => 70.0 + (m - 94.0) * 2.0,
        109..=123 => 100.0 + (m - 109.0) * 5.0,
        _ => 175.0,
    })
}

/// Decode a frame whose parity has already been checked. `icao` is the
/// address: from the message (DF11/17/18) or from the parity (others).
pub fn decode(bytes: &[u8], icao: u32) -> Decoded {
    let df = df(bytes);
    let mut d = Decoded { df, icao, ..Default::default() };
    let field13 = ((bytes[2] as u32 & 0x1F) << 8) | bytes[3] as u32;
    match df {
        0 | 16 => {
            d.on_ground = Some(bytes[0] & 0x04 != 0);
            d.alt_baro_ft = ac13(field13);
        }
        4 | 20 => {
            d.on_ground = fs_ground(bytes[0] & 7);
            d.alt_baro_ft = ac13(field13);
        }
        5 | 21 => {
            d.on_ground = fs_ground(bytes[0] & 7);
            d.squawk = Some(squawk(field13));
        }
        11 => d.on_ground = ca_ground(bytes[0] & 7),
        17 | 18 => {
            if df == 17 {
                d.on_ground = ca_ground(bytes[0] & 7);
            }
            let me = bytes[4..11].iter().fold(0u64, |a, &b| a << 8 | b as u64);
            extended_squitter(&mut d, me);
        }
        _ => {}
    }
    d
}

/// Flight status: 1 and 3 are on the ground, 0 and 2 airborne.
fn fs_ground(fs: u8) -> Option<bool> {
    match fs {
        0 | 2 => Some(false),
        1 | 3 => Some(true),
        _ => None,
    }
}

/// Capability: 4 on the ground, 5 airborne, others either.
fn ca_ground(ca: u8) -> Option<bool> {
    match ca {
        4 => Some(true),
        5 => Some(false),
        _ => None,
    }
}

fn extended_squitter(d: &mut Decoded, me: u64) {
    let tc = me_bits(me, 1, 5);
    match tc {
        1..=4 => {
            let set = b"DCBA"[tc as usize - 1];
            let ca = me_bits(me, 6, 3);
            if ca != 0 {
                d.category = Some(format!("{}{}", set as char, ca));
            }
            let s: String = (0..8).map(|k| CHARS[me_bits(me, 9 + 6 * k, 6) as usize] as char).collect();
            let s = s.trim_end().to_string();
            if !s.is_empty() && !s.contains('#') {
                d.callsign = Some(s);
            }
        }
        5..=8 => {
            d.on_ground = Some(true);
            let gs = movement_kt(me_bits(me, 6, 7));
            let trk = (me_bits(me, 13, 1) == 1).then(|| me_bits(me, 14, 7) as f32 * 360.0 / 128.0);
            d.surface = Some((gs, trk));
            d.cpr = Some(Cpr {
                odd: me_bits(me, 22, 1) == 1,
                lat: me_bits(me, 23, 17),
                lon: me_bits(me, 40, 17),
                surface: true,
            });
        }
        9..=18 | 20..=22 => {
            let alt = me_bits(me, 9, 12);
            if tc <= 18 {
                d.alt_baro_ft = ac12(alt);
            } else if alt != 0 {
                // GNSS height, metres.
                d.alt_gnss_ft = Some((alt as f32 * 3.28084).round() as i32);
            }
            if d.on_ground.is_none() {
                d.on_ground = Some(false);
            }
            let (lat, lon) = (me_bits(me, 23, 17), me_bits(me, 40, 17));
            // All-zero lat/lon means no position (only altitude).
            if lat != 0 || lon != 0 {
                d.cpr = Some(Cpr { odd: me_bits(me, 22, 1) == 1, lat, lon, surface: false });
            }
        }
        19 => {
            let st = me_bits(me, 6, 3);
            let mut v = Velocity::default();
            let vr = me_bits(me, 38, 9);
            if vr != 0 {
                let r = (vr as i32 - 1) * 64;
                v.vrate_fpm = Some(if me_bits(me, 37, 1) == 1 { -r } else { r });
            }
            match st {
                1 | 2 => {
                    let (vew, vns) = (me_bits(me, 15, 10), me_bits(me, 26, 10));
                    if vew != 0 && vns != 0 {
                        let k = if st == 2 { 4.0 } else { 1.0 };
                        let ew = (vew as f32 - 1.0) * k * if me_bits(me, 14, 1) == 1 { -1.0 } else { 1.0 };
                        let ns = (vns as f32 - 1.0) * k * if me_bits(me, 25, 1) == 1 { -1.0 } else { 1.0 };
                        v.gs_kt = Some((ew * ew + ns * ns).sqrt());
                        v.track_deg = Some(ew.atan2(ns).to_degrees().rem_euclid(360.0));
                    }
                }
                3 | 4 => {
                    if me_bits(me, 14, 1) == 1 {
                        v.heading_deg = Some(me_bits(me, 15, 10) as f32 * 360.0 / 1024.0);
                    }
                    let a = me_bits(me, 26, 10);
                    if a != 0 {
                        v.airspeed_kt = Some((a - 1) * if st == 4 { 4 } else { 1 });
                        v.tas = me_bits(me, 25, 1) == 1;
                    }
                }
                _ => return,
            }
            d.velocity = Some(v);
        }
        28 if me_bits(me, 6, 3) == 1 => {
            d.emergency = Some(me_bits(me, 9, 3) as u8);
            let id = me_bits(me, 12, 13);
            if id != 0 {
                d.squawk = Some(squawk(id));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn es(s: &str) -> Decoded {
        let b = hex(s);
        let icao = (b[1] as u32) << 16 | (b[2] as u32) << 8 | b[3] as u32;
        decode(&b, icao)
    }

    #[test]
    fn identification() {
        let d = es("8D4840D6202CC371C32CE0576098");
        assert_eq!(d.icao, 0x4840D6);
        assert_eq!(d.callsign.as_deref(), Some("KLM1023"));
        assert_eq!(d.category, None);
    }

    #[test]
    fn airborne_position() {
        let d = es("8D40621D58C382D690C8AC2863A7");
        assert_eq!(d.alt_baro_ft, Some(38000));
        assert_eq!(d.cpr, Some(Cpr { odd: false, lat: 93000, lon: 51372, surface: false }));
        let d = es("8D40621D58C386435CC412692AD6");
        assert!(d.cpr.unwrap().odd);
        assert_eq!(d.cpr.unwrap().lat, 74158);
        assert_eq!(d.cpr.unwrap().lon, 50194);
    }

    #[test]
    fn velocities() {
        // Ground speed: 159 kt, track 182.88°, −832 ft/min.
        let v = es("8D485020994409940838175B284F").velocity.unwrap();
        assert!((v.gs_kt.unwrap() - 159.2).abs() < 0.1, "{v:?}");
        assert!((v.track_deg.unwrap() - 182.88).abs() < 0.05, "{v:?}");
        assert_eq!(v.vrate_fpm, Some(-832));
        // Air speed: 375 kt TAS, heading 243.98°, −2304 ft/min.
        let v = es("8DA05F219B06B6AF189400CBC33F").velocity.unwrap();
        assert_eq!(v.airspeed_kt, Some(375));
        assert!(v.tas);
        assert!((v.heading_deg.unwrap() - 243.98).abs() < 0.05, "{v:?}");
        assert_eq!(v.vrate_fpm, Some(-2304));
    }

    #[test]
    fn altitude_codes() {
        // 25 ft steps (Q=1).
        assert_eq!(ac12(0xC38), Some(38000));
        // DF4 with 25 ft coding: 0x0C38-style field built by hand.
        let n = (38000 + 1000) / 25; // 1560
        let f13 = ((n & 0x7E0) << 2) | ((n & 0x10) << 1) | 0x10 | (n & 0x0F);
        assert_eq!(ac13(f13 as u32), Some(38000));
        // Gillham codes: no C bits is never valid; D1 is never used.
        assert_eq!(gillham_to_100ft(0x0000), None);
        assert_eq!(gillham_to_100ft(0x0011), None);
        // Neighbouring 100 ft steps differ by exactly 100 ft.
        let a = gillham_to_100ft(0x0010).unwrap();
        let b = gillham_to_100ft(0x0030).unwrap();
        assert_eq!((a - b).abs(), 1);
    }

    #[test]
    fn squawks() {
        // ID13 bits for 7700: A=7, B=7, C=0, D=0.
        let id = 0x0800 | 0x0200 | 0x0080 | 0x0020 | 0x0008 | 0x0002;
        assert_eq!(squawk(id), "7700");
        let d = decode(&[0x28, 0x00, (id >> 8) as u8, (id & 0xff) as u8, 0, 0, 0], 0xABCDEF);
        assert_eq!(d.squawk.as_deref(), Some("7700"));
    }
}
