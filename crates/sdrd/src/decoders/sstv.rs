//! SSTV receiver. For each configured channel inside the tuned band: channel
//! filter → FM or SSB demodulator → tone frequency → `sstv::decode`. Rows
//! are streamed to browsers as they are decoded; finished (or lost, if at
//! least a fifth arrived) images are saved as PNG with a thumbnail and
//! recorded as events.
//!
//! Live rows frame (binary WebSocket message, little endian, 16-byte header):
//!
//! | off | type | field                                       |
//! |-----|------|---------------------------------------------|
//! | 0   | u8   | frame type = 2                              |
//! | 1   | u8   | version = 0                                 |
//! | 2   | u8   | channel index                               |
//! | 3   | u8   | VIS code of the mode                        |
//! | 4   | u16  | image width                                 |
//! | 6   | u16  | image height                                |
//! | 8   | u16  | first line                                  |
//! | 10  | u16  | number of lines                             |
//! | 12  | u32  | session (changes with every new image)      |
//! | 16  | ...  | RGB, width × lines × 3                      |

use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::{BufMut, Bytes, BytesMut};
use num_complex::{Complex, Complex32};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map};
use tokio::sync::{broadcast, mpsc, watch};

use super::DecodedEvent;
use crate::db;
use crate::dsp::ddc::{audio_decimation, fits, Ddc};
use crate::dsp::fm::FmDemod;
use crate::radio::{IqBlock, MAX_FREQ_HZ, MIN_FREQ_HZ};
use crate::scheduler::{shift_hz, DopplerRx};
use crate::sstv::decode::{Decoder, Image, Output};
use crate::sstv::tone::ToneDemod;

pub const NAME: &str = "sstv";
const SETTINGS_KEY: &str = "decoder.sstv";
pub const FRAME_ROWS: u8 = 2;
const MAX_CHANNELS: usize = 4;
const THUMB_WIDTH: usize = 160;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Demod {
    Fm,
    Usb,
    Lsb,
}

impl Demod {
    /// Channel filter half-width.
    fn cutoff_hz(self) -> f64 {
        match self {
            // Wide enough for ±5 kHz deviation plus some tuning error.
            Demod::Fm => 10_000.0,
            Demod::Usb | Demod::Lsb => 1_500.0,
        }
    }

    /// Where the channel filter is centred, relative to the dial frequency.
    fn shift_hz(self) -> f64 {
        match self {
            Demod::Fm => 0.0,
            Demod::Usb => 1_500.0,
            Demod::Lsb => -1_500.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SstvChannel {
    pub label: String,
    pub frequency_hz: u64,
    pub demod: Demod,
    pub enabled: bool,
    /// Satellite downlink: follow the Doppler shift during passes.
    #[serde(default)]
    pub doppler: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SstvConfig {
    pub enabled: bool,
    pub channels: Vec<SstvChannel>,
}

impl Default for SstvConfig {
    fn default() -> Self {
        let ch = |label: &str, frequency_hz| SstvChannel {
            label: label.into(),
            frequency_hz,
            demod: Demod::Fm,
            enabled: true,
            doppler: true,
        };
        SstvConfig {
            enabled: true,
            channels: vec![ch("ISS SSTV", 145_800_000), ch("ARISS UHF", 437_550_000)],
        }
    }
}

impl SstvConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.channels.len() > MAX_CHANNELS {
            return Err(format!("at most {MAX_CHANNELS} SSTV channels"));
        }
        for c in &self.channels {
            if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&c.frequency_hz) {
                return Err("frequency out of range".into());
            }
            let n = c.label.chars().count();
            if n == 0 || n > 24 || c.label.chars().any(char::is_control) {
                return Err("channel labels must be 1–24 characters".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Receiving {
    pub mode: &'static str,
    pub lines: usize,
    pub height: u16,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ChannelStatus {
    pub label: String,
    pub frequency_hz: u64,
    pub demod: Option<Demod>,
    pub enabled: bool,
    pub in_band: bool,
    pub receiving: Option<Receiving>,
    pub images: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SstvStatus {
    pub enabled: bool,
    pub channels: Vec<ChannelStatus>,
}

#[derive(Clone)]
pub struct SstvHandle {
    cfg_tx: Arc<watch::Sender<SstvConfig>>,
    status_rx: watch::Receiver<SstvStatus>,
    pub images_dir: Arc<Path>,
}

impl SstvHandle {
    pub fn config(&self) -> SstvConfig {
        self.cfg_tx.borrow().clone()
    }

    pub fn status(&self) -> SstvStatus {
        self.status_rx.borrow().clone()
    }

    pub async fn set_config(&self, db: &sqlx::SqlitePool, cfg: SstvConfig) -> anyhow::Result<()> {
        db::set_setting(db, SETTINGS_KEY, &cfg).await?;
        self.cfg_tx.send_replace(cfg);
        Ok(())
    }
}

pub async fn spawn(
    db: &sqlx::SqlitePool,
    images_dir: PathBuf,
    iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    events: mpsc::Sender<DecodedEvent>,
    rows_tx: broadcast::Sender<Bytes>,
    doppler: DopplerRx,
) -> anyhow::Result<SstvHandle> {
    std::fs::create_dir_all(&images_dir)?;
    let cfg = match db::get_setting::<SstvConfig>(db, SETTINGS_KEY).await {
        Ok(Some(c)) if c.validate().is_ok() => c,
        Ok(_) => SstvConfig::default(),
        Err(e) => {
            tracing::warn!("ignoring stored SSTV config: {e}");
            SstvConfig::default()
        }
    };
    let (cfg_tx, cfg_rx) = watch::channel(cfg);
    let (status_tx, status_rx) = watch::channel(SstvStatus::default());
    let images_dir: Arc<Path> = images_dir.into();
    let worker = Worker {
        iq_rx,
        cfg_rx,
        status: status_tx,
        events,
        rows_tx,
        images_dir: images_dir.clone(),
        doppler,
    };
    std::thread::Builder::new()
        .name("sstv".into())
        .spawn(move || worker.run())?;
    Ok(SstvHandle {
        cfg_tx: Arc::new(cfg_tx),
        status_rx,
        images_dir,
    })
}

/// Turns channel IQ into audio.
enum AudioDemod {
    Fm(FmDemod),
    /// Shift the filtered channel back up by 1.5 kHz and take the real part.
    Ssb { phasor: Complex<f64>, step: Complex<f64> },
}

impl AudioDemod {
    fn new(demod: Demod, rate: u32) -> Self {
        match demod {
            Demod::Fm => AudioDemod::Fm(FmDemod::new(rate, 6_000.0)),
            Demod::Usb | Demod::Lsb => {
                let w = std::f64::consts::TAU * demod.shift_hz() / rate as f64;
                AudioDemod::Ssb {
                    phasor: Complex::new(1.0, 0.0),
                    step: Complex::new(w.cos(), w.sin()),
                }
            }
        }
    }

    fn process(&mut self, iq: &[Complex32], out: &mut Vec<f32>) {
        match self {
            AudioDemod::Fm(fm) => fm.process_f32(iq, out),
            AudioDemod::Ssb { phasor, step } => {
                for &z in iq {
                    let p = *phasor;
                    out.push(z.re * p.re as f32 - z.im * p.im as f32);
                    *phasor *= *step;
                }
                *phasor /= phasor.norm();
            }
        }
    }
}

struct Dsp {
    key: (u64, u32),
    ddc: Ddc,
    audio: AudioDemod,
    /// Doppler shift currently applied to the mixer.
    shift: f64,
}

struct Rx {
    rate: u32,
    tone: ToneDemod,
    dec: Decoder,
}

struct Chan {
    cfg: SstvChannel,
    dsp: Option<Dsp>,
    rx: Option<Rx>,
}

struct Worker {
    iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    cfg_rx: watch::Receiver<SstvConfig>,
    status: watch::Sender<SstvStatus>,
    events: mpsc::Sender<DecodedEvent>,
    rows_tx: broadcast::Sender<Bytes>,
    images_dir: Arc<Path>,
    doppler: DopplerRx,
}

impl Worker {
    fn channels(&self, cfg: &SstvConfig) -> Vec<Chan> {
        self.status.send_modify(|s| {
            s.enabled = cfg.enabled;
            s.channels = cfg
                .channels
                .iter()
                .map(|c| ChannelStatus {
                    label: c.label.clone(),
                    frequency_hz: c.frequency_hz,
                    demod: Some(c.demod),
                    enabled: c.enabled,
                    ..Default::default()
                })
                .collect();
        });
        cfg.channels
            .iter()
            .map(|c| Chan { cfg: c.clone(), dsp: None, rx: None })
            .collect()
    }

    fn run(mut self) {
        let mut cfg = self.cfg_rx.borrow_and_update().clone();
        let mut chans = self.channels(&cfg);
        let (mut iq, mut audio, mut freqs, mut outs) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());

        loop {
            let block = match self.iq_rx.blocking_recv() {
                Ok(b) => b,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("SSTV feeder lagged {n} blocks");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if self.cfg_rx.has_changed().unwrap_or(false) {
                cfg = self.cfg_rx.borrow_and_update().clone();
                for (i, ch) in chans.iter_mut().enumerate() {
                    self.stop_rx(i, ch);
                }
                chans = self.channels(&cfg);
            }
            let (decim, rate) = audio_decimation(block.sample_rate);

            for (i, ch) in chans.iter_mut().enumerate() {
                let d = ch.cfg.demod;
                let target = ch.cfg.frequency_hz as f64 + d.shift_hz();
                let in_band = cfg.enabled
                    && ch.cfg.enabled
                    && target > 0.0
                    && fits(block.center_hz, block.sample_rate, target as u64, 2.0 * d.cutoff_hz());
                self.status.send_if_modified(|s| match s.channels.get_mut(i) {
                    Some(c) if c.in_band != in_band => {
                        c.in_band = in_band;
                        true
                    }
                    _ => false,
                });
                if !in_band {
                    self.stop_rx(i, ch);
                    continue;
                }
                if ch.rx.as_ref().is_some_and(|r| r.rate != rate) {
                    self.stop_rx(i, ch);
                }
                let key = (block.center_hz, block.sample_rate);
                let shift = if ch.cfg.doppler { shift_hz(&self.doppler, ch.cfg.frequency_hz) } else { 0.0 };
                let offset = target + shift - block.center_hz as f64;
                if ch.dsp.as_ref().map(|s| s.key) != Some(key) {
                    ch.dsp = Some(Dsp {
                        key,
                        ddc: Ddc::with_cutoff(block.sample_rate, offset, decim, d.cutoff_hz()),
                        audio: AudioDemod::new(d, rate),
                        shift,
                    });
                }
                if let Some(s) = ch.dsp.as_mut().filter(|s| (s.shift - shift).abs() > 1.0) {
                    s.ddc.set_offset(block.sample_rate, offset);
                    s.shift = shift;
                }
                let rx = ch.rx.get_or_insert_with(|| {
                    let tone = ToneDemod::new(rate as f64);
                    let dec = Decoder::new(tone.rate());
                    Rx { rate, tone, dec }
                });
                let dsp = ch.dsp.as_mut().unwrap();
                iq.clear();
                dsp.ddc.process_cu8(&block.data, &mut iq);
                audio.clear();
                dsp.audio.process(&iq, &mut audio);
                freqs.clear();
                rx.tone.process(&audio, &mut freqs);
                outs.clear();
                rx.dec.push(&freqs, &mut outs);
                let label = ch.cfg.label.clone();
                for o in outs.drain(..) {
                    self.handle(i, &label, ch.cfg.frequency_hz, o);
                }
            }
        }
    }

    /// Drop a channel's receiver, saving any image in progress.
    fn stop_rx(&self, i: usize, ch: &mut Chan) {
        ch.dsp = None;
        if let Some(mut rx) = ch.rx.take() {
            let mut outs = Vec::new();
            rx.dec.abort(&mut outs);
            for o in outs {
                self.handle(i, &ch.cfg.label, ch.cfg.frequency_hz, o);
            }
        }
    }

    fn set_receiving(&self, i: usize, r: Option<Receiving>) {
        self.status.send_if_modified(|s| match s.channels.get_mut(i) {
            Some(c) if c.receiving != r => {
                c.receiving = r;
                true
            }
            _ => false,
        });
    }

    fn handle(&self, i: usize, label: &str, freq: u64, o: Output) {
        match o {
            Output::Started { mode, .. } => {
                tracing::info!("SSTV {label}: receiving {}", mode.name);
                self.set_receiving(i, Some(Receiving { mode: mode.name, lines: 0, height: mode.height }));
            }
            Output::Rows { session, mode, first_line, rgb } => {
                let w = mode.width as usize;
                let n = rgb.len() / (w * 3);
                let mut f = BytesMut::with_capacity(16 + rgb.len());
                f.put_u8(FRAME_ROWS);
                f.put_u8(0);
                f.put_u8(i as u8);
                f.put_u8(mode.vis);
                f.put_u16_le(mode.width);
                f.put_u16_le(mode.height);
                f.put_u16_le(first_line as u16);
                f.put_u16_le(n as u16);
                f.put_u32_le(((i as u32) << 24) | (session & 0x00ff_ffff));
                f.put_slice(&rgb);
                let _ = self.rows_tx.send(f.freeze());
                self.set_receiving(
                    i,
                    Some(Receiving { mode: mode.name, lines: first_line + n, height: mode.height }),
                );
            }
            Output::Done { image, .. } => {
                self.set_receiving(i, None);
                if image.lines * 5 < image.mode.height as usize {
                    tracing::info!("SSTV {label}: discarding {} fragment ({} lines)", image.mode.name, image.lines);
                    return;
                }
                match self.save(label, &image) {
                    Ok((file, thumb)) => {
                        let mut data = Map::new();
                        data.insert("model".into(), json!("SSTV"));
                        data.insert("id".into(), json!(label));
                        data.insert("mode".into(), json!(image.mode.name));
                        data.insert("width".into(), json!(image.mode.width));
                        data.insert("height".into(), json!(image.mode.height));
                        data.insert("lines".into(), json!(image.lines));
                        data.insert("complete".into(), json!(image.complete));
                        data.insert("image".into(), json!(file));
                        data.insert("thumb".into(), json!(thumb));
                        if self.events.try_send(DecodedEvent::new(NAME, Some(freq), data)).is_err() {
                            tracing::warn!("event queue full, dropping SSTV image event");
                        }
                        self.status.send_modify(|s| {
                            if let Some(c) = s.channels.get_mut(i) {
                                c.images += 1;
                            }
                        });
                    }
                    Err(e) => tracing::warn!("saving SSTV image failed: {e}"),
                }
            }
        }
    }

    fn save(&self, label: &str, img: &Image) -> anyhow::Result<(String, String)> {
        let slug = |s: &str| -> String {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
                .collect::<String>()
                .split('-')
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("-")
        };
        let base = format!("{}-{}-{}", db::now_ms(), slug(label), slug(img.mode.name));
        let (w, h) = (img.mode.width as usize, img.mode.height as usize);
        let file = format!("{base}.png");
        write_png(&self.images_dir.join(&file), w, h, &img.rgb)?;
        let (tw, th, trgb) = thumbnail(w, h, &img.rgb);
        let thumb = format!("{base}-thumb.png");
        write_png(&self.images_dir.join(&thumb), tw, th, &trgb)?;
        tracing::info!("SSTV {label}: saved {file}");
        Ok((file, thumb))
    }
}

/// Write via a temporary file so a reader never sees half an image.
fn write_png(path: &Path, w: usize, h: usize, rgb: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("png.tmp");
    {
        let f = std::fs::File::create(&tmp)?;
        let mut enc = png::Encoder::new(BufWriter::new(f), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header()?;
        wr.write_image_data(rgb)?;
        wr.finish()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Box-filtered downscale to `THUMB_WIDTH` pixels wide.
fn thumbnail(w: usize, h: usize, rgb: &[u8]) -> (usize, usize, Vec<u8>) {
    let tw = THUMB_WIDTH.min(w);
    let th = (h * tw / w).max(1);
    let mut out = Vec::with_capacity(tw * th * 3);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let mut acc = [0u32; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 3;
                    for c in 0..3 {
                        acc[c] += rgb[i + c] as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            out.extend(acc.map(|a| (a / n) as u8));
        }
    }
    (tw, th, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_averages() {
        let rgb: Vec<u8> = (0..640 * 496).flat_map(|i| if i % 2 == 0 { [0, 0, 0] } else { [200, 100, 50] }).collect();
        let (w, h, t) = thumbnail(640, 496, &rgb);
        assert_eq!((w, h), (160, 124));
        assert_eq!(&t[..3], &[100, 50, 25]);
    }

    #[test]
    fn ssb_demod_recovers_audio_tone() {
        // A USB tone 1900 Hz above the dial, after a channel filter centred
        // 1.5 kHz above the dial, sits at +400 Hz; the demod shifts it back.
        let rate = 48_000;
        let mut d = AudioDemod::new(Demod::Usb, rate);
        let iq: Vec<Complex32> = (0..4800)
            .map(|n| Complex32::from_polar(1.0, std::f32::consts::TAU * 400.0 * n as f32 / rate as f32))
            .collect();
        let mut audio = Vec::new();
        d.process(&iq, &mut audio);
        let mut tone = ToneDemod::new(rate as f64);
        let mut f = Vec::new();
        tone.process(&audio, &mut f);
        let mid = f[f.len() / 2];
        assert!((mid - 1900.0).abs() < 5.0, "{mid}");
    }
}
