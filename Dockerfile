# syntax=docker/dockerfile:1.7
#
# Multi-stage build for sdrd + web UI. Builds natively for the host arch, so
# run `docker compose build` on the Pi 5 itself (arm64) – cross-building Rust
# under QEMU works but is very slow.

# rtl-sdr-blog fork of librtlsdr: supports RTL-SDR Blog V4 (and V3 / generic
# RTL2832U). Pin to a tag or commit for reproducible builds.
ARG RTLSDR_BLOG_REF=master

# ---------------------------------------------------------------- web UI
FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# ---------------------------------------------------------------- librtlsdr
FROM rust:1-bookworm AS rtlsdr
ARG RTLSDR_BLOG_REF
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake libusb-1.0-0-dev pkg-config \
 && rm -rf /var/lib/apt/lists/*
RUN git clone https://github.com/rtlsdrblog/rtl-sdr-blog /src/rtl-sdr \
 && cd /src/rtl-sdr \
 && git checkout "$RTLSDR_BLOG_REF" \
 && cmake -S . -B build \
      -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_INSTALL_PREFIX=/opt/rtlsdr \
      -DCMAKE_INSTALL_LIBDIR=lib \
      -DINSTALL_UDEV_RULES=OFF \
      -DDETACH_KERNEL_DRIVER=ON \
 && cmake --build build -j"$(nproc)" \
 && cmake --install build

# ---------------------------------------------------------------- sdrd
FROM rtlsdr AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
ENV RUSTFLAGS="-L native=/opt/rtlsdr/lib"
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --features rtlsdr \
 && cp target/release/sdrd /sdrd

# ---------------------------------------------------------------- runtime
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends libusb-1.0-0 ca-certificates tini \
 && rm -rf /var/lib/apt/lists/*
# librtlsdr + rtl_test / rtl_sdr etc. for diagnostics
COPY --from=rtlsdr /opt/rtlsdr/ /usr/local/
RUN ldconfig \
 && useradd --system --uid 10001 --user-group --home-dir /data --shell /usr/sbin/nologin sdrd \
 && mkdir -p /data /app \
 && chown sdrd:sdrd /data
COPY --from=build /sdrd /usr/local/bin/sdrd
COPY --from=web /web/dist /app/web

ENV SDRD_BIND=0.0.0.0:8080 \
    SDRD_DATA_DIR=/data \
    SDRD_WEB_DIR=/app/web \
    RUST_LOG=sdrd=info,tower_http=warn

USER sdrd
EXPOSE 8080
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["sdrd", "healthcheck"]
ENTRYPOINT ["/usr/bin/tini", "--", "sdrd"]
