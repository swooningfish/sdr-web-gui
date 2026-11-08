# sdr-web-gui

A self-hosted, browser-controlled RTL-SDR monitor for a Raspberry Pi, packaged in Docker.
It's inspired by [OnAir](https://github.com/adswill/OnAir) but focuses on unattended
monitoring: TPMS/ISM sensors (rtl_433), APRS, and SSTV, including automatic ISS/ARISS pass recording.

See [SPEC.md](SPEC.md) for the full design and roadmap.

## Status: milestone 2 (ISM / TPMS)

- [x] Rust core (`sdrd`) owns the dongle via librtlsdr (rtl-sdr-blog fork: V4, V3, generic)
- [x] Live spectrum and waterfall in the browser over WebSocket
- [x] Tuning, gain, sample rate, PPM, bias-T and direct sampling from the UI
- [x] Login with admin and viewer roles, Argon2id, secure sessions, CSRF protection, login rate limiting
- [x] Synthetic and replay IQ sources for development without hardware
- [x] Docker image plus Caddy (automatic HTTPS) compose setup for the Pi
- [x] M2: rtl_433 / TPMS with a flex-decoder editor, sensor history charts, MQTT and Home Assistant
- [ ] M3: APRS (Direwolf) with a map
- [ ] M4: native SSTV decoder and image gallery
- [ ] M5: scheduler and ISS passes with Doppler correction
- [ ] M6: live audio

## Quick start: development (no dongle needed)

```bash
docker compose -f docker-compose.dev.yml up --build
```

Open <http://localhost:8080> and sign in as `admin` / `devpassword123`. The synthetic source
puts test signals on 144.800 (APRS-like bursts), 145.800, 145.825, 433.920 and 437.550 MHz
(Doppler-drifting carrier). Use the presets to see them. On 433.92 it simulates two Nexus
temperature/humidity sensors that rtl_433 really decodes: pick the ISM preset (or press
*Tune* on the Sensors page) and they appear within ~15 s.

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

### MQTT / Home Assistant

Set `SDRD_MQTT_HOST` (and user/password if needed) in `.env`. sdrd publishes:

| Topic | Content |
|---|---|
| `sdrd/status` | `online` / `offline` (retained, last will) |
| `sdrd/events/rtl_433` | every decoded event as JSON (turn off with `SDRD_MQTT_EVENTS=false`) |
| `sdrd/devices/<device>/state` | latest data of tracked devices (retained) |
| `homeassistant/sensor/…/config` | HA discovery for tracked devices: temperature, humidity, pressure, battery, signal, … |

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

## Architecture

```
RTL-SDR ─USB─► radio thread (librtlsdr) ─► IQ broadcast ─┬► waterfall FFT ──────────► WebSocket ─► browser
                    ▲ retune                              └► DDC 433.92 → rtl_433 ─► events ─► SQLite, WebSocket, MQTT
               REST /api/radio  ◄── admin UI                 (M3+) Direwolf, SSTV …
```

- `crates/sdrd`: Rust daemon (axum, tokio, rustfft, sqlx/SQLite)
- `web`: Svelte 5 + Vite single-page app

## Legal note

Receiving amateur radio (APRS, ISS SSTV) is fine to share. In the UK the Wireless Telegraphy
Act restricts disclosing the content of other transmissions you weren't meant to receive.
TPMS and ISM data from other people's devices falls under this. Keep those views to trusted
users if the site is public.
