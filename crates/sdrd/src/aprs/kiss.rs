//! KISS framing as spoken by Direwolf's TCP port.

const FEND: u8 = 0xc0;
const FESC: u8 = 0xdb;
const TFEND: u8 = 0xdc;
const TFESC: u8 = 0xdd;

#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    esc: bool,
    in_frame: bool,
}

impl Decoder {
    /// Feed bytes; returns the payloads of complete data frames (any port).
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for &b in bytes {
            match b {
                FEND => {
                    if self.in_frame && self.buf.len() > 1 && self.buf[0] & 0x0f == 0 {
                        out.push(self.buf[1..].to_vec());
                    }
                    self.buf.clear();
                    self.esc = false;
                    self.in_frame = true;
                }
                _ if !self.in_frame => {}
                FESC => self.esc = true,
                _ => {
                    let v = match (self.esc, b) {
                        (true, TFEND) => FEND,
                        (true, TFESC) => FESC,
                        _ => b,
                    };
                    self.esc = false;
                    if self.buf.len() < 1024 {
                        self.buf.push(v);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_and_splits() {
        let mut d = Decoder::default();
        let mut frames = d.push(&[0x01, FEND, 0x00, 1, FESC, TFEND, 2]);
        assert!(frames.is_empty());
        frames = d.push(&[FESC, TFESC, FEND, FEND, 0x00, 9, FEND]);
        assert_eq!(frames, vec![vec![1, FEND, 2, FESC], vec![9]]);
        // Non-data command frames are ignored.
        assert!(d.push(&[0x06, 5, FEND]).is_empty());
    }
}
