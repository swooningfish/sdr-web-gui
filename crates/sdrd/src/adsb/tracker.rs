//! Aircraft state built up from decoded messages: identity, altitude,
//! speed, position (CPR pairs, then local decoding), a trail, and the
//! polar range the station reaches.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::cpr;
use super::crc;
use super::message::{self, Decoded, Frame};

/// Forget an aircraft after this long without a message.
pub const EXPIRE_MS: i64 = 300_000;
/// Even and odd positions further apart than this don't pair.
const PAIR_MS: i64 = 10_000;
/// A position older than this isn't a reference for local decoding.
const LOCAL_REF_MS: i64 = 600_000;
const TRAIL_MAX: usize = 720;
const TRAIL_MS: i64 = 3_600_000;
/// Positions further than this from the station are taken as decoding errors.
const MAX_RANGE_KM: f64 = 650.0;
pub const SECTORS: usize = 36;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct TrailPoint {
    pub ts: i64,
    pub lat: f64,
    pub lon: f64,
    /// Feet; `None` when not known.
    pub alt: Option<i32>,
    pub ground: bool,
}

/// What the aircraft database knows about an address.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct DbInfo {
    pub reg: Option<String>,
    /// ICAO type designator, e.g. "A320".
    pub type_code: Option<String>,
    /// "AIRBUS A-320".
    pub desc: Option<String>,
    pub military: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Aircraft {
    #[serde(serialize_with = "hex24")]
    pub icao: u32,
    pub callsign: Option<String>,
    pub category: Option<String>,
    pub squawk: Option<String>,
    pub emergency: Option<&'static str>,
    pub alt_baro_ft: Option<i32>,
    pub alt_gnss_ft: Option<i32>,
    pub on_ground: bool,
    pub gs_kt: Option<f32>,
    pub track_deg: Option<f32>,
    pub heading_deg: Option<f32>,
    pub airspeed_kt: Option<u32>,
    pub tas: bool,
    pub vrate_fpm: Option<i32>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub pos_ms: Option<i64>,
    pub distance_km: Option<f64>,
    pub bearing_deg: Option<f64>,
    pub first_ms: i64,
    pub seen_ms: i64,
    pub msgs: u64,
    /// dBFS, smoothed.
    pub rssi: f32,
    #[serde(flatten)]
    pub db: DbInfo,
    #[serde(skip)]
    pub trail: VecDeque<TrailPoint>,
    #[serde(skip)]
    db_looked_up: bool,
    #[serde(skip)]
    even: Option<(u32, u32, i64, bool)>,
    #[serde(skip)]
    odd: Option<(u32, u32, i64, bool)>,
    #[serde(skip)]
    bad_positions: u32,
    /// Highest/lowest altitude and furthest range, for the sighting log.
    #[serde(skip)]
    pub max_km: f64,
    #[serde(skip)]
    pub alt_range: Option<(i32, i32)>,
    /// Has sent an ADS-B (DF17/18) message, or enough others to trust.
    #[serde(skip)]
    pub reliable: bool,
}

fn hex24<S: serde::Serializer>(v: &u32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{v:06x}"))
}

impl Aircraft {
    fn new(icao: u32, now: i64) -> Self {
        Aircraft {
            icao,
            callsign: None,
            category: None,
            squawk: None,
            emergency: None,
            alt_baro_ft: None,
            alt_gnss_ft: None,
            on_ground: false,
            gs_kt: None,
            track_deg: None,
            heading_deg: None,
            airspeed_kt: None,
            tas: false,
            vrate_fpm: None,
            lat: None,
            lon: None,
            pos_ms: None,
            distance_km: None,
            bearing_deg: None,
            first_ms: now,
            seen_ms: now,
            msgs: 0,
            rssi: -60.0,
            db: DbInfo::default(),
            trail: VecDeque::new(),
            db_looked_up: false,
            even: None,
            odd: None,
            bad_positions: 0,
            max_km: 0.0,
            alt_range: None,
            reliable: false,
        }
    }

    pub fn alt(&self) -> Option<i32> {
        self.alt_baro_ft.or(self.alt_gnss_ft)
    }

    /// Trail points newer than `since`.
    pub fn trail_since(&self, since: i64) -> Vec<TrailPoint> {
        self.trail.iter().filter(|p| p.ts > since).copied().collect()
    }
}

fn emergency_name(code: u8) -> Option<&'static str> {
    Some(match code {
        1 => "general",
        2 => "medical",
        3 => "minimum fuel",
        4 => "no communications",
        5 => "unlawful interference",
        6 => "downed aircraft",
        _ => return None,
    })
}

/// Furthest range seen in each 10° sector: since the last reset, and per
/// hour for the last 24 hours.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RangeStats {
    pub since_ms: i64,
    pub all_time: Vec<f32>,
    /// (hour number since the epoch, maxima per sector).
    pub hours: Vec<(i64, Vec<f32>)>,
}

impl RangeStats {
    pub fn new(now: i64) -> Self {
        RangeStats { since_ms: now, all_time: vec![0.0; SECTORS], hours: Vec::new() }
    }

    pub fn valid(&self) -> bool {
        self.all_time.len() == SECTORS && self.hours.iter().all(|h| h.1.len() == SECTORS)
    }

    fn add(&mut self, bearing: f64, km: f64, now: i64) {
        let s = ((bearing / 10.0).floor() as usize) % SECTORS;
        let km = km as f32;
        if km > self.all_time[s] {
            self.all_time[s] = km;
        }
        let hour = now / 3_600_000;
        if self.hours.last().is_none_or(|h| h.0 != hour) {
            self.hours.push((hour, vec![0.0; SECTORS]));
            self.hours.retain(|h| hour - h.0 < 24);
        }
        let h = &mut self.hours.last_mut().unwrap().1;
        if km > h[s] {
            h[s] = km;
        }
    }

    /// Maxima per sector over the last 24 hours.
    pub fn last_24h(&self, now: i64) -> Vec<f32> {
        let hour = now / 3_600_000;
        let mut out = vec![0f32; SECTORS];
        for (h, v) in &self.hours {
            if hour - h < 24 {
                for (o, x) in out.iter_mut().zip(v) {
                    *o = o.max(*x);
                }
            }
        }
        out
    }
}

/// An aircraft that has gone, for the sightings log.
#[derive(Clone, Debug)]
pub struct Sighting {
    pub icao: u32,
    pub callsign: Option<String>,
    pub reg: Option<String>,
    pub type_code: Option<String>,
    pub category: Option<String>,
    pub first_ms: i64,
    pub last_ms: i64,
    pub msgs: u64,
    pub max_km: Option<f64>,
    pub min_alt: Option<i32>,
    pub max_alt: Option<i32>,
}

pub struct Tracker {
    pub aircraft: HashMap<u32, Aircraft>,
    /// Station location (lat, lon), for range, bearing and surface positions.
    pub home: Option<(f64, f64)>,
    pub range: RangeStats,
    /// Messages per second over the last few seconds: (second, count).
    rate: VecDeque<(i64, u32)>,
    pub total: u64,
    /// Addresses waiting for a database lookup.
    pub lookups: Vec<u32>,
}

impl Tracker {
    pub fn new(now: i64) -> Self {
        Tracker {
            aircraft: HashMap::new(),
            home: None,
            range: RangeStats::new(now),
            rate: VecDeque::new(),
            total: 0,
            lookups: Vec::new(),
        }
    }

    /// Addresses heard recently, for checking short replies.
    pub fn known(&self, now: i64) -> HashSet<u32> {
        self.aircraft.values().filter(|a| a.reliable && now - a.seen_ms < 60_000).map(|a| a.icao).collect()
    }

    /// Messages per second, averaged over the last 5 s.
    pub fn msg_rate(&self, now: i64) -> f32 {
        let sec = now / 1000;
        let n: u32 = self.rate.iter().filter(|(s, _)| sec - s <= 5 && *s < sec).map(|r| r.1).sum();
        n as f32 / 5.0
    }

    pub fn ingest(&mut self, frame: &Frame, now: i64) {
        let b = &frame.bytes;
        let df = message::df(b);
        let icao = match df {
            11 | 17 | 18 => (b[1] as u32) << 16 | (b[2] as u32) << 8 | b[3] as u32,
            _ => crc::residual(b),
        };
        // DF18 with a non-ICAO address (TIS-B, ADS-R of others): skip.
        if df == 18 && b[0] & 7 != 0 {
            return;
        }
        let exists = self.aircraft.contains_key(&icao);
        // New aircraft only from messages that carry their address and passed
        // the CRC as received (a "corrected" one may be noise made to fit).
        if !exists && (!matches!(df, 11 | 17 | 18) || frame.corrected) {
            return;
        }
        let mut d = message::decode(b, icao);
        // Short replies are matched to an aircraft through the parity alone;
        // don't let a wrong match yank the altitude around.
        if let (Some(new), Some(a)) = (d.alt_baro_ft, self.aircraft.get(&icao)) {
            if !matches!(df, 17 | 18) && a.alt_baro_ft.is_some_and(|old| (old - new).abs() > 3000) && now - a.seen_ms < 30_000 {
                d.alt_baro_ft = None;
            }
        }
        let sec = now / 1000;
        match self.rate.back_mut() {
            Some((s, n)) if *s == sec => *n += 1,
            _ => {
                self.rate.push_back((sec, 1));
                while self.rate.len() > 10 {
                    self.rate.pop_front();
                }
            }
        }
        self.total += 1;
        let home = self.home;
        let a = self.aircraft.entry(icao).or_insert_with(|| Aircraft::new(icao, now));
        a.seen_ms = now;
        a.msgs += 1;
        a.rssi = if a.msgs == 1 { frame.rssi } else { 0.8 * a.rssi + 0.2 * frame.rssi };
        if (matches!(df, 17 | 18) && !frame.corrected) || a.msgs >= 3 {
            a.reliable = true;
        }
        if !a.db_looked_up {
            a.db_looked_up = true;
            self.lookups.push(icao);
        }
        apply(a, &d, now, home, &mut self.range);
    }

    /// Drop aircraft not heard for a while; returns them for the log.
    pub fn expire(&mut self, now: i64) -> Vec<Sighting> {
        let gone: Vec<u32> = self.aircraft.values().filter(|a| now - a.seen_ms > EXPIRE_MS).map(|a| a.icao).collect();
        gone.into_iter()
            .filter_map(|id| self.aircraft.remove(&id))
            .filter(|a| a.reliable)
            .map(|a| Sighting {
                icao: a.icao,
                callsign: a.callsign.clone(),
                reg: a.db.reg.clone(),
                type_code: a.db.type_code.clone(),
                category: a.category.clone(),
                first_ms: a.first_ms,
                last_ms: a.seen_ms,
                msgs: a.msgs,
                max_km: (a.max_km > 0.0).then_some(a.max_km),
                min_alt: a.alt_range.map(|r| r.0),
                max_alt: a.alt_range.map(|r| r.1),
            })
            .collect()
    }

    pub fn set_db(&mut self, icao: u32, info: DbInfo) {
        if let Some(a) = self.aircraft.get_mut(&icao) {
            a.db = info;
        }
    }
}

fn apply(a: &mut Aircraft, d: &Decoded, now: i64, home: Option<(f64, f64)>, range: &mut RangeStats) {
    if let Some(c) = &d.callsign {
        a.callsign = Some(c.clone());
    }
    if let Some(c) = &d.category {
        a.category = Some(c.clone());
    }
    if let Some(s) = &d.squawk {
        a.squawk = Some(s.clone());
    }
    if let Some(e) = d.emergency {
        a.emergency = emergency_name(e);
    }
    if let Some(alt) = d.alt_baro_ft {
        a.alt_baro_ft = Some(alt);
    }
    if let Some(alt) = d.alt_gnss_ft {
        a.alt_gnss_ft = Some(alt);
    }
    if let Some(alt) = a.alt() {
        a.alt_range = Some(a.alt_range.map_or((alt, alt), |(lo, hi)| (lo.min(alt), hi.max(alt))));
    }
    if let Some(g) = d.on_ground {
        a.on_ground = g;
    }
    if let Some(v) = d.velocity {
        if v.gs_kt.is_some() {
            a.gs_kt = v.gs_kt;
            a.track_deg = v.track_deg;
        }
        if v.heading_deg.is_some() {
            a.heading_deg = v.heading_deg;
        }
        if v.airspeed_kt.is_some() {
            a.airspeed_kt = v.airspeed_kt;
            a.tas = v.tas;
        }
        if v.vrate_fpm.is_some() {
            a.vrate_fpm = v.vrate_fpm;
        }
    }
    if let Some((gs, trk)) = d.surface {
        a.gs_kt = gs.or(a.gs_kt);
        a.track_deg = trk.or(a.track_deg);
        a.vrate_fpm = None;
    }
    if let Some(c) = d.cpr {
        position(a, c, now, home, range);
    }
}

fn position(a: &mut Aircraft, c: message::Cpr, now: i64, home: Option<(f64, f64)>, range: &mut RangeStats) {
    let slot = (c.lat, c.lon, now, c.surface);
    if c.odd {
        a.odd = Some(slot);
    } else {
        a.even = Some(slot);
    }
    let last = match (a.lat, a.lon, a.pos_ms) {
        (Some(lat), Some(lon), Some(t)) if now - t < LOCAL_REF_MS => Some((lat, lon, t)),
        _ => None,
    };
    let fix = if let Some((lat, lon, _)) = last {
        Some(cpr::local(c.lat, c.lon, c.odd, c.surface, (lat, lon)))
    } else {
        match (a.even, a.odd) {
            (Some(e), Some(o)) if (e.2 - o.2).abs() < PAIR_MS && e.3 == o.3 => {
                if c.surface {
                    home.and_then(|h| cpr::global_surface((e.0, e.1), (o.0, o.1), c.odd, h))
                } else {
                    cpr::global_airborne((e.0, e.1), (o.0, o.1), c.odd)
                }
            }
            _ => None,
        }
    };
    let Some((lat, lon)) = fix else { return };
    if !(-90.0..=90.0).contains(&lat) {
        return;
    }
    // Sanity: not further than the station could hear, and not faster than
    // anything that flies since the last position.
    let mut ok = home.is_none_or(|h| cpr::range_bearing(h, (lat, lon)).0 <= MAX_RANGE_KM);
    if let Some((plat, plon, pt)) = last {
        let km = cpr::range_bearing((plat, plon), (lat, lon)).0;
        let hours = ((now - pt) as f64 / 3.6e6).max(1.0 / 3600.0);
        ok &= km <= 3.0 + 2500.0 * hours;
    }
    if !ok {
        a.bad_positions += 1;
        if a.bad_positions >= 4 {
            // Our reference may be the bad one: start again from a pair.
            a.lat = None;
            a.lon = None;
            a.pos_ms = None;
            a.bad_positions = 0;
        }
        return;
    }
    a.bad_positions = 0;
    a.lat = Some(lat);
    a.lon = Some(lon);
    a.pos_ms = Some(now);
    if let Some(h) = home {
        let (km, brg) = cpr::range_bearing(h, (lat, lon));
        a.distance_km = Some(km);
        a.bearing_deg = Some(brg);
        a.max_km = a.max_km.max(km);
        if !c.surface && a.reliable {
            range.add(brg, km, now);
        }
    }
    add_trail(a, now, lat, lon);
}

fn add_trail(a: &mut Aircraft, now: i64, lat: f64, lon: f64) {
    let alt = a.alt();
    let p = TrailPoint { ts: now, lat, lon, alt, ground: a.on_ground };
    let add = match a.trail.back() {
        None => true,
        Some(l) => {
            let km = cpr::range_bearing((l.lat, l.lon), (lat, lon)).0;
            let dalt = match (l.alt, alt) {
                (Some(x), Some(y)) => (x - y).abs(),
                _ => 0,
            };
            km > 0.6 || now - l.ts > 20_000 || dalt >= 300 || (l.ground != p.ground)
        }
    };
    if add {
        a.trail.push_back(p);
        while a.trail.len() > TRAIL_MAX || a.trail.front().is_some_and(|f| now - f.ts > TRAIL_MS) {
            a.trail.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::encode;
    use super::*;

    fn frame(b: &[u8]) -> Frame {
        Frame { bytes: b.to_vec(), rssi: -20.0, corrected: false }
    }

    #[test]
    fn builds_an_aircraft() {
        let mut t = Tracker::new(0);
        t.home = Some((51.5, -0.1));
        let id = 0x40ABCD;
        let mut now = 1_000_000;
        t.ingest(&frame(&encode::ident(id, "BAW123", "A3")), now);
        // A position needs an even/odd pair first.
        t.ingest(&frame(&encode::position(id, 51.6, -0.3, 12000, false)), now);
        assert!(t.aircraft[&id].lat.is_none());
        now += 500;
        t.ingest(&frame(&encode::position(id, 51.601, -0.301, 12025, true)), now);
        let a = &t.aircraft[&id];
        assert!((a.lat.unwrap() - 51.601).abs() < 1e-3, "{:?}", a.lat);
        assert_eq!(a.callsign.as_deref(), Some("BAW123"));
        assert_eq!(a.category.as_deref(), Some("A3"));
        assert_eq!(a.alt_baro_ft, Some(12025));
        assert!(a.distance_km.unwrap() > 10.0);
        // Then single messages decode locally.
        now += 500;
        t.ingest(&frame(&encode::position(id, 51.605, -0.31, 12050, false)), now);
        assert!((t.aircraft[&id].lon.unwrap() + 0.31).abs() < 1e-3);
        t.ingest(&frame(&encode::velocity(id, 300.0, 270.0, -500)), now);
        assert_eq!(t.aircraft[&id].vrate_fpm, Some(-512));
        assert!(t.range.all_time.iter().any(|&k| k > 10.0));
        // A short reply from it is now accepted.
        assert!(t.known(now).contains(&id));
        // And it expires into a sighting.
        let s = t.expire(now + EXPIRE_MS + 1);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].callsign.as_deref(), Some("BAW123"));
        assert!(t.aircraft.is_empty());
    }

    #[test]
    fn rejects_a_jump() {
        let mut t = Tracker::new(0);
        let id = 0x400001;
        t.ingest(&frame(&encode::position(id, 51.6, -0.3, 12000, false)), 1000);
        t.ingest(&frame(&encode::position(id, 51.6, -0.3, 12000, true)), 1500);
        assert!(t.aircraft[&id].lat.is_some());
        // 300 km away a second later: dropped.
        t.ingest(&frame(&encode::position(id, 54.3, -0.3, 12000, false)), 2500);
        assert!((t.aircraft[&id].lat.unwrap() - 51.6).abs() < 1e-3);
    }
}
