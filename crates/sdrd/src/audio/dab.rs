//! DAB listening: follow the multiplex the receiver is centred on, decode
//! the chosen service's sub-channel, and turn its audio frames into PCM
//! with ffmpeg (DAB+ access units as LOAS/LATM, MP2 frames as they are).
//! ffmpeg runs as a separate process, like dsd-neo, and is fed and read on
//! threads of its own so a stall can't hold up the listener.
//!
//! Sends `{"type":"dab",…}` whenever what it knows changes: lock, the
//! ensemble and its services (label, coding, bit rate), the service playing,
//! its codec, and any error.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::dab::dabplus::{loas, Superframes};
use crate::dab::msc::{AudioKind, SubDecoder, CIF_BITS};
use crate::dab::ofdm::{Stream, FIC_SOFT};

/// PCM rate the decoder is asked for (mono).
pub const OUT_RATE: u32 = 32_000;

/// The ffmpeg process decoding one service.
struct Decoder {
    child: Child,
    input: Sender<Vec<u8>>,
    output: Receiver<Vec<f32>>,
    /// Its last complaint (stderr).
    last_error: Arc<Mutex<Option<String>>>,
}

impl Decoder {
    fn start(ffmpeg: &str, kind: AudioKind) -> Result<Decoder, String> {
        let format = if kind == AudioKind::DabPlus { "loas" } else { "mp3" };
        let rate = OUT_RATE.to_string();
        let mut child = Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-fflags", "nobuffer", "-probesize", "32"])
            .args(["-analyzeduration", "0", "-f", format, "-i", "pipe:0"])
            .args(["-ac", "1", "-ar", &rate, "-f", "s16le", "pipe:1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("DAB audio needs ffmpeg ({ffmpeg}): {e}"))?;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        let mut stderr = child.stderr.take().ok_or("no stderr")?;
        let (input, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("dab-feed".into())
            .spawn(move || {
                for chunk in rx {
                    if stdin.write_all(&chunk).and_then(|_| stdin.flush()).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let (tx, output) = mpsc::channel::<Vec<f32>>();
        std::thread::Builder::new()
            .name("dab-pcm".into())
            .spawn(move || {
                let mut buf = vec![0u8; 8192];
                let mut odd: Option<u8> = None;
                loop {
                    let n = match stdout.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let mut bytes: Vec<u8> = odd.take().into_iter().collect();
                    bytes.extend_from_slice(&buf[..n]);
                    if bytes.len() % 2 == 1 {
                        odd = bytes.pop();
                    }
                    let pcm: Vec<f32> =
                        bytes.chunks_exact(2).map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32_768.0).collect();
                    if tx.send(pcm).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let last_error = Arc::new(Mutex::new(None));
        let err = last_error.clone();
        std::thread::Builder::new()
            .name("dab-log".into())
            .spawn(move || {
                let mut s = String::new();
                let mut buf = [0u8; 1024];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if let Some(line) = s.lines().filter(|l| !l.trim().is_empty()).last() {
                        *err.lock().unwrap() = Some(line.trim().chars().take(200).collect());
                    }
                    let keep = s.len().saturating_sub(4096);
                    s.drain(..keep);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Decoder { child, input, output, last_error })
    }

    /// Decoded audio so far; Err once ffmpeg has gone.
    fn take(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        loop {
            match self.output.try_recv() {
                Ok(pcm) => out.extend_from_slice(&pcm),
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    let why = self.last_error.lock().unwrap().clone().unwrap_or_else(|| "it stopped".into());
                    return Err(format!("the DAB audio decoder stopped: {why}"));
                }
            }
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What's playing: the service, its sub-channel decoder and framing.
struct Playing {
    sid: u16,
    kind: AudioKind,
    sub: SubDecoder,
    superframes: Option<Superframes>,
    codec: Option<String>,
    /// Good / bad audio frames, for the stats.
    good: u64,
    bad: u64,
}

pub struct DabPlayer {
    ffmpeg: String,
    center_hz: u64,
    stream: Stream,
    want: Option<u16>,
    playing: Option<Playing>,
    decoder: Option<Decoder>,
    retry: Option<Instant>,
    error: Option<String>,
    sent: Option<Value>,
    pending: Vec<Vec<u8>>,
    /// Decoded audio waiting to go out: the decoder delivers a DAB frame
    /// (96 ms) at a time, listeners get an even flow.
    queue: std::collections::VecDeque<f32>,
    /// Whether the queue has filled enough to start (or restart) playing.
    running: bool,
}

impl DabPlayer {
    pub fn new(ffmpeg: String) -> DabPlayer {
        DabPlayer {
            ffmpeg,
            center_hz: 0,
            stream: Stream::new(),
            want: None,
            playing: None,
            decoder: None,
            retry: None,
            error: None,
            sent: None,
            pending: Vec::new(),
            queue: std::collections::VecDeque::new(),
            running: false,
        }
    }

    /// Feed a block; decoded audio (mono, [`OUT_RATE`]) is appended to
    /// `audio`. Returns whether audio is flowing, and a status message for
    /// the client when something changed.
    pub fn process(
        &mut self,
        data: &[u8],
        center_hz: u64,
        service: Option<u16>,
        audio: &mut Vec<f32>,
    ) -> (bool, Option<String>) {
        if center_hz != self.center_hz {
            // Another multiplex: start again.
            self.center_hz = center_hz;
            self.queue.clear();
            self.stream = Stream::new();
            self.playing = None;
            self.decoder = None;
            self.error = None;
        }
        if service != self.want {
            self.want = service;
            self.queue.clear();
            self.playing = None;
            self.decoder = None;
            self.error = None;
        }
        let mut frames: Vec<Vec<f32>> = Vec::new();
        self.stream.push(data, |soft| frames.push(soft[FIC_SOFT..].to_vec()));
        self.choose();
        if let Some(p) = self.playing.as_mut() {
            for msc in &frames {
                for cif in msc.chunks_exact(CIF_BITS) {
                    let Some(frame) = p.sub.push(cif) else { continue };
                    match p.superframes.as_mut() {
                        Some(sf) => {
                            if let Some(s) = sf.push(frame) {
                                if s.corrected > 0 || s.bad_aus > 0 {
                                    tracing::debug!(corrected = s.corrected, bad_aus = s.bad_aus, "DAB+ superframe");
                                }
                                p.codec = Some(s.format.codec().to_string());
                                p.good += s.aus.len() as u64;
                                p.bad += s.bad_aus as u64;
                                self.pending.extend(s.aus.iter().map(|au| loas(&s.format, au)));
                            }
                        }
                        None => {
                            p.good += 1;
                            self.pending.push(frame);
                        }
                    }
                }
            }
        }
        self.feed();
        if let Some(d) = self.decoder.as_mut() {
            let mut fresh = Vec::new();
            let res = d.take(&mut fresh);
            self.queue.extend(fresh);
            match res {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!("{e}");
                    self.error = Some(e);
                    self.decoder = None;
                    self.retry = Some(Instant::now() + Duration::from_secs(10));
                }
            }
        } else {
            self.queue.clear();
        }
        // This block's worth of audio, once 150 ms is waiting; a little faster
        // if more than half a second has built up, so the delay stays small.
        let rate = OUT_RATE as usize;
        let block = data.len() / 2 * rate / crate::dab::RATE as usize;
        if !self.running && self.queue.len() >= rate * 15 / 100 {
            self.running = true;
        }
        let want = if self.queue.len() > rate / 2 { block + block / 8 } else { block };
        let n = if self.running { want.min(self.queue.len()) } else { 0 };
        audio.extend(self.queue.drain(..n));
        if self.queue.is_empty() {
            self.running = false;
        }
        (n > 0, self.status())
    }

    /// Pick the service to play once the FIC says where it is.
    fn choose(&mut self) {
        if self.playing.is_some() {
            return;
        }
        let e = &self.stream.ensemble;
        let sid = match self.want {
            Some(s) => s,
            None => match e.audio.keys().find(|s| e.services.contains_key(s)).or(e.audio.keys().next()) {
                Some(&s) => s,
                None => return,
            },
        };
        let Some((sub, kind)) = e.audio_of(sid) else { return };
        match kind {
            AudioKind::Other => {
                self.error = Some("this service's audio coding isn't supported".into());
                return;
            }
            AudioKind::Mp2 | AudioKind::DabPlus => {}
        }
        let Some(dec) = SubDecoder::new(&sub) else {
            self.error = Some(format!("can't decode sub-channel {} ({:?})", sub.id, sub.protection));
            return;
        };
        let frame_len = sub.bitrate().unwrap_or(0) as usize * 3;
        self.playing = Some(Playing {
            sid,
            kind,
            sub: dec,
            superframes: (kind == AudioKind::DabPlus).then(|| Superframes::new(frame_len)),
            codec: (kind == AudioKind::Mp2).then(|| "MP2".to_string()),
            good: 0,
            bad: 0,
        });
        self.decoder = None;
        self.pending.clear();
    }

    /// Hand frames to ffmpeg, starting it when there's something to play.
    fn feed(&mut self) {
        let Some(p) = self.playing.as_ref() else {
            self.pending.clear();
            return;
        };
        if self.pending.is_empty() {
            return;
        }
        if self.decoder.is_none() && self.retry.is_none_or(|t| Instant::now() >= t) {
            match Decoder::start(&self.ffmpeg, p.kind) {
                Ok(d) => {
                    self.decoder = Some(d);
                    self.error = None;
                }
                Err(e) => {
                    tracing::warn!("{e}");
                    self.error = Some(e);
                    self.retry = Some(Instant::now() + Duration::from_secs(30));
                }
            }
        }
        match self.decoder.as_ref() {
            Some(d) => {
                for f in self.pending.drain(..) {
                    let _ = d.input.send(f);
                }
            }
            None => self.pending.clear(),
        }
    }

    fn status(&mut self) -> Option<String> {
        let e = &self.stream.ensemble;
        let services: Vec<Value> = e
            .services
            .iter()
            .map(|(sid, label)| {
                let audio = e.audio_of(*sid);
                json!({
                    "sid": format!("{sid:04X}"),
                    "label": label,
                    "kind": audio.as_ref().map(|a| a.1),
                    "bitrate": audio.as_ref().and_then(|a| a.0.bitrate()),
                })
            })
            .collect();
        let p = self.playing.as_ref();
        let v = json!({
            "type": "dab",
            "locked": self.stream.locked(),
            "ensemble": e.label,
            "eid": e.eid.map(|x| format!("{x:04X}")),
            "services": services,
            "service": p.map(|p| format!("{:04X}", p.sid)),
            "label": p.and_then(|p| e.services.get(&p.sid)),
            "codec": p.and_then(|p| p.codec.clone()),
            "error": self.error,
        });
        if self.sent.as_ref() == Some(&v) {
            return None;
        }
        self.sent = Some(v.clone());
        Some(v.to_string())
    }
}

/// The sub-channel the player is decoding (for tests).
#[cfg(test)]
impl DabPlayer {
    fn playing_sub(&self) -> Option<crate::dab::msc::SubChannel> {
        self.playing.as_ref().map(|p| p.sub.subchannel().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_service_decodes_to_mp2_frames() {
        let mut gen = crate::radio::synth_for_tests();
        let mut stream = Stream::new();
        let mut dec: Option<SubDecoder> = None;
        let mut frames = Vec::new();
        for _ in 0..60 {
            let data = gen(222_064_000, crate::dab::RATE, 0.05);
            let mut cifs = Vec::new();
            stream.push(&data, |soft| cifs.extend(soft[FIC_SOFT..].chunks_exact(CIF_BITS).map(<[f32]>::to_vec)));
            if dec.is_none() {
                dec = stream.ensemble.audio_of(0xC223).and_then(|(s, _)| SubDecoder::new(&s));
            }
            if let Some(d) = dec.as_mut() {
                frames.extend(cifs.iter().filter_map(|c| d.push(c)));
            }
        }
        assert!(frames.len() > 20, "{} frames", frames.len());
        for f in &frames {
            assert_eq!(f.len(), crate::dab::mp2::FRAME_BYTES);
            assert_eq!(&f[..4], &[0xFF, 0xFD, 0x84, 0xC0], "an MP2 frame header");
        }
    }

    #[test]
    fn follows_the_simulated_multiplex_and_reports_a_missing_decoder() {
        let mut gen = crate::radio::synth_for_tests();
        let mut player = DabPlayer::new("/nonexistent/ffmpeg".into());
        let mut audio = Vec::new();
        let mut last = None;
        for _ in 0..40 {
            let data = gen(222_064_000, crate::dab::RATE, 0.05);
            let (_, msg) = player.process(&data, 222_064_000, Some(0xC222), &mut audio);
            last = msg.or(last);
        }
        assert!(player.stream.locked());
        assert_eq!(player.playing_sub().map(|s| s.id), Some(2), "Sim Jazz is on sub-channel 2");
        let v: Value = serde_json::from_str(&last.unwrap()).unwrap();
        assert_eq!(v["ensemble"], "SIM DAB");
        assert_eq!(v["label"], "Sim Jazz");
        assert_eq!(v["codec"], "MP2");
        assert_eq!(v["services"].as_array().unwrap().len(), 5);
        assert_eq!(v["services"][0]["kind"], "mp2");
        assert_eq!(v["services"][0]["bitrate"], 128);
        assert!(v["error"].as_str().unwrap().contains("needs ffmpeg"), "{v}");
        assert!(audio.is_empty());
    }
}
