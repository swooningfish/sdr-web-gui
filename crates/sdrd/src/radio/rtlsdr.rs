//! RTL-SDR source via librtlsdr (built against the rtl-sdr-blog fork in the
//! Docker image, which supports V4, V3 and generic RTL2832U dongles).
//!
//! The device is read synchronously on the radio thread. If it can't be
//! opened, or disappears (unplugged), we keep retrying and report the error
//! in the radio state so the UI can show it.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};

use super::{block_len, IqBlock, RadioSettings, SourceCtx};

#[repr(C)]
struct RtlSdrDev {
    _private: [u8; 0],
}

#[link(name = "rtlsdr")]
extern "C" {
    fn rtlsdr_get_device_count() -> u32;
    fn rtlsdr_get_device_name(index: u32) -> *const c_char;
    fn rtlsdr_get_device_usb_strings(
        index: u32,
        manufact: *mut c_char,
        product: *mut c_char,
        serial: *mut c_char,
    ) -> c_int;
    fn rtlsdr_get_index_by_serial(serial: *const c_char) -> c_int;
    fn rtlsdr_open(dev: *mut *mut RtlSdrDev, index: u32) -> c_int;
    fn rtlsdr_close(dev: *mut RtlSdrDev) -> c_int;
    fn rtlsdr_set_center_freq(dev: *mut RtlSdrDev, freq: u32) -> c_int;
    fn rtlsdr_set_freq_correction(dev: *mut RtlSdrDev, ppm: c_int) -> c_int;
    fn rtlsdr_set_sample_rate(dev: *mut RtlSdrDev, rate: u32) -> c_int;
    fn rtlsdr_set_tuner_gain_mode(dev: *mut RtlSdrDev, manual: c_int) -> c_int;
    fn rtlsdr_set_tuner_gain(dev: *mut RtlSdrDev, gain: c_int) -> c_int;
    fn rtlsdr_get_tuner_gains(dev: *mut RtlSdrDev, gains: *mut c_int) -> c_int;
    fn rtlsdr_get_tuner_type(dev: *mut RtlSdrDev) -> c_int;
    fn rtlsdr_set_direct_sampling(dev: *mut RtlSdrDev, on: c_int) -> c_int;
    fn rtlsdr_set_bias_tee(dev: *mut RtlSdrDev, on: c_int) -> c_int;
    fn rtlsdr_reset_buffer(dev: *mut RtlSdrDev) -> c_int;
    fn rtlsdr_read_sync(
        dev: *mut RtlSdrDev,
        buf: *mut c_void,
        len: c_int,
        n_read: *mut c_int,
    ) -> c_int;
}

struct Device {
    dev: *mut RtlSdrDev,
    name: String,
    tuner: String,
    gains_tenths: Vec<i32>,
}

// The raw handle is only ever used from the radio thread.
unsafe impl Send for Device {}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            rtlsdr_close(self.dev);
        }
    }
}

fn cstr(buf: &[c_char]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_string()
}

fn tuner_name(t: c_int) -> &'static str {
    match t {
        1 => "E4000",
        2 => "FC0012",
        3 => "FC0013",
        4 => "FC2580",
        5 => "R820T/R860",
        6 => "R828D",
        _ => "unknown",
    }
}

impl Device {
    fn open(index: u32, serial: Option<&str>) -> Result<Device> {
        let count = unsafe { rtlsdr_get_device_count() };
        if count == 0 {
            bail!("no RTL-SDR device found (is it plugged in and passed through to the container?)");
        }
        let index = match serial {
            Some(s) => {
                let c = std::ffi::CString::new(s)?;
                let i = unsafe { rtlsdr_get_index_by_serial(c.as_ptr()) };
                if i < 0 {
                    bail!("no RTL-SDR with serial {s:?}");
                }
                i as u32
            }
            None => index,
        };
        if index >= count {
            bail!("device index {index} out of range ({count} found)");
        }

        let mut manufact = [0 as c_char; 256];
        let mut product = [0 as c_char; 256];
        let mut serial_buf = [0 as c_char; 256];
        let name = unsafe {
            rtlsdr_get_device_usb_strings(
                index,
                manufact.as_mut_ptr(),
                product.as_mut_ptr(),
                serial_buf.as_mut_ptr(),
            );
            let generic = CStr::from_ptr(rtlsdr_get_device_name(index))
                .to_string_lossy()
                .to_string();
            let (m, p, s) = (cstr(&manufact), cstr(&product), cstr(&serial_buf));
            if p.is_empty() {
                generic
            } else {
                format!("{m} {p} (SN {s})").trim().to_string()
            }
        };

        let mut dev: *mut RtlSdrDev = std::ptr::null_mut();
        let r = unsafe { rtlsdr_open(&mut dev, index) };
        if r < 0 || dev.is_null() {
            bail!(
                "rtlsdr_open failed ({r}); the device may be busy or claimed by the \
                 dvb_usb_rtl28xxu kernel driver — blacklist it on the host (see scripts/pi-setup.sh)"
            );
        }

        let tuner = tuner_name(unsafe { rtlsdr_get_tuner_type(dev) }).to_string();
        let n = unsafe { rtlsdr_get_tuner_gains(dev, std::ptr::null_mut()) };
        let mut gains_tenths = vec![0 as c_int; n.max(0) as usize];
        if n > 0 {
            unsafe { rtlsdr_get_tuner_gains(dev, gains_tenths.as_mut_ptr()) };
        }
        Ok(Device {
            dev,
            name,
            tuner,
            gains_tenths,
        })
    }

    /// Apply settings, only touching what changed relative to `prev`.
    fn apply(&mut self, s: &RadioSettings, prev: Option<&RadioSettings>) -> Result<()> {
        let changed = |f: fn(&RadioSettings) -> bool| prev.map_or(true, |p| f(p) != f(s));
        let d = self.dev;
        unsafe {
            if prev.map_or(true, |p| p.sample_rate != s.sample_rate) {
                check(rtlsdr_set_sample_rate(d, s.sample_rate), "set sample rate")?;
            }
            if prev.map_or(true, |p| p.direct_sampling != s.direct_sampling) {
                check(rtlsdr_set_direct_sampling(d, s.direct_sampling as c_int), "set direct sampling")?;
            }
            if prev.map_or(true, |p| p.ppm != s.ppm) {
                // Returns -2 when the value is unchanged; not an error.
                let r = rtlsdr_set_freq_correction(d, s.ppm);
                if r < 0 && r != -2 {
                    bail!("set ppm failed ({r})");
                }
            }
            if prev.map_or(true, |p| p.center_hz != s.center_hz || p.sample_rate != s.sample_rate) {
                let f = u32::try_from(s.center_hz).map_err(|_| anyhow!("frequency too high"))?;
                check(rtlsdr_set_center_freq(d, f), "set frequency")?;
            }
            if prev.map_or(true, |p| p.gain_db != s.gain_db) {
                match s.gain_db {
                    None => check(rtlsdr_set_tuner_gain_mode(d, 0), "enable AGC")?,
                    Some(g) => {
                        check(rtlsdr_set_tuner_gain_mode(d, 1), "manual gain mode")?;
                        let want = (g * 10.0).round() as i32;
                        let nearest = self
                            .gains_tenths
                            .iter()
                            .copied()
                            .min_by_key(|v| (v - want).abs())
                            .unwrap_or(want);
                        check(rtlsdr_set_tuner_gain(d, nearest), "set gain")?;
                    }
                }
            }
            if changed(|p| p.bias_tee) {
                check(rtlsdr_set_bias_tee(d, s.bias_tee as c_int), "set bias tee")?;
            }
            if prev.is_none() {
                check(rtlsdr_reset_buffer(d), "reset buffer")?;
            }
        }
        Ok(())
    }

    fn read(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let mut n_read: c_int = 0;
        let r = unsafe {
            rtlsdr_read_sync(self.dev, buf.as_mut_ptr() as *mut c_void, len as c_int, &mut n_read)
        };
        if r < 0 {
            bail!("read failed ({r}) — device unplugged?");
        }
        buf.truncate(n_read.max(0) as usize);
        Ok(buf)
    }
}

fn check(r: c_int, what: &str) -> Result<()> {
    if r < 0 {
        bail!("{what} failed ({r})");
    }
    Ok(())
}

pub(super) fn run(ctx: SourceCtx, index: u32, serial: Option<String>) {
    let mut s = ctx.initial.clone();
    let mut last_err: Option<String> = None;
    loop {
        if let Some(n) = ctx.latest_cmd() {
            s = n;
            ctx.update_state(|st| st.settings = s.clone());
        }
        let mut dev = match Device::open(index, serial.as_deref()) {
            Ok(d) => d,
            Err(e) => {
                let msg = e.to_string();
                if last_err.as_deref() != Some(msg.as_str()) {
                    tracing::warn!("{msg}");
                    last_err = Some(msg.clone());
                }
                ctx.update_state(|st| {
                    st.connected = false;
                    st.error = Some(msg);
                });
                sleep(Duration::from_secs(5));
                continue;
            }
        };
        last_err = None;
        if let Err(e) = dev.apply(&s, None) {
            tracing::warn!("initial configuration failed: {e}");
            ctx.update_state(|st| st.error = Some(e.to_string()));
        } else {
            ctx.update_state(|st| st.error = None);
        }
        tracing::info!(device = %dev.name, tuner = %dev.tuner, "RTL-SDR opened");
        ctx.update_state(|st| {
            st.connected = true;
            st.device = Some(dev.name.clone());
            st.tuner = Some(dev.tuner.clone());
            st.gains_db = dev.gains_tenths.iter().map(|g| *g as f32 / 10.0).collect();
            st.settings = s.clone();
        });

        let mut applied = s.clone();
        loop {
            if let Some(n) = ctx.latest_cmd() {
                match dev.apply(&n, Some(&applied)) {
                    Ok(()) => {
                        applied = n.clone();
                        ctx.update_state(|st| {
                            st.settings = n.clone();
                            st.error = None;
                        });
                    }
                    Err(e) => {
                        tracing::warn!("retune failed: {e}");
                        ctx.update_state(|st| st.error = Some(e.to_string()));
                    }
                }
                s = n;
            }
            match dev.read(block_len(applied.sample_rate)) {
                Ok(data) if !data.is_empty() => ctx.publish(IqBlock {
                    center_hz: applied.center_hz,
                    sample_rate: applied.sample_rate,
                    data,
                }),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("{e}");
                    ctx.update_state(|st| {
                        st.connected = false;
                        st.error = Some(e.to_string());
                    });
                    break;
                }
            }
        }
        drop(dev);
        sleep(Duration::from_secs(2));
    }
}
