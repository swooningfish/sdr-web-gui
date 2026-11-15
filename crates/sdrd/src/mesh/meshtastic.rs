//! Meshtastic packets: the 16-byte radio header, AES-CTR channel encryption
//! and the protobufs inside (text, position, node info, telemetry).
//!
//! Channel packets are encrypted with the channel's PSK (the default
//! "LongFast" key unless the owner changed it) using AES-CTR with a nonce of
//! packet id and sender. A one-byte hash of the channel name and key says
//! which channel a packet is on. Direct messages between nodes on firmware
//! 2.5+ use public-key encryption (channel hash 0): we can only show who sent
//! them to whom.

use aes::cipher::{KeyIvInit, StreamCipher};
use serde::Serialize;

use super::proto::{self, Writer};

pub const BROADCAST: u32 = 0xFFFF_FFFF;
/// The well-known default key (PSK index 1, "AQ==").
pub const DEFAULT_KEY: [u8; 16] =
    [0xd4, 0xf1, 0xbb, 0x3a, 0x20, 0x29, 0x07, 0x59, 0xf0, 0xbc, 0xff, 0xab, 0xcf, 0x4e, 0x69, 0x01];

/// A channel we can read: its name and AES key (empty = unencrypted).
#[derive(Clone, Debug)]
pub struct Channel {
    pub name: String,
    pub key: Vec<u8>,
    /// Not the default public channel: messages are shown to admins only.
    pub private: bool,
}

impl Channel {
    pub fn hash(&self) -> u8 {
        let x = |b: &[u8]| b.iter().fold(0u8, |a, v| a ^ v);
        x(self.name.as_bytes()) ^ x(&self.key)
    }
}

/// Key from a PSK as Meshtastic stores it: 0 bytes (none), 1 byte (index of
/// the default key: 1 = default, 2–10 = default with its last byte bumped),
/// or a full 16/32-byte AES key.
pub fn expand_psk(psk: &[u8]) -> Result<Vec<u8>, String> {
    match psk.len() {
        0 => Ok(Vec::new()),
        1 => match psk[0] {
            0 => Ok(Vec::new()),
            i @ 1..=10 => {
                let mut k = DEFAULT_KEY.to_vec();
                k[15] = k[15].wrapping_add(i - 1);
                Ok(k)
            }
            _ => Err("a one-byte PSK must be 0–10".into()),
        },
        16 | 32 => Ok(psk.to_vec()),
        n => Err(format!("a PSK is 0, 1, 16 or 32 bytes, not {n}")),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Header {
    pub to: u32,
    pub from: u32,
    pub id: u32,
    pub hop_limit: u8,
    pub hop_start: u8,
    pub want_ack: bool,
    pub via_mqtt: bool,
    pub channel_hash: u8,
    pub next_hop: u8,
    pub relay: u8,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    /// Couldn't decrypt: a channel we have no key for, or a PKI direct message.
    Encrypted { pki: bool },
    Text { text: String },
    Position { lat: f64, lon: f64, alt: Option<i32>, sats: Option<u32> },
    NodeInfo { id: String, long_name: String, short_name: String, hw: String, role: String, licensed: bool },
    Telemetry {
        battery: Option<u32>,
        voltage: Option<f32>,
        channel_util: Option<f32>,
        air_util_tx: Option<f32>,
        uptime_s: Option<u32>,
        temperature: Option<f32>,
        humidity: Option<f32>,
        pressure: Option<f32>,
    },
    Other { port: u32, port_name: String, len: usize },
}

#[derive(Clone, Debug, Serialize)]
pub struct Packet {
    #[serde(flatten)]
    pub header: Header,
    /// Name of the channel whose key opened it.
    pub channel: Option<String>,
    pub private: bool,
    pub content: Content,
}

pub fn node_id(n: u32) -> String {
    format!("!{n:08x}")
}

fn port_name(p: u32) -> &'static str {
    match p {
        1 => "text",
        2 => "remote hardware",
        3 => "position",
        4 => "node info",
        5 => "routing",
        6 => "admin",
        7 => "text (compressed)",
        8 => "waypoint",
        9 => "audio",
        10 => "detection sensor",
        32 => "reply",
        33 => "IP tunnel",
        34 => "paxcounter",
        64 => "serial",
        65 => "store & forward",
        66 => "range test",
        67 => "telemetry",
        70 => "traceroute",
        71 => "neighbour info",
        73 => "map report",
        _ => "other",
    }
}

fn hw_name(h: u32) -> String {
    match h {
        0 => "unset".into(),
        4 => "T-Beam".into(),
        7 => "T-Echo".into(),
        9 => "RAK4631".into(),
        43 => "Heltec V3".into(),
        44 => "Heltec Wireless Stick Lite V3".into(),
        48 => "Heltec Wireless Tracker".into(),
        50 => "T-Deck".into(),
        255 => "private hardware".into(),
        n => format!("hardware #{n}"),
    }
}

fn role_name(r: u32) -> String {
    const ROLES: [&str; 12] = [
        "client",
        "client (mute)",
        "router",
        "router client",
        "repeater",
        "tracker",
        "sensor",
        "TAK",
        "client (hidden)",
        "lost and found",
        "TAK tracker",
        "router (late)",
    ];
    ROLES.get(r as usize).map_or_else(|| format!("role #{r}"), |s| s.to_string())
}

fn crypt(key: &[u8], h: &Header, data: &mut [u8]) {
    let mut iv = [0u8; 16];
    iv[..4].copy_from_slice(&h.id.to_le_bytes());
    iv[8..12].copy_from_slice(&h.from.to_le_bytes());
    match key.len() {
        16 => ctr::Ctr128BE::<aes::Aes128>::new_from_slices(key, &iv).unwrap().apply_keystream(data),
        32 => ctr::Ctr128BE::<aes::Aes256>::new_from_slices(key, &iv).unwrap().apply_keystream(data),
        _ => {}
    }
}

pub fn parse_header(raw: &[u8]) -> Option<Header> {
    if raw.len() < 16 {
        return None;
    }
    let u = |i: usize| u32::from_le_bytes(raw[i..i + 4].try_into().unwrap());
    let flags = raw[12];
    Some(Header {
        to: u(0),
        from: u(4),
        id: u(8),
        hop_limit: flags & 7,
        want_ack: flags & 8 != 0,
        via_mqtt: flags & 16 != 0,
        hop_start: flags >> 5,
        channel_hash: raw[13],
        next_hop: raw[14],
        relay: raw[15],
    })
}

/// The `Data` message's port and payload, if these bytes look like one.
fn data(plain: &[u8]) -> Option<(u32, Vec<u8>)> {
    let f = proto::fields(plain)?;
    let port = proto::get(&f, 1)?.u32()?;
    if port == 0 || port > 511 {
        return None;
    }
    let payload = proto::get(&f, 2).and_then(|v| v.bytes()).unwrap_or(&[]).to_vec();
    Some((port, payload))
}

fn content(port: u32, p: &[u8]) -> Content {
    let other = || Content::Other { port, port_name: port_name(port).into(), len: p.len() };
    let Some(f) = proto::fields(p) else {
        return if port == 1 { Content::Text { text: String::from_utf8_lossy(p).into_owned() } } else { other() };
    };
    match port {
        1 => Content::Text { text: String::from_utf8_lossy(p).into_owned() },
        3 => {
            let (Some(lat), Some(lon)) = (proto::get(&f, 1).and_then(|v| v.i32()), proto::get(&f, 2).and_then(|v| v.i32())) else {
                return other();
            };
            Content::Position {
                lat: lat as f64 * 1e-7,
                lon: lon as f64 * 1e-7,
                alt: proto::get(&f, 3).and_then(|v| v.i32()),
                sats: proto::get(&f, 19).and_then(|v| v.u32()),
            }
        }
        4 => {
            let s = |n| proto::get(&f, n).and_then(|v| v.string()).unwrap_or_default();
            Content::NodeInfo {
                id: s(1),
                long_name: s(2),
                short_name: s(3),
                hw: hw_name(proto::get(&f, 5).and_then(|v| v.u32()).unwrap_or(0)),
                role: role_name(proto::get(&f, 7).and_then(|v| v.u32()).unwrap_or(0)),
                licensed: proto::get(&f, 6).and_then(|v| v.u64()).unwrap_or(0) != 0,
            }
        }
        67 => {
            let sub = |n| proto::get(&f, n).and_then(|v| v.bytes()).and_then(proto::fields);
            let dev = sub(2);
            let env = sub(3);
            if dev.is_none() && env.is_none() {
                return other();
            }
            let d = |n| dev.as_ref().and_then(|f| proto::get(f, n));
            let e = |n| env.as_ref().and_then(|f| proto::get(f, n));
            Content::Telemetry {
                battery: d(1).and_then(|v| v.u32()),
                voltage: d(2).and_then(|v| v.f32()),
                channel_util: d(3).and_then(|v| v.f32()),
                air_util_tx: d(4).and_then(|v| v.f32()),
                uptime_s: d(5).and_then(|v| v.u32()),
                temperature: e(1).and_then(|v| v.f32()),
                humidity: e(2).and_then(|v| v.f32()),
                pressure: e(3).and_then(|v| v.f32()),
            }
        }
        _ => other(),
    }
}

/// Parse a received payload, decrypting it with whichever channel fits.
pub fn decode(raw: &[u8], channels: &[Channel]) -> Option<Packet> {
    let header = parse_header(raw)?;
    let body = &raw[16..];
    for ch in channels.iter().filter(|c| c.hash() == header.channel_hash) {
        let mut plain = body.to_vec();
        crypt(&ch.key, &header, &mut plain);
        if let Some((port, payload)) = data(&plain) {
            return Some(Packet {
                content: content(port, &payload),
                channel: Some(ch.name.clone()),
                private: ch.private,
                header,
            });
        }
    }
    let pki = header.channel_hash == 0 && header.to != BROADCAST;
    Some(Packet { header, channel: None, private: false, content: Content::Encrypted { pki } })
}

/// Build an on-air packet (simulator).
pub fn encode(h: &Header, ch: &Channel, port: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + payload.len() + 8);
    out.extend_from_slice(&h.to.to_le_bytes());
    out.extend_from_slice(&h.from.to_le_bytes());
    out.extend_from_slice(&h.id.to_le_bytes());
    out.push((h.hop_limit & 7) | (h.want_ack as u8) << 3 | (h.via_mqtt as u8) << 4 | (h.hop_start & 7) << 5);
    out.push(ch.hash());
    out.push(h.next_hop);
    out.push(h.relay);
    let mut data = Writer::default();
    data.varint(1, port as u64).bytes(2, payload);
    let mut body = data.0;
    crypt(&ch.key, h, &mut body);
    out.extend_from_slice(&body);
    out
}

/// Payloads for the simulator.
pub mod build {
    use super::Writer;

    pub fn position(lat: f64, lon: f64, alt: i32, time: u32) -> Vec<u8> {
        let mut w = Writer::default();
        w.fixed32(1, (lat * 1e7).round() as i32 as u32)
            .fixed32(2, (lon * 1e7).round() as i32 as u32)
            .varint(3, alt as u32 as u64)
            .fixed32(4, time)
            .varint(19, 9);
        w.0
    }

    pub fn user(id: &str, long: &str, short: &str, hw: u32, role: u32) -> Vec<u8> {
        let mut w = Writer::default();
        w.bytes(1, id.as_bytes()).bytes(2, long.as_bytes()).bytes(3, short.as_bytes()).varint(5, hw as u64);
        if role != 0 {
            w.varint(7, role as u64);
        }
        w.0
    }

    pub fn device_metrics(time: u32, battery: u32, voltage: f32, ch_util: f32, air: f32, uptime: u32) -> Vec<u8> {
        let mut m = Writer::default();
        m.varint(1, battery as u64).float(2, voltage).float(3, ch_util).float(4, air).varint(5, uptime as u64);
        let mut w = Writer::default();
        w.fixed32(1, time).bytes(2, &m.0);
        w.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn longfast() -> Channel {
        Channel { name: "LongFast".into(), key: DEFAULT_KEY.to_vec(), private: false }
    }

    fn header(to: u32) -> Header {
        Header {
            to,
            from: 0x1234_abcd,
            id: 0x0bad_f00d,
            hop_limit: 2,
            hop_start: 3,
            want_ack: false,
            via_mqtt: false,
            channel_hash: 0,
            next_hop: 0,
            relay: 0xcd,
        }
    }

    #[test]
    fn default_channel_hash() {
        assert_eq!(longfast().hash(), 8);
        assert_eq!(expand_psk(&[1]).unwrap(), DEFAULT_KEY.to_vec());
        assert_eq!(expand_psk(&[2]).unwrap()[15], 0x02);
        assert!(expand_psk(&[1, 2, 3]).is_err());
    }

    #[test]
    fn text_round_trip() {
        let raw = encode(&header(BROADCAST), &longfast(), 1, b"hello from the sim");
        // The text isn't readable on air.
        assert!(!raw.windows(5).any(|w| w == b"hello"));
        let p = decode(&raw, &[longfast()]).unwrap();
        assert_eq!(p.channel.as_deref(), Some("LongFast"));
        assert_eq!(p.header.hop_start - p.header.hop_limit, 1);
        assert_eq!(p.content, Content::Text { text: "hello from the sim".into() });
        // Without the key it stays encrypted.
        let p = decode(&raw, &[]).unwrap();
        assert_eq!(p.content, Content::Encrypted { pki: false });
    }

    #[test]
    fn position_nodeinfo_telemetry() {
        let ch = longfast();
        let raw = encode(&header(BROADCAST), &ch, 3, &build::position(51.5072, -0.1276, 35, 1_700_000_000));
        match decode(&raw, &[ch.clone()]).unwrap().content {
            Content::Position { lat, lon, alt, sats } => {
                assert!((lat - 51.5072).abs() < 1e-6 && (lon + 0.1276).abs() < 1e-6);
                assert_eq!((alt, sats), (Some(35), Some(9)));
            }
            c => panic!("{c:?}"),
        }
        let raw = encode(&header(BROADCAST), &ch, 4, &build::user("!1234abcd", "Sim Base", "SIMB", 43, 2));
        match decode(&raw, &[ch.clone()]).unwrap().content {
            Content::NodeInfo { long_name, short_name, hw, role, .. } => {
                assert_eq!((long_name.as_str(), short_name.as_str()), ("Sim Base", "SIMB"));
                assert_eq!((hw.as_str(), role.as_str()), ("Heltec V3", "router"));
            }
            c => panic!("{c:?}"),
        }
        let raw = encode(&header(BROADCAST), &ch, 67, &build::device_metrics(1, 87, 4.01, 12.5, 1.25, 3600));
        match decode(&raw, &[ch]).unwrap().content {
            Content::Telemetry { battery, voltage, uptime_s, .. } => {
                assert_eq!((battery, uptime_s), (Some(87), Some(3600)));
                assert!((voltage.unwrap() - 4.01).abs() < 1e-6);
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn pki_direct_message_is_marked() {
        let mut raw = encode(&header(0x5555_0001), &longfast(), 1, b"secret");
        raw[13] = 0;
        let p = decode(&raw, &[longfast()]).unwrap();
        assert_eq!(p.content, Content::Encrypted { pki: true });
    }

    #[test]
    fn aes256_channel() {
        let ch = Channel { name: "Club".into(), key: (0..32).collect(), private: true };
        let raw = encode(&header(BROADCAST), &ch, 1, b"members only");
        let p = decode(&raw, &[longfast(), ch]).unwrap();
        assert_eq!(p.channel.as_deref(), Some("Club"));
        assert!(p.private);
    }
}
