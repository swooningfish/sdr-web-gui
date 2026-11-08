# sdr-web-gui

A self-hosted, browser-controlled RTL-SDR monitor for a Raspberry Pi, packaged in Docker.
It's inspired by [OnAir](https://github.com/adswill/OnAir) but focuses on unattended
monitoring: TPMS/ISM sensors (rtl_433), APRS, and SSTV, including automatic ISS/ARISS pass recording.

See [SPEC.md](SPEC.md) for the full design and roadmap.

## Status: milestone 6 (live audio)

- [x] Rust core (`sdrd`) owns the dongle via librtlsdr (rtl-sdr-blog fork: V4, V3, generic)
- [x] Live spectrum and waterfall in the browser over WebSocket
- [x] Tuning, gain, sample rate, PPM, bias-T and direct sampling from the UI
- [x] Login with admin and viewer roles, Argon2id, secure sessions, CSRF protection, login rate limiting
- [x] Synthetic and replay IQ sources for development without hardware
- [x] Docker image plus Caddy (automatic HTTPS) compose setup for the Pi
- [x] M2: rtl_433 / TPMS with a flex-decoder editor, sensor history charts, MQTT and Home Assistant
- [x] M3: APRS (Direwolf) with a live map, station tracks, packet log and weather charts
- [x] M4: native SSTV decoder (13 modes, slant correction) with live view and image gallery
- [x] M5: ISS pass prediction (SGP4, CelesTrak TLEs), automatic pass tuning, Doppler tracking, pass recordings, profile rotation
- [x] M6: live audio in the browser (NFM/AM/WFM/USB/LSB, squelch, noise reduction), tabbed settings, profile page, browser layout tests

## Prerequisites

- Docker with the **Compose** and **buildx** plugins. The Dockerfile uses BuildKit cache mounts
  (`RUN --mount=type=cache`) to keep Rust rebuilds fast. Without buildx, Docker falls back to the
  legacy builder and the build fails with `the --mount option requires BuildKit`.

  | Distro | Install buildx |
  |---|---|
  | Arch / Manjaro | `sudo pacman -S docker-buildx` |
  | Fedora / RHEL (Docker CE repo) | `sudo dnf install docker-buildx-plugin` |
  | Fedora (distro `moby-engine`) | `sudo dnf install docker-buildx` |
  | Debian / Ubuntu / Raspberry Pi OS (Docker apt repo) | `sudo apt install docker-buildx-plugin` |

  Check with `docker buildx version`. Docker installed via `get.docker.com` (as
  `scripts/pi-setup.sh` does) already includes it.
- For hot-reload UI work only: Node.js 22+.
- For deployment: an RTL-SDR dongle (RTL-SDR Blog V4/V3 or generic) on a Raspberry Pi 5.

## Quick start: development (no dongle needed)

```bash
docker compose -f docker-compose.dev.yml up --build
```

Open <http://localhost:8080> and sign in as `admin` / `devpassword123`. The synthetic source
puts test signals on 144.800, 145.800, 145.825, 433.920 and 437.550 MHz (Doppler-drifting
carrier). Use the presets to see them. The test signals are real, decodable transmissions:

- **144.800 / 145.825 (VHF block, the default):** APRS packets that Direwolf decodes: a car
  driving a loop around central London, a weather station, a status beacon, and an "ISS" on
  145.825. Open the APRS tab.
- **145.800 (also in the VHF block):** SSTV test cards cycling through Robot 36, Martin M2,
  Scottie S2 and PD 90, with 20 s gaps. Open the SSTV tab to watch them arrive line by line.
- **433.92 (ISM preset, or *Tune* on the Sensors page):** two Nexus temperature/humidity
  sensors that rtl_433 decodes within ~15 s.

By default the dev stack only listens on localhost. To open it from other machines on your LAN
(e.g. `http://192.168.0.231:8080`), set `SDRD_DEV_BIND`:

```bash
SDRD_DEV_BIND=0.0.0.0 docker compose -f docker-compose.dev.yml up --build
```

The dev admin password is the one above, so anyone on the network can log in and control the
dongle. Fine on a home LAN, not on a shared one.

The dev stack also runs a Mosquitto broker. Watch what sdrd publishes with:

```bash
docker compose -f docker-compose.dev.yml exec mqtt mosquitto_sub -v -t 'sdrd/#' -t 'homeassistant/#'
```

For UI work with hot reload, run sdrd as above, then:

```bash
cd web && npm install && npm run dev
```

Vite proxies `/api` to `localhost:8080`.

## Deploying on a Raspberry Pi 5

```bash
git clone https://github.com/swooningfish/sdr-web-gui && cd sdr-web-gui
sudo ./scripts/pi-setup.sh          # blacklist DVB driver, udev rule, Docker
cp .env.example .env && nano .env    # SDR_DOMAIN, SDRD_ADMIN_PASSWORD, ...
docker compose up -d --build         # first build takes a while on the Pi
docker compose logs -f sdrd
```

Caddy obtains a Let's Encrypt certificate for `SDR_DOMAIN`. Point DNS at your public IP and
forward ports 80 and 443 to the Pi. sdrd itself is only reachable through Caddy.

## Sensors (rtl_433, TPMS)

rtl_433 runs inside the container and is fed from sdrd's own IQ stream: sdrd cuts ~256 kHz
around the rtl_433 frequency (433.92 MHz by default) out of whatever the dongle is tuned to.
So it decodes whenever 433.92 MHz is inside the tuned band. With one dongle that means the ISM
preset; automatic time-sharing with the VHF block comes with the scheduler (M5).

- **Sensors** tab: every device heard, latest readings, signal, a live feed, and per-device
  history with charts (1 h / 24 h / 7 days). Repeats of the same transmission are merged.
- **Settings → ISM decoder**: enable/disable, frequency, `-R` protocol list, and flex decoders
  (`-X`). *Check* asks rtl_433 itself whether it accepts a spec; saving restarts rtl_433.
  The default config includes `n=TPMS,m=FSK_PCM,s=8,l=8,r=3000`.
- **Track** a device to keep it beyond the retention period and publish it to Home Assistant.
- Viewers can't see sensor data unless `SDRD_ISM_VIEWERS=true` (it's other people's TPMS IDs).

## APRS (Direwolf)

Each APRS channel (default 144.800 MHz and ISS APRS 145.825 MHz) is cut out of the capture
as a 16 kHz slice, FM-demodulated and piped to its own Direwolf process; decoded frames come
back over KISS. The default VHF block (145.3 MHz at 2.048 MS/s) covers both channels at once,
alongside ISS SSTV on 145.800.

- **APRS** tab: live map (OpenStreetMap) with station labels and tracks, station list,
  per-station details (position, speed/course, altitude, comment, raw packets, weather charts
  for weather stations) and a packet log. 1 h / 6 h / 24 h window.
- Parses uncompressed and compressed positions, Mic-E, objects, items, status, messages
  and weather reports. Copies of a packet repeated by digipeaters are merged.
- **Settings → APRS decoder**: up to 4 channels with labels.
- APRS is public amateur traffic, so viewer accounts can see it.
- The map uses OpenStreetMap's tile server. Set `SDRD_MAP_TILE_URL` to use another one (the
  Content-Security-Policy follows it).

## SSTV

A native Rust decoder (no external program). Each SSTV channel is filtered out of the
capture, demodulated (FM, or USB/LSB for HF), and turned into a tone-frequency track. The
VIS header picks the mode automatically:

- Robot 36 and 72
- Martin M1 and M2
- Scottie S1, S2 and DX
- PD 50, 90, 120, 160, 180 and 240

Every line's sync pulse is tracked, so slant from clock differences is corrected.

- Default channels: **ISS SSTV 145.800 MHz** (in the default VHF block) and **ARISS UHF
  437.550 MHz**, both FM. Up to 4 channels under Settings → SSTV decoder.
- **SSTV** tab: images appear line by line as they are received, then go into the gallery
  with thumbnails, a full-size viewer, download and (admins) delete.
- If the signal fades or the radio is retuned mid-image, whatever was received is still
  saved, marked as partial, as long as at least a fifth of the image came through.
- Images are PNG files in `/data/sstv` and are **not** removed by the event retention period.
- SSTV is public amateur traffic, so viewer accounts can see it.
- During ISS passes, channels marked **Doppler** follow the satellite's Doppler shift
  (±3.5 kHz at 145.8 MHz, ±10 kHz at 437.55 MHz).

## Live audio

Press **Play** in the *Listen* panel on the Spectrum tab, then click the waterfall (or type a
frequency) to listen anywhere inside the tuned band. The channel is shaded on the spectrum.

- Modes: **NFM**, **AM**, **WFM** (broadcast, 50 µs de-emphasis), **USB**, **LSB**.
- **Squelch**, **noise reduction** (spectral; removes steady hiss), **volume** and **mute**.
  Settings are remembered per browser; clicks snap to the mode's channel raster.
- Audio is demodulated per listener in sdrd and streamed over the existing WebSocket as
  IMA ADPCM (about 70 kb/s for voice modes), played through an AudioWorklet with a
  250 ms jitter buffer. No codec libraries are needed on the Pi.
- Up to 4 listeners at once (`SDRD_AUDIO_MAX_LISTENERS`); admins only unless
  `SDRD_AUDIO_VIEWERS=true`.

## Settings

Admins get **Settings**, split into tabs: Receiver (SDR devices), Decoders (rtl_433, APRS,
SSTV), Passes (location, satellites, rotation, TLEs, test pass), Integrations (MQTT / Home
Assistant), Users and System (read-only server settings). Everyone can open their
**profile** from their username in the header to change their password or sign out.

## Browser tests

`scripts/ui-test.sh` runs Playwright (in its official Docker image) against the dev stack at
1920, 1366, 820 and 375 px wide. It checks that no page scrolls sideways, that the spectrum
toolbar doesn't move on hover, and checks the settings tabs, the profile page and remembered
waterfall colours.

## Passes and scheduling

Set the station location under **Settings → Passes**: search for a town or postcode, type a
Maidenhead locator (e.g. `IO91wm`), enter coordinates, or press *Use this device's location*
(browsers only allow that over HTTPS or on `localhost`, not on a plain-HTTP LAN address).
A pass in progress (real or test) can be stopped from the Passes tab or Settings. sdrd then predicts passes for 48 hours ahead with SGP4. Orbit data (TLE) comes
from CelesTrak and is refreshed every 12 h; it can also be pasted in by hand for offline
setups. Predictions were cross-checked against skyfield: AOS/LOS within a second, look
angles and range-rate matching to 0.001.

During a pass of an enabled satellite (ISS by default, above 10°):

1. From 30 s before AOS the radio is tuned to the satellite's profile (default the VHF
   block, which covers ISS SSTV 145.800 and ISS APRS 145.825).
2. The Doppler shift is published to the decoders, so channels marked *Doppler* track it.
3. The downlink (default 145.800 MHz FM) is recorded to WAV, Doppler-corrected, in
   `/data/passes`. Recordings are kept until deleted.
4. At LOS the previous tuning is restored, or rotation carries on.

The **Passes** tab has:

- a live sky plot with azimuth, elevation, range and Doppler during a pass
- the next passes with their tracks
- recordings with an audio player, plus any SSTV images received during each pass

For ARISS events on 437.550 MHz, change the ISS tune and recording frequencies to 437.6 and
437.550 MHz.

**Rotation** (off by default) cycles through profiles when no pass is on, for example VHF
block 60 s then ISM 30 s. It never hops away while an SSTV image is coming in (up to 5 min).
Admins can run a **test pass** to check tuning and recording end to end.

## MQTT / Home Assistant

Set `SDRD_MQTT_HOST` (and user/password if needed) in `.env`. sdrd publishes:

| Topic | Content |
|---|---|
| `sdrd/status` | `online` / `offline` (retained, last will) |
| `sdrd/events/rtl_433`, `…/aprs`, `…/sstv` | every decoded event as JSON (turn off with `SDRD_MQTT_EVENTS=false`) |
| `sdrd/devices/<device>/state` | latest data of tracked devices (retained) |
| `homeassistant/sensor/…/config` | HA discovery for tracked devices: temperature, humidity, pressure, battery, signal, … |
| `homeassistant/device_tracker/…/config` | HA device tracker for tracked APRS stations (shows them on the HA map) |

Only tracked devices create Home Assistant entities, so passing cars don't flood HA.
Discovery is re-sent when Home Assistant restarts.

### User management from the shell

```bash
docker compose exec -T sdrd sdrd user add alice viewer <<< 'a-long-password'
docker compose exec -T sdrd sdrd user passwd admin <<< 'new-long-password'
```

## Configuration

All settings are `SDRD_*` environment variables. See [.env.example](.env.example). Notable ones:

| Variable | Default | Meaning |
|---|---|---|
| `SDRD_SOURCE` | `rtlsdr` | `rtlsdr`, `synthetic`, or `replay` (with `SDRD_REPLAY_FILE=/data/x.cu8`) |
| `SDRD_CENTER_HZ` | `145300000` | Startup centre frequency |
| `SDRD_SAMPLE_RATE` | `2048000` | Startup sample rate |
| `SDRD_GAIN` | `auto` | `auto` or dB |
| `SDRD_FFT_SIZE` / `SDRD_FPS` | `2048` / `20` | Waterfall resolution and rate |
| `SDRD_COOKIE_SECURE` | `true` | Must stay `true` when served over HTTPS |
| `SDRD_TRUST_PROXY` | `false` | Trust `X-Forwarded-*` headers (set by compose behind Caddy) |
| `SDRD_MQTT_HOST` | (off) | MQTT broker; also `_PORT`, `_USER`, `_PASSWORD`, `_PREFIX`, `_DISCOVERY_PREFIX` |
| `SDRD_EVENT_RETENTION_DAYS` | `30` | Delete older events and untracked devices |
| `SDRD_ISM_VIEWERS` | `false` | Let viewer accounts see sensors / TPMS |
| `SDRD_AUDIO_MAX_LISTENERS` | `4` | Concurrent live-audio listeners (0 turns live audio off) |
| `SDRD_AUDIO_VIEWERS` | `false` | Let viewer accounts listen to live audio |
| `SDRD_MAP_TILE_URL` | OpenStreetMap | Map tile URL template with `{z}`, `{x}`, `{y}` (and optional `{s}`) |

## Architecture

```
RTL-SDR ─USB─► radio thread (librtlsdr) ─► IQ broadcast ─┬► waterfall FFT ─────────────────► WebSocket ─► browser
                    ▲ retune                              ├► DDC 433.92 → rtl_433 ──────────┐
               REST /api/radio  ◄── admin UI             ├► DDC 144.800 → FM → Direwolf ───┼► events ─► SQLite, WebSocket, MQTT
                                                          ├► DDC 145.825 → FM → Direwolf ───┤
                                                          ├► DDC 145.800 → FM → SSTV ───────┘─► PNG in /data/sstv
                                                          │      (tone demod, VIS, sync tracking)   + live rows ─► WebSocket
                                                          └► recorder (FM, Doppler) ─► WAV in /data/passes
  scheduler ── TLE (CelesTrak) + SGP4 ─► passes ─► retune radio, Doppler ─► decoders/recorder
```

- `crates/sdrd`: Rust daemon (axum, tokio, rustfft, sqlx/SQLite)
- `web`: Svelte 5 + Vite single-page app

## Legal note

Receiving amateur radio (APRS, ISS SSTV) is fine to share. In the UK the Wireless Telegraphy
Act restricts disclosing the content of other transmissions you weren't meant to receive.
TPMS and ISM data from other people's devices falls under this. Keep those views to trusted
users if the site is public.
