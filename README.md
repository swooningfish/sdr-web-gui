# sdr-web-gui

A self-hosted, browser-controlled RTL-SDR monitor for a Raspberry Pi, packaged in Docker.
It's inspired by [OnAir](https://github.com/adswill/OnAir) but focuses on unattended
monitoring: TPMS/ISM sensors (rtl_433), APRS, and SSTV, including automatic ISS/ARISS pass recording.

See [SPEC.md](SPEC.md) for the full design and roadmap.

## Status: milestone 1 (skeleton)

- [x] Rust core (`sdrd`) owns the dongle via librtlsdr (rtl-sdr-blog fork: V4, V3, generic)
- [x] Live spectrum and waterfall in the browser over WebSocket
- [x] Tuning, gain, sample rate, PPM, bias-T and direct sampling from the UI
- [x] Login with admin and viewer roles, Argon2id, secure sessions, CSRF protection, login rate limiting
- [x] Synthetic and replay IQ sources for development without hardware
- [x] Docker image plus Caddy (automatic HTTPS) compose setup for the Pi
- [ ] M2: rtl_433 / TPMS with a flex-decoder editor, MQTT and Home Assistant
- [ ] M3: APRS (Direwolf) with a map
- [ ] M4: native SSTV decoder and image gallery
- [ ] M5: scheduler and ISS passes with Doppler correction
- [ ] M6: live audio

## Quick start: development (no dongle needed)

```bash
docker compose -f docker-compose.dev.yml up --build
```

Open <http://localhost:8080> and sign in as `admin` / `devpassword123`. The synthetic source
puts test signals on 144.800 (APRS-like bursts), 145.800, 145.825, 433.920 (OOK bursts) and
437.550 MHz (Doppler-drifting carrier). Use the presets to see them.

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

## Architecture

```
RTL-SDR ─USB─► radio thread (librtlsdr) ─► IQ broadcast ─► waterfall FFT ─► WebSocket ─► browser
                    ▲ retune                         └──► (M2+) rtl_433, Direwolf, SSTV …
               REST /api/radio  ◄── admin UI
```

- `crates/sdrd`: Rust daemon (axum, tokio, rustfft, sqlx/SQLite)
- `web`: Svelte 5 + Vite single-page app

## Legal note

Receiving amateur radio (APRS, ISS SSTV) is fine to share. In the UK the Wireless Telegraphy
Act restricts disclosing the content of other transmissions you weren't meant to receive.
TPMS and ISM data from other people's devices falls under this. Keep those views to trusted
users if the site is public.
