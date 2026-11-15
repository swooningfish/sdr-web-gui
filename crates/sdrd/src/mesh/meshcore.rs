//! MeshCore packets: header byte (route, payload type, version), optional
//! transport codes, the path of repeaters so far, then the payload.
//!
//! Adverts are in the clear (public key, time, signature, then name, type
//! and position). Group text is AES-128-ECB with a 2-byte HMAC-SHA256 tag,
//! keyed by the channel secret; the channel is named by the first byte of the
//! SHA-256 of its key. Direct messages, requests and responses are encrypted
//! with keys shared between two nodes: only the 1-byte hashes of sender and
//! recipient are visible.

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// The default "Public" channel key.
pub const PUBLIC_KEY: [u8; 16] =
    [0x8b, 0x33, 0x87, 0xe9, 0xc5, 0xcd, 0xea, 0x6a, 0xc9, 0xe5, 0xed, 0xba, 0xa1, 0x15, 0xcd, 0x72];

#[derive(Clone, Debug)]
pub struct Channel {
    pub name: String,
    pub key: [u8; 16],
    pub private: bool,
}

impl Channel {
    pub fn hash(&self) -> u8 {
        Sha256::digest(self.key)[0]
    }

    /// A hashtag channel ("#name"): its key is derived from the name.
    pub fn hashtag(name: &str) -> Channel {
        let mut key = [0u8; 16];
        key.copy_from_slice(&Sha256::digest(name.as_bytes())[..16]);
        Channel { name: name.to_string(), key, private: false }
    }

    fn mac(&self, data: &[u8]) -> [u8; 2] {
        let mut secret = [0u8; 32];
        secret[..16].copy_from_slice(&self.key);
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(&secret).unwrap();
        m.update(data);
        let r = m.finalize().into_bytes();
        [r[0], r[1]]
    }
}

pub fn route_name(r: u8) -> &'static str {
    ["transport flood", "flood", "direct", "transport direct"][r as usize & 3]
}

pub fn type_name(t: u8) -> &'static str {
    match t {
        0 => "request",
        1 => "response",
        2 => "direct message",
        3 => "ack",
        4 => "advert",
        5 => "group text",
        6 => "group data",
        7 => "anonymous request",
        8 => "path",
        9 => "trace",
        10 => "multipart",
        15 => "raw",
        _ => "unknown",
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Advert {
        /// Public key, hex (the node's identity).
        key: String,
        timestamp: u32,
        node_type: String,
        name: Option<String>,
        lat: Option<f64>,
        lon: Option<f64>,
    },
    /// Group message from `sender` (as the sender signs it, "name: text").
    GroupText { sender: Option<String>, text: String, timestamp: u32 },
    /// Couldn't read it: a channel we have no key for, or a direct message.
    Encrypted { channel_hash: Option<u8>, src_hash: Option<u8>, dest_hash: Option<u8> },
    Ack { crc: u32 },
    Other { len: usize },
}

#[derive(Clone, Debug, Serialize)]
pub struct Packet {
    pub route: &'static str,
    pub payload_type: &'static str,
    pub version: u8,
    /// Hashes of the repeaters it came through, hex.
    pub path: Vec<String>,
    pub channel: Option<String>,
    pub private: bool,
    pub content: Content,
}

fn node_type(t: u8) -> &'static str {
    match t {
        1 => "companion",
        2 => "repeater",
        3 => "room server",
        4 => "sensor",
        _ => "unknown",
    }
}

fn advert(p: &[u8]) -> Option<Content> {
    if p.len() < 101 {
        return None;
    }
    let key = hex::encode(&p[..32]);
    let timestamp = u32::from_le_bytes(p[32..36].try_into().ok()?);
    let app = &p[100..];
    let flags = *app.first().unwrap_or(&0);
    let mut i = 1;
    let (mut lat, mut lon) = (None, None);
    if flags & 0x10 != 0 {
        let v = app.get(i..i + 8)?;
        lat = Some(i32::from_le_bytes(v[..4].try_into().ok()?) as f64 * 1e-6);
        lon = Some(i32::from_le_bytes(v[4..].try_into().ok()?) as f64 * 1e-6);
        i += 8;
    }
    if flags & 0x20 != 0 {
        i += 2;
    }
    if flags & 0x40 != 0 {
        i += 2;
    }
    let name = (flags & 0x80 != 0)
        .then(|| app.get(i..).map(|b| String::from_utf8_lossy(b).trim_end_matches('\0').to_string()))
        .flatten();
    Some(Content::Advert { key, timestamp, node_type: node_type(flags & 0x0F).into(), name, lat, lon })
}

fn group_text<'c>(p: &[u8], channels: &'c [Channel]) -> (Option<&'c Channel>, Content) {
    let encrypted = || Content::Encrypted { channel_hash: p.first().copied(), src_hash: None, dest_hash: None };
    if p.len() < 3 + 16 || (p.len() - 3) % 16 != 0 {
        return (None, encrypted());
    }
    let (hash, mac, ct) = (p[0], [p[1], p[2]], &p[3..]);
    for ch in channels.iter().filter(|c| c.hash() == hash) {
        if ch.mac(ct) != mac {
            continue;
        }
        let aes = aes::Aes128::new_from_slice(&ch.key).unwrap();
        let mut plain = ct.to_vec();
        for block in plain.chunks_exact_mut(16) {
            aes.decrypt_block(block.into());
        }
        let timestamp = u32::from_le_bytes(plain[..4].try_into().unwrap());
        let body = &plain[5..];
        let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
        let text = String::from_utf8_lossy(&body[..end]).into_owned();
        let (sender, text) = match text.split_once(": ") {
            Some((s, t)) => (Some(s.to_string()), t.to_string()),
            None => (None, text),
        };
        return (Some(ch), Content::GroupText { sender, text, timestamp });
    }
    (None, encrypted())
}

pub fn decode(raw: &[u8], channels: &[Channel]) -> Option<Packet> {
    let hdr = *raw.first()?;
    let route = hdr & 3;
    let ptype = (hdr >> 2) & 0x0F;
    let version = hdr >> 6;
    let mut i = 1;
    if route == 0 || route == 3 {
        i += 4;
    }
    let path_len = *raw.get(i)?;
    i += 1;
    // Newer firmware can use 2–4 byte hashes, signalled in the top two bits.
    let (size, count) = if path_len > 64 { ((path_len >> 6) as usize + 1, (path_len & 63) as usize) } else { (1, path_len as usize) };
    let path_bytes = raw.get(i..i + size * count)?;
    i += size * count;
    let path = path_bytes.chunks(size).map(hex::encode).collect();
    let p = raw.get(i..)?;
    let mut channel = None;
    let mut private = false;
    let content = match ptype {
        4 => advert(p).unwrap_or(Content::Other { len: p.len() }),
        5 => {
            let (ch, c) = group_text(p, channels);
            if let Some(ch) = ch {
                channel = Some(ch.name.clone());
                private = ch.private;
            }
            c
        }
        6 => Content::Encrypted { channel_hash: p.first().copied(), src_hash: None, dest_hash: None },
        0 | 1 | 2 | 8 => Content::Encrypted { channel_hash: None, dest_hash: p.first().copied(), src_hash: p.get(1).copied() },
        7 => Content::Encrypted { channel_hash: None, dest_hash: p.first().copied(), src_hash: None },
        3 if p.len() >= 4 => Content::Ack { crc: u32::from_le_bytes(p[..4].try_into().unwrap()) },
        _ => Content::Other { len: p.len() },
    };
    Some(Packet {
        route: route_name(route),
        payload_type: type_name(ptype),
        version,
        path,
        channel,
        private,
        content,
    })
}

fn header(route: u8, ptype: u8, path: &[u8]) -> Vec<u8> {
    let mut out = vec![(route & 3) | (ptype & 0x0F) << 2];
    out.push(path.len() as u8);
    out.extend_from_slice(path);
    out
}

/// A flood advert (simulator). The signature is random-looking filler: we
/// don't verify signatures, and the simulator has no private keys.
pub fn encode_advert(key: &[u8; 32], timestamp: u32, kind: u8, name: &str, pos: Option<(f64, f64)>, path: &[u8]) -> Vec<u8> {
    let mut out = header(1, 4, path);
    out.extend_from_slice(key);
    out.extend_from_slice(&timestamp.to_le_bytes());
    let sig = Sha256::digest([&key[..], &timestamp.to_le_bytes()].concat());
    out.extend_from_slice(&sig);
    out.extend_from_slice(&sig);
    let mut flags = kind | 0x80;
    if pos.is_some() {
        flags |= 0x10;
    }
    out.push(flags);
    if let Some((lat, lon)) = pos {
        out.extend_from_slice(&((lat * 1e6).round() as i32).to_le_bytes());
        out.extend_from_slice(&((lon * 1e6).round() as i32).to_le_bytes());
    }
    out.extend_from_slice(name.as_bytes());
    out
}

/// A flood group message "sender: text" (simulator).
pub fn encode_group_text(ch: &Channel, timestamp: u32, sender: &str, text: &str, path: &[u8]) -> Vec<u8> {
    let mut plain = timestamp.to_le_bytes().to_vec();
    plain.push(0);
    plain.extend_from_slice(format!("{sender}: {text}").as_bytes());
    while plain.len() % 16 != 0 {
        plain.push(0);
    }
    let aes = aes::Aes128::new_from_slice(&ch.key).unwrap();
    for block in plain.chunks_exact_mut(16) {
        aes.encrypt_block(block.into());
    }
    let mut out = header(1, 5, path);
    out.push(ch.hash());
    out.extend_from_slice(&ch.mac(&plain));
    out.extend_from_slice(&plain);
    out
}

/// A direct message's outer layer, with an opaque body (simulator).
pub fn encode_direct(dest: u8, src: u8, body: &[u8], path: &[u8]) -> Vec<u8> {
    let mut out = header(2, 2, path);
    out.push(dest);
    out.push(src);
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public() -> Channel {
        Channel { name: "Public".into(), key: PUBLIC_KEY, private: false }
    }

    #[test]
    fn advert_round_trip() {
        let key = [7u8; 32];
        let raw = encode_advert(&key, 1_700_000_000, 2, "Hilltop RPT", Some((51.75, -1.25)), &[0xab]);
        let p = decode(&raw, &[]).unwrap();
        assert_eq!(p.payload_type, "advert");
        assert_eq!(p.route, "flood");
        assert_eq!(p.path, vec!["ab".to_string()]);
        match p.content {
            Content::Advert { key: k, node_type, name, lat, lon, .. } => {
                assert_eq!(k, hex::encode(key));
                assert_eq!(node_type, "repeater");
                assert_eq!(name.as_deref(), Some("Hilltop RPT"));
                assert!((lat.unwrap() - 51.75).abs() < 1e-6 && (lon.unwrap() + 1.25).abs() < 1e-6);
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn group_text_round_trip() {
        let raw = encode_group_text(&public(), 1_700_000_123, "Alice", "Morning all", &[]);
        let p = decode(&raw, &[public()]).unwrap();
        assert_eq!(p.channel.as_deref(), Some("Public"));
        assert_eq!(
            p.content,
            Content::GroupText { sender: Some("Alice".into()), text: "Morning all".into(), timestamp: 1_700_000_123 }
        );
        // A different channel with the same hash byte fails the MAC check.
        let p = decode(&raw, &[]).unwrap();
        assert!(matches!(p.content, Content::Encrypted { .. }));
        let tag = Channel::hashtag("#uk");
        let raw = encode_group_text(&tag, 1, "Bob", "hi", &[1, 2]);
        let p = decode(&raw, &[public(), tag]).unwrap();
        assert_eq!(p.channel.as_deref(), Some("#uk"));
        assert_eq!(p.path.len(), 2);
    }

    #[test]
    fn direct_message_shows_hashes() {
        let raw = encode_direct(0x12, 0x34, &[0u8; 18], &[]);
        let p = decode(&raw, &[public()]).unwrap();
        assert_eq!(p.payload_type, "direct message");
        assert_eq!(p.content, Content::Encrypted { channel_hash: None, dest_hash: Some(0x12), src_hash: Some(0x34) });
    }
}
