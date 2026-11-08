//! Audio recorder for satellite passes: FM-demodulates one Doppler-corrected
//! channel, low-passes it to ~5 kHz and writes 16-bit mono WAV at a quarter
//! of the channel's audio rate (~12 kHz). The WAV header is refreshed every
//! few seconds so a file is playable even if sdrd stops mid-pass.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, watch};

use crate::dsp::ddc::{audio_decimation, fits, lowpass, Ddc};
use crate::dsp::fm::FmDemod;
use crate::orbit::doppler_hz;
use crate::radio::IqBlock;
use crate::scheduler::DopplerNow;

const AUDIO_DECIM: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct RecordSpec {
    pub path: PathBuf,
    pub freq_hz: u64,
    /// Correct for the Doppler shift of the satellite being tracked.
    pub doppler: bool,
}

#[derive(Debug)]
pub struct RecordDone {
    pub path: PathBuf,
    pub seconds: f64,
}

pub struct RecorderHandle {
    pub control: watch::Sender<Option<RecordSpec>>,
    pub done: mpsc::UnboundedReceiver<RecordDone>,
}

pub fn spawn(
    iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    doppler_rx: watch::Receiver<Option<DopplerNow>>,
) -> anyhow::Result<RecorderHandle> {
    let (control, control_rx) = watch::channel(None);
    let (done_tx, done) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("recorder".into())
        .spawn(move || run(iq_rx, control_rx, doppler_rx, done_tx))?;
    Ok(RecorderHandle { control, done })
}

struct Wav {
    path: PathBuf,
    w: BufWriter<File>,
    rate: u32,
    samples: u64,
    flushed: Instant,
}

impl Wav {
    fn create(path: PathBuf, rate: u32) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut w = BufWriter::new(File::create(&path)?);
        w.write_all(&header(rate, 0))?;
        Ok(Wav { path, w, rate, samples: 0, flushed: Instant::now() })
    }

    fn write(&mut self, s: &[i16]) -> std::io::Result<()> {
        for v in s {
            self.w.write_all(&v.to_le_bytes())?;
        }
        self.samples += s.len() as u64;
        if self.flushed.elapsed() > Duration::from_secs(5) {
            self.fix_header()?;
        }
        Ok(())
    }

    fn fix_header(&mut self) -> std::io::Result<()> {
        self.w.flush()?;
        let f = self.w.get_mut();
        let pos = f.stream_position()?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header(self.rate, self.samples))?;
        f.seek(SeekFrom::Start(pos))?;
        self.flushed = Instant::now();
        Ok(())
    }

    fn finish(mut self) -> RecordDone {
        if let Err(e) = self.fix_header() {
            tracing::warn!("finishing {}: {e}", self.path.display());
        }
        RecordDone { seconds: self.samples as f64 / self.rate as f64, path: self.path }
    }
}

/// 44-byte PCM WAV header for `samples` 16-bit mono samples.
fn header(rate: u32, samples: u64) -> [u8; 44] {
    let data = (samples * 2).min(u32::MAX as u64 - 36) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&1u16.to_le_bytes()); // mono
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * 2).to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

/// Real FIR low-pass + decimation for the audio.
struct AudioDecim {
    taps: Vec<f32>,
    hist: Vec<f32>,
    count: usize,
}

impl AudioDecim {
    fn new(audio_rate: u32) -> Self {
        let taps = lowpass(63, 5_000.0 / audio_rate as f64);
        AudioDecim { hist: vec![0.0; taps.len() - 1], taps, count: 0 }
    }

    fn process(&mut self, x: &[f32], out: &mut Vec<i16>) {
        let keep = self.taps.len() - 1;
        for &v in x {
            self.hist.push(v);
            self.count += 1;
            if self.count == AUDIO_DECIM {
                self.count = 0;
                let w = &self.hist[self.hist.len() - self.taps.len()..];
                let y: f32 = w.iter().zip(&self.taps).map(|(a, b)| a * b).sum();
                out.push((y.clamp(-1.0, 1.0) * 30_000.0) as i16);
            }
        }
        let drop = self.hist.len() - keep;
        self.hist.drain(..drop);
    }
}

struct Chain {
    key: (u64, u32),
    in_rate: u32,
    base_offset: f64,
    shift: f64,
    ddc: Ddc,
    fm: FmDemod,
    lp: AudioDecim,
}

fn run(
    mut iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    mut control: watch::Receiver<Option<RecordSpec>>,
    doppler_rx: watch::Receiver<Option<DopplerNow>>,
    done: mpsc::UnboundedSender<RecordDone>,
) {
    let mut spec: Option<RecordSpec> = None;
    let mut wav: Option<Wav> = None;
    let mut chain: Option<Chain> = None;
    let (mut iq, mut audio, mut pcm) = (Vec::new(), Vec::new(), Vec::new());
    loop {
        let block = match iq_rx.blocking_recv() {
            Ok(b) => b,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        };
        if control.has_changed().unwrap_or(false) {
            let next = control.borrow_and_update().clone();
            if next != spec {
                if let Some(w) = wav.take() {
                    let _ = done.send(w.finish());
                }
                chain = None;
                spec = next;
            }
        }
        let Some(s) = spec.as_ref() else { continue };
        let shift = match (s.doppler, doppler_rx.borrow().as_ref()) {
            (true, Some(d)) => doppler_hz(s.freq_hz as f64, d.range_rate_km_s),
            _ => 0.0,
        };
        let target = s.freq_hz as f64 + shift;
        if !fits(block.center_hz, block.sample_rate, target as u64, 20_000.0) {
            chain = None; // a gap in the recording
            continue;
        }
        let (decim, rate) = audio_decimation(block.sample_rate);
        let key = (block.center_hz, block.sample_rate);
        if chain.as_ref().map(|c| c.key) != Some(key) {
            let base = s.freq_hz as f64 - block.center_hz as f64;
            chain = Some(Chain {
                key,
                in_rate: block.sample_rate,
                base_offset: base,
                shift,
                ddc: Ddc::with_cutoff(block.sample_rate, base + shift, decim, 8_000.0),
                fm: FmDemod::new(rate, 5_000.0),
                lp: AudioDecim::new(rate),
            });
        }
        let c = chain.as_mut().unwrap();
        if (c.shift - shift).abs() > 1.0 {
            c.shift = shift;
            c.ddc.set_offset(c.in_rate, c.base_offset + shift);
        }
        if wav.is_none() {
            let out_rate = rate / AUDIO_DECIM as u32;
            match Wav::create(s.path.clone(), out_rate) {
                Ok(w) => {
                    tracing::info!("recording {} at {out_rate} Hz", s.path.display());
                    wav = Some(w);
                }
                Err(e) => {
                    tracing::warn!("can't record to {}: {e}", s.path.display());
                    spec = None;
                    continue;
                }
            }
        }
        iq.clear();
        c.ddc.process_cu8(&block.data, &mut iq);
        audio.clear();
        c.fm.process_f32(&iq, &mut audio);
        pcm.clear();
        c.lp.process(&audio, &mut pcm);
        if let Some(w) = wav.as_mut() {
            if let Err(e) = w.write(&pcm) {
                tracing::warn!("recording failed: {e}");
                let _ = done.send(wav.take().unwrap().finish());
                spec = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_valid() {
        let h = header(12_800, 1000);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 36 + 2000);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 12_800);
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 2000);
    }
}
