//! Simulated air traffic around London for the synthetic receiver: each
//! aircraft flies a fixed path (driven by the wall clock, so two simulated
//! receivers agree) and sends real ADS-B squitters on 1090 MHz. Addresses
//! are in 0x41xxxx, which has no entries in the aircraft database, so the
//! made-up flights never pick up real registrations.

use std::f64::consts::TAU;

use crate::adsb::{cpr, encode};

#[derive(Clone, Copy)]
enum Path {
    /// From → to over `period` s, then again; altitude moves linearly.
    Line { from: (f64, f64), to: (f64, f64), alt: (i32, i32), period: f64 },
    /// Circling at a fixed altitude.
    Circle { centre: (f64, f64), radius_km: f64, alt: i32, period: f64 },
    /// Taxiing in a circle on the ground.
    Taxi { centre: (f64, f64), radius_km: f64, period: f64 },
}

pub(super) struct Flight {
    pub icao: u32,
    pub callsign: &'static str,
    category: &'static str,
    squawk: &'static str,
    path: Path,
}

const fn line(from: (f64, f64), to: (f64, f64), alt: (i32, i32), period: f64) -> Path {
    Path::Line { from, to, alt, period }
}

const fn circle(centre: (f64, f64), radius_km: f64, alt: i32, period: f64) -> Path {
    Path::Circle { centre, radius_km, alt, period }
}

pub(super) const FLIGHTS: &[Flight] = &[
    Flight { icao: 0x41A001, callsign: "BAW1KX", category: "A3", squawk: "4417", path: line((52.6, -2.2), (50.6, 1.6), (37000, 37000), 1800.0) },
    Flight { icao: 0x41A002, callsign: "VIR3N", category: "A5", squawk: "6312", path: line((51.55, 0.9), (51.47, -0.40), (8000, 1500), 900.0) },
    Flight { icao: 0x41A003, callsign: "EZY45TR", category: "A3", squawk: "5235", path: line((51.15, -0.18), (52.2, 0.8), (2000, 24000), 1200.0) },
    Flight { icao: 0x41A004, callsign: "NJE7GA", category: "A2", squawk: "3731", path: line((50.9, -1.4), (51.88, 0.24), (25000, 25000), 1500.0) },
    Flight { icao: 0x41A005, callsign: "GBXYZ", category: "A1", squawk: "7000", path: circle((51.35, -0.55), 8.0, 2500, 600.0) },
    Flight { icao: 0x41A006, callsign: "HLE21", category: "A7", squawk: "0020", path: circle((51.51, -0.08), 3.0, 1200, 300.0) },
    Flight { icao: 0x41A007, callsign: "GCKSX", category: "B1", squawk: "7000", path: circle((51.70, -0.80), 2.0, 3500, 240.0) },
    Flight { icao: 0x41A008, callsign: "GBALN", category: "B2", squawk: "7000", path: line((51.60, -0.40), (51.63, -0.20), (2000, 2600), 3600.0) },
    Flight { icao: 0x41A009, callsign: "DRN01", category: "B6", squawk: "7000", path: circle((51.49, -0.15), 0.5, 300, 180.0) },
    Flight { icao: 0x41A00A, callsign: "BAW9T", category: "A3", squawk: "2000", path: Path::Taxi { centre: (51.4700, -0.4543), radius_km: 0.8, period: 420.0 } },
];

/// Where a flight is at wall time `t`: (lat, lon, alt ft, ground speed kt,
/// track °, vertical rate ft/min, on the ground).
pub(super) struct State {
    pub lat: f64,
    pub lon: f64,
    alt: i32,
    gs: f64,
    track: f64,
    vrate: i32,
    ground: bool,
}

/// Offset a position by (north, east) km.
fn offset(p: (f64, f64), north_km: f64, east_km: f64) -> (f64, f64) {
    (p.0 + north_km / 111.2, p.1 + east_km / (111.2 * p.0.to_radians().cos()))
}

fn state(f: &Flight, t: f64) -> State {
    match f.path {
        Path::Line { from, to, alt, period } => {
            let k = t.rem_euclid(period) / period;
            let lat = from.0 + (to.0 - from.0) * k;
            let lon = from.1 + (to.1 - from.1) * k;
            let (km, track) = cpr::range_bearing(from, to);
            State {
                lat,
                lon,
                alt: alt.0 + ((alt.1 - alt.0) as f64 * k) as i32,
                gs: km / period * 3600.0 / 1.852,
                track,
                vrate: ((alt.1 - alt.0) as f64 / period * 60.0) as i32,
                ground: false,
            }
        }
        Path::Circle { centre, radius_km, period, .. } | Path::Taxi { centre, radius_km, period } => {
            let a = TAU * t.rem_euclid(period) / period;
            let (lat, lon) = offset(centre, radius_km * a.cos(), radius_km * a.sin());
            let ground = matches!(f.path, Path::Taxi { .. });
            let alt = if let Path::Circle { alt, .. } = f.path { alt } else { 0 };
            State {
                lat,
                lon,
                alt,
                gs: TAU * radius_km / period * 3600.0 / 1.852,
                // Clockwise seen from above: heading 90° ahead of the radius.
                track: (a.to_degrees() + 90.0).rem_euclid(360.0),
                vrate: 0,
                ground,
            }
        }
    }
}

/// The `seq`th message from flight `i` at wall time `t`, and its distance
/// from central London (for the signal level).
pub(super) fn message(i: usize, seq: u32, t: f64) -> (Vec<u8>, f64) {
    let f = &FLIGHTS[i];
    let s = state(f, t);
    let km = cpr::range_bearing((51.5, -0.12), (s.lat, s.lon)).0;
    let odd = (seq / 2) % 2 == 1;
    let msg: Vec<u8> = match seq % 8 {
        0 | 2 | 4 | 6 if s.ground => encode::surface(f.icao, s.lat, s.lon, s.gs, s.track, odd).to_vec(),
        0 | 2 | 4 | 6 => encode::position(f.icao, s.lat, s.lon, s.alt, odd).to_vec(),
        1 | 5 if !s.ground => encode::velocity(f.icao, s.gs, s.track, s.vrate).to_vec(),
        3 => encode::all_call(f.icao).to_vec(),
        7 if seq % 16 == 15 => encode::status(f.icao, f.squawk).to_vec(),
        _ => encode::ident(f.icao, f.callsign, f.category).to_vec(),
    };
    (msg, km)
}

#[cfg(test)]
mod tests {
    use super::super::synthetic::SynthGen;
    use crate::adsb::demod::Demod;
    use crate::adsb::tracker::Tracker;

    /// The simulated receiver on 1090 MHz, through the demodulator and
    /// tracker: every flight shows up with its callsign.
    #[test]
    fn simulated_traffic_decodes() {
        let rate = 2_400_000;
        let mut gen = SynthGen::new();
        let mut demod = Demod::new(rate);
        let mut t = Tracker::new(0);
        t.home = Some((51.5, -0.12));
        let mut frames = Vec::new();
        for k in 0..100 {
            // 50 ms blocks, 5 s in all.
            let iq = gen.generate(1_090_000_000, rate, Some(49.6), rate as usize / 20);
            let known = t.known(1_000 + k * 50);
            frames.clear();
            demod.feed(&iq, &known, &mut frames);
            for f in &frames {
                t.ingest(f, 1_000 + k * 50);
            }
        }
        let seen: Vec<_> = t.aircraft.values().filter(|a| a.reliable).collect();
        assert_eq!(seen.len(), super::FLIGHTS.len(), "{:?}", seen.iter().map(|a| a.callsign.clone()).collect::<Vec<_>>());
        for f in super::FLIGHTS {
            let a = &t.aircraft[&f.icao];
            assert_eq!(a.callsign.as_deref(), Some(f.callsign), "{:06x}", f.icao);
            assert!(a.lat.is_some(), "{} has no position", f.callsign);
        }
        // Overlapping squitters cost a few bit errors, not many.
        assert!(demod.stats.corrected * 5 < demod.stats.good, "{:?}", demod.stats);
    }
}
