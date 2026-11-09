//! Mode S parity: a 24-bit CRC (generator 0xFFF409) over the message,
//! overlaid on the address in most downlink formats.

use std::collections::HashMap;
use std::sync::OnceLock;

const POLY: u32 = 0xFF_F409;

const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 16;
        let mut k = 0;
        while k < 8 {
            r = if r & 0x80_0000 != 0 { (r << 1) ^ POLY } else { r << 1 };
            k += 1;
        }
        t[i] = r & 0xFF_FFFF;
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = table();

/// CRC of `data` (the message without its last three bytes).
pub fn crc(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |c, &b| ((c << 8) ^ TABLE[(((c >> 16) as u8) ^ b) as usize]) & 0xFF_FFFF)
}

/// What's left after taking the CRC out of the parity field: 0 for a good
/// DF17/18, the interrogator id for DF11, the address for DF0/4/5/16/20/21.
pub fn residual(msg: &[u8]) -> u32 {
    let n = msg.len();
    let parity = (msg[n - 3] as u32) << 16 | (msg[n - 2] as u32) << 8 | msg[n - 1] as u32;
    crc(&msg[..n - 3]) ^ parity
}

/// Syndromes of single-bit errors in a 112-bit message → bit index. Bits 0–4
/// (the downlink format) are left out: "fixing" those turns noise into a
/// different kind of message.
fn syndromes() -> &'static HashMap<u32, usize> {
    static S: OnceLock<HashMap<u32, usize>> = OnceLock::new();
    S.get_or_init(|| {
        let mut m = HashMap::new();
        for bit in 5..112 {
            let mut msg = [0u8; 14];
            msg[bit / 8] |= 0x80 >> (bit % 8);
            m.insert(residual(&msg), bit);
        }
        m
    })
}

/// Fix one flipped bit in a long (DF17/18) message whose residual isn't 0.
/// Returns whether it was fixed.
pub fn fix_one_bit(msg: &mut [u8; 14]) -> bool {
    let r = residual(msg);
    if r == 0 {
        return true;
    }
    match syndromes().get(&r) {
        Some(&bit) => {
            msg[bit / 8] ^= 0x80 >> (bit % 8);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn known_messages_check() {
        assert_eq!(residual(&hex("8D4840D6202CC371C32CE0576098")), 0);
        assert_eq!(residual(&hex("8D40621D58C382D690C8AC2863A7")), 0);
        assert_ne!(residual(&hex("8D4840D6202CC371C32CE0576099")), 0);
    }

    #[test]
    fn fixes_a_flipped_bit() {
        let good: [u8; 14] = hex("8D4840D6202CC371C32CE0576098").try_into().unwrap();
        for bit in [5, 40, 87, 100, 111] {
            let mut m = good;
            m[bit / 8] ^= 0x80 >> (bit % 8);
            assert!(fix_one_bit(&mut m), "bit {bit}");
            assert_eq!(m, good);
        }
        let mut two = good;
        two[6] ^= 0x81;
        // Two flips usually alias to nothing (or to a wrong fix, caught later).
        let _ = fix_one_bit(&mut two);
    }
}
