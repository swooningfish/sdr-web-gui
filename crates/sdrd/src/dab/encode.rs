//! A DAB mode I transmitter for the tests and the simulated receiver: the
//! FIC names the ensemble and its services and, for services with audio,
//! says which sub-channel carries it; the MSC carries those sub-channels
//! (the rest is random data).

use num_complex::Complex32;
use rustfft::FftPlanner;

use super::fic::{conv_encode, crc16, ebu_byte, prbs, sent, BLOCK_BITS};
use super::msc::{interleave_loop, protect, AudioKind, Protection, SubChannel, CIF_BITS, CU_BITS};
use super::{bin, interleaver, prs, K, L, T_F, T_G, T_NULL, T_U};

/// FIBs per frame.
const FIBS: usize = 12;

/// A service's audio: its sub-channel and the logical frames it sends
/// (repeated).
pub struct Audio {
    pub sid: u16,
    pub sub: SubChannel,
    pub kind: AudioKind,
    pub frames: Vec<Vec<u8>>,
}

/// An ensemble to transmit.
pub struct Spec<'a> {
    pub eid: u16,
    pub label: &'a str,
    /// Programme services: (service id, label).
    pub services: &'a [(u16, &'a str)],
    pub audio: &'a [Audio],
}

/// FIG 1 with a 16-bit id: ensemble (ext 0) or programme service (ext 1)
/// label in EBU Latin, the first eight characters as the short label.
fn fig1(ext: u8, id: u16, label: &str) -> Vec<u8> {
    let mut text: Vec<u8> = label.chars().map(ebu_byte).take(16).collect();
    let short = text.len().min(8);
    text.resize(16, b' ');
    let flags: u16 = !(u16::MAX >> short);
    let mut f = vec![0x20 | 21, ext];
    f.extend_from_slice(&id.to_be_bytes());
    f.extend_from_slice(&text);
    f.extend_from_slice(&flags.to_be_bytes());
    f
}

/// A FIG 0 with this extension and body.
fn fig0(ext: u8, body: &[u8]) -> Vec<u8> {
    let mut f = vec![(body.len() + 1) as u8, ext];
    f.extend_from_slice(body);
    f
}

/// FIG 0/0: ensemble id, no change, CIF counter 0.
fn fig0_0(eid: u16) -> Vec<u8> {
    let [a, b] = eid.to_be_bytes();
    fig0(0, &[a, b, 0, 0])
}

/// FIG 0/1 (long form) for these sub-channels.
fn fig0_1(subs: &[&SubChannel]) -> Vec<u8> {
    let mut body = Vec::new();
    for s in subs {
        let (option, level) = match s.protection {
            Protection::EepA(l) => (0, l),
            Protection::EepB(l) => (1, l),
            _ => panic!("the simulator sends EEP only"),
        };
        body.push(s.id << 2 | (s.start >> 8) as u8);
        body.push(s.start as u8);
        body.push(0x80 | option << 4 | (level - 1) << 2 | (s.size >> 8) as u8);
        body.push(s.size as u8);
    }
    fig0(1, &body)
}

/// FIG 0/2: each service's one audio component.
fn fig0_2(audio: &[&Audio]) -> Vec<u8> {
    let mut body = Vec::new();
    for a in audio {
        body.extend_from_slice(&a.sid.to_be_bytes());
        body.push(1); // one component
        let ascty = match a.kind {
            AudioKind::DabPlus => 63,
            _ => 0,
        };
        body.push(ascty); // TMId 0
        body.push(a.sub.id << 2 | 0x02); // primary
    }
    fig0(2, &body)
}

/// The 12 FIBs of a frame (CRC added): the FIGs packed in order.
fn fibs(spec: &Spec) -> Vec<[u8; 32]> {
    let mut figs = vec![fig0_0(spec.eid), fig1(0, spec.eid, spec.label)];
    figs.extend(spec.services.iter().map(|&(sid, name)| fig1(1, sid, name)));
    let audio: Vec<&Audio> = spec.audio.iter().collect();
    for chunk in audio.chunks(5) {
        figs.push(fig0_1(&chunk.iter().map(|a| &a.sub).collect::<Vec<_>>()));
        figs.push(fig0_2(chunk));
    }
    let mut out: Vec<Vec<u8>> = vec![Vec::new()];
    for f in figs {
        if out.last().unwrap().len() + f.len() > 30 {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().extend_from_slice(&f);
    }
    assert!(out.len() <= FIBS, "too much for one frame's FIC");
    while out.len() < FIBS {
        out.push(fig0_0(spec.eid));
    }
    out.into_iter()
        .map(|mut data| {
            if data.len() < 30 {
                data.push(0xFF); // end marker, then padding
            }
            data.resize(30, 0);
            let mut fib = [0u8; 32];
            fib[..30].copy_from_slice(&data);
            fib[30..].copy_from_slice(&crc16(&data).to_be_bytes());
            fib
        })
        .collect()
}

/// The frame's FIC bits after coding: 4 blocks of 2304.
fn fic_bits(spec: &Spec) -> Vec<u8> {
    let mask = sent();
    let dispersal = prbs(BLOCK_BITS);
    let mut out = Vec::new();
    for block in fibs(spec).chunks(3) {
        let bits: Vec<u8> = block
            .iter()
            .flat_map(|fib| fib.iter().flat_map(|&b| (0..8).rev().map(move |i| (b >> i) & 1)))
            .zip(&dispersal)
            .map(|(b, p)| b ^ p)
            .collect();
        let coded = conv_encode(&bits);
        out.extend(coded.iter().zip(&mask).filter(|(_, &s)| s).map(|(&b, _)| b));
    }
    out
}

/// `count` transmission frames at 2.048 MS/s (null symbol first), which
/// repeat seamlessly, scaled to an average power of 1 outside the nulls.
/// `seed` varies the unused MSC data.
pub fn frames(spec: &Spec, count: usize, seed: u64) -> Vec<Vec<Complex32>> {
    let fic = fic_bits(spec);
    let map = interleaver();
    let ifft = FftPlanner::<f32>::new().plan_fft_inverse(T_U);
    let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut random_bit = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng & 1) as u8
    };
    let cifs = count * 4;
    // Each sub-channel's CIF bits over the loop, time-interleaved.
    let subs: Vec<(usize, Vec<Vec<u8>>)> = spec
        .audio
        .iter()
        .map(|a| {
            let coded: Vec<Vec<u8>> = (0..cifs).map(|t| protect(&a.sub, &a.frames[t % a.frames.len()])).collect();
            (a.sub.start as usize * CU_BITS, interleave_loop(&coded))
        })
        .collect();
    let qpsk = |a: u8, b: u8| Complex32::new(1.0 - 2.0 * a as f32, 1.0 - 2.0 * b as f32) * std::f32::consts::FRAC_1_SQRT_2;
    let mut out = Vec::with_capacity(count);
    for f in 0..count {
        let mut bits: Vec<u8> = fic.clone();
        for c in 0..4 {
            let mut cif: Vec<u8> = (0..CIF_BITS).map(|_| random_bit()).collect();
            for (start, loop_bits) in &subs {
                let b = &loop_bits[f * 4 + c];
                cif[*start..*start + b.len()].copy_from_slice(b);
            }
            bits.extend_from_slice(&cif);
        }
        let mut samples = vec![Complex32::default(); T_NULL];
        let mut carriers = prs();
        for l in 0..L {
            if l > 0 {
                let b = &bits[(l - 1) * 2 * K..l * 2 * K];
                for n in 0..K {
                    carriers[bin(map[n])] *= qpsk(b[n], b[K + n]);
                }
            }
            let mut t = carriers.clone();
            ifft.process(&mut t);
            samples.extend_from_slice(&t[T_U - T_G..]);
            samples.extend_from_slice(&t);
        }
        debug_assert_eq!(samples.len(), T_F);
        let power = samples[T_NULL..].iter().map(|z| z.norm_sqr()).sum::<f32>() / (T_F - T_NULL) as f32;
        let scale = 1.0 / power.sqrt();
        samples.iter_mut().for_each(|z| *z *= scale);
        out.push(samples);
    }
    out
}

/// One frame (for a multiplex with no audio, where every frame is alike).
#[cfg(test)]
pub fn frame(spec: &Spec, seed: u64) -> Vec<Complex32> {
    frames(spec, 1, seed).remove(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::{FIC_BLOCK, FIC_SYMBOLS};

    #[test]
    fn fibs_check_and_fic_fills_three_symbols() {
        let spec = Spec { eid: 0xC181, label: "SIM DAB", services: &[(0xC221, "One"), (0xC222, "Two")], audio: &[] };
        let f = fibs(&spec);
        assert_eq!(f.len(), 12);
        assert!(f.iter().all(|fib| crc16(&fib[..30]) == u16::from_be_bytes([fib[30], fib[31]])));
        assert_eq!(fic_bits(&spec).len(), 4 * FIC_BLOCK);
        assert_eq!(4 * FIC_BLOCK, FIC_SYMBOLS * 2 * K);
        let fr = frame(&spec, 3);
        assert_eq!(fr.len(), T_F);
        assert!(fr[..T_NULL].iter().all(|z| z.norm() == 0.0));
    }
}
