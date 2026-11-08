//! The radio owns the IQ source (a dongle, a test generator or a recording)
//! on a dedicated thread and broadcasts raw cu8 blocks to the DSP consumers.

use std::sync::{mpsc, Arc, Mutex};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::config::{Config, SourceKind};

mod replay;
#[cfg(feature = "rtlsdr")]
mod rtlsdr;
mod synthetic;

/// Sample rates offered in the UI. All are valid for the RTL2832U; rates above
/// 2.4 MS/s tend to drop samples on USB so they are not offered.
pub const SAMPLE_RATES: &[u32] = &[
    250_000, 1_024_000, 1_536_000, 1_800_000, 1_920_000, 2_048_000, 2_400_000,
];
pub const MIN_FREQ_HZ: u64 = 500_000;
pub const MAX_FREQ_HZ: u64 = 1_766_000_000;

/// A block of interleaved unsigned 8-bit I/Q samples, as produced by librtlsdr.
pub struct IqBlock {
    pub center_hz: u64,
    pub sample_rate: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RadioSettings {
    pub center_hz: u64,
    pub sample_rate: u32,
    /// `None` = tuner AGC.
    pub gain_db: Option<f32>,
    pub ppm: i32,
    pub bias_tee: bool,
    /// 0 = off, 1 = I branch, 2 = Q branch (HF on V3-style dongles).
    pub direct_sampling: u8,
}

/// Partial update from the API. `auto_gain: true` wins over `gain_db`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioPatch {
    pub center_hz: Option<u64>,
    pub sample_rate: Option<u32>,
    pub gain_db: Option<f32>,
    pub auto_gain: Option<bool>,
    pub ppm: Option<i32>,
    pub bias_tee: Option<bool>,
    pub direct_sampling: Option<u8>,
}

impl RadioSettings {
    pub fn apply_patch(&self, p: &RadioPatch) -> Result<RadioSettings, String> {
        let mut s = self.clone();
        if let Some(f) = p.center_hz {
            if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&f) {
                return Err(format!(
                    "frequency must be between {} and {} Hz",
                    MIN_FREQ_HZ, MAX_FREQ_HZ
                ));
            }
            s.center_hz = f;
        }
        if let Some(r) = p.sample_rate {
            if !SAMPLE_RATES.contains(&r) {
                return Err(format!("unsupported sample rate {r}"));
            }
            s.sample_rate = r;
        }
        if let Some(g) = p.gain_db {
            if !(0.0..=60.0).contains(&g) || !g.is_finite() {
                return Err("gain must be between 0 and 60 dB".into());
            }
            s.gain_db = Some(g);
        }
        if p.auto_gain == Some(true) {
            s.gain_db = None;
        }
        if let Some(ppm) = p.ppm {
            if !(-200..=200).contains(&ppm) {
                return Err("ppm must be between -200 and 200".into());
            }
            s.ppm = ppm;
        }
        if let Some(b) = p.bias_tee {
            s.bias_tee = b;
        }
        if let Some(ds) = p.direct_sampling {
            if ds > 2 {
                return Err("direct_sampling must be 0, 1 or 2".into());
            }
            s.direct_sampling = ds;
        }
        Ok(s)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RadioState {
    pub source: &'static str,
    pub can_tune: bool,
    pub connected: bool,
    pub device: Option<String>,
    pub tuner: Option<String>,
    pub settings: RadioSettings,
    pub gains_db: Vec<f32>,
    pub sample_rates: &'static [u32],
    pub error: Option<String>,
}

/// Shared between the source thread implementations.
pub(crate) struct SourceCtx {
    cmd_rx: mpsc::Receiver<RadioSettings>,
    state_tx: watch::Sender<RadioState>,
    iq_tx: broadcast::Sender<Arc<IqBlock>>,
    pub initial: RadioSettings,
}

impl SourceCtx {
    /// Most recent queued settings, if any (older ones are superseded).
    pub fn latest_cmd(&self) -> Option<RadioSettings> {
        let mut last = None;
        while let Ok(s) = self.cmd_rx.try_recv() {
            last = Some(s);
        }
        last
    }

    pub fn update_state(&self, f: impl FnOnce(&mut RadioState)) {
        self.state_tx.send_modify(f);
    }

    pub fn publish(&self, block: IqBlock) {
        // No receivers is fine; DSP consumers come and go.
        let _ = self.iq_tx.send(Arc::new(block));
    }
}

#[derive(Clone)]
pub struct RadioHandle {
    cmd_tx: mpsc::Sender<RadioSettings>,
    state_rx: watch::Receiver<RadioState>,
    /// Desired settings (what the user last asked for).
    desired: Arc<Mutex<RadioSettings>>,
    can_tune: bool,
}

impl RadioHandle {
    pub fn subscribe(&self) -> watch::Receiver<RadioState> {
        self.state_rx.clone()
    }

    pub fn state(&self) -> RadioState {
        self.state_rx.borrow().clone()
    }

    pub fn configure(&self, patch: &RadioPatch) -> Result<RadioSettings, String> {
        if !self.can_tune {
            return Err("this source cannot be retuned".into());
        }
        let mut desired = self.desired.lock().unwrap();
        let next = desired.apply_patch(patch)?;
        self.cmd_tx
            .send(next.clone())
            .map_err(|_| "radio thread has stopped".to_string())?;
        *desired = next.clone();
        Ok(next)
    }
}

pub fn spawn(cfg: &Config, iq_tx: broadcast::Sender<Arc<IqBlock>>) -> Result<RadioHandle> {
    let settings = RadioSettings {
        center_hz: cfg.center_hz,
        sample_rate: cfg.sample_rate,
        gain_db: cfg.gain_db,
        ppm: cfg.ppm,
        bias_tee: cfg.bias_tee,
        direct_sampling: 0,
    };
    let can_tune = cfg.source != SourceKind::Replay;
    let (state_tx, state_rx) = watch::channel(RadioState {
        source: cfg.source.name(),
        can_tune,
        connected: false,
        device: None,
        tuner: None,
        settings: settings.clone(),
        gains_db: Vec::new(),
        sample_rates: SAMPLE_RATES,
        error: None,
    });
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let ctx = SourceCtx {
        cmd_rx,
        state_tx,
        iq_tx,
        initial: settings.clone(),
    };

    let cfg = cfg.clone();
    std::thread::Builder::new()
        .name("radio".into())
        .spawn(move || match cfg.source {
            SourceKind::Synthetic => synthetic::run(ctx),
            SourceKind::Replay => replay::run(ctx, cfg.replay_file.clone().unwrap()),
            #[cfg(feature = "rtlsdr")]
            SourceKind::Rtlsdr => rtlsdr::run(ctx, cfg.device_index, cfg.device_serial.clone()),
            #[cfg(not(feature = "rtlsdr"))]
            SourceKind::Rtlsdr => unreachable!("checked in Config::from_env"),
        })?;

    Ok(RadioHandle {
        cmd_tx,
        state_rx,
        desired: Arc::new(Mutex::new(settings)),
        can_tune,
    })
}

/// Convert a float sample (nominally -1..1) to the cu8 representation.
#[inline]
pub(crate) fn to_cu8(v: f32) -> u8 {
    (v * 127.5 + 127.5).round().clamp(0.0, 255.0) as u8
}

/// Block length in bytes for ~50 ms of samples, a multiple of 16 KiB
/// (librtlsdr requires multiples of 512; 16 KiB keeps USB transfers efficient).
pub(crate) fn block_len(sample_rate: u32) -> usize {
    let bytes = (sample_rate as usize * 2) / 20;
    bytes.div_ceil(16384).max(1) * 16384
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> RadioSettings {
        RadioSettings {
            center_hz: 145_300_000,
            sample_rate: 2_048_000,
            gain_db: Some(30.0),
            ppm: 0,
            bias_tee: false,
            direct_sampling: 0,
        }
    }

    #[test]
    fn patch_validates() {
        let s = base();
        assert!(s
            .apply_patch(&RadioPatch { center_hz: Some(10), ..Default::default() })
            .is_err());
        assert!(s
            .apply_patch(&RadioPatch { sample_rate: Some(3_000_000), ..Default::default() })
            .is_err());
        let n = s
            .apply_patch(&RadioPatch {
                center_hz: Some(433_920_000),
                auto_gain: Some(true),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(n.center_hz, 433_920_000);
        assert_eq!(n.gain_db, None);
    }

    #[test]
    fn block_len_is_aligned() {
        for &r in SAMPLE_RATES {
            assert_eq!(block_len(r) % 16384, 0);
        }
    }
}
