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
     spectrum, and a bookmark manager that slides out from the receiver's front panel
     (search, list/group filter, ★ Save; Manage: edit, multi-select delete, CSV export of
     all/station/mine, CSV import of our own format, CHIRP or gqrx). A bookmark keeps the
     sample rate it was saved at (optional); picking one switches to that rate and, outside
     the band, retunes the radio for admins. Anyone who can edit a bookmark can update it to
     what's tuned now (frequency, mode, sample rate).
   - CW mode (700 Hz beat note, band-pass, AGC) and a Morse decoder (optional, any mode
     but WFM): tone lock, adaptive speed (5–40 WPM), word-by-word output with junk
     filtering; reads FM repeater idents in NFM.
   - RDS on WFM: station name, programme type, radio text and PI, in the strip under the spectrum and
     on the receiver's display (an RDS annunciator, the station name, and radio text
     that scrolls when it's wider than the display); the compact bar shows the name.
   - Simulator additions: CW beacon on 144.470, repeater with a Morse ident on 145.7125,
     FM station with RDS on 96.1 MHz.
   - Band scan (Bookmarks → Manage, admins start it): FM 87.5–108 MHz in 1.5 MHz windows
     at 2.048 MS/s (power on the 50 kHz raster, peaks 10 dB over the floor, then RDS PS/PI
     per station, in parallel) or DAB Band III (null-symbol check per channel, then a native
     mode I FIC decoder: null sync, cyclic-prefix and phase-reference frequency and timing,
     differential QPSK, frequency de-interleaving, de-puncturing, Viterbi, energy dispersal,
     FIB CRC, FIG 0/0, 1/0 and 1/1 labels). Results are ticked and added as bookmarks
     (FM/DAB groups; DAB ones in DAB mode with their service id). Rotation waits during a
     scan; a pass stops it. Simulator: FM stations on 88.6, 101.3 (RDS) and 104.9 (none),
     DAB multiplexes on 11D and 12B.
   - DAB/DAB+ listening (mode "dab", per listener): a streaming mode I receiver (null sync
     per frame, frequency and timing per frame, all 75 symbols), FIG 0/1 (sub-channels, EEP
     A/B and the 64-entry UEP table) and 0/2 (service → sub-channel, MP2 or DAB+), time
     de-interleaving over 16 CIFs, de-puncturing, Viterbi and energy dispersal per logical
     frame. DAB+: superframe sync on the firecode, RS(120,110) correction, AU CRCs, AUs
     wrapped as LOAS/LATM (960-sample AAC, SBR/PS signalled implicitly). MP2 frames pass
     through. ffmpeg (separate process, minimal LGPL build in the image: AAC/MP2 decoders,
     LOAS/MPEG-audio demuxers, s16le out) decodes to 32 kHz mono for the usual ADPCM
     stream; `{"type":"dab"}` messages carry the ensemble, its stations (coding, bit rate),
     the one playing and its codec. Station picker under the spectrum; name, multiplex and
     codec on the display; bookmarks and F-keys keep the service id (new `service_id`
     columns, CSV column). Simulator: 11D's five services each play a tune in MP2 (sound
     written straight into sub-band 0, so no filterbank) on 128 kbit/s EEP 3-A
     sub-channels. Not yet heard on air: DAB+ through ffmpeg (960-sample HE-AAC) is
     untested, as there's no DAB+ encoder for the simulator.
   - Added after: spectrum zoom (×64 in to about 100 MHz out, buttons, Ctrl + wheel,
     pinch, keys). Zoomed in, panning past the sampled band retunes; zoomed out, the
     sampled band is outlined and dragged to retune, with a *Widen* to the top sample rate.
     A log-scale mini map of the receiver's whole range above the spectrum shows the band
     plan, bookmarks, the sampled band and the view, and moves the radio on click or drag;
     it zooms (buttons, Ctrl + wheel, pinch) down to a few MHz across, pans by dragging,
     and fits the whole range again from its level button.
   - Digital voice decoding (NFM, optional): the raw discriminator, resampled to 48 kHz,
     goes to a per-listener DSD-neo (`SDRD_DSD_PATH`, built into the image, pinned) that
     auto-detects DMR, D-STAR, YSF, NXDN, P25, dPMR and M17. Its JSON telemetry API gives
     the calls on air and the event history (IDs/callsigns, talkgroup, CC/NAC/RAN/CAN,
     alias, text); its console is shown on request. No decoded voice is played. Simulator:
     M17 from GB7SIM on 145.375 every 15 s.
8. **ADS-B aircraft map** ✅
   - Receiver: an optional second dongle kept on 1090 MHz at 2.4 MS/s (its own gain, PPM and
     bias-T), alongside the main one; the two can't open the same dongle. Without one,
     aircraft are decoded whenever the main receiver is on 1090 MHz (tune there, or add an
     "ADS-B 1090" rotation profile from the Aircraft tab); they drop off between visits.
   - Decoder: built into sdrd rather than readsb, so it works on any source (USB, rtl_tcp,
     simulator, rotation): demodulator for 2–2.4 MS/s (fractional half-bit integration,
     sub-sample timing search), CRC with one-bit correction, DF0/4/5/11/16/17/18/20/21,
     identification and category, airborne/surface position (CPR pairs, then local
     decoding, with jump and range sanity checks), velocity, squawk and emergency status.
   - Aircraft tab: Leaflet map with icons by emitter category (airliner, heavy, jet, light,
     fast, helicopter, glider, balloon, drone, vehicle; our own drawings, rotated to the
     track), labels, trails coloured by altitude with a legend, range rings, the furthest
     range per 10° sector (24 h and all time); sortable, filterable list; details (callsign,
     ICAO address, registration and type, category, altitude, climb, speeds, track, squawk,
     emergency, position, range/bearing, signal, messages); follow the selected aircraft.
   - Aircraft database: registration, type and description by address from the Mictronics
     database (tar1090-db CSV, ~9 MB), downloaded on request into SQLite (~600k aircraft).
   - MQTT/HA: an "ADS-B" device with aircraft count, aircraft with position, messages per
     second, furthest and closest aircraft (callsign, distance).
   - Gone aircraft are logged (callsign, first/last seen, furthest range, altitudes) for the
     "seen in 24 h" count; kept as long as other events.
   - Simulator: ten aircraft around London (airliners, a heavy on approach, a business jet,
     a light aircraft, helicopter, glider, balloon, drone, and one taxiing at Heathrow) with
     real squitters on 1090 MHz, at addresses that aren't in the aircraft database.
9. **Signal classifier** (next) — label signals on the spectrum as voice / data / broadcast /
   carrier / CW / digital mode etc. using a small on-device model (CPU on the Pi 5); labels
   on the waterfall and a "what's this?" readout on hover. Uses bookmarks and band plans
   (M7) as context and for training/validation data.
10. **Data and pager modes** (later)
   - Pager decoding: POCSAG (512/1200/2400 baud) and FLEX on NFM, per-channel like APRS
     (multimon-ng or native); message log with capcode, function, type (numeric/alpha/tone),
     search and filters; capcode aliases; optional MQTT/HA for chosen capcodes.
   - Data modes: a Data panel under the spectrum that decodes the listening channel — e.g. RTTY,
     PSK31, packet/AFSK1200 outside APRS, FT8/FT4 and WSPR on the usual dial frequencies,
     and simple ones (DTMF, ZVEI/selcall, EAS). Exact list to settle when it's built.
   - Bookmarks/band plan pick the decoder where the mode is known; a rotation profile can
     hold a pager or data channel like the other decoders.
   - Simulator: a POCSAG pager channel and a data-mode beacon to test against.
11. **Meshtastic and MeshCore** (later)
   - LoRa demodulator for the UK/EU 868 MHz presets (Meshtastic LongFast and the others;
     MeshCore's narrow EU/UK preset), on the main dongle or a second one kept on 868 MHz
     like ADS-B.
   - Meshtastic: decode the packet header and protobufs; decrypt the default public channel
     and any private channels whose PSK an admin adds (keys stored server-side, admin only).
     Direct messages under PKI can't be read without the node's private key — show them as
     encrypted, with sender/recipient.
   - MeshCore: decode adverts (node name, type, position), the public channel, and private
     channels from admin-added keys; DMs shown as encrypted.
   - Views: node list (name, short/long ID, hardware, role, battery/telemetry, SNR/RSSI,
     hops, last heard), nodes on the map (shared with APRS/aircraft maps), channel message
     view per channel (public/private), and a packet log.
   - MQTT/HA for tracked nodes (position, battery, telemetry), as for APRS stations.
   - Simulator: a few Meshtastic and MeshCore nodes with positions and channel chatter.

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
  rotation on/off switch under the receiver's presets and on the Satellites tab.
- **Harmonics check:** measures a short-range device's carrier and its harmonics (retuning
  to each) and rates them Good / Medium / Bad against Ofcom (EN 300 220-1 Table 19, UK SRD
  ERP limits) and FCC Part 15 (§15.231(b), §15.249, §15.227, §15.205/§15.209) limits, in dBc
  assuming a carrier at the legal maximum. Also rates spurs near the carrier from the same
  capture (EN 300 220-1 Figure 5 OOB mask / Table 19, FCC §15.231(b)), telling the device's
  spurs from other signals by whether they go when it stops. Simulated SRD on 434.2 MHz
  with harmonics and a 50 kHz spur comb.
- **UI:** settings in tabs, profile page (password), system info, waterfall palettes,
  spectrum toolbar that doesn't move on hover, phone-friendly tab bar.
- **Tests:** API smoke test (`scripts/smoke-test.sh`) and Playwright layout tests at four
  screen sizes (`scripts/ui-test.sh`).

## Left to do (besides M7–M9)
- **Test on the real hardware:** nothing has run against a physical dongle on the Pi 5
  yet. Check device open/retune, gain, USB passthrough, rtl_433/TPMS, APRS, an ARISS SSTV
  event and a live ISS pass, and CPU headroom (waterfall + decoders + audio listeners).
- **CI:** GitHub Actions (`.github/workflows/ci.yml`) runs `cargo test`, `svelte-check`
  and the web build, then builds the image and runs the smoke and Playwright tests against
  the synthetic source. Still to add: clippy (after the clean-ups below) and an arm64
  image build.
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
