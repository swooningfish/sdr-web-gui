/** Signal strength for the S-meter, read from the spectrum frames so it
 * works whether or not audio is playing. */

import type { WaterfallFrame } from './stream';

/**
 * Channel power (dBFS) of `bw` Hz around `hz`, summed from the spectrum bins
 * (a full-scale tone reads ~0 dBFS, as for the audio level). Null when the
 * channel isn't inside the frame.
 */
export function channelDbfs(f: WaterfallFrame, lo: number, hi: number): number | null {
  const n = f.bins.length;
  if (!n) return null;
  const binHz = f.sampleRate / n;
  const start = f.centerHz - f.sampleRate / 2;
  let a = Math.floor((lo - start) / binHz);
  let b = Math.ceil((hi - start) / binHz);
  if (b < 0 || a >= n) return null;
  a = Math.max(0, a);
  b = Math.min(n - 1, Math.max(a, b));
  const span = f.maxDb - f.minDb;
  let sum = 0;
  for (let i = a; i <= b; i++) sum += 10 ** ((f.minDb + (f.bins[i] / 255) * span) / 10);
  return 10 * Math.log10(sum);
}

/**
 * Rough dBm at the antenna: an RTL-SDR's full scale is about −10 dBm at 0 dB
 * gain, less the tuner gain (AGC counted as 30 dB). `trim` is the user's
 * calibration.
 */
export function toDbm(dbfs: number, gainDb: number | null, trim: number): number {
  return dbfs - (gainDb ?? 30) - 10 + trim;
}

/** dBm at S9: −73 on HF (and in most receivers' meters), −93 above 30 MHz per IARU. */
export function s9Dbm(vhfScale: boolean, hz: number): number {
  return vhfScale && hz >= 30e6 ? -93 : -73;
}

/** "S7", "S9 +12 dB". */
export function sReading(dbm: number, s9: number): string {
  if (dbm >= s9 + 0.5) return `S9 +${Math.round(dbm - s9)} dB`;
  const s = Math.max(0, Math.min(9, Math.round(9 + (dbm - s9) / 6)));
  return `S${s}`;
}

/** Needle position 0..1: S0..S9 over the first 60 %, S9..+60 dB over the rest. */
export function needle(dbm: number, s9: number): number {
  const over = dbm - s9;
  if (over <= 0) return Math.max(0, 0.6 * (1 + over / 54));
  return Math.min(1, 0.6 + 0.4 * (over / 60));
}
