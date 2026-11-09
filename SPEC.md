# onair-web — design spec (draft v0.1, 2026-11-08)

A self-hosted, web-controlled SDR monitor for a single RTL-SDR on a Raspberry Pi 5,
packaged in Docker. Inspired by OnAir SDR's breadth, but focused on unattended
monitoring: TPMS/ISM, APRS, and SSTV (ISS/ARISS + any user-set frequency).

## Decisions (from Q&A)

| Topic | Decision |
|---|---|
| Host | Raspberry Pi 5 (arm64), Linux, Docker |
| SDR | One dongle, model unknown → build against the **rtl-sdr-blog** librtlsdr fork (supports V4, V3 and generic RTL2832U) |
| Core language | **Rust** (single process owns the dongle) |
| SSTV | Any frequency the user configures + **record every ISS pass** automatically |
| v1 extras | Live waterfall, live audio in browser, APRS map, MQTT / Home Assistant |
| Exposure | **Internet-facing** → real auth, TLS via reverse proxy |
| License | Own code (not derived from OnAir, which is GPL-3.0). rtl_433 / Direwolf are run as separate processes. |

## Frequencies (UK defaults, all editable in UI)

| Profile | Centre / rate | Channels |
|---|---|---|
| **VHF block** | ~145.30 MHz @ 2.048 MS/s | APRS 144.800 · ISS APRS 145.825 · ISS SSTV 145.800 |
| **ISM block** | 433.92 MHz @ 1.024 MS/s | rtl_433 (built-in decoders + user flex decoders, e.g. `n=TPMS,m=FSK_PCM,s=8,l=8,r=3000`) |
| **ISS pass** (preempts) | 437.550 MHz (Robot36, current ARISS) — configurable, Doppler-corrected | SSTV + WAV recording of whole pass |
| **Custom** | any | user-defined channels: NFM/WFM/AM/USB → SSTV / APRS / record |

One dongle ⇒ a scheduler time-shares profiles (e.g. 60 s VHF / 30 s ISM, configurable).
ISS passes (elevation ≥ configurable min) take priority over everything.
Note: SSTV images on VHF last 36–120 s; the scheduler won't hop away mid-image
(hold while a VIS header/image is in progress).

## Architecture

```
            ┌──────────────────────── sdrd (Rust, tokio) ───────────────────────────┐
 RTL-SDR ──►│ device (librtlsdr FFI) ─► IQ bus ─┬─► FFT ─► waterfall frames ─┐      │
  (USB)     │        ▲ retune                   ├─► channelizer (NCO+FIR decim)  │      │
            │   scheduler ◄── ISS passes (sgp4 + CelesTrak TLE, Doppler)       │      │
            │                                   │    ├─ FM demod ─► SSTV decoder ─► PNG
            │                                   │    ├─ FM demod ─► direwolf (stdin) ─► KISS ─► APRS parser
            │                                   │    └─ demod ─► Opus ─► live audio  │
            │                                   └─► cu8 ─► rtl_433 (stdin) ─► JSON   │
            │ events ─► SQLite + /data (images, WAV) ─► MQTT (HA discovery)          │
            │ axum HTTP + WebSocket API (auth, sessions)                             │
            └──────────────────────────────────────────────────────────────────────┘
                     ▲ HTTPS (Caddy reverse proxy, auto TLS)
                 Browser SPA (Svelte + Vite): waterfall (WebGL), audio (AudioWorklet),
                 TPMS/ISM table, APRS table + Leaflet map, SSTV gallery, scheduler,
                 ISS pass list, settings
```

Key crates: `tokio`, `axum`, `rustfft`, `sqlx` (SQLite), `rumqttc`, `sgp4`,
`audiopus`, `argon2`, `tower-governor` (rate limit), `image`/`png`.

### SSTV decoder (native Rust)
- VIS detection (1900/1200 Hz leader + 30 ms bits) → auto mode select.
- Modes v1: Robot 36/72, PD 90/120/180, Martin M1/M2, Scottie S1/S2/DX.
- Instantaneous-frequency demod (Hilbert), sync tracking + slant correction,
  YUV→RGB, partial images saved if signal drops.
- Tested against reference WAVs (generated with an encoder + real ARISS recordings).

## Security (internet-facing)
- Argon2id password hashes; admin + viewer roles; first admin from env / setup token.
- HttpOnly + Secure + SameSite=Strict session cookies, CSRF token on mutating calls,
  login rate limiting / lockout, WebSocket auth by session.
- TLS terminated by Caddy in docker-compose; core binds to internal network only.
- Viewer role can be set to hide TPMS/ISM detail (third-party vehicle data) —
  amateur APRS/SSTV is fine to publish, other people's TPMS IDs probably shouldn't be public.

## Docker
- Multi-stage build: Rust build → slim runtime with rtl_433, direwolf, rtl-sdr-blog libs.
- `devices: /dev/bus/usb`; host needs `dvb_usb_rtl28xxu` blacklisted (setup script provided).
- `docker-compose.yml`: `sdrd` + `caddy`; volume `/data` for DB, images, recordings.
- Build natively on the Pi (or GitHub Actions arm64 runner) — QEMU cross-builds of Rust are slow.

## Milestones
1. **Skeleton** ✅ — Cargo workspace, device open/tune, live waterfall to browser, auth, Docker.
2. **ISM/TPMS** ✅ — rtl_433 pipe, flex-decoder editor, events table, MQTT + HA discovery,
   per-device history charts. HA discovery only for devices an admin marks as tracked.
3. **APRS** ✅ — channelizer + FM demod, Direwolf (one per channel, KISS over TCP), APRS parser
   (positions, compressed, Mic-E, objects, items, status, messages, weather), station table,
   Leaflet map with tracks, packet log, HA device tracker for tracked stations.
4. **SSTV** ✅ — native decoder (Robot 36/72, Martin M1/M2, Scottie S1/S2/DX, PD 50–240;
   VIS auto-detect, per-line sync tracking + least-squares slant correction, partial images),
   encoder for tests and the synthetic source, live line-by-line view, gallery. FM/USB/LSB.
   Per-image WAV recording moved to M5 (whole-pass recording).
5. **Scheduler + ISS** ✅ — profiles with dwell times (rotation, SSTV hold), CelesTrak TLEs +
   SGP4 pass prediction (cross-checked against skyfield), automatic pass tuning, Doppler
   tracking for flagged decoder channels, Doppler-corrected WAV recording per pass, sky plot,
   test passes.
6. **Live audio + polish** ✅ — per-listener demod (NFM/AM/WFM/USB/LSB) with squelch, spectral
   noise reduction and AGC, streamed as IMA ADPCM over the WebSocket (no Opus/libopus
   dependency) into an AudioWorklet jitter buffer; click-to-listen on the waterfall; tabbed
   admin settings, profile page, system info, waterfall palettes; Playwright layout tests.
7. **Spectrum tools** ✅
   - Band-plan strip above the spectrum: UK amateur bands 160 m–23 cm with RSGB sub-bands
     (CW, SSB, digital, beacons, FM simplex, repeaters, satellites), plus broadcast (MW, SW,
     FM, DAB), aviation, marine VHF/AIS, CB/PMR446, ISM/SRD and satellite bands; groups
     toggle per browser; hover names the band. Clicking the spectrum can pick the band's
     usual mode (on by default).
   - Tuning: drag the spectrum, trackpad sideways, mouse wheel (once the spectrum has focus,
     so page scrolling doesn't retune), Left/Right arrows (Shift = bigger steps); admins
     only. Up/Down step the listening frequency for everyone. The waterfall history slides
     to stay lined up after a retune.
   - Bookmarks: a shared station list (admins edit) plus each user's own. Flags on the
     spectrum, a Bookmarks panel under Listen (search, list/group filter, ★ Save), and a
     Bookmarks page from its cog (sortable table, edit, multi-select delete, CSV export of
     all/station/mine, CSV import of our own format, CHIRP or gqrx). Picking a bookmark
     outside the band retunes the radio for admins.
   - CW mode (700 Hz beat note, band-pass, AGC) and a Morse decoder (optional, any mode
     but WFM): tone lock, adaptive speed (5–40 WPM), word-by-word output with junk
     filtering; reads FM repeater idents in NFM.
   - RDS on WFM: station name, programme type, radio text and PI, in the Listen panel.
   - Simulator additions: CW beacon on 144.470, repeater with a Morse ident on 145.7125,
     FM station with RDS on 96.1 MHz.
   - Added after: spectrum zoom (×64 in to about 100 MHz out, buttons, Ctrl + wheel,
     pinch, keys). Zoomed in, panning past the sampled band retunes; zoomed out, the
     sampled band is outlined and dragged to retune, with a *Widen* to the top sample rate.
     A log-scale mini map of the receiver's whole range above the spectrum shows the band
     plan, bookmarks, the sampled band and the view, and moves the radio on click or drag.
8. **ADS-B aircraft map** (next)
   - Receiver: a second dongle with a 1090 MHz antenna running ADS-B all the time (readsb),
     alongside the main dongle; fallback: ADS-B as a rotation profile on a single dongle
     (aircraft drop off between visits). Needs two dongles open at once (device
     selection currently opens one).
   - Map (same Leaflet style as APRS): live aircraft with icons by type (airliner, light,
     helicopter, glider, balloon, drone… from the broadcast emitter category; our own icons),
     trails coloured by altitude with a legend, details on click (callsign, altitude, speed,
     heading, climb rate, squawk, registration/model from the open aircraft database),
     aircraft list, max-range rings / range stats for antenna placement.
   - Optional MQTT/HA: aircraft count, closest aircraft.
9. **Signal classifier** — label signals on the spectrum as voice / data / broadcast /
   carrier / CW / digital mode etc. using a small on-device model (CPU on the Pi 5); labels
   on the waterfall and a "what's this?" readout on hover. Uses bookmarks and band plans
   (M7) as context and for training/validation data.

## Added beyond the original plan
Along the way (from requests during the build, and Greg's own commits):
- **Receivers:** choose between several RTL-SDR dongles, with each tuner's frequency
  coverage (R820T/R828D, Blog V4 with HF, E4000, FC0012/13, FC2580); features outside
  the selected device's range are shown as unavailable. Selectable source: USB, rtl_tcp or
  simulated, plus Docker USB passthrough.
- **Sensors:** dashboard of pinned sensors (live cards with sparklines), visible
  transmitter IDs with copy, sortable list (name, signal, last heard, count), multi-select
  delete.
- **APRS:** home (station location) marker and "centre on home"; multi-select delete.
- **SSTV:** multi-select gallery with ZIP download and bulk delete.
- **Passes:** stop a test or real pass; station location by place/postcode search
  (Nominatim) or Maidenhead locator, with a clear hint when geolocation needs HTTPS;
  rotation on/off switch in the Receiver panel and Passes tab.
- **UI:** settings in tabs, profile page (password), system info, waterfall palettes,
  spectrum toolbar that doesn't move on hover, phone-friendly tab bar.
- **Tests:** API smoke test (`scripts/smoke-test.sh`) and Playwright layout tests at four
  screen sizes (`scripts/ui-test.sh`).

## Left to do (besides M7–M9)
- **Test on the real hardware:** nothing has run against a physical dongle on the Pi 5
  yet. Check device open/retune, gain, USB passthrough, rtl_433/TPMS, APRS, an ARISS SSTV
  event and a live ISS pass, and CPU headroom (waterfall + decoders + audio listeners).
- **CI:** no GitHub Actions yet. Add `cargo test` + clippy, `svelte-check`, the arm64
  image build, and (optionally) the Playwright tests against the synthetic source.
- **Licence:** the repository is public with no licence (all rights reserved by default).
  Pick one (e.g. MIT, Apache-2.0 or GPL-3.0).
- **ARISS on 437.550 MHz:** the default ISS profile tunes the VHF block; ARISS events on
  70 cm need the profile switched to 437.6 MHz (maybe per-event presets).
- **Small clean-ups:** two clippy simplifications in `radio/rtltcp.rs`; Direwolf comes
  from Debian (1.6), 1.7 could be built from source if needed.
- **Dev stack:** the saved receiver choice overrides `SDRD_SOURCE`; with an unplugged USB
  dongle selected, the spectrum is empty (pick *Simulated* in Settings → Receiver).

## Notes
- Dev loop: no Rust toolchain on the Windows dev PC; build and test in Docker, run with a
  dongle on the Pi. Without hardware, use the simulated receiver (real APRS, SSTV and
  Nexus sensor transmissions) or the replay source (`SDRD_SOURCE=replay`, recorded cu8 IQ).
