# syntax=docker/dockerfile:1.7
#
# Multi-stage build for sdrd + web UI. Builds natively for the host arch, so
# run `docker compose build` on the Pi 5 itself (arm64) – cross-building Rust
# under QEMU works but is very slow.

# rtl-sdr-blog fork of librtlsdr: supports RTL-SDR Blog V4 (and V3 / generic
# RTL2832U). Pin to a tag or commit for reproducible builds.
ARG RTLSDR_BLOG_REF=master
# rtl_433 release. Built without SDR backends: sdrd feeds it IQ over a pipe.
ARG RTL433_REF=25.12
# DSD-neo (digital voice: DMR, D-STAR, YSF, NXDN, P25, M17 ...) and the vocoder
# library it requires. Its CLI still changes between releases: keep these pinned.
ARG MBELIB_NEO_REF=v2.6.7
ARG DSD_NEO_REF=v2.12.1
# ffmpeg, cut down to decoding DAB audio (DAB+ AAC as LOAS/LATM, MP2) to PCM.
# The distro package would add ~480 MB; this is a few MB, without libraries.
ARG FFMPEG_REF=n7.1

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

# ---------------------------------------------------------------- rtl_433
FROM debian:bookworm-slim AS rtl433
ARG RTL433_REF
RUN apt-get update  && apt-get install -y --no-install-recommends git ca-certificates cmake build-essential pkg-config  && rm -rf /var/lib/apt/lists/*
RUN git clone --depth 1 --branch "$RTL433_REF" https://github.com/merbanan/rtl_433 /src/rtl_433  && cmake -S /src/rtl_433 -B /src/build       -DCMAKE_BUILD_TYPE=Release       -DENABLE_RTLSDR=OFF       -DENABLE_SOAPYSDR=OFF       -DENABLE_OPENSSL=OFF       -DBUILD_TESTING=OFF       -DBUILD_DOCUMENTATION=OFF  && cmake --build /src/build -j"$(nproc)"  && install -m 0755 /src/build/src/rtl_433 /usr/local/bin/rtl_433

# ---------------------------------------------------------------- dsd-neo
# Headless (no audio backend, terminal UI or SDR input): sdrd pipes it the FM
# discriminator on stdin and reads its JSON telemetry API.
FROM debian:bookworm-slim AS dsd
ARG MBELIB_NEO_REF
ARG DSD_NEO_REF
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates cmake build-essential pkg-config \
      libsndfile1-dev libssl-dev \
 && rm -rf /var/lib/apt/lists/*
RUN git clone --depth 1 --branch "$MBELIB_NEO_REF" https://github.com/arancormonk/mbelib-neo /src/mbelib-neo \
 && cmake -S /src/mbelib-neo -B /src/mbelib-neo/build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/opt/dsd \
 && cmake --build /src/mbelib-neo/build -j"$(nproc)" \
 && cmake --install /src/mbelib-neo/build
# GCC 12 (bookworm) raises a false stringop-overflow warning that the project
# builds with -Werror; tests are skipped.
RUN git clone --depth 1 --branch "$DSD_NEO_REF" https://github.com/arancormonk/dsd-neo /src/dsd-neo \
 && cmake -S /src/dsd-neo -B /src/dsd-neo/build -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_PREFIX_PATH=/opt/dsd -DCMAKE_INSTALL_PREFIX=/opt/dsd \
      -DCMAKE_C_FLAGS="-Wno-stringop-overflow -Wno-error=stringop-overflow" \
      -DBUILD_TESTING=OFF -DDSD_AUDIO_BACKEND=none -DDSD_ENABLE_TERMINAL_UI=OFF \
      -DDSD_ENABLE_RTLSDR=OFF -DDSD_ENABLE_SOAPYSDR=OFF -DDSD_ENABLE_NATIVE=OFF \
      -DCOLORS=OFF -DCOLORSLOGS=OFF -DCMAKE_DISABLE_FIND_PACKAGE_CODEC2=ON \
 && cmake --build /src/dsd-neo/build -j"$(nproc)" \
 && cmake --install /src/dsd-neo/build

# ---------------------------------------------------------------- ffmpeg
# Just what DAB listening needs; LGPL, run as a separate process by sdrd.
FROM debian:bookworm-slim AS ffmpeg
ARG FFMPEG_REF
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates build-essential pkg-config \
 && rm -rf /var/lib/apt/lists/*
RUN git clone --depth 1 --branch "$FFMPEG_REF" https://github.com/FFmpeg/FFmpeg /src/ffmpeg \
 && cd /src/ffmpeg \
 && ./configure --disable-everything --disable-autodetect --disable-asm --disable-doc \
      --disable-debug --disable-network --disable-ffplay --disable-ffprobe --disable-avdevice \
      --disable-swscale --enable-small \
      --enable-decoder=aac,aac_latm,mp2,mp2float --enable-demuxer=loas,mp3 \
      --enable-parser=aac_latm,mpegaudio --enable-muxer=pcm_s16le --enable-encoder=pcm_s16le \
      --enable-protocol=pipe --enable-filter=aresample,aformat,anull \
 && make -j"$(nproc)" ffmpeg \
 && strip ffmpeg && install -m 0755 ffmpeg /usr/local/bin/ffmpeg

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
 && apt-get install -y --no-install-recommends libusb-1.0-0 ca-certificates tini direwolf \
      libsndfile1 libssl3 libstdc++6 \
 && rm -rf /var/lib/apt/lists/*
# librtlsdr + rtl_test / rtl_sdr etc. for diagnostics
COPY --from=rtlsdr /opt/rtlsdr/ /usr/local/
# the vocoder library dsd-neo links against
COPY --from=dsd /opt/dsd/lib/libmbe-neo.so* /usr/local/lib/
RUN ldconfig \
 && useradd --system --uid 10001 --user-group --home-dir /data --shell /usr/sbin/nologin sdrd \
 && mkdir -p /data /app \
 && chown sdrd:sdrd /data
COPY --from=rtl433 /usr/local/bin/rtl_433 /usr/local/bin/rtl_433
COPY --from=dsd /opt/dsd/bin/dsd-neo /usr/local/bin/dsd-neo
COPY --from=ffmpeg /usr/local/bin/ffmpeg /usr/local/bin/ffmpeg
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
