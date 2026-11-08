//! Spectrum / waterfall: windowed FFTs of the full capture, power-averaged down
//! to the requested frame rate, quantised to u8 and sent as compact binary
//! frames to WebSocket clients.
//!
//! Frame layout (little endian), 32-byte header followed by `bins` u8 values
//! ordered from lowest to highest frequency:
//!
//! | off | type | field                                   |
//! |-----|------|-----------------------------------------|
//! | 0   | u8   | frame type = 1 (waterfall)              |
//! | 1   | u8   | version = 0                             |
//! | 2   | u16  | bins                                    |
//! | 4   | u32  | sample rate (Hz)                        |
//! | 8   | u64  | centre frequency (Hz)                   |
//! | 16  | f32  | dB value that maps to 0                 |
//! | 20  | f32  | dB value that maps to 255               |
//! | 24  | u64  | timestamp (ms since Unix epoch)         |

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::{BufMut, Bytes, BytesMut};
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::radio::IqBlock;

pub const FRAME_WATERFALL: u8 = 1;
pub const HEADER_LEN: usize = 32;
pub const MIN_DB: f32 = -120.0;
pub const MAX_DB: f32 = 0.0;

pub struct Waterfall {
    size: usize,
    fps: u32,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// (sum of window)^2 – normalises a full-scale tone to ~0 dBFS.
    norm: f32,
    lut: [f32; 256],
    buf: Vec<Complex32>,
    scratch: Vec<Complex32>,
    acc: Vec<f32>,
    acc_n: u32,
    center_hz: u64,
    sample_rate: u32,
}

impl Waterfall {
    pub fn new(size: usize, fps: u32) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(size);
        // Blackman–Harris: low leakage so weak signals next to strong ones show up.
        let window: Vec<f32> = (0..size)
            .map(|i| {
                let x = std::f32::consts::TAU * i as f32 / (size - 1) as f32;
                0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()
            })
            .collect();
        let sum: f32 = window.iter().sum();
        let mut lut = [0f32; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = (i as f32 - 127.5) / 127.5;
        }
        let scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        Waterfall {
            size,
            fps,
            fft,
            window,
            norm: sum * sum,
            lut,
            buf: Vec::with_capacity(size),
            scratch,
            acc: vec![0.0; size],
            acc_n: 0,
            center_hz: 0,
            sample_rate: 0,
        }
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.acc.iter_mut().for_each(|v| *v = 0.0);
        self.acc_n = 0;
    }

    fn ffts_per_frame(&self) -> u32 {
        (self.sample_rate / (self.size as u32 * self.fps)).max(1)
    }

    /// Feed a block; returns any completed frames.
    pub fn process(&mut self, block: &IqBlock) -> Vec<Bytes> {
        if block.center_hz != self.center_hz || block.sample_rate != self.sample_rate {
            self.reset();
            self.center_hz = block.center_hz;
            self.sample_rate = block.sample_rate;
        }
        let per_frame = self.ffts_per_frame();
        let mut frames = Vec::new();
        for pair in block.data.chunks_exact(2) {
            let i = self.buf.len();
            let w = self.window[i];
            self.buf.push(Complex32::new(
                self.lut[pair[0] as usize] * w,
                self.lut[pair[1] as usize] * w,
            ));
            if self.buf.len() == self.size {
                self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);
                for (a, c) in self.acc.iter_mut().zip(self.buf.iter()) {
                    *a += c.norm_sqr();
                }
                self.buf.clear();
                self.acc_n += 1;
                if self.acc_n >= per_frame {
                    frames.push(self.emit());
                }
            }
        }
        frames
    }

    fn emit(&mut self) -> Bytes {
        let n = self.size;
        let scale = 1.0 / (self.acc_n as f32 * self.norm);
        let mut out = BytesMut::with_capacity(HEADER_LEN + n);
        out.put_u8(FRAME_WATERFALL);
        out.put_u8(0);
        out.put_u16_le(n as u16);
        out.put_u32_le(self.sample_rate);
        out.put_u64_le(self.center_hz);
        out.put_f32_le(MIN_DB);
        out.put_f32_le(MAX_DB);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        out.put_u64_le(ts);
        let span = MAX_DB - MIN_DB;
        for k in 0..n {
            // FFT bin 0 is DC; reorder so the most negative frequency comes first.
            let idx = (k + n / 2) % n;
            let db = 10.0 * (self.acc[idx] * scale + 1e-20).log10();
            let q = ((db - MIN_DB) / span * 255.0).round().clamp(0.0, 255.0);
            out.put_u8(q as u8);
        }
        self.acc.iter_mut().for_each(|v| *v = 0.0);
        self.acc_n = 0;
        out.freeze()
    }
}

/// Run the waterfall on its own thread. Skips the FFT work entirely while no
/// client is listening.
pub fn spawn(
    mut rx: broadcast::Receiver<Arc<IqBlock>>,
    tx: broadcast::Sender<Bytes>,
    fft_size: usize,
    fps: u32,
) {
    std::thread::Builder::new()
        .name("waterfall".into())
        .spawn(move || {
            let mut wf = Waterfall::new(fft_size, fps);
            loop {
                match rx.blocking_recv() {
                    Ok(block) => {
                        if tx.receiver_count() == 0 {
                            wf.reset();
                            continue;
                        }
                        for frame in wf.process(&block) {
                            let _ = tx.send(frame);
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!("waterfall lagged {n} blocks");
                        wf.reset();
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        })
        .expect("spawn waterfall thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::to_cu8;

    fn tone_block(rate: u32, offset_hz: f64, amp: f64, n: usize) -> IqBlock {
        let mut data = Vec::with_capacity(n * 2);
        for i in 0..n {
            let ph = std::f64::consts::TAU * offset_hz * i as f64 / rate as f64;
            data.push(to_cu8((amp * ph.cos()) as f32));
            data.push(to_cu8((amp * ph.sin()) as f32));
        }
        IqBlock { center_hz: 100_000_000, sample_rate: rate, data }
    }

    #[test]
    fn tone_lands_in_expected_bin_at_expected_level() {
        let size = 1024;
        let rate = 1_024_000;
        let mut wf = Waterfall::new(size, 1000); // one FFT per frame
        // +rate/4 → bin size/2 + size/4 after fftshift.
        let frames = wf.process(&tone_block(rate, rate as f64 / 4.0, 1.0, size));
        assert_eq!(frames.len(), 1);
        let f = &frames[0];
        assert_eq!(f[0], FRAME_WATERFALL);
        assert_eq!(u16::from_le_bytes([f[2], f[3]]) as usize, size);
        let bins = &f[HEADER_LEN..];
        let (peak, &val) = bins.iter().enumerate().max_by_key(|(_, v)| **v).unwrap();
        assert_eq!(peak, size / 2 + size / 4);
        // Full-scale tone ≈ 0 dBFS → top of the range.
        assert!(val >= 250, "peak quantised to {val}");
        // Far-away bins are well below the peak.
        // 80 steps ≈ 38 dB of dynamic range (8-bit quantisation limits the floor).
        assert!(val - bins[10] > 80, "leakage {}", bins[10]);
    }

    #[test]
    fn averages_to_requested_fps() {
        let mut wf = Waterfall::new(2048, 20);
        let rate = 2_048_000;
        // One second of samples → ~20 frames.
        let frames = wf.process(&tone_block(rate, 1000.0, 0.5, rate as usize));
        assert!((19..=21).contains(&frames.len()), "{} frames", frames.len());
    }
}
