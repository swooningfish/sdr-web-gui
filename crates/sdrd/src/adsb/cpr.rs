//! Compact Position Reporting: positions arrive as 17-bit fractions of a
//! latitude zone, alternating "even" and "odd" zone sizes. A pair decodes
//! globally; one message decodes against a nearby reference.

const NZ: f64 = 15.0;
const SCALE: f64 = 131_072.0; // 2^17

/// Number of longitude zones at a latitude.
pub fn nl(lat: f64) -> u32 {
    let lat = lat.abs();
    if lat < 1e-9 {
        return 59;
    }
    if (lat - 87.0).abs() < 1e-9 {
        return 2;
    }
    if lat > 87.0 {
        return 1;
    }
    let a = 1.0 - (std::f64::consts::PI / (2.0 * NZ)).cos();
    let b = (std::f64::consts::PI / 180.0 * lat).cos().powi(2);
    (2.0 * std::f64::consts::PI / (1.0 - a / b).acos()).floor() as u32
}

fn modf(a: f64, b: f64) -> f64 {
    a - b * (a / b).floor()
}

/// Global airborne decode from an even and an odd message (raw 17-bit
/// values); `odd_newer` says which arrived last (its position is returned).
pub fn global_airborne(even: (u32, u32), odd: (u32, u32), odd_newer: bool) -> Option<(f64, f64)> {
    global(even, odd, odd_newer, 360.0)
}

/// Global surface decode: zones are a quarter the size, so the pair gives
/// latitudes 90° apart and longitudes in four quadrants; the candidates
/// nearest `reference` (the receiver) win.
pub fn global_surface(even: (u32, u32), odd: (u32, u32), odd_newer: bool, reference: (f64, f64)) -> Option<(f64, f64)> {
    let (lat0, lon0) = (even.0 as f64 / SCALE, even.1 as f64 / SCALE);
    let (lat1, lon1) = (odd.0 as f64 / SCALE, odd.1 as f64 / SCALE);
    let j = (59.0 * lat0 - 60.0 * lat1 + 0.5).floor();
    let near = |l: f64| if (l - 90.0 - reference.0).abs() < (l - reference.0).abs() { l - 90.0 } else { l };
    let rlat0 = near(90.0 / 60.0 * (modf(j, 60.0) + lat0));
    let rlat1 = near(90.0 / 59.0 * (modf(j, 59.0) + lat1));
    if nl(rlat0) != nl(rlat1) {
        return None;
    }
    let (lat, i, lon_frac) = if odd_newer { (rlat1, 1, lon1) } else { (rlat0, 0, lon0) };
    let n = nl(lat) as f64;
    let ni = (n - i as f64).max(1.0);
    let m = (lon0 * (n - 1.0) - lon1 * n + 0.5).floor();
    let lon = (90.0 / ni) * (modf(m, ni) + lon_frac);
    let lon = (0..4)
        .map(|k| (lon + 90.0 * k as f64 + 180.0).rem_euclid(360.0) - 180.0)
        .min_by(|a, b| angle_diff(*a, reference.1).total_cmp(&angle_diff(*b, reference.1)))?;
    Some((lat, lon))
}

fn angle_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn global(even: (u32, u32), odd: (u32, u32), odd_newer: bool, span: f64) -> Option<(f64, f64)> {
    debug_assert!(span == 360.0);
    let dlat0 = span / 60.0;
    let dlat1 = span / 59.0;
    let (lat0, lon0) = (even.0 as f64 / SCALE, even.1 as f64 / SCALE);
    let (lat1, lon1) = (odd.0 as f64 / SCALE, odd.1 as f64 / SCALE);
    let j = (59.0 * lat0 - 60.0 * lat1 + 0.5).floor();
    let mut rlat0 = dlat0 * (modf(j, 60.0) + lat0);
    let mut rlat1 = dlat1 * (modf(j, 59.0) + lat1);
    if span == 360.0 {
        if rlat0 >= 270.0 {
            rlat0 -= 360.0;
        }
        if rlat1 >= 270.0 {
            rlat1 -= 360.0;
        }
    }
    if !(-90.0..=90.0).contains(&rlat0) || !(-90.0..=90.0).contains(&rlat1) {
        return None;
    }
    // Both must be in the same longitude zone band, or the pair straddles one.
    if nl(rlat0) != nl(rlat1) {
        return None;
    }
    let (lat, nlat, i, lon_frac) = if odd_newer { (rlat1, nl(rlat1), 1, lon1) } else { (rlat0, nl(rlat0), 0, lon0) };
    let ni = (nlat as i64 - i).max(1) as f64;
    let m = (lon0 * (nlat as f64 - 1.0) - lon1 * nlat as f64 + 0.5).floor();
    let mut lon = (span / ni) * (modf(m, ni) + lon_frac);
    if lon >= 180.0 {
        lon -= 360.0;
    }
    Some((lat, lon))
}

/// Decode one message against a reference position within ~180 NM
/// (airborne) or ~45 NM (surface).
pub fn local(cpr_lat: u32, cpr_lon: u32, odd: bool, surface: bool, reference: (f64, f64)) -> (f64, f64) {
    let span = if surface { 90.0 } else { 360.0 };
    let i = odd as i32;
    let (lat_frac, lon_frac) = (cpr_lat as f64 / SCALE, cpr_lon as f64 / SCALE);
    let dlat = span / (60 - i) as f64;
    let j = (reference.0 / dlat).floor() + (0.5 + modf(reference.0, dlat) / dlat - lat_frac).floor();
    let lat = dlat * (j + lat_frac);
    let ni = (nl(lat) as i32 - i).max(1) as f64;
    let dlon = span / ni;
    let m = (reference.1 / dlon).floor() + (0.5 + modf(reference.1, dlon) / dlon - lon_frac).floor();
    let mut lon = dlon * (m + lon_frac);
    if lon >= 180.0 {
        lon -= 360.0;
    } else if lon < -180.0 {
        lon += 360.0;
    }
    (lat, lon)
}

/// Encode a position as airborne CPR (for the simulator and tests).
pub fn encode(lat: f64, lon: f64, odd: bool, surface: bool) -> (u32, u32) {
    let span = if surface { 90.0 } else { 360.0 };
    let i = odd as i32;
    let dlat = span / (60 - i) as f64;
    let yz = (SCALE * modf(lat, dlat) / dlat + 0.5).floor();
    let rlat = dlat * (yz / SCALE + (lat / dlat).floor());
    let ni = (nl(rlat) as i32 - i).max(1) as f64;
    let dlon = span / ni;
    let xz = (SCALE * modf(lon, dlon) / dlon + 0.5).floor();
    ((yz as u32) & 0x1FFFF, (xz as u32) & 0x1FFFF)
}

/// Great-circle distance (km) and initial bearing (°) from a to b.
pub fn range_bearing(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (la1, lo1) = (a.0.to_radians(), a.1.to_radians());
    let (la2, lo2) = (b.0.to_radians(), b.1.to_radians());
    let dla = la2 - la1;
    let dlo = lo2 - lo1;
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    let km = 2.0 * 6371.0 * h.sqrt().min(1.0).asin();
    let y = dlo.sin() * la2.cos();
    let x = la1.cos() * la2.sin() - la1.sin() * la2.cos() * dlo.cos();
    (km, y.atan2(x).to_degrees().rem_euclid(360.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones() {
        assert_eq!(nl(0.0), 59);
        assert_eq!(nl(52.0), 36);
        assert_eq!(nl(87.0), 2);
        assert_eq!(nl(89.0), 1);
        assert_eq!(nl(-52.0), 36);
    }

    #[test]
    fn global_pair() {
        // "The 1090 MHz Riddle" example: 52.2572°, 3.9194°.
        let (lat, lon) = global_airborne((93000, 51372), (74158, 50194), false).unwrap();
        assert!((lat - 52.25720).abs() < 1e-4, "{lat}");
        assert!((lon - 3.91937).abs() < 1e-4, "{lon}");
    }

    #[test]
    fn local_decode() {
        let (lat, lon) = local(93000, 51372, false, false, (52.258, 3.918));
        assert!((lat - 52.25720).abs() < 1e-4, "{lat}");
        assert!((lon - 3.91937).abs() < 1e-4, "{lon}");
    }

    #[test]
    fn round_trips() {
        for &(lat, lon) in &[(51.47, -0.4543), (51.6, 0.2), (-33.9, 151.2), (40.64, -73.78), (64.1, -21.9)] {
            let e = encode(lat, lon, false, false);
            let o = encode(lat, lon, true, false);
            for odd_newer in [false, true] {
                let (a, b) = global_airborne(e, o, odd_newer).unwrap();
                assert!((a - lat).abs() < 1e-3 && (b - lon).abs() < 1e-3, "{lat},{lon} → {a},{b}");
            }
            let (a, b) = local(o.0, o.1, true, false, (lat + 0.5, lon - 0.7));
            assert!((a - lat).abs() < 1e-3 && (b - lon).abs() < 1e-3, "local {lat},{lon} → {a},{b}");
            let se = encode(lat, lon, false, true);
            let so = encode(lat, lon, true, true);
            let (a, b) = global_surface(se, so, true, (lat + 0.2, lon + 0.2)).unwrap();
            assert!((a - lat).abs() < 1e-3 && (b - lon).abs() < 1e-3, "surface {lat},{lon} → {a},{b}");
            let (a, b) = local(se.0, se.1, false, true, (lat - 0.1, lon + 0.1));
            assert!((a - lat).abs() < 1e-3 && (b - lon).abs() < 1e-3, "surface local {lat},{lon} → {a},{b}");
        }
    }

    #[test]
    fn distances() {
        let (km, brg) = range_bearing((51.5, -0.1), (51.5, 0.9));
        assert!((km - 69.2).abs() < 0.5, "{km}");
        assert!((brg - 90.0).abs() < 0.5, "{brg}");
    }
}
