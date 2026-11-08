//! SSTV mode timings. Every mode is described relative to its line sync
//! pulse: scans are placed at offsets (ms) from the start of that pulse, which
//! is also what the decoder locks onto. Values follow the published mode
//! specifications (Martin, Scottie, Robot, PD).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// Green, blue, red scans (Martin, Scottie).
    Gbr,
    /// Robot 36: luminance plus one chroma scan per line, alternating
    /// R-Y (even lines) and B-Y (odd lines).
    Robot36,
    /// Robot 72: Y, R-Y, B-Y every line.
    Yuv,
    /// PD: Y of line 1, R-Y, B-Y, Y of line 2 per sync period.
    Pd,
}

#[derive(Debug)]
pub struct Mode {
    pub name: &'static str,
    pub vis: u8,
    pub width: u16,
    pub height: u16,
    pub color: Color,
    pub sync_ms: f64,
    /// Porch after the sync pulse (used to pin down the end of the sync).
    pub porch_ms: f64,
    /// Sync-to-sync period.
    pub line_ms: f64,
    /// From the end of the VIS code to the first line sync.
    pub first_sync_ms: f64,
    /// Scans in transmission order: (offset from sync start, duration), ms.
    pub scans: &'static [(f64, f64)],
}

impl Mode {
    pub fn lines_per_sync(&self) -> u16 {
        if self.color == Color::Pd {
            2
        } else {
            1
        }
    }

    /// Number of sync periods in an image.
    pub fn periods(&self) -> usize {
        (self.height / self.lines_per_sync()) as usize
    }

    /// Total image transmission time after the VIS code, seconds.
    #[cfg(test)]
    pub fn duration_s(&self) -> f64 {
        (self.first_sync_ms + self.periods() as f64 * self.line_ms) / 1000.0
    }
}

macro_rules! martin {
    ($name:expr, $vis:expr, $s:expr) => {
        Mode {
            name: $name,
            vis: $vis,
            width: 320,
            height: 256,
            color: Color::Gbr,
            sync_ms: 4.862,
            porch_ms: 0.572,
            line_ms: 4.862 + 0.572 + 3.0 * ($s + 0.572),
            first_sync_ms: 0.0,
            scans: &[
                (5.434, $s),
                (5.434 + $s + 0.572, $s),
                (5.434 + 2.0 * ($s + 0.572), $s),
            ],
        }
    };
}

macro_rules! scottie {
    ($name:expr, $vis:expr, $s:expr) => {
        Mode {
            name: $name,
            vis: $vis,
            width: 320,
            height: 256,
            color: Color::Gbr,
            sync_ms: 9.0,
            porch_ms: 1.5,
            line_ms: 3.0 * 1.5 + 9.0 + 3.0 * $s,
            // A one-off 9 ms sync, then green and blue come before the line sync.
            first_sync_ms: 9.0 + 3.0 + 2.0 * $s,
            scans: &[(-(2.0 * $s + 1.5), $s), (-$s, $s), (10.5, $s)],
        }
    };
}

macro_rules! pd {
    ($name:expr, $vis:expr, $w:expr, $h:expr, $px:expr) => {
        Mode {
            name: $name,
            vis: $vis,
            width: $w,
            height: $h,
            color: Color::Pd,
            sync_ms: 20.0,
            porch_ms: 2.08,
            line_ms: 22.08 + 4.0 * $px * $w as f64,
            first_sync_ms: 0.0,
            scans: &[
                (22.08, $px * $w as f64),
                (22.08 + $px * $w as f64, $px * $w as f64),
                (22.08 + 2.0 * $px * $w as f64, $px * $w as f64),
                (22.08 + 3.0 * $px * $w as f64, $px * $w as f64),
            ],
        }
    };
}

pub static MODES: &[Mode] = &[
    Mode {
        name: "Robot 36",
        vis: 8,
        width: 320,
        height: 240,
        color: Color::Robot36,
        sync_ms: 9.0,
        porch_ms: 3.0,
        line_ms: 150.0,
        first_sync_ms: 0.0,
        scans: &[(12.0, 88.0), (106.0, 44.0)],
    },
    Mode {
        name: "Robot 72",
        vis: 12,
        width: 320,
        height: 240,
        color: Color::Yuv,
        sync_ms: 9.0,
        porch_ms: 3.0,
        line_ms: 300.0,
        first_sync_ms: 0.0,
        scans: &[(12.0, 138.0), (156.0, 69.0), (231.0, 69.0)],
    },
    martin!("Martin M1", 44, 146.432),
    martin!("Martin M2", 40, 73.216),
    scottie!("Scottie S1", 60, 138.24),
    scottie!("Scottie S2", 56, 88.064),
    scottie!("Scottie DX", 76, 345.6),
    pd!("PD 50", 93, 320, 256, 0.286),
    pd!("PD 90", 99, 320, 256, 0.532),
    pd!("PD 120", 95, 640, 496, 0.19),
    pd!("PD 160", 98, 512, 400, 0.382),
    pd!("PD 180", 96, 640, 496, 0.286),
    pd!("PD 240", 97, 640, 496, 0.382),
];

pub fn by_vis(vis: u8) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.vis == vis)
}

pub fn by_name(name: &str) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.name == name)
}

/// SSTV pixel frequency: 1500 Hz black … 2300 Hz white.
pub fn value_to_freq(v: u8) -> f32 {
    1500.0 + v as f32 * (800.0 / 255.0)
}

pub fn freq_to_value(f: f32) -> u8 {
    ((f - 1500.0) * (255.0 / 800.0)).round().clamp(0.0, 255.0) as u8
}

/// BT.601 (studio range), as used by Robot and PD modes.
pub fn rgb_to_yuv(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    let y = 16.0 + (65.738 * r + 129.057 * g + 25.064 * b) / 256.0;
    let u = 128.0 + (-37.945 * r - 74.494 * g + 112.439 * b) / 256.0;
    let v = 128.0 + (112.439 * r - 94.154 * g - 18.285 * b) / 256.0;
    let c = |x: f32| x.round().clamp(0.0, 255.0) as u8;
    (c(y), c(u), c(v))
}

pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let y = 1.164 * (y as f32 - 16.0);
    let (u, v) = (u as f32 - 128.0, v as f32 - 128.0);
    let c = |x: f32| x.round().clamp(0.0, 255.0) as u8;
    (c(y + 1.596 * v), c(y - 0.392 * u - 0.813 * v), c(y + 2.017 * u))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_times_match_specs() {
        let t = |n: &str| by_name(n).unwrap().line_ms;
        assert!((t("Martin M1") - 446.446).abs() < 1e-6);
        assert!((t("Martin M2") - 226.798).abs() < 1e-3);
        assert!((t("Scottie S1") - 428.22).abs() < 1e-6);
        assert!((t("Scottie S2") - 277.692).abs() < 1e-6);
        assert!((t("Scottie DX") - 1050.3).abs() < 1e-6);
        assert!((t("PD 90") - 703.04).abs() < 1e-6);
        assert!((t("PD 120") - 508.48).abs() < 1e-6);
        assert!((t("PD 180") - 754.24).abs() < 1e-6);
        assert!((t("PD 240") - 1000.0).abs() < 1e-6);
        // Robot 36 is 36 s, Martin M1 ~114 s, PD 120 ~126 s.
        assert!((by_name("Robot 36").unwrap().duration_s() - 36.0).abs() < 0.01);
        assert!((by_name("Martin M1").unwrap().duration_s() - 114.3).abs() < 0.1);
        assert!((by_name("PD 120").unwrap().duration_s() - 126.1).abs() < 0.1);
    }

    #[test]
    fn vis_codes_unique_and_yuv_roundtrip() {
        for m in MODES {
            assert_eq!(MODES.iter().filter(|x| x.vis == m.vis).count(), 1, "{}", m.name);
        }
        for &(r, g, b) in &[(255, 0, 0), (0, 255, 0), (0, 0, 255), (128, 128, 128), (250, 200, 30)] {
            let (y, u, v) = rgb_to_yuv(r, g, b);
            let (r2, g2, b2) = yuv_to_rgb(y, u, v);
            assert!((r as i32 - r2 as i32).abs() <= 2 && (g as i32 - g2 as i32).abs() <= 2 && (b as i32 - b2 as i32).abs() <= 2);
        }
    }
}
