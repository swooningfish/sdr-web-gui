//! Extra receivers for Mesh, Sensors (rtl_433) and APRS.
//!
//! Each of these decoders normally works from the main receiver's capture,
//! so it only hears anything while the main receiver is tuned to its band. A
//! spare dongle can be given to one of them instead: it stays tuned to that
//! decoder's band, and the decoder is fed from it rather than from the main
//! receiver. If that dongle isn't connected (unplugged, rtl_tcp server down)
//! the decoder falls back to the main receiver until it is.
//!
//! A relay task per decoder picks which capture to forward, and also
//! publishes the state of the receiver in use (for the gain and device shown
//! with decoded packets, and whether a decoder's frequencies are in band).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::config::Config;
use crate::radio::{self, IqBlock, RadioHandle, RadioSettings, RadioState, Registry, OFF};

/// How often a receiver checks that it's still tuned where its decoder needs.
const FOLLOW_EVERY: Duration = Duration::from_secs(2);

/// Which decoder an extra receiver is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Use {
    Mesh,
    Sensors,
    Aprs,
}

impl Use {
    pub const ALL: [Use; 3] = [Use::Mesh, Use::Sensors, Use::Aprs];

    pub fn key(self) -> &'static str {
        match self {
            Use::Mesh => "mesh",
            Use::Sensors => "sensors",
            Use::Aprs => "aprs",
        }
    }

    pub fn parse(s: &str) -> Option<Use> {
        Use::ALL.into_iter().find(|u| u.key() == s)
    }

    fn role(self) -> radio::Role {
        match self {
            Use::Mesh => radio::Role::Mesh,
            Use::Sensors => radio::Role::Sensors,
            Use::Aprs => radio::Role::Aprs,
        }
    }

    pub fn label(self) -> &'static str {
        self.role().label()
    }

    /// Setting: the selected device id.
    pub fn device_key(self) -> String {
        format!("receiver.{}.device", self.key())
    }

    /// Setting: [`ReceiverSettings`].
    pub fn settings_key(self) -> String {
        format!("receiver.{}.radio", self.key())
    }
}

/// Where an extra receiver is tuned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Tuning {
    pub center_hz: u64,
    pub sample_rate: u32,
}

/// What the user sets for an extra receiver; the tuning follows its decoder.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReceiverSettings {
    /// `None` = tuner AGC.
    pub gain_db: Option<f32>,
    pub ppm: i32,
    pub bias_tee: bool,
}

impl ReceiverSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.gain_db.is_some_and(|g| !(0.0..=60.0).contains(&g) || !g.is_finite()) {
            return Err("gain must be between 0 and 60 dB".into());
        }
        if !(-200..=200).contains(&self.ppm) {
            return Err("ppm must be between -200 and 200".into());
        }
        Ok(())
    }

    fn radio(&self, t: Tuning) -> RadioSettings {
        RadioSettings {
            center_hz: t.center_hz,
            sample_rate: t.sample_rate,
            gain_db: self.gain_db,
            ppm: self.ppm,
            bias_tee: self.bias_tee,
            direct_sampling: 0,
        }
    }
}

/// Whether this receiver's own capture is the one being used.
fn dedicated(st: &RadioState) -> bool {
    st.connected && st.device_id.as_deref().is_some_and(|d| d != OFF)
}

#[derive(Clone)]
pub struct AuxReceiver {
    pub use_: Use,
    pub radio: RadioHandle,
    feed: broadcast::Sender<Arc<IqBlock>>,
    in_use: watch::Receiver<RadioState>,
    settings: Arc<std::sync::Mutex<(ReceiverSettings, Tuning)>>,
}

impl AuxReceiver {
    /// The capture the decoder should work from.
    pub fn feed(&self) -> broadcast::Receiver<Arc<IqBlock>> {
        self.feed.subscribe()
    }

    /// State of the receiver the decoder is being fed from.
    pub fn in_use(&self) -> watch::Receiver<RadioState> {
        self.in_use.clone()
    }

    /// The decoder is fed from this receiver rather than the main one.
    pub fn is_dedicated(&self) -> bool {
        dedicated(&self.radio.state())
    }

    pub fn settings(&self) -> ReceiverSettings {
        self.settings.lock().unwrap().0.clone()
    }

    pub fn tuning(&self) -> Tuning {
        self.settings.lock().unwrap().1
    }

    /// Change gain, ppm and bias-T (kept while no device is selected).
    pub fn set_settings(&self, s: ReceiverSettings) -> Result<(), String> {
        s.validate()?;
        let mut g = self.settings.lock().unwrap();
        self.radio.force_settings(s.radio(g.1))?;
        g.0 = s;
        Ok(())
    }

    fn retune(&self, t: Tuning) {
        let mut g = self.settings.lock().unwrap();
        if g.1 == t {
            return;
        }
        match self.radio.force_settings(g.0.radio(t)) {
            Ok(()) => {
                tracing::info!(receiver = self.use_.key(), center_hz = t.center_hz, rate = t.sample_rate, "extra receiver retuned");
                g.1 = t;
            }
            Err(e) => tracing::warn!(receiver = self.use_.key(), "can't retune extra receiver: {e}"),
        }
    }

    /// Keep this receiver tuned where `target` says its decoder needs.
    pub fn follow(&self, target: impl Fn() -> Tuning + Send + 'static) {
        let me = self.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(FOLLOW_EVERY);
            loop {
                t.tick().await;
                me.retune(target());
            }
        });
    }
}

pub fn spawn(
    cfg: &Config,
    use_: Use,
    registry: Arc<Registry>,
    main: &RadioHandle,
    main_iq: &broadcast::Sender<Arc<IqBlock>>,
    saved_device: Option<String>,
    settings: ReceiverSettings,
    tuning: Tuning,
) -> Result<AuxReceiver> {
    let (own_tx, _) = broadcast::channel(16);
    let radio = radio::spawn(cfg, use_.role(), registry, own_tx.clone(), saved_device, settings.radio(tuning))?;
    let (feed, _) = broadcast::channel(16);
    let (in_use_tx, in_use) = watch::channel(main.state());
    tokio::spawn(relay(
        main_iq.subscribe(),
        own_tx.subscribe(),
        main.subscribe(),
        radio.subscribe(),
        feed.clone(),
        in_use_tx,
    ));
    Ok(AuxReceiver { use_, radio, feed, in_use, settings: Arc::new(std::sync::Mutex::new((settings, tuning))) })
}

/// Forward the capture of whichever receiver is in use.
async fn relay(
    mut main_iq: broadcast::Receiver<Arc<IqBlock>>,
    mut own_iq: broadcast::Receiver<Arc<IqBlock>>,
    mut main_st: watch::Receiver<RadioState>,
    mut own_st: watch::Receiver<RadioState>,
    feed: broadcast::Sender<Arc<IqBlock>>,
    in_use: watch::Sender<RadioState>,
) {
    use broadcast::error::RecvError;
    let mut own = dedicated(&own_st.borrow());
    loop {
        let mut changed = false;
        tokio::select! {
            r = main_iq.recv() => match r {
                Ok(b) if !own => { let _ = feed.send(b); }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            r = own_iq.recv() => match r {
                Ok(b) if own => { let _ = feed.send(b); }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            r = main_st.changed() => { if r.is_err() { break } changed = true; }
            r = own_st.changed() => { if r.is_err() { break } changed = true; }
        }
        if changed {
            let m = main_st.borrow_and_update().clone();
            let o = own_st.borrow_and_update().clone();
            own = dedicated(&o);
            in_use.send_replace(if own { o } else { m });
        }
    }
}

/// Sensors: rtl_433's frequency, 250 kHz off centre to keep clear of the
/// dongle's DC spike, in a 1 MS/s capture (rtl_433 gets 256 kS/s of it).
pub fn sensors_tuning(frequency_hz: u64) -> Tuning {
    Tuning { center_hz: frequency_hz + 250_000, sample_rate: 1_024_000 }
}

/// APRS: centred between the enabled channels in a 2 MS/s capture (one
/// channel sits 250 kHz off centre).
pub fn aprs_tuning(channels: &[u64]) -> Tuning {
    let lo = channels.iter().copied().min().unwrap_or(144_800_000);
    let hi = channels.iter().copied().max().unwrap_or(lo);
    let center = if hi - lo < 100_000 { lo + 250_000 } else { lo + (hi - lo) / 2 };
    Tuning { center_hz: center, sample_rate: 2_048_000 }
}

/// Mesh: where the main receiver would be tuned to hear every preset.
pub fn mesh_tuning() -> Tuning {
    Tuning { center_hz: crate::mesh::TUNE_HZ, sample_rate: 1_024_000 }
}

/// The three extra receivers.
#[derive(Clone)]
pub struct Receivers {
    pub mesh: AuxReceiver,
    pub sensors: AuxReceiver,
    pub aprs: AuxReceiver,
}

impl Receivers {
    pub fn get(&self, u: Use) -> &AuxReceiver {
        match u {
            Use::Mesh => &self.mesh,
            Use::Sensors => &self.sensors,
            Use::Aprs => &self.aprs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::ddc::fits;

    #[test]
    fn tunings_cover_their_decoders() {
        let t = sensors_tuning(433_920_000);
        assert!(fits(t.center_hz, t.sample_rate, 433_920_000, 0.8 * 256_000.0));
        let t = aprs_tuning(&[144_800_000, 145_825_000]);
        assert!(fits(t.center_hz, t.sample_rate, 144_800_000, 16_000.0));
        assert!(fits(t.center_hz, t.sample_rate, 145_825_000, 16_000.0));
        let t = aprs_tuning(&[144_800_000]);
        assert!(fits(t.center_hz, t.sample_rate, 144_800_000, 16_000.0));
        assert_ne!(t.center_hz, 144_800_000);
        let t = mesh_tuning();
        assert!(crate::mesh::PRESETS.iter().all(|p| crate::mesh::in_band(t.center_hz, t.sample_rate, p)));
    }

    #[test]
    fn uses_round_trip() {
        for u in Use::ALL {
            assert_eq!(Use::parse(u.key()), Some(u));
        }
        assert_eq!(Use::parse("adsb"), None);
    }
}
