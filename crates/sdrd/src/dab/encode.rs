//! A DAB mode I transmitter for the FIC, for the tests and the simulated
//! receiver: one frame that names the ensemble and its services (the main
//! service channel is filled with random data).

use num_complex::Complex32;
use rustfft::FftPlanner;

use super::fic::{conv_encode, crc16, ebu_byte, prbs, sent, BLOCK_BITS};
use super::{bin, interleaver, prs, FIC_SYMBOLS, K, L, T_F, T_G, T_NULL, T_U};

/// Most services one frame can name (one label per FIB, 12 FIBs, one
/// for the ensemble).
pub const MAX_SERVICES: usize = 11;

/// An ensemble to transmit.
pub struct Spec<'a> {
    pub eid: u16,
    pub label: &'a str,
    /// Programme services: (service id, label).
    pub services: &'a [(u16, &'a str)],
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

/// FIG 0/0: ensemble id, no change, CIF counter 0.
fn fig0_0(eid: u16) -> Vec<u8> {
    let mut f = vec![0x05, 0x00];
    f.extend_from_slice(&eid.to_be_bytes());
    f.extend_from_slice(&[0x00, 0x00]);
    f
}

/// The 12 FIBs of a frame (CRC added).
fn fibs(spec: &Spec) -> Vec<[u8; 32]> {
    assert!(spec.services.len() <= MAX_SERVICES, "at most {MAX_SERVICES} services");
    let mut figs: Vec<Vec<Vec<u8>>> = vec![vec![fig0_0(spec.eid), fig1(0, spec.eid, spec.label)]];
    for &(sid, name) in spec.services {
        figs.push(vec![fig1(1, sid, name)]);
    }
    while figs.len() < 12 {
        figs.push(vec![fig0_0(spec.eid)]);
    }
    figs.iter()
        .map(|list| {
            let mut data: Vec<u8> = list.concat();
            assert!(data.len() <= 30);
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

/// One transmission frame at 2.048 MS/s (null symbol first), scaled to an
/// average power of 1 outside the null. `seed` varies the MSC data.
pub fn frame(spec: &Spec, seed: u64) -> Vec<Complex32> {
    let bits = fic_bits(spec);
    let map = interleaver();
    let ifft = FftPlanner::<f32>::new().plan_fft_inverse(T_U);
    let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut random_bit = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng & 1) as u8
    };
    let qpsk = |a: u8, b: u8| Complex32::new(1.0 - 2.0 * a as f32, 1.0 - 2.0 * b as f32) * std::f32::consts::FRAC_1_SQRT_2;

    let mut out = vec![Complex32::default(); T_NULL];
    let mut carriers = prs();
    for l in 0..L {
        if l > 0 {
            let sym: Vec<(u8, u8)> = if l <= FIC_SYMBOLS {
                let b = &bits[(l - 1) * 2 * K..l * 2 * K];
                (0..K).map(|n| (b[n], b[K + n])).collect()
            } else {
                (0..K).map(|_| (random_bit(), random_bit())).collect()
            };
            for (n, &(a, b)) in sym.iter().enumerate() {
                carriers[bin(map[n])] *= qpsk(a, b);
            }
        }
        let mut t = carriers.clone();
        ifft.process(&mut t);
        out.extend_from_slice(&t[T_U - T_G..]);
        out.extend_from_slice(&t);
    }
    debug_assert_eq!(out.len(), T_F);
    let power = out[T_NULL..].iter().map(|z| z.norm_sqr()).sum::<f32>() / (T_F - T_NULL) as f32;
    let scale = 1.0 / power.sqrt();
    out.iter_mut().for_each(|z| *z *= scale);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::FIC_BLOCK;

    #[test]
    fn fibs_check_and_fic_fills_three_symbols() {
        let spec = Spec { eid: 0xC181, label: "SIM DAB", services: &[(0xC221, "One"), (0xC222, "Two")] };
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
