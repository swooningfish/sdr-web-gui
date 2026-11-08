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
1. **Skeleton** — Cargo workspace, device open/tune, live waterfall to browser, auth, Docker.
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
6. **Live audio + polish** — Opus streaming, settings UI, health/diagnostics, docs.

## Open items
- Observer location (lat/lon/alt) for pass prediction — set in UI/env at deploy time.
- Dev loop: no Rust toolchain on this Windows PC; build via Docker here, run with a
  dongle on the Pi. A file-replay source (recorded IQ / WAV) will allow testing without hardware.
