//! Satellite tracking: SGP4 propagation (via the `sgp4` crate), conversion to
//! look angles from the ground station, Doppler, and pass prediction.
//!
//! Frames: SGP4 gives TEME position/velocity; we rotate by Greenwich mean
//! sidereal time to an Earth-fixed frame (ignoring polar motion and the
//! equation of the equinoxes, < 1 km at LEO, far below what matters here).

use anyhow::{anyhow, Result};
use serde::Serialize;

const EARTH_A_KM: f64 = 6378.137;
const EARTH_F: f64 = 1.0 / 298.257_223_563;
const EARTH_W: f64 = 7.292_115_146_7e-5; // rad/s
const C_KM_S: f64 = 299_792.458;

pub struct Satellite {
    pub name: String,
    pub norad_id: u32,
    pub epoch_ms: i64,
    constants: sgp4::Constants,
}

impl Satellite {
    pub fn from_tle(name: &str, line1: &str, line2: &str) -> Result<Self> {
        let el = sgp4::Elements::from_tle(Some(name.to_string()), line1.trim().as_bytes(), line2.trim().as_bytes())
            .map_err(|e| anyhow!("bad TLE: {e}"))?;
        let constants = sgp4::Constants::from_elements(&el).map_err(|e| anyhow!("bad TLE: {e}"))?;
        Ok(Satellite {
            name: name.to_string(),
            norad_id: el.norad_id as u32,
            epoch_ms: el.datetime.and_utc().timestamp_millis(),
            constants,
        })
    }

    /// TEME position (km) and velocity (km/s) at `t_ms`.
    pub fn teme(&self, t_ms: i64) -> Option<([f64; 3], [f64; 3])> {
        let minutes = (t_ms - self.epoch_ms) as f64 / 60_000.0;
        let p = self.constants.propagate(sgp4::MinutesSinceEpoch(minutes)).ok()?;
        Some((p.position, p.velocity))
    }
}

/// Split a CelesTrak-style 3-line (or 2-line) TLE text into (name, l1, l2).
pub fn parse_tle_text(text: &str) -> Vec<(String, String, String)> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < lines.len() {
        if lines[i].starts_with("1 ") && lines[i + 1].starts_with("2 ") {
            out.push((String::new(), lines[i].to_string(), lines[i + 1].to_string()));
            i += 2;
        } else if i + 2 < lines.len() && lines[i + 1].starts_with("1 ") && lines[i + 2].starts_with("2 ") {
            out.push((lines[i].trim().to_string(), lines[i + 1].to_string(), lines[i + 2].to_string()));
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Serialize, serde::Deserialize, PartialEq)]
pub struct Observer {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub alt_m: f64,
}

impl Observer {
    pub fn validate(&self) -> Result<(), String> {
        if !(-90.0..=90.0).contains(&self.lat_deg) || !(-180.0..=180.0).contains(&self.lon_deg) {
            return Err("latitude must be −90…90 and longitude −180…180".into());
        }
        if !(-500.0..=9000.0).contains(&self.alt_m) {
            return Err("altitude must be −500…9000 m".into());
        }
        Ok(())
    }

    fn ecef(&self) -> [f64; 3] {
        let (lat, lon) = (self.lat_deg.to_radians(), self.lon_deg.to_radians());
        let e2 = EARTH_F * (2.0 - EARTH_F);
        let n = EARTH_A_KM / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        let h = self.alt_m / 1000.0;
        [
            (n + h) * lat.cos() * lon.cos(),
            (n + h) * lat.cos() * lon.sin(),
            (n * (1.0 - e2) + h) * lat.sin(),
        ]
    }
}

/// Greenwich mean sidereal time (IAU 1982), radians.
pub fn gmst(t_ms: i64) -> f64 {
    let jd = t_ms as f64 / 86_400_000.0 + 2_440_587.5;
    let t = (jd - 2_451_545.0) / 36_525.0;
    let secs = 67_310.548_41 + (876_600.0 * 3600.0 + 8_640_184.812_866) * t + 0.093_104 * t * t - 6.2e-6 * t * t * t;
    (secs.rem_euclid(86_400.0) / 86_400.0) * std::f64::consts::TAU
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Look {
    pub az_deg: f64,
    pub el_deg: f64,
    pub range_km: f64,
    /// Positive when moving away.
    pub range_rate_km_s: f64,
}

pub fn look(sat: &Satellite, obs: &Observer, t_ms: i64) -> Option<Look> {
    let (r, v) = sat.teme(t_ms)?;
    let g = gmst(t_ms);
    let (s, c) = g.sin_cos();
    // TEME → Earth-fixed.
    let re = [c * r[0] + s * r[1], -s * r[0] + c * r[1], r[2]];
    let ve = [
        c * v[0] + s * v[1] + EARTH_W * re[1],
        -s * v[0] + c * v[1] - EARTH_W * re[0],
        v[2],
    ];
    let o = obs.ecef();
    let d = [re[0] - o[0], re[1] - o[1], re[2] - o[2]];
    let range = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let (lat, lon) = (obs.lat_deg.to_radians(), obs.lon_deg.to_radians());
    let east = -lon.sin() * d[0] + lon.cos() * d[1];
    let north = -lat.sin() * lon.cos() * d[0] - lat.sin() * lon.sin() * d[1] + lat.cos() * d[2];
    let up = lat.cos() * lon.cos() * d[0] + lat.cos() * lon.sin() * d[1] + lat.sin() * d[2];
    Some(Look {
        az_deg: east.atan2(north).to_degrees().rem_euclid(360.0),
        el_deg: up.atan2((east * east + north * north).sqrt()).to_degrees(),
        range_km: range,
        range_rate_km_s: (d[0] * ve[0] + d[1] * ve[1] + d[2] * ve[2]) / range,
    })
}

/// The point on the ground directly below the satellite (geodetic latitude,
/// longitude east, degrees) and its height above the ellipsoid in km.
pub fn ground(sat: &Satellite, t_ms: i64) -> Option<(f64, f64, f64)> {
    let (r, _) = sat.teme(t_ms)?;
    let (s, c) = gmst(t_ms).sin_cos();
    let (x, y, z) = (c * r[0] + s * r[1], -s * r[0] + c * r[1], r[2]);
    let e2 = EARTH_F * (2.0 - EARTH_F);
    let p = (x * x + y * y).sqrt();
    let mut lat = z.atan2(p * (1.0 - e2));
    let mut n = EARTH_A_KM;
    for _ in 0..5 {
        n = EARTH_A_KM / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        lat = (z + e2 * n * lat.sin()).atan2(p);
    }
    let alt = if lat.cos().abs() > 1e-6 { p / lat.cos() - n } else { z.abs() - n * (1.0 - e2) };
    Some((lat.to_degrees(), y.atan2(x).to_degrees(), alt))
}

/// Frequency shift seen on the ground for a transmitter at `freq_hz`.
pub fn doppler_hz(freq_hz: f64, range_rate_km_s: f64) -> f64 {
    -freq_hz * range_rate_km_s / C_KM_S
}

#[derive(Clone, Debug, Serialize)]
pub struct Pass {
    pub satellite: String,
    pub aos_ms: i64,
    pub los_ms: i64,
    pub max_ms: i64,
    pub max_el_deg: f64,
    pub aos_az_deg: f64,
    pub los_az_deg: f64,
    /// (time ms, azimuth°, elevation°) every 15 s, for sky plots.
    pub track: Vec<(i64, f32, f32)>,
}

fn el(sat: &Satellite, obs: &Observer, t: i64) -> f64 {
    look(sat, obs, t).map_or(-90.0, |l| l.el_deg)
}

/// Time (to the second) where elevation crosses zero between `a` and `b`.
fn crossing(sat: &Satellite, obs: &Observer, mut a: i64, mut b: i64) -> i64 {
    let rising = el(sat, obs, a) < 0.0;
    while b - a > 1000 {
        let m = (a + b) / 2;
        if (el(sat, obs, m) < 0.0) == rising {
            a = m;
        } else {
            b = m;
        }
    }
    b
}

/// Passes that start between `from_ms` and `to_ms` and reach `min_el_deg`.
pub fn passes(sat: &Satellite, obs: &Observer, from_ms: i64, to_ms: i64, min_el_deg: f64) -> Vec<Pass> {
    const STEP: i64 = 20_000;
    let mut out = Vec::new();
    let mut t = from_ms;
    let mut prev = el(sat, obs, t);
    // A pass already in progress counts from now.
    let mut aos = (prev >= 0.0).then_some(t);
    while t < to_ms || aos.is_some() {
        let t2 = t + STEP;
        let e2 = el(sat, obs, t2);
        if prev < 0.0 && e2 >= 0.0 {
            aos = Some(crossing(sat, obs, t, t2));
        } else if prev >= 0.0 && e2 < 0.0 {
            if let Some(a) = aos.take() {
                let los = crossing(sat, obs, t, t2);
                if let Some(p) = describe(sat, obs, a, los) {
                    if p.max_el_deg >= min_el_deg {
                        out.push(p);
                    }
                }
            }
        }
        prev = e2;
        t = t2;
        if t > to_ms + 30 * 60_000 {
            break; // a pass can't last this long
        }
    }
    out
}

fn describe(sat: &Satellite, obs: &Observer, aos: i64, los: i64) -> Option<Pass> {
    let mut best = (f64::MIN, aos);
    let mut t = aos;
    while t <= los {
        let e = el(sat, obs, t);
        if e > best.0 {
            best = (e, t);
        }
        t += 5_000;
    }
    // Near the zenith elevation changes fast; refine around the coarse peak.
    let mut t = (best.1 - 5_000).max(aos);
    while t <= (best.1 + 5_000).min(los) {
        let e = el(sat, obs, t);
        if e > best.0 {
            best = (e, t);
        }
        t += 250;
    }
    let mut track = Vec::new();
    let mut t = aos;
    loop {
        let l = look(sat, obs, t)?;
        track.push((t, l.az_deg as f32, l.el_deg.max(0.0) as f32));
        if t >= los {
            break;
        }
        t = (t + 15_000).min(los);
    }
    Some(Pass {
        satellite: sat.name.clone(),
        aos_ms: aos,
        los_ms: los,
        max_ms: best.1,
        max_el_deg: best.0,
        aos_az_deg: look(sat, obs, aos)?.az_deg,
        los_az_deg: look(sat, obs, los)?.az_deg,
        track,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Append the TLE checksum digit to the first 68 characters.
    fn with_checksum(l: &str) -> String {
        let sum: u32 = l
            .chars()
            .take(68)
            .map(|c| match c {
                '0'..='9' => c as u32 - '0' as u32,
                '-' => 1,
                _ => 0,
            })
            .sum();
        format!("{}{}", &l[..68], sum % 10)
    }

    /// An ISS-like element set (epoch 2026-11-08 12:00 UTC).
    pub fn iss() -> Satellite {
        let l1 = with_checksum("1 25544U 98067A   26312.50000000  .00016717  00000-0  10270-3 0  9990");
        let l2 = with_checksum("2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.50377579400000");
        Satellite::from_tle("ISS (ZARYA)", &l1, &l2).unwrap()
    }

    const LONDON: Observer = Observer { lat_deg: 51.5, lon_deg: -0.12, alt_m: 20.0 };

    #[test]
    fn orbit_is_iss_like() {
        let s = iss();
        assert_eq!(s.norad_id, 25544);
        let (r, v) = s.teme(s.epoch_ms).unwrap();
        let alt = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt() - EARTH_A_KM;
        let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        assert!((390.0..440.0).contains(&alt), "altitude {alt}");
        assert!((7.6..7.72).contains(&speed), "speed {speed}");
    }

    #[test]
    fn ground_point_is_below_the_satellite() {
        let s = iss();
        let mut north = false;
        for i in 0..95 {
            let t = s.epoch_ms + i * 60_000;
            let (lat, lon, alt) = ground(&s, t).unwrap();
            // Never beyond the inclination; ISS altitude.
            assert!(lat.abs() < 51.9, "lat {lat}");
            assert!((-180.0..=180.0).contains(&lon));
            assert!((390.0..440.0).contains(&alt), "alt {alt}");
            north |= lat > 45.0;
            // Standing on that point, the satellite is straight up.
            let obs = Observer { lat_deg: lat, lon_deg: lon, alt_m: 0.0 };
            let l = look(&s, &obs, t).unwrap();
            assert!(l.el_deg > 89.9, "el {}", l.el_deg);
            assert!((l.range_km - alt).abs() < 0.5);
        }
        assert!(north, "one orbit reaches the far north");
    }

    #[test]
    fn gmst_reference() {
        // 2000-01-01 12:00 UT: GMST = 18h 41m 50.548s = 280.4606°.
        let g = gmst(946_728_000_000).to_degrees();
        assert!((g - 280.4606).abs() < 0.001, "{g}");
    }

    #[test]
    fn passes_look_right() {
        let s = iss();
        let ps = passes(&s, &LONDON, s.epoch_ms, s.epoch_ms + 86_400_000, 0.0);
        // The ISS passes over London 4–7 times a day.
        assert!((3..=8).contains(&ps.len()), "{} passes", ps.len());
        for p in &ps {
            let mins = (p.los_ms - p.aos_ms) as f64 / 60_000.0;
            assert!(mins > 0.0 && mins < 12.0, "{mins} min");
            assert!(p.max_el_deg > 0.0 && p.max_el_deg <= 90.0);
            assert!(p.max_ms >= p.aos_ms && p.max_ms <= p.los_ms);
            // Approaching at AOS (shifted up), receding at LOS (down); never
            // more than ~3.6 kHz at 145.8 MHz.
            let a = look(&s, &LONDON, p.aos_ms).unwrap();
            let b = look(&s, &LONDON, p.los_ms).unwrap();
            let (da, db) = (doppler_hz(145.8e6, a.range_rate_km_s), doppler_hz(145.8e6, b.range_rate_km_s));
            assert!(da > 0.0 && db < 0.0, "{da} {db}");
            assert!(da.abs() < 3700.0 && db.abs() < 3700.0);
            assert!(p.track.first().unwrap().2 < 1.0 && p.track.len() >= 2);
        }
    }

    /// Manual cross-check against another library:
    /// `TLE_FILE=iss.txt T0_MS=... cargo test print_passes -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_passes() {
        let text = std::fs::read_to_string(std::env::var("TLE_FILE").unwrap()).unwrap();
        let (name, l1, l2) = parse_tle_text(&text).remove(0);
        let s = Satellite::from_tle(&name, &l1, &l2).unwrap();
        let t0: i64 = std::env::var("T0_MS").unwrap().parse().unwrap();
        for p in passes(&s, &LONDON, t0, t0 + 86_400_000, 0.0) {
            let l = look(&s, &LONDON, p.max_ms).unwrap();
            println!(
                "PASS aos={} los={} max_el={:.2} max_at={} az_at_max={:.2} range_at_max={:.1}",
                p.aos_ms / 1000,
                p.los_ms / 1000,
                p.max_el_deg,
                p.max_ms / 1000,
                l.az_deg,
                l.range_km
            );
        }
        let l = look(&s, &LONDON, t0).unwrap();
        println!("AT_T0 az={:.3} el={:.3} range={:.2} rr={:.4}", l.az_deg, l.el_deg, l.range_km, l.range_rate_km_s);
    }

    #[test]
    fn parses_tle_text() {
        let s = iss();
        let _ = s;
        let txt = "ISS (ZARYA)             \n1 25544U 98067A   26312.5 x\n2 25544  51.6 x\n\nFOO\n1 1\n2 2\n";
        let t = parse_tle_text(txt);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].0, "ISS (ZARYA)");
        assert_eq!(t[1].0, "FOO");
    }
}
