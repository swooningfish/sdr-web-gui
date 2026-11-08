#!/usr/bin/env bash
# One-time host setup for running sdr-web-gui on a Raspberry Pi (or any Linux box).
#  - stops the kernel DVB-T driver from grabbing the RTL-SDR
#  - lets non-root containers open the dongle
#  - installs Docker if missing
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "run with sudo: sudo $0" >&2
  exit 1
fi

echo "==> Blacklisting DVB-T kernel drivers"
cat > /etc/modprobe.d/blacklist-rtlsdr.conf <<'EOF'
blacklist dvb_usb_rtl28xxu
blacklist rtl2832
blacklist rtl2830
blacklist rtl2832_sdr
EOF
for m in rtl2832_sdr dvb_usb_rtl28xxu rtl2832 rtl2830; do
  modprobe -r "$m" 2>/dev/null || true
done

echo "==> Installing udev rule for RTL2832U dongles"
cat > /etc/udev/rules.d/60-rtl-sdr.rules <<'EOF'
# Generic RTL2832U / RTL-SDR Blog V3 / V4
SUBSYSTEM=="usb", ATTRS{idVendor}=="0bda", ATTRS{idProduct}=="2838", MODE="0666"
SUBSYSTEM=="usb", ATTRS{idVendor}=="0bda", ATTRS{idProduct}=="2832", MODE="0666"
EOF
udevadm control --reload-rules
udevadm trigger

if ! command -v docker >/dev/null 2>&1; then
  echo "==> Installing Docker"
  curl -fsSL https://get.docker.com | sh
  if [[ -n "${SUDO_USER:-}" ]]; then
    usermod -aG docker "$SUDO_USER"
    echo "    added $SUDO_USER to the docker group (log out and back in)"
  fi
fi

echo "==> Done. Re-plug the dongle, then check it is visible with: lsusb | grep -i 0bda"
