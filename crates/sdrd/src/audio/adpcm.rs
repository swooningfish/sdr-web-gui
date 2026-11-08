//! IMA ADPCM: 4 bits per sample, 4:1 against 16-bit PCM. Tiny to implement
//! on both ends, needs no codec library on the Pi or in the browser, and is
//! plenty for radio audio (~64 kb/s at 16 kHz).

const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];
const INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub predictor: i16,
    pub index: u8,
}

/// Encode `pcm` (low nibble first). Returns the state at the start of the
/// block, which the receiver needs, and updates `st` for the next block.
pub fn encode(st: &mut State, pcm: &[i16], out: &mut Vec<u8>) -> State {
    let start = *st;
    let (mut pred, mut idx) = (st.predictor as i32, st.index as i32);
    let mut byte = 0u8;
    for (n, &s) in pcm.iter().enumerate() {
        let step = STEPS[idx as usize];
        let mut diff = s as i32 - pred;
        let mut code = 0u8;
        if diff < 0 {
            code = 8;
            diff = -diff;
        }
        let mut delta = step >> 3;
        if diff >= step {
            code |= 4;
            diff -= step;
            delta += step;
        }
        if diff >= step >> 1 {
            code |= 2;
            diff -= step >> 1;
            delta += step >> 1;
        }
        if diff >= step >> 2 {
            code |= 1;
            delta += step >> 2;
        }
        pred = if code & 8 != 0 { pred - delta } else { pred + delta }.clamp(-32768, 32767);
        idx = (idx + INDEX[(code & 7) as usize]).clamp(0, 88);
        if n % 2 == 0 {
            byte = code;
        } else {
            out.push(byte | (code << 4));
        }
    }
    if pcm.len() % 2 == 1 {
        out.push(byte);
    }
    *st = State { predictor: pred as i16, index: idx as u8 };
    start
}

/// Reference decoder (the browser has its own copy in lib/audio.ts).
#[cfg(test)]
pub fn decode(st: State, data: &[u8], n: usize) -> Vec<i16> {
    let (mut pred, mut idx) = (st.predictor as i32, st.index as i32);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let code = (data[i / 2] >> ((i % 2) * 4)) & 0x0f;
        let step = STEPS[idx as usize];
        let mut delta = step >> 3;
        if code & 4 != 0 {
            delta += step;
        }
        if code & 2 != 0 {
            delta += step >> 1;
        }
        if code & 1 != 0 {
            delta += step >> 2;
        }
        pred = if code & 8 != 0 { pred - delta } else { pred + delta }.clamp(-32768, 32767);
        idx = (idx + INDEX[(code & 7) as usize]).clamp(0, 88);
        out.push(pred as i16);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_is_close() {
        let pcm: Vec<i16> = (0..4000)
            .map(|i| (12_000.0 * (i as f64 * 0.07).sin() + 3000.0 * (i as f64 * 0.31).sin()) as i16)
            .collect();
        let mut st = State::default();
        let mut out = Vec::new();
        // Two blocks: the second must decode on its own from its start state.
        let s0 = encode(&mut st, &pcm[..2001], &mut out);
        let a_len = out.len();
        let s1 = encode(&mut st, &pcm[2001..], &mut out);
        assert_eq!(a_len, 1001);
        let a = decode(s0, &out[..a_len], 2001);
        let b = decode(s1, &out[a_len..], 1999);
        let err: f64 = a
            .iter()
            .chain(&b)
            .zip(&pcm)
            .skip(50) // the step size needs a few samples to adapt
            .map(|(x, y)| (*x as f64 - *y as f64).powi(2))
            .sum::<f64>()
            / (pcm.len() - 50) as f64;
        let sig: f64 = pcm.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / pcm.len() as f64;
        let snr = 10.0 * (sig / err).log10();
        assert!(snr > 25.0, "ADPCM SNR {snr:.1} dB");
    }
}
