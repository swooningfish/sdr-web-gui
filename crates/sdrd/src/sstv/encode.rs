//! SSTV encoder: turns an RGB image into a list of tones. Used by the
//! synthetic source (so the decoder can be exercised over "RF") and by tests.

use super::modes::{rgb_to_yuv, value_to_freq, Color, Mode};

/// (frequency Hz, duration s)
pub type Tone = (f32, f64);

const BLACK: f32 = 1500.0;
const SYNC: f32 = 1200.0;

/// Calibration header and VIS code for a mode.
fn header(out: &mut Vec<Tone>, vis: u8) {
    out.push((1900.0, 0.300));
    out.push((SYNC, 0.010));
    out.push((1900.0, 0.300));
    out.push((SYNC, 0.030)); // start bit
    let mut ones = 0;
    for i in 0..7 {
        let bit = (vis >> i) & 1;
        ones += bit;
        out.push((if bit == 1 { 1100.0 } else { 1300.0 }, 0.030));
    }
    out.push((if ones % 2 == 1 { 1100.0 } else { 1300.0 }, 0.030)); // even parity
    out.push((SYNC, 0.030)); // stop bit
}

fn scan(out: &mut Vec<Tone>, values: impl Iterator<Item = u8>, ms: f64, width: usize) {
    let px = ms / 1000.0 / width as f64;
    out.extend(values.map(|v| (value_to_freq(v), px)));
}

/// Tones for a complete transmission of `rgb` (width × height × 3) in `mode`.
pub fn tones(mode: &Mode, rgb: &[u8]) -> Vec<Tone> {
    let (w, h) = (mode.width as usize, mode.height as usize);
    assert_eq!(rgb.len(), w * h * 3);
    let px = |x: usize, y: usize| {
        let i = (y.min(h - 1) * w + x) * 3;
        (rgb[i], rgb[i + 1], rgb[i + 2])
    };
    let yuv = |x: usize, y: usize| {
        let (r, g, b) = px(x, y);
        rgb_to_yuv(r, g, b)
    };
    let mut out = Vec::with_capacity(h * w * 3 + 64);
    header(&mut out, mode.vis);
    let s = |i: usize| mode.scans[i].1;

    match mode.color {
        Color::Gbr if mode.sync_ms > 5.0 => {
            // Scottie: starting sync, then sep G sep B sync porch R.
            out.push((SYNC, 0.009));
            for y in 0..h {
                out.push((BLACK, 0.0015));
                scan(&mut out, (0..w).map(|x| px(x, y).1), s(0), w);
                out.push((BLACK, 0.0015));
                scan(&mut out, (0..w).map(|x| px(x, y).2), s(1), w);
                out.push((SYNC, 0.009));
                out.push((BLACK, 0.0015));
                scan(&mut out, (0..w).map(|x| px(x, y).0), s(2), w);
            }
        }
        Color::Gbr => {
            // Martin: sync porch G sep B sep R sep.
            for y in 0..h {
                out.push((SYNC, mode.sync_ms / 1000.0));
                out.push((BLACK, 0.000572));
                for (ch, i) in [(1, 0), (2, 1), (0, 2)] {
                    scan(
                        &mut out,
                        (0..w).map(|x| {
                            let p = px(x, y);
                            [p.0, p.1, p.2][ch]
                        }),
                        s(i),
                        w,
                    );
                    out.push((BLACK, 0.000572));
                }
            }
        }
        Color::Robot36 => {
            for y in 0..h {
                let even = y % 2 == 0;
                out.push((SYNC, 0.009));
                out.push((BLACK, 0.003));
                scan(&mut out, (0..w).map(|x| yuv(x, y).0), s(0), w);
                out.push((if even { BLACK } else { 2300.0 }, 0.0045));
                out.push((1900.0, 0.0015));
                // Chroma averaged over the line pair it's shared by.
                let pair = y & !1;
                scan(
                    &mut out,
                    (0..w).map(|x| {
                        let (a, b) = (yuv(x, pair), yuv(x, pair + 1));
                        let (c1, c2) = if even { (a.2, b.2) } else { (a.1, b.1) };
                        ((c1 as u16 + c2 as u16) / 2) as u8
                    }),
                    s(1),
                    w,
                );
            }
        }
        Color::Yuv => {
            for y in 0..h {
                out.push((SYNC, 0.009));
                out.push((BLACK, 0.003));
                scan(&mut out, (0..w).map(|x| yuv(x, y).0), s(0), w);
                out.push((BLACK, 0.0045));
                out.push((1900.0, 0.0015));
                scan(&mut out, (0..w).map(|x| yuv(x, y).2), s(1), w);
                out.push((2300.0, 0.0045));
                out.push((BLACK, 0.0015));
                scan(&mut out, (0..w).map(|x| yuv(x, y).1), s(2), w);
            }
        }
        Color::Pd => {
            for y in (0..h).step_by(2) {
                out.push((SYNC, 0.020));
                out.push((BLACK, 0.00208));
                let avg = |x: usize, pick: fn((u8, u8, u8)) -> u8| {
                    ((pick(yuv(x, y)) as u16 + pick(yuv(x, y + 1)) as u16) / 2) as u8
                };
                scan(&mut out, (0..w).map(|x| yuv(x, y).0), s(0), w);
                scan(&mut out, (0..w).map(|x| avg(x, |c| c.2)), s(1), w);
                scan(&mut out, (0..w).map(|x| avg(x, |c| c.1)), s(2), w);
                scan(&mut out, (0..w).map(|x| yuv(x, y + 1).0), s(3), w);
            }
        }
    }
    out
}

/// Render tones as audio (phase-continuous sine, exact cumulative timing).
#[cfg(test)]
pub fn synth(tones: &[Tone], rate: f64, amp: f32) -> Vec<f32> {
    let total: f64 = tones.iter().map(|t| t.1).sum();
    let mut out = Vec::with_capacity((total * rate) as usize + 1);
    let mut phase = 0f64;
    let mut end = 0f64;
    let mut n = 0usize;
    for &(f, d) in tones {
        end += d;
        let step = std::f64::consts::TAU * f as f64 / rate;
        while (n as f64) < end * rate {
            out.push(amp * phase.sin() as f32);
            phase = (phase + step) % std::f64::consts::TAU;
            n += 1;
        }
    }
    out
}

/// A recognisable test card: colour bars, a grey ramp and a disc whose
/// position depends on `seed` so consecutive images differ.
pub fn test_pattern(w: usize, h: usize, seed: u32) -> Vec<u8> {
    const BARS: [(u8, u8, u8); 8] = [
        (235, 235, 235),
        (235, 235, 16),
        (16, 235, 235),
        (16, 235, 16),
        (235, 16, 235),
        (235, 16, 16),
        (16, 16, 235),
        (16, 16, 16),
    ];
    let mut img = vec![0u8; w * h * 3];
    let cx = (w as f32) * (0.2 + 0.6 * ((seed % 7) as f32 / 6.0));
    let cy = h as f32 * 0.8;
    let r = h as f32 * 0.12;
    for y in 0..h {
        for x in 0..w {
            let c = if y < h * 6 / 10 {
                BARS[x * 8 / w]
            } else if y < h * 7 / 10 {
                let v = (x * 255 / (w - 1)) as u8;
                (v, v, v)
            } else {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if d < r {
                    (250, 160, 30)
                } else {
                    (20, 40, 90)
                }
            };
            let i = (y * w + x) * 3;
            img[i..i + 3].copy_from_slice(&[c.0, c.1, c.2]);
        }
    }
    img
}
