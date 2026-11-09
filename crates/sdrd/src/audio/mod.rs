//! Live audio for the browser. Each listener (one per WebSocket that asked
//! for audio) gets its own thread: demodulate its channel from the shared IQ
//! stream, encode IMA ADPCM, send binary frames.
//!
//! Audio frame (binary WebSocket message, little endian, 24-byte header):
//!
//! | off | type | field                                              |
//! |-----|------|----------------------------------------------------|
//! | 0   | u8   | frame type = 3                                     |
//! | 1   | u8   | version = 0                                        |
//! | 2   | u8   | flags: bit 0 in band, bit 1 squelch open           |
//! | 3   | u8   | mode (0 NFM, 1 WFM, 2 AM, 3 USB, 4 LSB, 5 CW)      |
//! | 4   | u32  | sample rate (Hz)                                   |
//! | 8   | f32  | channel level (dBFS)                               |
//! | 12  | u32  | sequence number                                    |
//! | 16  | i16  | ADPCM predictor at the start of this frame         |
//! | 18  | u8   | ADPCM step index at the start of this frame        |
//! | 19  | u8   | reserved                                           |
//! | 20  | u32  | number of samples                                  |
//! | 24  | ...  | ADPCM, two samples per byte, low nibble first      |
//!
//! Every frame carries its own ADPCM state, so a dropped frame costs only
//! its own 50 ms.
//!
//! Decoded text goes out as JSON messages on the same socket:
//! `{"type":"cw","text":"CQ DE ","wpm":18.5}` (new characters only) and
//! `{"type":"rds","pi":"C204","ps":"BBC R2  ","pty":10,"rt":"…","groups":42}`
//! (whenever something changes).

pub mod adpcm;
pub mod cw;
pub mod demod;
pub mod nr;
pub mod rds;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::{BufMut, Bytes, BytesMut};
use serde_json::json;
use tokio::sync::{broadcast, mpsc, watch};

use crate::radio::IqBlock;
use demod::{Demod, Params};

pub const FRAME_AUDIO: u8 = 3;

/// Counts listeners; the guard frees the slot (and stops the worker) on drop.
pub struct ListenerGuard {
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.count.fetch_sub(1, Ordering::Relaxed);
    }
}

/// What a listener sends its client.
pub enum Out {
    /// Binary audio frame.
    Frame(Bytes),
    /// JSON message (decoded Morse or RDS).
    Text(String),
}

pub struct Listener {
    pub params: watch::Sender<Params>,
    pub frames: mpsc::Receiver<Out>,
    _guard: ListenerGuard,
}

/// Start a listener if a slot is free.
pub fn start(
    count: &Arc<AtomicUsize>,
    max: usize,
    iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    params: Params,
) -> Result<Listener, String> {
    let n = count.fetch_add(1, Ordering::Relaxed);
    if n >= max {
        count.fetch_sub(1, Ordering::Relaxed);
        return Err(if max == 0 {
            "live audio is turned off on this server".into()
        } else {
            format!("all {max} audio slots are in use")
        });
    }
    let stop = Arc::new(AtomicBool::new(false));
    let guard = ListenerGuard { count: count.clone(), stop: stop.clone() };
    let (params_tx, params_rx) = watch::channel(params);
    let (frames_tx, frames) = mpsc::channel(40);
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || run(iq_rx, params_rx, frames_tx, stop))
        .map_err(|e| e.to_string())?;
    Ok(Listener { params: params_tx, frames, _guard: guard })
}

/// One audio frame (the header fields map one-to-one onto the arguments).
#[allow(clippy::too_many_arguments)]
fn frame(flags: u8, mode: u8, rate: u32, level: f32, seq: u32, st: adpcm::State, n: usize, data: &[u8]) -> Bytes {
    let mut f = BytesMut::with_capacity(24 + data.len());
    f.put_u8(FRAME_AUDIO);
    f.put_u8(0);
    f.put_u8(flags);
    f.put_u8(mode);
    f.put_u32_le(rate);
    f.put_f32_le(level);
    f.put_u32_le(seq);
    f.put_i16_le(st.predictor);
    f.put_u8(st.index);
    f.put_u8(0);
    f.put_u32_le(n as u32);
    f.put_slice(data);
    f.freeze()
}

fn run(
    mut iq_rx: broadcast::Receiver<Arc<IqBlock>>,
    params_rx: watch::Receiver<Params>,
    out: mpsc::Sender<Out>,
    stop: Arc<AtomicBool>,
) {
    let mut demod: Option<(Params, (u64, u32), Demod)> = None;
    let mut st = adpcm::State::default();
    let mut seq = 0u32;
    let (mut audio, mut pcm, mut enc) = (Vec::new(), Vec::new(), Vec::new());
    while !stop.load(Ordering::Relaxed) && !out.is_closed() {
        let block = match iq_rx.blocking_recv() {
            Ok(b) => b,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        };
        let p = params_rx.borrow().clone();
        seq = seq.wrapping_add(1);
        if !p.in_band(block.center_hz, block.sample_rate) {
            demod = None;
            let _ = out.try_send(Out::Frame(frame(0, p.mode.id(), 0, f32::NAN, seq, st, 0, &[])));
            continue;
        }
        let key = (block.center_hz, block.sample_rate);
        if demod.as_ref().is_none_or(|(dp, k, _)| *dp != p || *k != key) {
            demod = Some((p.clone(), key, Demod::new(&p, block.center_hz, block.sample_rate)));
            st = adpcm::State::default();
        }
        let (_, _, d) = demod.as_mut().unwrap();
        audio.clear();
        let (level, open) = d.process(&block.data, &mut audio);
        pcm.clear();
        pcm.extend(audio.iter().map(|v| (v.clamp(-1.0, 1.0) * 30_000.0) as i16));
        enc.clear();
        let start = adpcm::encode(&mut st, &pcm, &mut enc);
        let flags = 1 | if open { 2 } else { 0 };
        let f = frame(flags, p.mode.id(), d.out_rate.round() as u32, level, seq, start, pcm.len(), &enc);
        // A slow client just loses frames; the player rides them out.
        let _ = out.try_send(Out::Frame(f));
        if let Some(t) = d.take_cw() {
            let msg = json!({ "type": "cw", "text": t.text, "wpm": (t.wpm * 10.0).round() / 10.0 });
            let _ = out.try_send(Out::Text(msg.to_string()));
        }
        if let Some(r) = d.take_rds() {
            let mut msg = json!(r);
            msg["type"] = json!("rds");
            let _ = out.try_send(Out::Text(msg.to_string()));
        }
    }
}
