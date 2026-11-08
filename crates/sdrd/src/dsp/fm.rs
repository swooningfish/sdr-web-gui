//! Narrowband FM demodulator: phase difference between consecutive samples.

use num_complex::Complex32;

pub struct FmDemod {
    prev: Complex32,
    /// Output units per radian of phase step.
    gain: f32,
}

impl FmDemod {
    /// `full_scale_hz`: deviation that maps to ±1.0.
    pub fn new(rate: u32, full_scale_hz: f32) -> Self {
        FmDemod {
            prev: Complex32::new(1.0, 0.0),
            gain: rate as f32 / (std::f32::consts::TAU * full_scale_hz),
        }
    }

    /// Demodulate to signed 16-bit little-endian audio, appended to `out`.
    pub fn process_s16le(&mut self, iq: &[Complex32], out: &mut Vec<u8>) {
        for &x in iq {
            let d = (x * self.prev.conj()).arg();
            self.prev = x;
            let v = (d * self.gain).clamp(-1.0, 1.0) * 32_000.0;
            out.extend_from_slice(&(v as i16).to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_offset_gives_constant_level() {
        let rate = 48_000;
        let mut d = FmDemod::new(rate, 6000.0);
        // Carrier 3 kHz above centre → +0.5 full scale.
        let iq: Vec<Complex32> = (0..480)
            .map(|i| Complex32::from_polar(1.0, std::f32::consts::TAU * 3000.0 * i as f32 / rate as f32))
            .collect();
        let mut out = Vec::new();
        d.process_s16le(&iq, &mut out);
        let last = i16::from_le_bytes([out[out.len() - 2], out[out.len() - 1]]);
        assert!((last as f32 - 16_000.0).abs() < 50.0, "{last}");
    }
}
