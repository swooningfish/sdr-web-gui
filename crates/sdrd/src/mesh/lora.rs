//! LoRa physical layer: chirp spread spectrum, explicit header, CRC.
//!
//! Follows the layout described by the gr-lora_sdr project (EPFL) and the
//! Semtech datasheets: a preamble of up-chirps, two sync-word symbols, 2.25
//! down-chirps, then the payload. Each payload symbol carries SF bits (SF − 2
//! in the header block and with low data rate optimisation), Gray-mapped,
//! diagonally interleaved and Hamming coded; the first block is always sent
//! at 4/8 and holds the 20-bit explicit header. Payload bytes are whitened and
//! followed by a CRC-16.
//!
//! The demodulator takes [`OS`] samples per chip (sample rate = OS × bandwidth):
//! the caller resamples to exactly that. [`Waveform`] is the matching
//! transmitter, used by the simulator and the tests.

use std::sync::Arc;

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Spreading factor, 7–12.
    pub sf: u8,
    /// Bandwidth (= chip rate), Hz.
    pub bw: u32,
    /// Coding rate 4/(4 + cr), cr = 1–4.
    pub cr: u8,
    /// Sync word, e.g. 0x2B (Meshtastic) or 0x12 (MeshCore, "private").
    pub sync: u8,
    /// Preamble length in symbols, as transmitted.
    pub preamble: usize,
}

impl Params {
    pub fn n(&self) -> usize {
        1 << self.sf
    }

    /// Low data rate optimisation: on when a symbol lasts over 16 ms.
    pub fn ldro(&self) -> bool {
        self.n() as f64 / self.bw as f64 > 0.016
    }

    /// Bits a payload symbol carries after the header block.
    fn sf_app(&self) -> usize {
        if self.ldro() {
            self.sf as usize - 2
        } else {
            self.sf as usize
        }
    }
}

/// Whitening sequence: LFSR x⁸+x⁶+x⁵+x⁴+1 from 0xFF.
fn whitening() -> &'static [u8; 255] {
    static SEQ: std::sync::OnceLock<[u8; 255]> = std::sync::OnceLock::new();
    SEQ.get_or_init(|| {
        let mut seq = [0u8; 255];
        let mut s: u8 = 0xFF;
        for v in seq.iter_mut() {
            *v = s;
            let b = ((s >> 7) ^ (s >> 5) ^ (s >> 4) ^ (s >> 3)) & 1;
            s = (s << 1) | b;
        }
        seq
    })
}

/// CRC-16/CCITT (0x1021, init 0), as LoRa uses it: computed over all but the
/// last two payload bytes, then XORed with them.
pub fn payload_crc(p: &[u8]) -> u16 {
    fn crc16(data: &[u8]) -> u16 {
        let mut crc: u16 = 0;
        for &b in data {
            crc ^= (b as u16) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
            }
        }
        crc
    }
    if p.len() < 2 {
        return crc16(p);
    }
    let n = p.len();
    crc16(&p[..n - 2]) ^ (p[n - 1] as u16) ^ ((p[n - 2] as u16) << 8)
}

/// The 5-bit explicit header checksum over the first three header nibbles.
fn header_checksum(h: [u8; 3]) -> u8 {
    let b = |n: usize, i: u8| (h[n] >> i) & 1;
    let c4 = b(0, 3) ^ b(0, 2) ^ b(0, 1) ^ b(0, 0);
    let c3 = b(0, 3) ^ b(1, 3) ^ b(1, 2) ^ b(1, 1) ^ b(2, 0);
    let c2 = b(0, 2) ^ b(1, 3) ^ b(1, 0) ^ b(2, 3) ^ b(2, 1);
    let c1 = b(0, 1) ^ b(1, 2) ^ b(1, 0) ^ b(2, 2) ^ b(2, 1) ^ b(2, 0);
    let c0 = b(0, 0) ^ b(1, 1) ^ b(2, 3) ^ b(2, 2) ^ b(2, 1) ^ b(2, 0);
    (c4 << 4) | (c3 << 3) | (c2 << 2) | (c1 << 1) | c0
}

/// Hamming codeword for a nibble, `4 + cr` bits, MSB first: the data bits
/// LSB first, then the parity bits.
fn hamming_encode(nib: u8, cr: u8) -> u8 {
    let d = |i: u8| (nib >> i) & 1;
    let data = (d(0) << 3) | (d(1) << 2) | (d(2) << 1) | d(3);
    if cr == 1 {
        return (data << 1) | (d(0) ^ d(1) ^ d(2) ^ d(3));
    }
    let p0 = d(0) ^ d(1) ^ d(2);
    let p1 = d(1) ^ d(2) ^ d(3);
    let p2 = d(0) ^ d(1) ^ d(3);
    let p3 = d(0) ^ d(2) ^ d(3);
    let full = (data << 4) | (p0 << 3) | (p1 << 2) | (p2 << 1) | p3;
    full >> (4 - cr)
}

/// Nearest nibble to a received codeword (corrects one error at 4/7 and 4/8;
/// at 4/5 and 4/6 the data bits are taken as they are).
fn hamming_decode(cw: u8, cr: u8) -> u8 {
    let systematic = {
        let data = cw >> cr;
        ((data >> 3) & 1) | (((data >> 2) & 1) << 1) | (((data >> 1) & 1) << 2) | ((data & 1) << 3)
    };
    if cr < 3 {
        return systematic;
    }
    let mut best = (u32::MAX, systematic);
    for nib in 0..16u8 {
        let d = (hamming_encode(nib, cr) ^ cw).count_ones();
        if d < best.0 || (d == best.0 && nib == systematic) {
            best = (d, nib);
        }
    }
    best.1
}

fn gray_decode(mut g: u32) -> u32 {
    let mut v = g;
    while g > 0 {
        g >>= 1;
        v ^= g;
    }
    v
}

/// The payload as transmitted symbols (chirp shifts, 0..N), after the header.
pub fn encode(p: &Params, payload: &[u8]) -> Vec<u32> {
    assert!(payload.len() < 256);
    let wh = whitening();
    let len = payload.len() as u8;
    let mut nibbles = vec![len >> 4, len & 0xF, (p.cr << 1) | 1];
    let chk = header_checksum([nibbles[0], nibbles[1], nibbles[2]]);
    nibbles.push(chk >> 4);
    nibbles.push(chk & 0xF);
    for (i, &b) in payload.iter().enumerate() {
        let w = b ^ wh[i % 255];
        nibbles.push(w & 0xF);
        nibbles.push(w >> 4);
    }
    let crc = payload_crc(payload);
    for b in [(crc & 0xFF) as u8, (crc >> 8) as u8] {
        nibbles.push(b & 0xF);
        nibbles.push(b >> 4);
    }

    let n = p.n() as u32;
    let mut out = Vec::new();
    let mut i = 0;
    let mut first = true;
    while i < nibbles.len() {
        let (sf_app, cr) = if first { (p.sf as usize - 2, 4) } else { (p.sf_app(), p.cr) };
        let reduced = sf_app < p.sf as usize;
        let cw_len = 4 + cr as usize;
        let cws: Vec<u8> = (0..sf_app).map(|k| hamming_encode(*nibbles.get(i + k).unwrap_or(&0), cr)).collect();
        i += sf_app;
        for col in 0..cw_len {
            let mut v: u32 = 0;
            for j in 0..sf_app {
                let cw = cws[(col + 2 * sf_app - j - 1) % sf_app];
                let bit = (cw >> (cw_len - 1 - col)) & 1;
                v = (v << 1) | bit as u32;
            }
            let g = gray_decode(v);
            let s = if reduced { g << 2 } else { g };
            out.push((s + 1) % n);
        }
        first = false;
    }
    out
}

/// A packet as instantaneous frequency against time, for generating it at
/// any sample rate (the simulator) with continuous phase.
#[derive(Clone, Debug)]
pub struct Waveform {
    preamble: usize,
    sync: [u32; 2],
    payload: Vec<u32>,
    n: f64,
    bw: f64,
    /// Symbol duration, s.
    pub ts: f64,
}

impl Waveform {
    pub fn new(p: &Params, payload: &[u8]) -> Self {
        Waveform {
            preamble: p.preamble,
            sync: [((p.sync >> 4) as u32) << 3, ((p.sync & 0xF) as u32) << 3],
            payload: encode(p, payload),
            n: p.n() as f64,
            bw: p.bw as f64,
            ts: p.n() as f64 / p.bw as f64,
        }
    }

    pub fn duration(&self) -> f64 {
        (self.preamble as f64 + 4.25 + self.payload.len() as f64) * self.ts
    }

    /// Phase in cycles `t` seconds into the packet: the exact integral of
    /// [`freq_at`](Self::freq_at). (Summing the frequency sample by sample
    /// goes wrong where a chirp wraps from +bw/2 to −bw/2 mid-sample.)
    pub fn phase_at(&self, t: f64) -> Option<f64> {
        if t < 0.0 {
            return None;
        }
        let x = t / self.ts;
        let pre = (self.preamble + 2) as f64;
        // Every whole chirp integrates to zero; only the quarter one doesn't.
        let up = |s: u32, u: f64| {
            let a = s as f64 / self.n;
            let mut c = u * (a - 0.5) + u * u / 2.0;
            if a + u >= 1.0 {
                c -= a + u - 1.0;
            }
            c * self.n
        };
        let down = |u: f64| (u / 2.0 - u * u / 2.0) * self.n;
        if x < pre {
            let i = x as usize;
            let s = if i < self.preamble { 0 } else { self.sync[i - self.preamble] };
            return Some(up(s, x.fract()));
        }
        if x < pre + 2.25 {
            return Some(down((x - pre).fract()));
        }
        let y = x - pre - 2.25;
        let quarter = down(0.25);
        self.payload.get(y as usize).map(|&s| quarter + up(s, y.fract()))
    }

    /// Offset from the carrier (Hz) `t` seconds into the packet.
    pub fn freq_at(&self, t: f64) -> Option<f64> {
        if t < 0.0 {
            return None;
        }
        let x = t / self.ts;
        let pre = (self.preamble + 2) as f64;
        let up = |s: u32, u: f64| Some(self.bw * ((s as f64 / self.n + u).fract() - 0.5));
        if x < pre {
            let i = x as usize;
            let s = if i < self.preamble { 0 } else { self.sync[i - self.preamble] };
            return up(s, x.fract());
        }
        if x < pre + 2.25 {
            return Some(self.bw * (0.5 - (x - pre).fract()));
        }
        let y = x - pre - 2.25;
        self.payload.get(y as usize).and_then(|&s| up(s, y.fract()))
    }
}

/// One received packet.
#[derive(Clone, Debug)]
pub struct Frame {
    pub payload: Vec<u8>,
    pub crc_ok: bool,
    /// Signal to noise ratio in the bandwidth, dB.
    pub snr_db: f32,
    /// Frequency offset of the transmitter, Hz.
    pub offset_hz: f32,
    /// Sync word the packet was received on; only read by tests for now.
    #[allow(dead_code)]
    pub sync: u8,
}

enum State {
    Search,
    /// Preamble seen at chip `pos`, with its fractional CFO.
    Sync { pos: usize, frac: f32 },
    /// Header block starts at chip `start` (fractional).
    Payload { start: f64, cfo: i32, frac: f32, drift: f64, sync: u8, header: Option<Header> },
}

#[derive(Clone, Copy)]
struct Header {
    len: usize,
    cr: u8,
    crc: bool,
    /// Nibbles already decoded from the header block.
    first: [u8; 8],
    first_n: usize,
    symbols: usize,
}

/// Streaming demodulator for one LoRa configuration. Input is at [`OS`]
/// samples per chip; windows are read at any fractional chip position (cubic
/// interpolation), so each chirp is dechirped from its true start. At one
/// sample per chip a sub-sample offset can't be fixed afterwards: the jump
/// where a data chirp wraps lands mid-sample and splits the tone.
pub struct Demod {
    p: Params,
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    up0: Vec<Complex32>,
    buf: Vec<Complex32>,
    /// Next search window, in chips from the start of `buf`.
    pos: usize,
    /// Recent search peaks (bin, peak value).
    hist: Vec<(usize, Complex32)>,
    state: State,
    /// Carrier frequency, for the transmitter's chip-rate error.
    carrier_hz: f64,
    scratch: Vec<Complex32>,
    snr_acc: (f64, usize),
}

/// Samples per chip the demodulator expects.
pub const OS: usize = 4;
/// Consecutive matching windows that make a preamble.
const PREAMBLE_MIN: usize = 5;
/// Peak to mean bin power for a window to count.
const PEAK_RATIO: f32 = 8.0;

impl Demod {
    pub fn new(p: Params, carrier_hz: f64) -> Self {
        let n = p.n();
        let fft = FftPlanner::new().plan_fft_forward(n);
        let up0 = (0..n)
            .map(|k| {
                let x = k as f64;
                let ph = std::f64::consts::TAU * (x * x / (2.0 * n as f64) - 0.5 * x);
                Complex32::new(ph.cos() as f32, ph.sin() as f32)
            })
            .collect();
        Demod {
            p,
            n,
            fft,
            up0,
            buf: Vec::new(),
            pos: 0,
            hist: Vec::new(),
            state: State::Search,
            carrier_hz,
            scratch: vec![Complex32::default(); n],
            snr_acc: (0.0, 0),
        }
    }

    /// Whether the buffer holds a whole window starting at chip `x`.
    fn have(&self, x: f64) -> bool {
        x >= 0.0 && ((x + self.n as f64) * OS as f64).ceil() as usize + 3 <= self.buf.len()
    }

    /// The input at fractional sample index `i` (Catmull-Rom).
    fn sample(&self, i: f64) -> Complex32 {
        let k = i.floor() as usize;
        let t = (i - k as f64) as f32;
        if t == 0.0 || k == 0 {
            return self.buf[k];
        }
        let (p0, p1, p2, p3) = (self.buf[k - 1], self.buf[k], self.buf[k + 1], self.buf[k + 2]);
        let a = p1 * 2.0;
        let b = p2 - p0;
        let c = p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3;
        let d = -p0 + p1 * 3.0 - p2 * 3.0 + p3;
        (a + b * t + c * (t * t) + d * (t * t * t)) * 0.5
    }

    /// Dechirp the window starting at chip `x` (against an up-chirp, or a
    /// down-chirp when `down`), with `frac` bins of frequency offset removed.
    /// Returns the peak bin, its value and the mean power of the other bins.
    fn peak(&mut self, x: f64, down: bool, frac: f32) -> (usize, Complex32, f32) {
        let n = self.n;
        let w = -std::f32::consts::TAU * frac / n as f32;
        let base = x * OS as f64;
        let whole = x.fract() == 0.0;
        for k in 0..n {
            let r = if down { self.up0[k] } else { self.up0[k].conj() };
            let rot = Complex32::from_polar(1.0, w * k as f32);
            let s = if whole { self.buf[base as usize + k * OS] } else { self.sample(base + (k * OS) as f64) };
            self.scratch[k] = s * r * rot;
        }
        self.fft.process(&mut self.scratch);
        let mut best = (0, 0f32);
        let mut total = 0f32;
        for (k, v) in self.scratch.iter().enumerate() {
            let p = v.norm_sqr();
            total += p;
            if p > best.1 {
                best = (k, p);
            }
        }
        // The tone may fall between bins: count its near neighbours as signal.
        let near: f32 = (n - 2..n + 3).map(|d| self.scratch[(best.0 + d) % n].norm_sqr()).sum();
        let mean = (total - near).max(0.0) / (n - 5) as f32;
        (best.0, self.scratch[best.0], mean)
    }

    /// Feed samples at exactly `OS × bw` samples a second.
    pub fn feed(&mut self, x: &[Complex32], out: &mut Vec<Frame>) {
        self.buf.extend_from_slice(x);
        while self.step(out) {}
        // Keep a little history for the timing correction's look-back.
        let keep_from = match &self.state {
            State::Search => self.pos.saturating_sub(2 * self.n),
            State::Sync { pos, .. } => pos.saturating_sub(2 * self.n),
            State::Payload { start, .. } => (*start as usize).saturating_sub(2 * self.n),
        }
        .min(self.pos);
        if keep_from > 16 * self.n {
            self.buf.drain(..keep_from * OS);
            self.pos -= keep_from;
            match &mut self.state {
                State::Search => {}
                State::Sync { pos, .. } => *pos -= keep_from,
                State::Payload { start, .. } => *start -= keep_from as f64,
            }
        }
    }

    fn wrap(&self, v: i64) -> usize {
        v.rem_euclid(self.n as i64) as usize
    }

    fn signed(&self, v: i64) -> i64 {
        let n = self.n as i64;
        let m = v.rem_euclid(n);
        if m > n / 2 {
            m - n
        } else {
            m
        }
    }

    /// Advance as far as the buffer allows; false when waiting for samples.
    fn step(&mut self, out: &mut Vec<Frame>) -> bool {
        let n = self.n;
        let nf = n as f64;
        match self.state {
            State::Search => {
                if !self.have(self.pos as f64) {
                    return false;
                }
                let (bin, val, mean) = self.peak(self.pos as f64, false, 0.0);
                if val.norm_sqr() < PEAK_RATIO * mean {
                    self.hist.clear();
                } else {
                    if let Some(&(b, _)) = self.hist.last() {
                        if bin != b {
                            self.hist.clear();
                        }
                    }
                    self.hist.push((bin, val));
                }
                self.pos += n;
                if self.hist.len() >= PREAMBLE_MIN {
                    // Fractional CFO from the phase step between windows
                    // (same bin each time: neighbouring bins differ by ~π).
                    let mut acc = Complex32::default();
                    for w in self.hist.windows(2) {
                        acc += w[1].1 * w[0].1.conj();
                    }
                    let frac = acc.arg() / std::f32::consts::TAU;
                    self.hist.clear();
                    self.state = State::Sync { pos: self.pos - n, frac };
                }
                true
            }
            State::Sync { pos, frac } => {
                // Look for the down-chirps, then solve for timing and CFO.
                let span = (self.p.preamble + 8) * n;
                if !self.have((pos + span + 2 * n) as f64) {
                    return false;
                }
                let mut found = None;
                let mut q = pos;
                while q + 2 * n <= pos + span {
                    let (_, uv, _) = self.peak(q as f64, false, frac);
                    let (_, dv, dm) = self.peak(q as f64, true, frac);
                    if dv.norm_sqr() > uv.norm_sqr() && dv.norm_sqr() > PEAK_RATIO * dm {
                        // Of this window and the next, take the stronger.
                        let (_, v2, _) = self.peak((q + n) as f64, true, frac);
                        found = Some(if v2.norm_sqr() > dv.norm_sqr() { q + n } else { q });
                        break;
                    }
                    q += n;
                }
                let Some(qd) = found else {
                    self.state = State::Search;
                    self.pos = pos + n;
                    return true;
                };
                // Fractional timing from where the preamble's tone falls
                // between bins (once the fractional CFO is out): up-chirps
                // read the delay low, down-chirps high.
                let (b0, _, _) = self.peak(pos as f64, false, frac);
                let tau_frac = -self.delta(b0) as f64;
                let (ku, _, _) = self.peak(pos as f64 + tau_frac, false, frac);
                let (kd, _, _) = self.peak(qd as f64 + tau_frac, true, frac);
                let (ku, kd) = (ku as i64, kd as i64);
                // ku = c − τ, kd = c + τ (mod N); pick the smaller CFO.
                let mut c = self.signed(ku + kd) / 2;
                if (ku + kd).rem_euclid(2) != 0 {
                    c = self.signed(ku + kd + 1) / 2;
                }
                let alt = self.signed(c + n as i64 / 2);
                if alt.abs() < c.abs() {
                    c = alt;
                }
                let tau = self.signed(kd - c);
                // Aligned grid (chirp starts); find the first down-chirp on it.
                let g0 = pos as f64 + tau as f64 + tau_frac;
                let mut w = g0 - nf;
                let mut sfd = None;
                while w + nf <= (pos + span) as f64 {
                    if w >= 0.0 {
                        let (_, uv, _) = self.peak(w, false, frac);
                        let (_, dv, _) = self.peak(w, true, frac);
                        if dv.norm_sqr() > uv.norm_sqr() {
                            sfd = Some(w);
                            break;
                        }
                    }
                    w += nf;
                }
                let Some(sfd) = sfd else {
                    self.state = State::Search;
                    self.pos = pos + n;
                    return true;
                };
                // Sync word from the two symbols before the down-chirps.
                let mut sync = 0u8;
                if sfd >= 2.0 * nf {
                    for (i, at) in [sfd - 2.0 * nf, sfd - nf].into_iter().enumerate() {
                        let (b, _, _) = self.peak(at, false, frac);
                        let v = (self.wrap(b as i64 - c) + 4) / 8;
                        sync |= ((v & 0xF) as u8) << (4 * (1 - i));
                    }
                }
                let cfo_hz = (c as f64 + frac as f64) * self.p.bw as f64 / nf;
                // A fast transmitter clock raises its carrier and shortens its chips.
                let drift = -cfo_hz / self.carrier_hz;
                self.snr_acc = (0.0, 0);
                self.state = State::Payload { start: sfd + 2.25 * nf, cfo: c as i32, frac, drift, sync, header: None };
                true
            }
            State::Payload { start, cfo, frac, drift, sync, header } => {
                // Where symbol i starts (fractional chips).
                let at = |i: usize| start + i as f64 * nf * (1.0 + drift);
                let Some(h) = header else {
                    if !self.have(at(7)) {
                        return false;
                    }
                    let syms = self.symbols(at, 0, 8, cfo, frac, true);
                    let nib = self.block(&syms, self.p.sf as usize - 2, 4);
                    let len = ((nib[0] << 4) | nib[1]) as usize;
                    let cr = nib[2] >> 1;
                    let crc = nib[2] & 1 == 1;
                    let ok = header_checksum([nib[0], nib[1], nib[2]]) == ((nib[3] & 1) << 4 | nib[4]);
                    if !ok || !(1..=4).contains(&cr) || len == 0 {
                        self.state = State::Search;
                        self.pos = at(8) as usize;
                        return true;
                    }
                    let mut first = [0u8; 8];
                    let first_n = nib.len() - 5;
                    first[..first_n].copy_from_slice(&nib[5..]);
                    let p = Params { cr, ..self.p };
                    let nibbles = 2 * len + if crc { 4 } else { 0 };
                    let rest = nibbles.saturating_sub(first_n);
                    let symbols = rest.div_ceil(p.sf_app()) * (4 + cr as usize);
                    let h = Header { len, cr, crc, first, first_n, symbols };
                    self.state = State::Payload { start, cfo, frac, drift, sync, header: Some(h) };
                    return true;
                };
                let last = 8 + h.symbols;
                if !self.have(at(last.max(9) - 1)) {
                    return false;
                }
                let p = Params { cr: h.cr, ..self.p };
                let sf_app = p.sf_app();
                let reduced = sf_app < p.sf as usize;
                let cw_len = 4 + h.cr as usize;
                let mut nibbles: Vec<u8> = h.first[..h.first_n].to_vec();
                let mut i = 8;
                while i < last {
                    let syms = self.symbols(at, i, cw_len, cfo, frac, reduced);
                    nibbles.extend(self.block(&syms, sf_app, h.cr));
                    i += cw_len;
                }
                let wh = whitening();
                let mut payload = Vec::with_capacity(h.len);
                for k in 0..h.len {
                    let b = nibbles.get(2 * k).copied().unwrap_or(0) | (nibbles.get(2 * k + 1).copied().unwrap_or(0) << 4);
                    payload.push(b ^ wh[k % 255]);
                }
                let crc_ok = if h.crc {
                    let b = |k: usize| nibbles.get(k).copied().unwrap_or(0);
                    let o = 2 * h.len;
                    let rx = (b(o) | b(o + 1) << 4) as u16 | ((b(o + 2) | b(o + 3) << 4) as u16) << 8;
                    rx == payload_crc(&payload)
                } else {
                    true
                };
                let snr = if self.snr_acc.1 > 0 { self.snr_acc.0 / self.snr_acc.1 as f64 } else { 0.0 };
                // Peak to noise in one bin over N bins → SNR in the bandwidth.
                let snr_db = (10.0 * snr.max(1e-3).log10() - 10.0 * nf.log10()) as f32;
                out.push(Frame {
                    payload,
                    crc_ok,
                    snr_db,
                    offset_hz: ((cfo as f32 + frac) * self.p.bw as f32 / n as f32),
                    sync,
                });
                self.state = State::Search;
                self.pos = at(last) as usize;
                true
            }
        }
    }

    /// Demodulate `count` symbols from index `from` into their Gray-coded bit
    /// values (`sf − 2` bits when `reduced`).
    fn symbols(&mut self, at: impl Fn(usize) -> f64, from: usize, count: usize, cfo: i32, frac: f32, reduced: bool) -> Vec<u32> {
        let n = self.n as i64;
        (from..from + count)
            .map(|i| {
                let (b, v, mean) = self.peak(at(i), false, frac);
                // Signal over noise in the peak bin (capped for a clean signal).
                let p = v.norm_sqr();
                self.snr_acc.0 += ((p - mean).max(0.0) / mean.max(p * 1e-7)) as f64;
                self.snr_acc.1 += 1;
                let k = (b as i64 - cfo as i64 - 1).rem_euclid(n);
                let s = if reduced { (((k + 2).rem_euclid(n)) >> 2) as u32 } else { k as u32 };
                s ^ (s >> 1)
            })
            .collect()
    }

    /// Where the tone in the last window falls between bins, −0.5 to 0.5
    /// from the peak bin `k` (Jacobsen's estimator).
    fn delta(&self, k: usize) -> f32 {
        let n = self.n;
        let (a, b, c) = (self.scratch[(k + n - 1) % n], self.scratch[k], self.scratch[(k + 1) % n]);
        let den = b * 2.0 - a - c;
        if den.norm_sqr() == 0.0 {
            return 0.0;
        }
        ((a - c) / den).re.clamp(-0.5, 0.5)
    }

    /// Deinterleave one block of `4 + cr` symbols into `sf_app` nibbles.
    fn block(&self, syms: &[u32], sf_app: usize, cr: u8) -> Vec<u8> {
        let cw_len = 4 + cr as usize;
        let mut cws = vec![0u8; sf_app];
        for (col, &v) in syms.iter().enumerate().take(cw_len) {
            for j in 0..sf_app {
                let bit = ((v >> (sf_app - 1 - j)) & 1) as u8;
                let k = (col + 2 * sf_app - j - 1) % sf_app;
                cws[k] |= bit << (cw_len - 1 - col);
            }
        }
        cws.into_iter().map(|cw| hamming_decode(cw, cr)).collect()
    }
}

/// Cubic (Catmull-Rom) resampler from any rate to another, for getting the
/// down-converted channel to exactly one sample per chip.
pub struct Resampler {
    step: f64,
    pos: f64,
    hist: Vec<Complex32>,
}

impl Resampler {
    pub fn new(in_rate: f64, out_rate: f64) -> Self {
        Resampler { step: in_rate / out_rate, pos: 1.0, hist: vec![Complex32::default(); 3] }
    }

    pub fn process(&mut self, x: &[Complex32], out: &mut Vec<Complex32>) {
        self.hist.extend_from_slice(x);
        // Interpolate between hist[i] and hist[i + 1], needing i − 1 and i + 2.
        while self.pos + 2.0 < self.hist.len() as f64 {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let (p0, p1, p2, p3) = (self.hist[i - 1], self.hist[i], self.hist[i + 1], self.hist[i + 2]);
            let a = p1 * 2.0;
            let b = p2 - p0;
            let c = p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3;
            let d = -p0 + p1 * 3.0 - p2 * 3.0 + p3;
            out.push((a + b * t + c * (t * t) + d * (t * t * t)) * 0.5);
            self.pos += self.step;
        }
        let drop = (self.pos.floor() as usize).saturating_sub(1);
        self.hist.drain(..drop);
        self.pos -= drop as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(sf: u8, bw: u32, cr: u8, sync: u8) -> Params {
        Params { sf, bw, cr, sync, preamble: 16 }
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f32 {
            let (u, v) = (self.next().max(1e-12), self.next());
            ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
        }
    }

    /// The packet `lead` samples (fractional) into the capture, from a
    /// transmitter `ppm` off (carrier and chip clock alike), in noise.
    fn channel(p: &Params, payload: &[u8], lead: f64, ppm: f64, noise: f32, seed: u64) -> Vec<Complex32> {
        let w = Waveform::new(p, payload);
        let bw = p.bw as f64;
        let eps = ppm * 1e-6;
        let cfo = eps * CARRIER;
        let total = ((lead + w.duration() * bw) as usize + 20 * p.n()) * OS;
        let mut rng = Rng(seed);
        (0..total)
            .map(|k| {
                let t = (k as f64 / OS as f64 - lead) / bw;
                let mut v = Complex32::new(rng.gauss(), rng.gauss()) * (noise / std::f32::consts::SQRT_2);
                if let Some(c) = w.phase_at(t * (1.0 + eps)) {
                    let ph = std::f64::consts::TAU * (c + cfo * t).fract();
                    v += Complex32::from_polar(1.0, ph as f32);
                }
                v
            })
            .collect()
    }

    const CARRIER: f64 = 869.5e6;

    fn run(p: Params, payload: &[u8], lead: f64, ppm: f64, noise: f32) -> Vec<Frame> {
        let x = channel(&p, payload, lead, ppm, noise, 0x1234_5678 ^ lead as u64);
        let mut d = Demod::new(p, CARRIER);
        let mut out = Vec::new();
        for chunk in x.chunks(3001) {
            d.feed(chunk, &mut out);
        }
        out
    }

    #[test]
    fn hamming_round_trip_and_correction() {
        for cr in 1..=4 {
            for nib in 0..16 {
                assert_eq!(hamming_decode(hamming_encode(nib, cr), cr), nib);
            }
        }
        for nib in 0..16 {
            let cw = hamming_encode(nib, 4);
            for bit in 0..8 {
                assert_eq!(hamming_decode(cw ^ (1 << bit), 4), nib);
            }
        }
    }

    #[test]
    fn phase_is_the_integral_of_frequency() {
        let p = params(7, 125_000, 1, 0x12);
        let w = Waveform::new(&p, b"abc");
        let dt = w.ts / 4096.0;
        let mut acc = 0.0;
        let mut t = 0.0;
        while t + dt < w.duration() {
            acc += w.freq_at(t + dt / 2.0).unwrap() * dt;
            t += dt;
            let d = (acc - w.phase_at(t).unwrap()).rem_euclid(1.0);
            assert!(d.min(1.0 - d) < 0.02, "at {t}: {acc} vs {}", w.phase_at(t).unwrap());
        }
    }

    #[test]
    fn whitening_sequence_start() {
        assert_eq!(&whitening()[..8], &[0xFF, 0xFE, 0xFC, 0xF8, 0xF0, 0xE1, 0xC2, 0x85]);
    }

    #[test]
    fn decodes_clean_packets_at_each_preset() {
        let msg: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(37) ^ 0x5A).collect();
        for p in [params(11, 250_000, 1, 0x2B), params(8, 62_500, 4, 0x12), params(7, 125_000, 2, 0x12), params(12, 125_000, 1, 0x2B)] {
            assert_eq!(p.ldro(), p.sf == 12);
            let f = run(p, &msg, 3.0 * p.n() as f64 + 77.4, 0.0, 0.0);
            assert_eq!(f.len(), 1, "{p:?}");
            assert!(f[0].crc_ok, "{p:?}");
            assert_eq!(f[0].payload, msg, "{p:?}");
            assert_eq!(f[0].sync, p.sync, "{p:?}");
            // A clean signal reads strong (the estimate has a floor of its
            // own at low SF, from interpolation; real noise dominates it).
            assert!(f[0].snr_db > 5.0, "{p:?}: {}", f[0].snr_db);
        }
    }

    #[test]
    fn copes_with_timing_frequency_errors_and_noise() {
        let msg = b"Hello mesh, this is a test of the LoRa demodulator!".to_vec();
        let p = params(8, 62_500, 4, 0x12);
        // Noise at 0 dB SNR per sample, well within SF8's reach.
        for (lead, ppm) in [(1000.3, 10.0), (5123.5, -12.0), (777.9, 4.4), (2048.0, -0.07)] {
            let f = run(p, &msg, lead, ppm, 1.0);
            assert_eq!(f.len(), 1, "lead {lead} ppm {ppm}");
            assert!(f[0].crc_ok, "lead {lead} ppm {ppm}: {:?}", f[0]);
            assert_eq!(f[0].payload, msg);
            let want = (ppm * 1e-6 * CARRIER) as f32;
            assert!((f[0].offset_hz - want).abs() < 100.0, "{} vs {want}", f[0].offset_hz);
        }
        // LongFast at −9.5 dB, a transmitter 15 ppm off, and SF12 with LDRO.
        let p = params(11, 250_000, 1, 0x2B);
        let f = run(p, &msg, 9000.6, -15.0, 3.0);
        assert_eq!(f.len(), 1);
        assert!(f[0].crc_ok, "{:?}", f[0]);
        assert!((-12.0..-7.0).contains(&f[0].snr_db), "snr {}", f[0].snr_db);
        let p = params(12, 125_000, 1, 0x2B);
        let f = run(p, &msg, 4321.2, 8.0, 2.0);
        assert!(f.len() == 1 && f[0].crc_ok, "{f:?}");
    }

    #[test]
    fn nothing_from_noise() {
        let p = params(8, 62_500, 4, 0x12);
        let mut rng = Rng(99);
        let x: Vec<Complex32> = (0..200 * p.n() * OS).map(|_| Complex32::new(rng.gauss(), rng.gauss())).collect();
        let mut d = Demod::new(p, CARRIER);
        let mut out = Vec::new();
        d.feed(&x, &mut out);
        assert!(out.iter().all(|f| !f.crc_ok));
    }

    #[test]
    fn resampler_keeps_a_tone() {
        let (fi, fo) = (128_000.0, 62_500.0);
        let tone: Vec<Complex32> = (0..20_000)
            .map(|k| Complex32::from_polar(1.0, (std::f64::consts::TAU * 5_000.0 * k as f64 / fi) as f32))
            .collect();
        let mut r = Resampler::new(fi, fo);
        let mut out = Vec::new();
        for c in tone.chunks(999) {
            r.process(c, &mut out);
        }
        let expect = (20_000.0 * fo / fi) as usize;
        assert!((out.len() as i64 - expect as i64).abs() < 4, "{} vs {expect}", out.len());
        let step = (std::f64::consts::TAU * 5_000.0 / fo) as f32;
        for w in out[10..].windows(2) {
            let d = (w[1] * w[0].conj()).arg();
            assert!((d - step).abs() < 0.01, "{d} vs {step}");
            assert!((w[1].norm() - 1.0).abs() < 0.02);
        }
    }
}
