//! Synthetic IQ source: noise plus a few emitters at real-world frequencies so
//! the UI can be developed and tested without a dongle. Retuning works – the
//! emitters are placed at absolute frequencies and appear when in band.

use std::f64::consts::TAU;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{to_cu8, IqBlock, SourceCtx};

const BLOCK_SECS: f64 = 0.05;

#[derive(Clone, Copy)]
enum Kind {
    /// APRS-like: FM with 1200/2200 Hz AFSK, short bursts.
    Afsk { period: f64, burst: f64 },
    /// FM carrier with a sweeping audio tone (SSTV-ish).
    SweepFm,
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
    bit: bool,
    /// Next transmission time and its on-intervals (absolute seconds).
    next: f64,
    pulses: Vec<(f64, f64)>,
    pulse_idx: usize,
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
            bit: false,
            next: 0.0,
            pulses: Vec::new(),
            pulse_idx: 0,
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
                e(144_800_000.0, 0.25, Kind::Afsk { period: 6.0, burst: 0.9 }),
                e(145_825_000.0, 0.08, Kind::Afsk { period: 11.0, burst: 0.7 }),
                e(145_800_000.0, 0.15, Kind::SweepFm),
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
                // Random data at ~1200 baud: each bit period has a 50 % chance of a change.
                let flip = self.uniform() < 600.0 * dt;
                let em = &mut self.emitters[idx];
                let offset = em.freq_hz - center_hz as f64;
                let (inst_hz, gate) = match em.kind {
                    Kind::Afsk { period, burst } => {
                        if flip {
                            em.bit = !em.bit;
                        }
                        let tone = if em.bit { 2200.0 } else { 1200.0 };
                        em.tone_phase = (em.tone_phase + TAU * tone * dt) % TAU;
                        (offset + 3000.0 * em.tone_phase.sin(), (t % period) < burst)
                    }
                    Kind::SweepFm => {
                        let tone = 1500.0 + 800.0 * ((t * 2.0) % 1.0);
                        em.tone_phase = (em.tone_phase + TAU * tone * dt) % TAU;
                        (offset + 3000.0 * em.tone_phase.sin(), true)
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
                                let wall = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .map(|d| d.as_secs_f64())
                                    .unwrap_or(0.0);
                                let temp = base_c + 3.0 * (TAU * wall / 1800.0).sin();
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
