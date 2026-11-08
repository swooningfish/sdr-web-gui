//! AX.25 UI frames: decode (from Direwolf's KISS output) and encode (for the
//! synthetic test source).

/// A decoded UI frame. `path` holds digipeaters with their "has been
/// repeated" bit.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub src: String,
    pub dst: String,
    pub path: Vec<(String, bool)>,
    pub info: Vec<u8>,
}

fn decode_addr(b: &[u8]) -> Option<(String, bool)> {
    let mut call = String::with_capacity(9);
    for &c in &b[..6] {
        let ch = (c >> 1) as char;
        if ch == ' ' {
            continue;
        }
        if !ch.is_ascii_alphanumeric() {
            return None;
        }
        call.push(ch);
    }
    if call.is_empty() {
        return None;
    }
    let ssid = (b[6] >> 1) & 0x0f;
    if ssid != 0 {
        call.push_str(&format!("-{ssid}"));
    }
    Some((call, b[6] & 0x80 != 0))
}

/// Decode a frame without its FCS (as delivered over KISS). Only UI frames
/// with "no layer 3" PID are accepted, which is what APRS uses.
pub fn decode(b: &[u8]) -> Option<Frame> {
    let mut addrs = Vec::new();
    let mut i = 0;
    loop {
        let a = b.get(i..i + 7)?;
        addrs.push(decode_addr(a)?);
        i += 7;
        if a[6] & 1 == 1 {
            break;
        }
        if addrs.len() == 10 {
            return None;
        }
    }
    if addrs.len() < 2 || b.get(i..i + 2)? != [0x03, 0xf0] {
        return None;
    }
    let mut it = addrs.into_iter();
    let (dst, _) = it.next()?;
    let (src, _) = it.next()?;
    Some(Frame {
        src,
        dst,
        path: it.collect(),
        info: b[i + 2..].to_vec(),
    })
}

fn encode_addr(call: &str, last: bool, repeated: bool) -> [u8; 7] {
    let (c, ssid) = call.split_once('-').unwrap_or((call, "0"));
    let mut out = [b' ' << 1; 7];
    for (o, ch) in out.iter_mut().zip(c.bytes().take(6)) {
        *o = ch.to_ascii_uppercase() << 1;
    }
    out[6] = 0x60 | ((ssid.parse::<u8>().unwrap_or(0) & 0x0f) << 1) | u8::from(last) | if repeated { 0x80 } else { 0 };
    out
}

/// Encode a UI frame (without FCS).
pub fn encode(src: &str, dst: &str, path: &[&str], info: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + 7 * path.len() + info.len());
    out.extend_from_slice(&encode_addr(dst, false, false));
    out.extend_from_slice(&encode_addr(src, path.is_empty(), false));
    for (i, p) in path.iter().enumerate() {
        out.extend_from_slice(&encode_addr(p, i == path.len() - 1, false));
    }
    out.extend_from_slice(&[0x03, 0xf0]);
    out.extend_from_slice(info);
    out
}

/// AX.25 frame check sequence (CRC-16/X.25), sent low byte first.
pub fn fcs(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xffff;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    !crc
}

/// Printable form of the info field: control bytes shown as `<0xNN>`.
pub fn printable(info: &[u8]) -> String {
    let mut s = String::with_capacity(info.len());
    for &b in info {
        if (0x20..0x7f).contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("<0x{b:02x}>"));
        }
    }
    s
}

impl Frame {
    /// TNC2 monitor format: `SRC>DST,DIGI1*,DIGI2:info`.
    pub fn tnc2(&self) -> String {
        let mut s = format!("{}>{}", self.src, self.dst);
        let last_used = self.path.iter().rposition(|(_, h)| *h);
        for (i, (call, _)) in self.path.iter().enumerate() {
            s.push(',');
            s.push_str(call);
            if Some(i) == last_used {
                s.push('*');
            }
        }
        s.push(':');
        s.push_str(&printable(&self.info));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let raw = encode("M0ABC-9", "APRS", &["WIDE1-1", "WIDE2-1"], b"!5130.00N/00007.50W>hi");
        let f = decode(&raw).unwrap();
        assert_eq!(f.src, "M0ABC-9");
        assert_eq!(f.dst, "APRS");
        assert_eq!(f.path, vec![("WIDE1-1".into(), false), ("WIDE2-1".into(), false)]);
        assert_eq!(f.tnc2(), "M0ABC-9>APRS,WIDE1-1,WIDE2-1:!5130.00N/00007.50W>hi");
    }

    #[test]
    fn repeated_digi_marked() {
        let mut raw = encode("G4XYZ", "APDW16", &["MB7UK", "WIDE2-1"], b">status");
        raw[7 * 2 + 6] |= 0x80; // MB7UK has repeated it
        assert_eq!(decode(&raw).unwrap().tnc2(), "G4XYZ>APDW16,MB7UK*,WIDE2-1:>status");
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(&[0x82, 0xa0]).is_none());
        let mut raw = encode("M0ABC", "APRS", &[], b"x");
        raw[14] = 0x13; // not a UI frame
        assert!(decode(&raw).is_none());
    }

    #[test]
    fn fcs_check_value() {
        // CRC-16/X.25 check value for "123456789".
        assert_eq!(fcs(b"123456789"), 0x906e);
    }
}
