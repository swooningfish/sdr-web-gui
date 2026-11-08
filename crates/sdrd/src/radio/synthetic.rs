//! Synthetic IQ source: noise plus a few emitters at real-world frequencies so
//! the UI can be developed and tested without a dongle. Retuning works – the
//! emitters are placed at absolute frequencies and appear when in band.

use std::f64::consts::TAU;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{to_cu8, IqBlock, SourceCtx};
use crate::aprs::ax25;
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
}

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
        SynthGen {
            emitters: vec![
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
            ],
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
                };
                em.phase = (em.phase + TAU * inst_hz * dt) % TAU;
                if gate {
                    let a = em.amp * scale;
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

pub(super) fn run(ctx: SourceCtx) {
    let mut s = ctx.initial.clone();
    ctx.update_state(|st| {
        st.connected = true;
        st.device = Some("Synthetic test source".into());
        st.tuner = Some("simulated".into());
        st.gains_db = vec![0.0, 9.0, 19.7, 29.7, 40.2, 49.6];
        st.settings = s.clone();
    });
    tracing::info!("synthetic source running");

    let mut gen = SynthGen::new();
    let mut next = Instant::now();
    loop {
        if let Some(n) = ctx.latest_cmd() {
            s = n;
            ctx.update_state(|st| st.settings = s.clone());
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
