//! Finding public channels nobody added.
//!
//! MeshCore hashtag channels take their key from the name, so a packet on an
//! unknown channel can be tried against a list of likely names: the built-in
//! list below plus every `#tag` seen in a message we could read. The 1-byte
//! channel hash narrows the names to try and the 2-byte MAC confirms a hit.
//!
//! A Meshtastic channel's name only feeds its 1-byte hash, not its key, so a
//! renamed channel still on the default key (or one of the simple "AQ==" to
//! "Cg==" keys) can be read without knowing the name. The name is a guess
//! from a short list when exactly one fits the hash.

use std::collections::{HashMap, HashSet};

use base64::Engine;
use serde::{Deserialize, Serialize};

use super::{meshcore, meshtastic, Proto};

/// Hashtag channels worth trying (UK-leaning: the presets are UK/EU).
const HASHTAGS: &[&str] = &[
    // General
    "test", "testing", "ping", "pong", "echo", "bot", "bots", "chat", "general", "random", "public", "mesh",
    "meshcore", "meshtastic", "lora", "help", "sos", "emergency", "news", "weather", "wx", "alerts", "local",
    "repeater", "repeaters", "rpt", "infra", "admin", "ops", "dev", "debug", "hello", "hi", "welcome",
    "offgrid", "prepper", "prepping", "ham", "radio", "amateur", "hamradio", "sdr", "hiking", "outdoors",
    "cycling", "sailing", "boating", "events", "social", "chill", "nerd", "tech", "games", "gaming", "music",
    "night", "morning", "traffic", "travel", "trains", "planes", "aviation", "space", "satellite", "aprs",
    "sensors", "telemetry", "power", "solar", "outage", "flood", "storm", "fire", "police", "medical",
    // Places
    "uk", "gb", "england", "scotland", "wales", "cymru", "ni", "northernireland", "ireland", "eu", "europe",
    "london", "manchester", "birmingham", "leeds", "liverpool", "sheffield", "bristol", "newcastle",
    "nottingham", "leicester", "coventry", "bradford", "hull", "stoke", "derby", "southampton", "portsmouth",
    "plymouth", "exeter", "cardiff", "swansea", "newport", "glasgow", "edinburgh", "aberdeen", "dundee",
    "inverness", "belfast", "derry", "york", "oxford", "cambridge", "brighton", "reading", "norwich",
    "ipswich", "peterborough", "milton", "miltonkeynes", "mk", "luton", "watford", "slough", "swindon",
    "bath", "gloucester", "cheltenham", "worcester", "hereford", "shrewsbury", "chester", "lancaster",
    "preston", "blackpool", "bolton", "wigan", "warrington", "stockport", "salford", "huddersfield",
    "wakefield", "doncaster", "rotherham", "barnsley", "lincoln", "grimsby", "sunderland", "durham",
    "middlesbrough", "carlisle", "kendal", "canterbury", "maidstone", "dover", "guildford", "woking",
    "crawley", "chelmsford", "colchester", "southend", "bournemouth", "poole", "salisbury", "winchester",
    "basingstoke", "truro", "taunton", "yeovil", "northampton", "bedford", "stalbans", "hertford",
    "kent", "essex", "surrey", "sussex", "hampshire", "dorset", "devon", "cornwall", "somerset", "wiltshire",
    "berkshire", "bucks", "buckinghamshire", "oxfordshire", "herts", "hertfordshire", "beds", "bedfordshire",
    "cambs", "cambridgeshire", "norfolk", "suffolk", "lincolnshire", "yorkshire", "lancashire", "cumbria",
    "northumberland", "cheshire", "derbyshire", "staffordshire", "shropshire", "warwickshire",
    "leicestershire", "nottinghamshire", "gloucestershire", "worcestershire", "herefordshire",
    "northamptonshire", "rutland", "midlands", "westmidlands", "eastmidlands", "northwest", "northeast",
    "southwest", "southeast", "eastanglia", "thenorth", "highlands", "lakes", "lakedistrict", "peaks",
    "cotswolds", "fens", "isleofwight", "iow", "jersey", "guernsey", "isleofman",
];

/// Meshtastic channel names to try when naming a default-key channel.
const MT_NAMES: &[&str] = &[
    "Test", "test", "Testing", "Admin", "admin", "Emergency", "SOS", "Help", "UK", "Local", "Chat", "General",
    "Public", "Mesh", "Family", "Friends", "Weather", "Ham", "Radio", "Hiking", "Events", "Club", "Group",
    "News", "Backup", "Secondary", "Private", "Gaming", "Repeater", "Infra", "Ops", "Sensors", "Telemetry",
];

/// Most `#tags` learned from messages (and tried as names).
const MAX_LEARNED: usize = 500;
/// Most channels kept in the found list.
const MAX_FOUND: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Found {
    pub proto: Proto,
    /// "#test", a guessed Meshtastic name, or "Unknown 0x1f".
    pub name: String,
    /// Meshtastic: the PSK as the apps show it ("AQ=="). Empty for MeshCore.
    #[serde(default)]
    pub key: String,
    pub hash: u8,
    /// The name is right (MeshCore hashtags; Meshtastic names that fit the hash).
    pub named: bool,
    pub first_ms: i64,
    pub last_ms: i64,
    pub packets: u64,
}

/// What's saved between restarts.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default)]
    pub found: Vec<Found>,
    #[serde(default)]
    pub learned: Vec<String>,
}

pub struct Discovery {
    /// Hashtag channels to try, by channel hash.
    mc: HashMap<u8, Vec<meshcore::Channel>>,
    tried: HashSet<String>,
    learned: Vec<String>,
    /// The simple Meshtastic keys (PSK index 1–10), with their base64.
    mt_keys: Vec<(Vec<u8>, String)>,
    pub found: Vec<Found>,
    pub dirty: bool,
}

impl Default for Discovery {
    fn default() -> Self {
        Discovery::new(Saved::default())
    }
}

impl Discovery {
    pub fn new(saved: Saved) -> Self {
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut d = Discovery {
            mc: HashMap::new(),
            tried: HashSet::new(),
            learned: Vec::new(),
            mt_keys: (1u8..=10).map(|i| (meshtastic::expand_psk(&[i]).unwrap(), b64.encode([i]))).collect(),
            found: saved.found,
            dirty: false,
        };
        for t in HASHTAGS {
            d.add_tag(&format!("#{t}"));
        }
        for t in saved.learned {
            if d.add_tag(&t) {
                d.learned.push(t);
            }
        }
        d.dirty = false;
        d
    }

    pub fn saved(&self) -> Saved {
        Saved { found: self.found.clone(), learned: self.learned.clone() }
    }

    fn add_tag(&mut self, tag: &str) -> bool {
        if !self.tried.insert(tag.to_string()) {
            return false;
        }
        let ch = meshcore::Channel::hashtag(tag);
        self.mc.entry(ch.hash()).or_default().push(ch);
        true
    }

    /// Remember the `#tags` in a message as channel names to try.
    pub fn learn(&mut self, text: &str) {
        for tag in hashtags_in(text) {
            for t in [tag.clone(), tag.to_lowercase()] {
                if self.learned.len() < MAX_LEARNED && self.add_tag(&t) {
                    self.learned.push(t);
                    self.dirty = true;
                }
            }
        }
    }

    /// A MeshCore group message no configured channel opened: try the
    /// hashtag names with its channel hash.
    pub fn meshcore(&mut self, raw: &[u8], hash: u8, now: i64) -> Option<meshcore::Packet> {
        let cands = self.mc.get(&hash)?;
        let pk = meshcore::decode(raw, cands)?;
        match &pk.content {
            meshcore::Content::GroupText { text, sender, .. }
                if !text.contains('\u{FFFD}') && !sender.as_deref().unwrap_or("").contains('\u{FFFD}') => {}
            _ => return None,
        }
        let name = pk.channel.clone()?;
        self.record(Proto::Meshcore, &name, "", hash, true, now);
        Some(pk)
    }

    /// A Meshtastic packet on a channel we have no key for: try the simple
    /// keys. `known` are the configured channels' names (not guessed again).
    pub fn meshtastic(&mut self, raw: &[u8], hash: u8, now: i64) -> Option<meshtastic::Packet> {
        let x = |b: &[u8]| b.iter().fold(0u8, |a, v| a ^ v);
        for (key, b64) in &self.mt_keys {
            let fits: Vec<&str> = MT_NAMES.iter().copied().filter(|n| x(n.as_bytes()) ^ x(key) == hash).collect();
            let (name, named) = match fits.as_slice() {
                [one] => (one.to_string(), true),
                _ => (format!("Unknown {hash:#04x}"), false),
            };
            let ch = meshtastic::Channel { name: name.clone(), key: key.clone(), private: false };
            if let Some(pk) = meshtastic::decode_as(raw, &ch) {
                let b64 = b64.clone();
                self.record(Proto::Meshtastic, &name, &b64, hash, named, now);
                return Some(pk);
            }
        }
        None
    }

    fn record(&mut self, proto: Proto, name: &str, key: &str, hash: u8, named: bool, now: i64) {
        self.dirty = true;
        if let Some(f) = self.found.iter_mut().find(|f| f.proto == proto && f.name == name && f.key == key) {
            f.last_ms = now;
            f.packets += 1;
            return;
        }
        if self.found.len() >= MAX_FOUND {
            // Drop the one heard least recently.
            if let Some(i) = self.found.iter().enumerate().min_by_key(|(_, f)| f.last_ms).map(|(i, _)| i) {
                self.found.remove(i);
            }
        }
        tracing::info!(proto = proto.as_str(), name, "mesh channel discovered");
        self.found.push(Found { proto, name: name.into(), key: key.into(), hash, named, first_ms: now, last_ms: now, packets: 1 });
    }

    pub fn forget(&mut self, proto: Proto, name: &str) {
        self.found.retain(|f| !(f.proto == proto && f.name == name));
        self.dirty = true;
    }
}

/// `#tags` in some text: `#` then 1–31 letters, digits, `_` or `-`.
fn hashtags_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let starts = b[i] == '#' && (i == 0 || !b[i - 1].is_alphanumeric());
        if starts {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == '_' || b[j] == '-') {
                j += 1;
            }
            if (2..=32).contains(&(j - i)) {
                out.push(b[i..j].iter().collect());
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tags_in_text() {
        assert_eq!(hashtags_in("try #Test and #uk-south, not a#b or #"), vec!["#Test", "#uk-south"]);
    }

    #[test]
    fn meshcore_hashtag_found() {
        let mut d = Discovery::default();
        let ch = meshcore::Channel::hashtag("#test");
        let raw = meshcore::encode_group_text(&ch, 1, "Ann", "anyone about?", &[]);
        let pk = d.meshcore(&raw, ch.hash(), 10).unwrap();
        assert_eq!(pk.channel.as_deref(), Some("#test"));
        assert!(!pk.private);
        assert_eq!(d.found.len(), 1);
        assert!(d.found[0].named);
        // An unlisted tag is found once a message mentions it.
        let odd = meshcore::Channel::hashtag("#zzq-club");
        let raw = meshcore::encode_group_text(&odd, 1, "Ann", "hi", &[]);
        assert!(d.meshcore(&raw, odd.hash(), 11).is_none());
        d.learn("come over to #zzq-club");
        assert_eq!(d.meshcore(&raw, odd.hash(), 12).unwrap().channel.as_deref(), Some("#zzq-club"));
        // Learned tags survive a restart.
        let d2 = Discovery::new(d.saved());
        assert_eq!(d2.found.len(), 2);
        assert!(d2.tried.contains("#zzq-club"));
    }

    #[test]
    fn meshtastic_default_key_found() {
        let mut d = Discovery::default();
        let ch = meshtastic::Channel { name: "Admin".into(), key: meshtastic::DEFAULT_KEY.to_vec(), private: false };
        let hdr = meshtastic::Header {
            to: meshtastic::BROADCAST,
            from: 7,
            id: 9,
            hop_limit: 3,
            hop_start: 3,
            want_ack: false,
            via_mqtt: false,
            channel_hash: ch.hash(),
            next_hop: 0,
            relay: 0,
        };
        let raw = meshtastic::encode(&hdr, &ch, 1, b"secret meeting");
        let pk = d.meshtastic(&raw, ch.hash(), 5).unwrap();
        assert!(matches!(pk.content, meshtastic::Content::Text { ref text } if text == "secret meeting"));
        assert_eq!(d.found[0].key, "AQ==");
        // A real private key isn't found.
        let other = meshtastic::Channel { name: "Club".into(), key: vec![0x5a; 16], private: true };
        let raw = meshtastic::encode(&hdr, &other, 1, b"hidden");
        assert!(d.meshtastic(&raw, other.hash(), 6).is_none());
    }
}
