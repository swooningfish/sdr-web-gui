//! Synthetic IQ source: noise plus a few emitters at real-world frequencies so
//! the UI can be developed and tested without a dongle. Retuning works – the
//! emitters are placed at absolute frequencies and appear when in band.

use std::f64::consts::TAU;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{kind_of, sim_aircraft, to_cu8, tuner_ranges, DeviceInfo, IqBlock, RadioSettings, SourceCtx};
use crate::config::SourceKind;
use crate::aprs::ax25;
use crate::audio::{cw, rds};
use crate::sstv::{encode, modes};

const BLOCK_SECS: f64 = 0.05;

#[derive(Clone, Copy)]
enum Kind {
    /// Real APRS packets: AX.25 in HDLC, 1200 baud AFSK on NFM.
    Aprs { period: f64, channel: AprsChannel },
    /// SSTV images on NFM, cycling through a few modes with `gap` s between.
    Sstv { gap: f64 },
    /// Nexus-TH temperature/humidity sensor (OOK PPM) that rtl_433 decodes.
    Nexus { period: f64, id: u8, channel: u8, base_c: f64, humidity: u8 },
    /// Unmodulated carrier drifting like a LEO satellite's Doppler.
    Doppler { span_hz: f64, period: f64 },
    /// CW beacon keying `BEACON_TEXT` every `period` s.
    Beacon { period: f64, wpm: f64 },
    /// FM repeater: an over of "speech", then its Morse ident on a 1 kHz
    /// tone, then the carrier drops; every `period` s.
    Repeater { period: f64 },
    /// Broadcast FM: music-like tones, stereo pilot and RDS.
    Broadcast,
    /// SRD telemetry (narrow 2-FSK, on 0.8 s of every 1 s), or its `n`th
    /// harmonic (n > 1) for trying the harmonics check: same keying, n times
    /// the deviation, as a real transmitter's harmonics would be.
    Srd { n: u8 },
    /// A steady line keyed with the SRD (one of its close-in spurs).
    SrdSpur,
    /// An unmodulated carrier that's always on.
    Steady,
    /// An aircraft's ADS-B squitters on 1090 MHz (index into the flights).
    Aircraft { idx: usize },
}

/// Simulated SRD: carrier, FSK deviation, bit rate.
pub(super) const SRD_HZ: f64 = 434_200_000.0;
const SRD_DEV: f64 = 3_000.0;
const SRD_BAUD: f64 = 2_400.0;
/// Harmonic levels in dBc (2nd, 3rd, 4th): marginal, failing and clean
/// against the Ofcom limits (−46 / −40 / −40 dBc at 10 mW ERP). The 3rd
/// passes FCC (−26.8 dBc) by 8 dB, clear of the 6 dB "Good" margin.
pub(super) const SRD_HARMONICS_DBC: [f64; 3] = [-43.0, -35.0, -60.0];
/// Close-in spurs every 50 kHz (offset Hz, dBc), as a clock leaking onto the
/// carrier would make: the inner four fail Ofcom's −46 dBc (−100 kHz only
/// just), the 150 kHz one is under the simulator's noise floor (about
/// −51 dBc); all pass FCC's −20 dBc. The inner four are kept well above that
/// floor so the comb shows every time.
pub(super) const SRD_SPURS: [(f64, f64); 5] =
    [(50_000.0, -40.0), (-50_000.0, -42.0), (100_000.0, -44.0), (-100_000.0, -46.0), (150_000.0, -60.0)];
/// Another transmitter's steady carrier near the SRD (not one of its spurs).
const SRD_NEIGHBOUR: (f64, f64) = (250_000.0, -35.0);
const SRD_AMP: f64 = 0.3;

/// Pseudo-random data bit `k` of the SRD's stream (same for every harmonic).
fn srd_bit(k: u64) -> bool {
    let mut x = k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (x >> 32) & 1 == 1
}

const BEACON_TEXT: &str = "VVV DE GB3SIM IO91 QRV";
const REPEATER_IDENT: &str = "GB3SR";
/// Repeater cycle: talk until 6 s, ident from 6.5 s, carrier off after.
const REPEATER_TALK: f64 = 6.0;

struct Emitter {
    freq_hz: f64,
    amp: f64,
    kind: Kind,
    phase: f64,
    tone_phase: f64,
    /// AFSK: one entry per bit (true = 1200 Hz mark), and when it started.
    tones: Vec<bool>,
    tx_start: f64,
    seq: u32,
    /// Next transmission time and its on-intervals (absolute seconds).
    next: f64,
    pulses: Vec<(f64, f64)>,
    pulse_idx: usize,
    /// SSTV: (tone Hz, end time relative to `tx_start`) and the current tone.
    sstv: Vec<(f32, f64)>,
    sstv_idx: usize,
    /// CW: key-down intervals relative to `tx_start`; RDS: coded bits.
    keys: Vec<(f64, f64)>,
    rds: Vec<bool>,
    phase2: f64,
}

pub(super) struct SynthGen {
    emitters: Vec<Emitter>,
    t: f64,
    rng: u64,
}

impl SynthGen {
    pub fn new() -> Self {
        let e = |freq_hz: f64, amp: f64, kind: Kind| Emitter {
            freq_hz,
            amp,
            kind,
            phase: 0.0,
            tone_phase: 0.0,
            tones: Vec::new(),
            tx_start: 0.0,
            seq: 0,
            next: 0.0,
            pulses: Vec::new(),
            pulse_idx: 0,
            sstv: Vec::new(),
            sstv_idx: 0,
            keys: Vec::new(),
            rds: Vec::new(),
            phase2: 0.0,
        };
        let nexus = |id, channel, base_c, humidity, first: f64| {
            let mut em = e(
                433_920_000.0,
                0.35,
                Kind::Nexus { period: 15.0, id, channel, base_c, humidity },
            );
            em.next = first;
            em
        };
        let srd = |n: u8| {
            let amp = if n == 1 { SRD_AMP } else { SRD_AMP * 10f64.powf(SRD_HARMONICS_DBC[n as usize - 2] / 20.0) };
            e(SRD_HZ * n as f64, amp, Kind::Srd { n })
        };
        let near = |(off, dbc): (f64, f64), kind: Kind| e(SRD_HZ + off, SRD_AMP * 10f64.powf(dbc / 20.0), kind);
        SynthGen {
            emitters: vec![
                srd(1),
                srd(2),
                srd(3),
                srd(4),
                near(SRD_SPURS[0], Kind::SrdSpur),
                near(SRD_SPURS[1], Kind::SrdSpur),
                near(SRD_SPURS[2], Kind::SrdSpur),
                near(SRD_SPURS[3], Kind::SrdSpur),
                near(SRD_SPURS[4], Kind::SrdSpur),
                near(SRD_NEIGHBOUR, Kind::Steady),
                e(144_800_000.0, 0.25, Kind::Aprs { period: 10.0, channel: AprsChannel::Terrestrial }),
                e(145_825_000.0, 0.12, Kind::Aprs { period: 30.0, channel: AprsChannel::Iss }),
                {
                    let mut em = e(145_800_000.0, 0.15, Kind::Sstv { gap: 20.0 });
                    em.next = 3.0;
                    em
                },
                nexus(0x5a, 1, 19.0, 55, 1.0),
                nexus(0x21, 2, 6.0, 82, 8.5),
                e(437_550_000.0, 0.12, Kind::Doppler { span_hz: 10_000.0, period: 120.0 }),
                e(144_470_000.0, 0.08, Kind::Beacon { period: 20.0, wpm: 16.0 }),
                e(145_712_500.0, 0.2, Kind::Repeater { period: 30.0 }),
                {
                    let mut em = e(96_100_000.0, 0.3, Kind::Broadcast);
                    em.rds = rds::encode::differential(&rds::encode::groups(
                        0xC0DE,
                        10,
                        "SIM FM",
                        "You're listening to the simulated receiver",
                    ));
                    em
                },
            ]
            .into_iter()
            .chain((0..sim_aircraft::FLIGHTS.len()).map(|idx| {
                let mut em = e(1_090_000_000.0, 0.3, Kind::Aircraft { idx });
                // Stagger the first squitters.
                em.next = 0.037 * idx as f64;
                em
            }))
            .collect(),
            t: 0.0,
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    #[inline]
    fn uniform(&mut self) -> f64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let v = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 11) as f64 / (1u64 << 53) as f64
    }

    #[inline]
    fn gauss(&mut self) -> f64 {
        // Irwin–Hall approximation: cheap and good enough for a noise floor.
        (self.uniform() + self.uniform() + self.uniform() + self.uniform() - 2.0) * 1.732
    }

    /// Generate `n` complex samples as cu8 bytes.
    pub fn generate(&mut self, center_hz: u64, rate: u32, gain_db: Option<f32>, n: usize) -> Vec<u8> {
        let rate_f = rate as f64;
        let dt = 1.0 / rate_f;
        let half_bw = rate_f / 2.0;
        let scale = 0.6 + gain_db.unwrap_or(30.0) as f64 / 100.0;
        let noise = 0.015 * scale;
        let mut out = Vec::with_capacity(n * 2);

        let active: Vec<usize> = (0..self.emitters.len())
            .filter(|&i| (self.emitters[i].freq_hz - center_hz as f64).abs() < half_bw - 15_000.0)
            .collect();

        for _ in 0..n {
            let t = self.t;
            let mut i_acc = noise * self.gauss();
            let mut q_acc = noise * self.gauss();
            for &idx in &active {
                let em = &mut self.emitters[idx];
                let offset = em.freq_hz - center_hz as f64;
                // Share of this sample a pulse covers (pulsed emitters).
                let mut weight = 1.0;
                let (inst_hz, gate) = match em.kind {
                    Kind::Aprs { period, channel } => {
                        if t >= em.next {
                            let late = t - em.next;
                            em.next += period * ((late / period).floor() + 1.0);
                            if late < 0.5 {
                                em.tones = afsk_tones(&aprs_packet(channel, em.seq, wall_secs()));
                                em.tx_start = t;
                                em.seq = em.seq.wrapping_add(1);
                            }
                        }
                        let bit = ((t - em.tx_start) * 1200.0) as usize;
                        match em.tones.get(bit) {
                            Some(&mark) => {
                                let tone = if mark { 1200.0 } else { 2200.0 };
                                em.tone_phase = (em.tone_phase + TAU * tone * dt) % TAU;
                                (offset + 3000.0 * em.tone_phase.sin(), true)
                            }
                            None => (offset, false),
                        }
                    }
                    Kind::Sstv { gap } => {
                        if t >= em.next {
                            if t - em.next < 0.5 {
                                em.sstv = sstv_transmission(em.seq);
                                em.sstv_idx = 0;
                                em.tx_start = t;
                                em.seq = em.seq.wrapping_add(1);
                                em.next = t + em.sstv.last().map_or(0.0, |s| s.1) + gap;
                            } else {
                                // Came back into band after missing one: start shortly.
                                em.next = t + 5.0;
                            }
                        }
                        let rel = t - em.tx_start;
                        while em.sstv_idx < em.sstv.len() && em.sstv[em.sstv_idx].1 <= rel {
                            em.sstv_idx += 1;
                        }
                        match em.sstv.get(em.sstv_idx) {
                            Some(&(tone, _)) => {
                                em.tone_phase = (em.tone_phase + TAU * tone as f64 * dt) % TAU;
                                (offset + 3000.0 * em.tone_phase.sin(), true)
                            }
                            None => (offset, false),
                        }
                    }
                    Kind::Nexus { period, id, channel, base_c, humidity } => {
                        if t >= em.next {
                            // Keep each sensor on its own fixed schedule, so two
                            // sensors never collide after a retune; transmissions
                            // due while we were tuned elsewhere are skipped.
                            let late = t - em.next;
                            em.next += period * ((late / period).floor() + 1.0);
                            if late < 0.5 {
                                // Temperature drifts slowly so the charts show something;
                                // wall-clock based so it stays continuous across restarts.
                                let temp = base_c + 3.0 * (TAU * wall_secs() / 1800.0).sin();
                                em.pulses = nexus_pulses(t, id, channel, temp, humidity);
                                em.pulse_idx = 0;
                            }
                        }
                        while em.pulse_idx < em.pulses.len() && em.pulses[em.pulse_idx].1 < t {
                            em.pulse_idx += 1;
                        }
                        let on = em.pulses.get(em.pulse_idx).is_some_and(|p| p.0 <= t);
                        (offset, on)
                    }
                    Kind::Doppler { span_hz, period } => {
                        (offset + span_hz * (TAU * t / period).cos(), true)
                    }
                    Kind::Beacon { period, wpm } => {
                        if t >= em.next {
                            let late = t - em.next;
                            em.next += period * ((late / period).floor() + 1.0);
                            if late < 0.5 {
                                em.keys = cw::keying(BEACON_TEXT, wpm).0;
                                em.pulse_idx = 0;
                                em.tx_start = t;
                            }
                        }
                        let rel = t - em.tx_start;
                        while em.pulse_idx < em.keys.len() && em.keys[em.pulse_idx].1 < rel {
                            em.pulse_idx += 1;
                        }
                        (offset, em.keys.get(em.pulse_idx).is_some_and(|k| k.0 <= rel))
                    }
                    Kind::Repeater { period } => {
                        if t >= em.next {
                            let late = t - em.next;
                            em.next += period * ((late / period).floor() + 1.0);
                            em.tx_start = t - late;
                            em.keys = cw::keying(REPEATER_IDENT, 18.0).0;
                            em.pulse_idx = 0;
                        }
                        let rel = t - em.tx_start;
                        let ident = rel - REPEATER_TALK - 0.5;
                        let ident_end = em.keys.last().map_or(0.0, |k| k.1) + 0.4;
                        let audio = if rel < REPEATER_TALK {
                            // Two gliding tones with a syllable-like envelope.
                            let f1 = 220.0 + 90.0 * (TAU * 0.6 * rel).sin();
                            let f2 = 2.7 * f1 + 300.0 * (TAU * 0.23 * rel).sin();
                            em.tone_phase = (em.tone_phase + TAU * f1 * dt) % TAU;
                            em.phase2 = (em.phase2 + TAU * f2 * dt) % TAU;
                            let env = (0.5 + 0.5 * (TAU * 3.1 * rel).sin()).powi(2);
                            env * (0.7 * em.tone_phase.sin() + 0.3 * em.phase2.sin())
                        } else if ident >= 0.0 && ident < ident_end {
                            while em.pulse_idx < em.keys.len() && em.keys[em.pulse_idx].1 < ident {
                                em.pulse_idx += 1;
                            }
                            em.tone_phase = (em.tone_phase + TAU * 1000.0 * dt) % TAU;
                            let on = em.keys.get(em.pulse_idx).is_some_and(|k| k.0 <= ident);
                            if on { 0.5 * em.tone_phase.sin() } else { 0.0 }
                        } else {
                            0.0
                        };
                        let on_air = ident < ident_end;
                        (offset + 5_000.0 * audio, on_air)
                    }
                    Kind::Broadcast => {
                        // Music: a slow chord progression; 19 kHz pilot; RDS on 57 kHz.
                        let root = [220.0, 174.6, 196.0, 261.6][((t / 2.0) as usize) % 4];
                        em.tone_phase = (em.tone_phase + TAU * root * dt) % TAU;
                        em.phase2 = (em.phase2 + TAU * root * 1.5 * dt) % TAU;
                        let music = 0.3 * em.tone_phase.sin() + 0.2 * em.phase2.sin();
                        let pilot = 0.09 * (TAU * 19_000.0 * t).sin();
                        let data = 0.05
                            * rds::encode::baseband(&em.rds, t, rds::BIT_RATE)
                            * (TAU * rds::SUBCARRIER_HZ * t).sin();
                        (offset + 75_000.0 * (music + pilot + data), true)
                    }
                    Kind::Srd { n } => {
                        // Gaussian-ish FSK: the ±1 data steers a smoothed deviation.
                        let target = if srd_bit((t * SRD_BAUD) as u64) { 1.0 } else { -1.0 };
                        let alpha = 1.0 - (-dt * SRD_BAUD * 3.0).exp();
                        em.phase2 += (target - em.phase2) * alpha;
                        (offset + n as f64 * SRD_DEV * em.phase2, t.rem_euclid(1.0) < 0.8)
                    }
                    Kind::SrdSpur => (offset, t.rem_euclid(1.0) < 0.8),
                    Kind::Steady => (offset, true),
                    Kind::Aircraft { idx } => {
                        if t >= em.next {
                            let wall = wall_secs();
                            let (msg, km) = sim_aircraft::message(idx, em.seq, wall);
                            em.seq = em.seq.wrapping_add(1);
                            // Nearer is louder: about −6 dB per doubling of range.
                            em.amp = (0.45 * 15.0 / km.max(15.0)).max(0.1);
                            em.pulses = crate::adsb::encode::pulses(&msg)
                                .into_iter()
                                .map(|(a, b)| (t + a * 1e-6, t + b * 1e-6))
                                .collect();
                            em.pulse_idx = 0;
                            // Roughly 5 squitters a second, a little different per aircraft.
                            em.next = t + 0.2 + 0.011 * idx as f64;
                        }
                        while em.pulse_idx < em.pulses.len() && em.pulses[em.pulse_idx].1 <= t {
                            em.pulse_idx += 1;
                        }
                        weight = match em.pulses.get(em.pulse_idx) {
                            Some(&(a, b)) => ((b.min(t + dt) - a.max(t)) / dt).clamp(0.0, 1.0),
                            None => 0.0,
                        };
                        (offset, weight > 0.0)
                    }
                };
                em.phase = (em.phase + TAU * inst_hz * dt) % TAU;
                if gate {
                    let a = em.amp * scale * weight;
                    i_acc += a * em.phase.cos();
                    q_acc += a * em.phase.sin();
                }
            }
            out.push(to_cu8(i_acc as f32));
            out.push(to_cu8(q_acc as f32));
            self.t += dt;
        }
        out
    }
}

fn wall_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// The next synthetic SSTV image: a test card in one of a few modes, as
/// (tone, cumulative end time) pairs.
fn sstv_transmission(seq: u32) -> Vec<(f32, f64)> {
    const CYCLE: [&str; 4] = ["Robot 36", "Martin M2", "Scottie S2", "PD 90"];
    let mode = modes::by_name(CYCLE[seq as usize % CYCLE.len()]).expect("known mode");
    let img = encode::test_pattern(mode.width as usize, mode.height as usize, seq);
    let mut end = 0.0;
    encode::tones(mode, &img)
        .into_iter()
        .map(|(f, d)| {
            end += d;
            (f, end)
        })
        .collect()
}

#[derive(Clone, Copy)]
enum AprsChannel {
    /// 144.800: a car driving a loop, a weather station and a status beacon.
    Terrestrial,
    /// 145.825: an ISS-like station whose position sweeps along an orbit.
    Iss,
}

/// `DDMM.mmN`
fn aprs_lat(lat: f64) -> String {
    let a = lat.abs();
    format!("{:02}{:05.2}{}", a.trunc(), a.fract() * 60.0, if lat >= 0.0 { 'N' } else { 'S' })
}

/// `DDDMM.mmW`
fn aprs_lon(lon: f64) -> String {
    let a = lon.abs();
    format!("{:03}{:05.2}{}", a.trunc(), a.fract() * 60.0, if lon >= 0.0 { 'E' } else { 'W' })
}

/// The next AX.25 frame (without FCS) for a synthetic APRS channel.
fn aprs_packet(channel: AprsChannel, seq: u32, wall: f64) -> Vec<u8> {
    match channel {
        AprsChannel::Terrestrial => match seq % 4 {
            0 | 2 => {
                // Loop of ~3 km radius around central London, 10 minutes per lap.
                let a = TAU * (wall / 600.0).fract();
                let lat = 51.505 + 0.027 * a.sin();
                let lon = -0.09 + 0.043 * a.cos();
                let course = (90.0 - a.to_degrees() + 360.0 - 180.0).rem_euclid(360.0).round().max(1.0);
                let info = format!(
                    "!{}/{}>{:03.0}/018/A=000098 sdrd test mobile",
                    aprs_lat(lat),
                    aprs_lon(lon),
                    course
                );
                ax25::encode("M0SDR-9", "APSDRD", &["WIDE1-1", "WIDE2-1"], info.as_bytes())
            }
            1 => {
                let temp_f = 50.0 + 6.0 * (TAU * wall / 3600.0).sin();
                let info = format!(
                    "!5131.20N/00009.80W_090/005g010t{temp_f:03.0}r000p000P000h65b10132sdrd test weather"
                );
                ax25::encode("G0SDR-13", "APSDRD", &["WIDE2-1"], info.as_bytes())
            }
            _ => ax25::encode("M0SDR-10", "APSDRD", &["WIDE2-2"], b">sdrd synthetic APRS beacon"),
        },
        AprsChannel::Iss => {
            let a = TAU * (wall / 5520.0).fract();
            let lat = 51.6 * a.sin();
            let lon = (a.to_degrees() * 4.0).rem_euclid(360.0) - 180.0;
            // Symbol \S: satellite.
            let info = format!("!{}\\{}SARISS test", aprs_lat(lat), aprs_lon(lon));
            ax25::encode("RS0ISS", "CQ", &["ARISS"], info.as_bytes())
        }
    }
}

/// HDLC-frame an AX.25 frame (adds FCS, flags and bit stuffing) and NRZI
/// encode it into per-bit tones: true = mark (1200 Hz), false = space.
fn afsk_tones(frame: &[u8]) -> Vec<bool> {
    let mut bits: Vec<u8> = Vec::with_capacity(400 + frame.len() * 10);
    let flag = |bits: &mut Vec<u8>| bits.extend((0..8).map(|i| (0x7e >> i) & 1));
    for _ in 0..40 {
        flag(&mut bits);
    }
    let fcs = ax25::fcs(frame);
    let mut ones = 0;
    for &byte in frame.iter().chain(&fcs.to_le_bytes()) {
        for i in 0..8 {
            let b = (byte >> i) & 1;
            bits.push(b);
            if b == 1 {
                ones += 1;
                if ones == 5 {
                    bits.push(0);
                    ones = 0;
                }
            } else {
                ones = 0;
            }
        }
    }
    for _ in 0..3 {
        flag(&mut bits);
    }
    // NRZI: a 0 bit changes the tone, a 1 keeps it.
    let mut mark = true;
    bits.into_iter()
        .map(|b| {
            if b == 0 {
                mark = !mark;
            }
            mark
        })
        .collect()
}

/// On-intervals for one Nexus transmission: 12 rows of 36 bits, each bit a
/// 500 µs pulse followed by a 1000 µs (0) or 2000 µs (1) gap, rows separated
/// by a 4 ms sync gap. Layout: id(8) battery(1) test(1) channel(2) temp×10(12)
/// 1111 humidity(8).
fn nexus_pulses(start: f64, id: u8, channel: u8, temp_c: f64, humidity: u8) -> Vec<(f64, f64)> {
    let temp = ((temp_c * 10.0).round() as i32 as u32) & 0xfff;
    let word: u64 = (id as u64) << 28
        | 1 << 27 // battery ok
        | ((channel.clamp(1, 3) - 1) as u64) << 24
        | (temp as u64) << 12
        | 0xf << 8
        | humidity as u64;
    let mut out = Vec::with_capacity(12 * 37);
    let mut t = start;
    for _ in 0..12 {
        for i in (0..36).rev() {
            out.push((t, t + 500e-6));
            t += 500e-6 + if (word >> i) & 1 == 1 { 2000e-6 } else { 1000e-6 };
        }
        out.push((t, t + 500e-6));
        t += 500e-6 + 4000e-6;
    }
    out
}

/// Simulated receivers, so device selection and per-device frequency limits
/// can be tried without hardware.
pub(super) struct SimDevice {
    pub id: &'static str,
    pub name: &'static str,
    pub tuner: &'static str,
}

pub(super) const DEVICES: &[SimDevice] = &[
    SimDevice { id: "sim-r820t", name: "Simulated RTL-SDR Blog V3", tuner: "R820T/R860" },
    SimDevice { id: "sim-v4", name: "Simulated RTL-SDR Blog V4", tuner: "R828D" },
    SimDevice { id: "sim-e4000", name: "Simulated E4000 dongle", tuner: "E4000" },
    SimDevice { id: "sim-fc2580", name: "Simulated FC2580 dongle", tuner: "FC2580" },
];

fn sim(id: &str) -> &'static SimDevice {
    DEVICES.iter().find(|d| d.id == id).unwrap_or(&DEVICES[0])
}

pub(super) fn devices(active: Option<&str>) -> Vec<DeviceInfo> {
    DEVICES
        .iter()
        .map(|d| DeviceInfo {
            id: d.id.into(),
            source: SourceKind::Synthetic.name(),
            name: d.name.into(),
            serial: None,
            tuner: Some(d.tuner.into()),
            ranges: tuner_ranges(d.tuner, d.name),
            active: active == Some(d.id),
            note: None,
            used_by: None,
        })
        .collect()
}

fn select(ctx: &SourceCtx, s: &mut RadioSettings, id: &str) {
    let d = sim(id);
    let ranges = tuner_ranges(d.tuner, d.name);
    ctx.fit_to(s, &ranges);
    tracing::info!(device = d.name, "synthetic device selected");
    ctx.update_state(|st| {
        st.connected = true;
        st.device_id = Some(d.id.into());
        st.device = Some(d.name.into());
        st.tuner = Some(d.tuner.into());
        st.ranges = ranges;
        st.gains_db = vec![0.0, 9.0, 19.7, 29.7, 40.2, 49.6];
        st.settings = s.clone();
        st.error = None;
    });
}

/// Runs until a device of another kind is selected; returns its id.
pub(super) fn run(ctx: &mut SourceCtx) -> String {
    let mut s = ctx.current();
    let first = ctx.device.clone();
    select(ctx, &mut s, &first);
    tracing::info!("synthetic source running");

    let mut gen = SynthGen::new();
    let mut next = Instant::now();
    loop {
        let p = ctx.take();
        if let Some(n) = p.settings {
            s = n;
            ctx.update_state(|st| st.settings = s.clone());
        }
        if let Some(id) = p.device {
            if kind_of(&id) != Some(SourceKind::Synthetic) {
                return id;
            }
            select(ctx, &mut s, &id);
        }
        let n = (s.sample_rate as f64 * BLOCK_SECS) as usize;
        let data = gen.generate(s.center_hz, s.sample_rate, s.gain_db, n);
        ctx.publish(IqBlock {
            center_hz: s.center_hz,
            sample_rate: s.sample_rate,
            data,
        });
        next += Duration::from_secs_f64(BLOCK_SECS);
        let now = Instant::now();
        if next > now {
            sleep(next - now);
        } else {
            // Falling behind (slow CPU): don't try to catch up.
            next = now;
        }
    }
}
