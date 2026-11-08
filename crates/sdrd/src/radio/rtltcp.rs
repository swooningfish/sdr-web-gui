//! rtl_tcp client: a dongle plugged into another machine (or the Docker host)
//! and shared with `rtl_tcp -a 0.0.0.0`. Useful where USB can't be passed
//! into the container, e.g. Docker Desktop on Windows or macOS.
//!
//! Protocol: the server greets with 12 bytes (`RTL0`, tuner type, gain count;
//! big-endian u32s), then streams raw cu8 IQ. Commands are 5 bytes: an opcode
//! and a big-endian u32 argument. Only one client is served at a time.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use super::{
    block_len, kind_of, parse_tcp_addr, tuner_name, tuner_ranges, DeviceInfo, FreqRange, IqBlock, RadioSettings,
    RadioState, SourceCtx,
};
use crate::config::SourceKind;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How often reads give up waiting so commands are handled promptly.
const POLL: Duration = Duration::from_millis(200);
/// No data for this long counts as a dead connection.
const STALL: Duration = Duration::from_secs(5);

const SET_FREQ: u8 = 0x01;
const SET_SAMPLE_RATE: u8 = 0x02;
const SET_GAIN_MODE: u8 = 0x03;
const SET_GAIN: u8 = 0x04;
const SET_FREQ_CORRECTION: u8 = 0x05;
const SET_DIRECT_SAMPLING: u8 = 0x09;
const SET_BIAS_TEE: u8 = 0x0e;

/// rtl_tcp only sends the number of gain steps, so use librtlsdr's tables
/// (tenths of a dB).
fn gain_table(tuner: &str) -> &'static [i32] {
    match tuner {
        "E4000" => &[-10, 15, 40, 65, 90, 115, 140, 165, 190, 215, 240, 290, 340, 420],
        "FC0012" => &[-99, -40, 71, 179, 192],
        "FC0013" => &[
            -99, -73, -65, -63, -60, -58, -54, 58, 61, 63, 65, 67, 68, 70, 71, 179, 181, 182, 184, 186, 188, 191, 197,
        ],
        "FC2580" => &[0],
        _ => &[
            0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372, 386, 402,
            421, 434, 439, 445, 480, 496,
        ],
    }
}

/// What a tuner seen over rtl_tcp can receive. The USB product string isn't
/// sent, so an R828D is taken to be an RTL-SDR Blog V4 (by far the most
/// common R828D dongle), whose HF upconverter the server switches in itself.
fn ranges_for(tuner: &str) -> Vec<FreqRange> {
    tuner_ranges(tuner, if tuner == "R828D" { "V4" } else { "" })
}

fn display_name(addr: &str) -> String {
    format!("rtl_tcp {addr}")
}

/// The device list entry for a remembered server. Servers aren't probed (that
/// would take the single client slot), so the tuner is what we saw last time.
pub(super) fn device_info(addr: &str, st: &RadioState, tuners: &HashMap<String, String>) -> DeviceInfo {
    let id = format!("tcp:{addr}");
    let active = st.device_id.as_deref() == Some(id.as_str());
    let (tuner, ranges) = if active && st.connected {
        (st.tuner.clone(), st.ranges.clone())
    } else {
        let t = tuners.get(&id).cloned();
        let r = t.as_deref().map(ranges_for).unwrap_or_default();
        (t, r)
    };
    let note = if active && !st.connected {
        st.error.clone()
    } else if tuner.is_none() {
        Some("network receiver; tuner is read when it's first used".into())
    } else {
        None
    };
    DeviceInfo {
        id,
        source: SourceKind::RtlTcp.name(),
        name: display_name(addr),
        serial: None,
        tuner,
        ranges,
        active,
        note,
    }
}

struct Conn {
    stream: TcpStream,
    id: String,
    name: String,
    tuner: String,
    ranges: Vec<FreqRange>,
    gains_tenths: &'static [i32],
}

impl Conn {
    fn open(id: &str) -> Result<Conn> {
        let addr = id.strip_prefix("tcp:").ok_or_else(|| anyhow!("bad device id {id:?}"))?;
        let (host, port) = parse_tcp_addr(addr).ok_or_else(|| anyhow!("bad rtl_tcp address {addr:?}"))?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let targets: Vec<SocketAddr> = (host, port)
            .to_socket_addrs()
            .with_context(|| format!("can't resolve {host}"))?
            .collect();
        let mut last = None;
        let mut stream = None;
        for t in &targets {
            match TcpStream::connect_timeout(t, CONNECT_TIMEOUT) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        let mut stream = match (stream, last) {
            (Some(s), _) => s,
            (None, Some(e)) => bail!("can't connect to rtl_tcp at {addr}: {e} (is rtl_tcp running with -a 0.0.0.0?)"),
            (None, None) => bail!("{host} has no addresses"),
        };
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(CONNECT_TIMEOUT))?;
        let mut hello = [0u8; 12];
        stream.read_exact(&mut hello).with_context(|| {
            format!("no greeting from {addr} (not an rtl_tcp server, or another client is connected)")
        })?;
        if &hello[0..4] != b"RTL0" {
            bail!("{addr} is not an rtl_tcp server");
        }
        let tuner = tuner_name(u32::from_be_bytes(hello[4..8].try_into().unwrap())).to_string();
        stream.set_read_timeout(Some(POLL))?;
        Ok(Conn {
            stream,
            id: id.to_string(),
            name: display_name(addr),
            ranges: ranges_for(&tuner),
            gains_tenths: gain_table(&tuner),
            tuner,
        })
    }

    fn cmd(&mut self, op: u8, arg: u32) -> Result<()> {
        let mut b = [0u8; 5];
        b[0] = op;
        b[1..].copy_from_slice(&arg.to_be_bytes());
        self.stream.write_all(&b).context("sending command to rtl_tcp")
    }

    /// Apply settings, only sending what changed relative to `prev`.
    fn apply(&mut self, s: &RadioSettings, prev: Option<&RadioSettings>) -> Result<()> {
        let changed = |f: fn(&RadioSettings) -> i64| prev.map_or(true, |p| f(p) != f(s));
        if changed(|x| x.sample_rate as i64) {
            self.cmd(SET_SAMPLE_RATE, s.sample_rate)?;
        }
        if changed(|x| x.direct_sampling as i64) {
            self.cmd(SET_DIRECT_SAMPLING, s.direct_sampling as u32)?;
        }
        if changed(|x| x.ppm as i64) {
            self.cmd(SET_FREQ_CORRECTION, s.ppm as u32)?;
        }
        if changed(|x| x.center_hz as i64) || changed(|x| x.sample_rate as i64) {
            let f = u32::try_from(s.center_hz).map_err(|_| anyhow!("frequency too high"))?;
            self.cmd(SET_FREQ, f)?;
        }
        if prev.map_or(true, |p| p.gain_db != s.gain_db) {
            match s.gain_db {
                None => self.cmd(SET_GAIN_MODE, 0)?,
                Some(g) => {
                    self.cmd(SET_GAIN_MODE, 1)?;
                    let want = (g * 10.0).round() as i32;
                    let nearest =
                        self.gains_tenths.iter().copied().min_by_key(|v| (v - want).abs()).unwrap_or(want);
                    self.cmd(SET_GAIN, nearest as u32)?;
                }
            }
        }
        if changed(|x| x.bias_tee as i64) {
            self.cmd(SET_BIAS_TEE, s.bias_tee as u32)?;
        }
        Ok(())
    }
}

/// Runs until a device of another kind is selected; returns its id.
pub(super) fn run(ctx: &mut SourceCtx) -> String {
    let mut s = ctx.current();
    let mut id = ctx.device.clone();
    let mut last_err: Option<String> = None;
    loop {
        let p = ctx.take();
        if let Some(n) = p.settings {
            s = n;
            ctx.update_state(|st| st.settings = s.clone());
        }
        if let Some(d) = p.device {
            if kind_of(&d) != Some(SourceKind::RtlTcp) {
                return d;
            }
            id = d;
            last_err = None;
        }
        ctx.update_state(|st| st.device_id = Some(id.clone()));
        let mut conn = match Conn::open(&id) {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("{e:#}");
                if last_err.as_deref() != Some(msg.as_str()) {
                    tracing::warn!("{msg}");
                    last_err = Some(msg.clone());
                }
                ctx.update_state(|st| {
                    st.connected = false;
                    st.device = Some(id.strip_prefix("tcp:").map(display_name).unwrap_or_default());
                    st.tuner = None;
                    st.ranges = Vec::new();
                    st.gains_db = Vec::new();
                    st.error = Some(msg);
                });
                ctx.wait(Duration::from_secs(5));
                continue;
            }
        };
        last_err = None;
        ctx.tuners.lock().unwrap().insert(conn.id.clone(), conn.tuner.clone());
        ctx.fit_to(&mut s, &conn.ranges);
        if let Err(e) = conn.apply(&s, None) {
            tracing::warn!("rtl_tcp configuration failed: {e}");
            ctx.update_state(|st| st.error = Some(e.to_string()));
            ctx.wait(Duration::from_secs(5));
            continue;
        }
        tracing::info!(server = %conn.name, tuner = %conn.tuner, "rtl_tcp connected");
        ctx.update_state(|st| {
            st.connected = true;
            st.device_id = Some(conn.id.clone());
            st.device = Some(conn.name.clone());
            st.tuner = Some(conn.tuner.clone());
            st.ranges = conn.ranges.clone();
            st.gains_db = conn.gains_tenths.iter().map(|g| *g as f32 / 10.0).collect();
            st.settings = s.clone();
            st.error = None;
        });

        let mut applied = s.clone();
        let mut buf = vec![0u8; block_len(applied.sample_rate)];
        let mut filled = 0;
        let mut last_data = Instant::now();
        let mut switching = false;
        loop {
            let p = ctx.take();
            if let Some(n) = p.settings {
                if let Err(e) = conn.apply(&n, Some(&applied)) {
                    tracing::warn!("{e}");
                    ctx.update_state(|st| {
                        st.connected = false;
                        st.error = Some(e.to_string());
                    });
                    s = n;
                    break;
                }
                if n.sample_rate != applied.sample_rate {
                    // Drop the partial block taken at the old rate.
                    buf = vec![0u8; block_len(n.sample_rate)];
                    filled = 0;
                }
                applied = n.clone();
                s = n;
                ctx.update_state(|st| {
                    st.settings = s.clone();
                    st.error = None;
                });
            }
            if let Some(d) = p.device {
                if kind_of(&d) != Some(SourceKind::RtlTcp) {
                    tracing::info!(from = %conn.id, to = %d, "closing rtl_tcp");
                    return d;
                }
                tracing::info!(from = %conn.id, to = %d, "switching rtl_tcp server");
                id = d;
                switching = true;
                break;
            }
            match conn.stream.read(&mut buf[filled..]) {
                Ok(0) => {
                    let msg = "rtl_tcp server closed the connection".to_string();
                    tracing::warn!("{msg}");
                    ctx.update_state(|st| {
                        st.connected = false;
                        st.error = Some(msg);
                    });
                    break;
                }
                Ok(n) => {
                    last_data = Instant::now();
                    filled += n;
                    if filled == buf.len() {
                        let data = std::mem::replace(&mut buf, vec![0u8; block_len(applied.sample_rate)]);
                        filled = 0;
                        ctx.publish(IqBlock {
                            center_hz: applied.center_hz,
                            sample_rate: applied.sample_rate,
                            data,
                        });
                    }
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                    if last_data.elapsed() > STALL {
                        let msg = "no data from the rtl_tcp server".to_string();
                        tracing::warn!("{msg}");
                        ctx.update_state(|st| {
                            st.connected = false;
                            st.error = Some(msg);
                        });
                        break;
                    }
                }
                Err(e) => {
                    let msg = format!("rtl_tcp read failed: {e}");
                    tracing::warn!("{msg}");
                    ctx.update_state(|st| {
                        st.connected = false;
                        st.error = Some(msg);
                    });
                    break;
                }
            }
        }
        drop(conn);
        if !switching {
            ctx.wait(Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn handshake_and_commands() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut hello = b"RTL0".to_vec();
            hello.extend(5u32.to_be_bytes());
            hello.extend(29u32.to_be_bytes());
            c.write_all(&hello).unwrap();
            let mut cmds = [0u8; 5 * 7];
            c.read_exact(&mut cmds).unwrap();
            cmds
        });
        let mut conn = Conn::open(&format!("tcp:127.0.0.1:{port}")).unwrap();
        assert_eq!(conn.tuner, "R820T/R860");
        let s = RadioSettings {
            center_hz: 145_800_000,
            sample_rate: 2_048_000,
            gain_db: Some(30.0),
            ppm: -2,
            bias_tee: false,
            direct_sampling: 0,
        };
        conn.apply(&s, None).unwrap();
        let cmds = server.join().unwrap();
        let ops: Vec<(u8, u32)> =
            cmds.chunks(5).map(|c| (c[0], u32::from_be_bytes(c[1..5].try_into().unwrap()))).collect();
        assert_eq!(
            ops,
            vec![
                (SET_SAMPLE_RATE, 2_048_000),
                (SET_DIRECT_SAMPLING, 0),
                (SET_FREQ_CORRECTION, (-2i32) as u32),
                (SET_FREQ, 145_800_000),
                (SET_GAIN_MODE, 1),
                (SET_GAIN, 297),
                (SET_BIAS_TEE, 0),
            ]
        );
    }

    #[test]
    fn v4_over_tcp_gets_hf() {
        assert!(super::super::covers(&ranges_for("R828D"), 7_100_000));
        assert!(!super::super::covers(&ranges_for("R820T/R860"), 7_100_000));
    }
}
