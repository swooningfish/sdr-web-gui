//! Just enough protobuf to read (and, for the simulator, write) the handful
//! of Meshtastic messages we show. Unknown fields are skipped.

pub enum Value<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

impl Value<'_> {
    pub fn u64(&self) -> Option<u64> {
        match *self {
            Value::Varint(v) | Value::Fixed64(v) => Some(v),
            Value::Fixed32(v) => Some(v as u64),
            Value::Bytes(_) => None,
        }
    }
    pub fn u32(&self) -> Option<u32> {
        self.u64().map(|v| v as u32)
    }
    pub fn i32(&self) -> Option<i32> {
        self.u64().map(|v| v as i32)
    }
    pub fn f32(&self) -> Option<f32> {
        match *self {
            Value::Fixed32(v) => Some(f32::from_bits(v)),
            _ => None,
        }
    }
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }
    pub fn string(&self) -> Option<String> {
        self.bytes().and_then(|b| std::str::from_utf8(b).ok()).map(str::to_string)
    }
}

fn varint(buf: &[u8], i: &mut usize) -> Option<u64> {
    let mut v: u64 = 0;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*i)?;
        *i += 1;
        v |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// All (field, value) pairs, or None if the bytes aren't valid protobuf.
pub fn fields(buf: &[u8]) -> Option<Vec<(u32, Value<'_>)>> {
    let mut i = 0;
    let mut out = Vec::new();
    while i < buf.len() {
        let key = varint(buf, &mut i)?;
        let field = (key >> 3) as u32;
        if field == 0 {
            return None;
        }
        let v = match key & 7 {
            0 => Value::Varint(varint(buf, &mut i)?),
            1 => {
                let b = buf.get(i..i + 8)?;
                i += 8;
                Value::Fixed64(u64::from_le_bytes(b.try_into().ok()?))
            }
            2 => {
                let len = varint(buf, &mut i)? as usize;
                let b = buf.get(i..i.checked_add(len)?)?;
                i += len;
                Value::Bytes(b)
            }
            5 => {
                let b = buf.get(i..i + 4)?;
                i += 4;
                Value::Fixed32(u32::from_le_bytes(b.try_into().ok()?))
            }
            _ => return None,
        };
        out.push((field, v));
    }
    Some(out)
}

/// The last value of `field` (protobuf's rule for repeated scalars).
pub fn get<'a, 'b>(f: &'b [(u32, Value<'a>)], field: u32) -> Option<&'b Value<'a>> {
    f.iter().rev().find(|(n, _)| *n == field).map(|(_, v)| v)
}

/// Writer for the simulator and tests.
#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    fn key(&mut self, field: u32, wire: u8) {
        self.raw_varint(((field as u64) << 3) | wire as u64);
    }
    fn raw_varint(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(b);
                return;
            }
            self.0.push(b | 0x80);
        }
    }
    pub fn varint(&mut self, field: u32, v: u64) -> &mut Self {
        self.key(field, 0);
        self.raw_varint(v);
        self
    }
    pub fn fixed32(&mut self, field: u32, v: u32) -> &mut Self {
        self.key(field, 5);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn float(&mut self, field: u32, v: f32) -> &mut Self {
        self.fixed32(field, v.to_bits())
    }
    pub fn bytes(&mut self, field: u32, v: &[u8]) -> &mut Self {
        self.key(field, 2);
        self.raw_varint(v.len() as u64);
        self.0.extend_from_slice(v);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut w = Writer::default();
        w.varint(1, 300).fixed32(2, 0xDEAD_BEEF).bytes(3, b"hi").float(4, 1.5).varint(5, (-5i32) as u32 as u64);
        let f = fields(&w.0).unwrap();
        assert_eq!(get(&f, 1).unwrap().u64(), Some(300));
        assert_eq!(get(&f, 2).unwrap().u32(), Some(0xDEAD_BEEF));
        assert_eq!(get(&f, 3).unwrap().string().as_deref(), Some("hi"));
        assert_eq!(get(&f, 4).unwrap().f32(), Some(1.5));
        assert_eq!(get(&f, 5).unwrap().i32(), Some(-5));
        assert!(fields(&[0x0A, 0x05, 1]).is_none());
    }
}
