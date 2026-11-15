//! Simulated Meshtastic and MeshCore nodes around London for the synthetic
//! receiver. One emitter per network sends the next packet of a fixed script
//! every few seconds, the two staggered so they never overlap on air (the
//! MeshCore channel sits inside Meshtastic's 250 kHz one).
//!
//! Most traffic is on the public channels. Two things need a key an admin
//! adds under Settings → Mesh, to show how private channels work:
//! - Meshtastic channel `SimClub`, PSK `b25haXItc2ltLWNsdWIhIQ==`
//! - MeshCore hashtag channel `#onair` (no key: it comes from the name)

use sha2::{Digest, Sha256};

use crate::mesh::{meshcore, meshtastic, PRESETS};

/// Seconds between packets on each network, and MeshCore's offset into it.
pub(super) const PERIOD: f64 = 12.0;
pub(super) const MESHCORE_OFFSET: f64 = 6.0;
/// Preset indexes (into `mesh::PRESETS`) the two emitters use.
pub(super) const MT_PRESET: usize = 0;
pub(super) const MC_PRESET: usize = 3;

pub(super) fn freq_hz(preset: usize) -> f64 {
    PRESETS[preset].freq_hz as f64
}

/// The PSK of the simulated private Meshtastic channel.
pub const SIM_CLUB_KEY: &[u8; 16] = b"onair-sim-club!!";

const BASE: u32 = 0x5131_ba5e;
const HIKER: u32 = 0x5131_41c3;
const WX: u32 = 0x5131_3e47;

fn mt_header(from: u32, to: u32, id: u32, hops: u8) -> meshtastic::Header {
    meshtastic::Header {
        to,
        from,
        id,
        hop_limit: 3 - hops,
        hop_start: 3,
        want_ack: false,
        via_mqtt: false,
        channel_hash: 0,
        next_hop: 0,
        relay: (from & 0xFF) as u8,
    }
}

/// The hiker walks a loop on Hampstead Heath, once an hour.
fn hiker_pos(wall: f64) -> (f64, f64) {
    let a = std::f64::consts::TAU * (wall / 3600.0).fract();
    (51.5608 + 0.004 * a.sin(), -0.1650 + 0.007 * a.cos())
}

/// The next Meshtastic packet and its amplitude (nearer nodes are louder).
pub(super) fn meshtastic(seq: u32, wall: f64) -> (Vec<u8>, f64) {
    let public = meshtastic::Channel { name: "LongFast".into(), key: meshtastic::DEFAULT_KEY.to_vec(), private: false };
    let club = meshtastic::Channel { name: "SimClub".into(), key: SIM_CLUB_KEY.to_vec(), private: true };
    let id = (wall as u32).wrapping_mul(2_654_435_761) ^ seq;
    let t = wall as u32;
    let bc = meshtastic::BROADCAST;
    use meshtastic::build;
    let (from, hops, ch, port, payload) = match seq % 12 {
        0 => (BASE, 0, &public, 4, build::user("!5131ba5e", "Sim Base", "SBAS", 43, 2)),
        1 => (BASE, 0, &public, 3, build::position(51.5007, -0.1246, 22, t)),
        2 => (HIKER, 1, &public, 1, b"Anyone on LongFast near Hampstead?".to_vec()),
        3 => (HIKER, 1, &public, 4, build::user("!513141c3", "Heath Hiker", "HIKE", 7, 0)),
        4 => {
            let (lat, lon) = hiker_pos(wall);
            (HIKER, 1, &public, 3, build::position(lat, lon, 120, t))
        }
        5 => (WX, 2, &public, 67, build::device_metrics(t, 101, 5.02, 8.5, 0.6, 86_400 + t % 86_400)),
        6 => (WX, 2, &public, 4, build::user("!51313e47", "Allotment WX", "AWX", 9, 6)),
        7 => (BASE, 0, &public, 1, b"Morning all, sim base station here".to_vec()),
        8 => (HIKER, 1, &club, 1, b"Meet at the usual place at 7".to_vec()),
        9 => {
            // A direct message under PKI: channel hash 0, unreadable.
            let mut raw = meshtastic::encode(&mt_header(BASE, HIKER, id, 0), &public, 1, b"see you there");
            raw[13] = 0;
            return (raw, 0.06);
        }
        10 => (BASE, 0, &public, 67, build::device_metrics(t, 87, 4.01, 11.0, 2.1, t % 604_800)),
        _ => (WX, 2, &public, 3, build::position(51.4710, -0.0310, 8, t)),
    };
    let amp = match from {
        BASE => 0.06,
        HIKER => 0.025,
        _ => 0.012,
    };
    let to = bc;
    (meshtastic::encode(&mt_header(from, to, id, hops), ch, port, &payload), amp)
}

fn mc_key(name: &str) -> [u8; 32] {
    Sha256::digest(name.as_bytes()).into()
}

/// The next MeshCore packet and its amplitude.
pub(super) fn meshcore(seq: u32, wall: f64) -> (Vec<u8>, f64) {
    let public = meshcore::Channel { name: "Public".into(), key: meshcore::PUBLIC_KEY, private: false };
    let tag = meshcore::Channel::hashtag("#onair");
    let (rpt, carol, dave) = (mc_key("sim-hill-rpt"), mc_key("sim-carol"), mc_key("sim-dave"));
    let t = wall as u32;
    match seq % 7 {
        0 => (meshcore::encode_advert(&rpt, t, 2, "Sim Hill RPT", Some((51.5869, -0.1000)), &[]), 0.05),
        1 => (meshcore::encode_advert(&carol, t, 1, "Carol", Some((51.5230, -0.0800)), &[rpt[0]]), 0.03),
        2 => (meshcore::encode_group_text(&public, t, "Carol", "Hello from the simulator", &[rpt[0]]), 0.03),
        3 => (meshcore::encode_advert(&dave, t, 1, "Dave", None, &[]), 0.02),
        4 => (meshcore::encode_group_text(&public, t, "Dave", "Testing 1 2 3", &[]), 0.02),
        5 => (meshcore::encode_direct(dave[0], carol[0], &[0x5a; 34], &[rpt[0]]), 0.03),
        _ => (meshcore::encode_group_text(&tag, t, "Carol", "Hashtag channel test", &[rpt[0]]), 0.03),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::MeshConfig;

    #[test]
    fn script_decodes() {
        let (mt, mc) = MeshConfig::default().channels().unwrap();
        for seq in 0..12 {
            let (raw, _) = meshtastic(seq, 1.7e9);
            let p = meshtastic::decode(&raw, &mt).unwrap();
            let readable = !matches!(p.content, meshtastic::Content::Encrypted { .. });
            assert_eq!(readable, seq != 8 && seq != 9, "seq {seq}: {p:?}");
        }
        for seq in 0..7 {
            let (raw, _) = meshcore(seq, 1.7e9);
            let p = meshcore::decode(&raw, &mc).unwrap();
            let readable = !matches!(p.content, meshcore::Content::Encrypted { .. });
            assert_eq!(readable, seq != 5 && seq != 6, "seq {seq}: {p:?}");
        }
        // With the keys an admin would add, the private ones open too.
        let cfg = MeshConfig {
            meshtastic_channels: vec![crate::mesh::ChannelKey { name: "SimClub".into(), key: "b25haXItc2ltLWNsdWIhIQ==".into() }],
            meshcore_channels: vec![crate::mesh::ChannelKey { name: "#onair".into(), key: String::new() }],
            ..MeshConfig::default()
        };
        let (mt, mc) = cfg.channels().unwrap();
        let p = meshtastic::decode(&meshtastic(8, 1.7e9).0, &mt).unwrap();
        assert_eq!(p.channel.as_deref(), Some("SimClub"));
        let p = meshcore::decode(&meshcore(6, 1.7e9).0, &mc).unwrap();
        assert_eq!(p.channel.as_deref(), Some("#onair"));
    }

    /// The simulated receiver on 869.575 MHz: both networks come through
    /// the real receive chain.
    #[test]
    fn simulator_is_heard() {
        let rate = 2_048_000;
        let mut gen = super::super::synthetic::SynthGen::new();
        let mut cu8 = Vec::new();
        for _ in 0..16 {
            cu8.extend(gen.generate(crate::mesh::TUNE_HZ, rate, Some(30.0), rate as usize));
        }
        let got = crate::mesh::decode_cu8(crate::mesh::TUNE_HZ, rate, &cu8);
        let ok = |p: usize| got.iter().filter(|(q, f)| *q == p && f.crc_ok).count();
        assert!(ok(MT_PRESET) >= 2 && ok(MC_PRESET) >= 1, "{got:?}");
        assert!(got.iter().all(|(_, f)| f.crc_ok), "{got:?}");
    }
}
