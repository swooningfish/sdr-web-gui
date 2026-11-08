//! Replay source: loops a recorded cu8 file (e.g. from `rtl_sdr -f ... out.cu8`)
//! in real time. Centre frequency and sample rate come from SDRD_CENTER_HZ /
//! SDRD_SAMPLE_RATE since raw cu8 files carry no metadata.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::{block_len, IqBlock, SourceCtx};

/// Runs until another device is selected; returns its id.
pub(super) fn run(ctx: &mut SourceCtx, path: Option<&Path>) -> String {
    let fail = |ctx: &mut SourceCtx, msg: String| {
        tracing::error!("{msg}");
        ctx.update_state(|st| st.error = Some(msg));
        ctx.park()
    };
    let Some(path) = path else {
        return fail(ctx, "no recording configured (SDRD_REPLAY_FILE)".into());
    };
    let s = ctx.current();
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return fail(ctx, format!("cannot open replay file {}: {e}", path.display())),
    };
    if file.metadata().map(|m| m.len()).unwrap_or(0) < 2 {
        return fail(ctx, "replay file is empty".into());
    }
    let mut reader = BufReader::new(file);
    ctx.update_state(|st| {
        st.connected = true;
        st.device_id = Some("replay".into());
        st.device = Some(format!("Replay: {}", path.display()));
        st.tuner = Some("file".into());
    });
    tracing::info!(file = %path.display(), "replay source running");

    let len = block_len(s.sample_rate);
    let block_dur = Duration::from_secs_f64(len as f64 / 2.0 / s.sample_rate as f64);
    let mut next = Instant::now();
    loop {
        // Replay cannot be retuned; drain commands so the queue doesn't grow.
        if let Some(d) = ctx.take().device.filter(|d| d != "replay") {
            return d;
        }

        let mut data = vec![0u8; len];
        let mut filled = 0;
        while filled < len {
            match reader.read(&mut data[filled..]) {
                Ok(0) => {
                    // End of file: loop back to the start.
                    if let Err(e) = reader.seek(SeekFrom::Start(0)) {
                        return fail(ctx, format!("replay seek error: {e}"));
                    }
                }
                Ok(n) => filled += n,
                Err(e) => return fail(ctx, format!("replay read error: {e}")),
            }
        }
        ctx.publish(IqBlock {
            center_hz: s.center_hz,
            sample_rate: s.sample_rate,
            data,
        });
        next += block_dur;
        let now = Instant::now();
        if next > now {
            sleep(next - now);
        } else {
            next = now;
        }
    }
}
