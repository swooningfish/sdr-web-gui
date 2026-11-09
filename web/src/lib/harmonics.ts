// Harmonics check for a short-range device (SRD): measure the carrier and its
// harmonics from waterfall frames, then compare with Ofcom (UK, IR 2030 via
// ETSI EN 300 220-1) and FCC (47 CFR Part 15) limits.
//
// The receiver isn't calibrated, so everything is relative: harmonic power in
// dB below the carrier (dBc). The rules set absolute limits, so each one is
// turned into a dBc limit by assuming the carrier is at the legal maximum
// (less any `backoffDb` the user says it runs below that).

import type { FreqRange } from './api';

export type Regime = 'ofcom' | 'fcc';
export type Rating = 'good' | 'medium' | 'bad' | 'unknown';

/** Margin (dB) a harmonic needs to be "good": RTL-SDR and antenna response
 * vary by several dB across frequency, so less than this is too close to call. */
export const GOOD_MARGIN_DB = 6;

const MHZ = 1e6;

// ------------------------------------------------------------------ Ofcom

export interface OfcomBand {
  lo: number;
  hi: number;
  erpMw: number;
  label: string;
}

/** UK licence-exempt SRD bands (IR 2030 / ERC Rec 70-03 Annex 1), max ERP. */
export const OFCOM_BANDS: OfcomBand[] = [
  { lo: 26.957 * MHZ, hi: 27.283 * MHZ, erpMw: 10, label: '26.957–27.283 MHz' },
  { lo: 40.66 * MHZ, hi: 40.7 * MHZ, erpMw: 10, label: '40.66–40.70 MHz' },
  { lo: 138.2 * MHZ, hi: 138.45 * MHZ, erpMw: 10, label: '138.20–138.45 MHz' },
  { lo: 169.4 * MHZ, hi: 169.475 * MHZ, erpMw: 500, label: '169.400–169.475 MHz' },
  { lo: 433.05 * MHZ, hi: 434.79 * MHZ, erpMw: 10, label: '433.05–434.79 MHz' },
  { lo: 458.5 * MHZ, hi: 458.95 * MHZ, erpMw: 500, label: '458.5–458.95 MHz (UK only)' },
  { lo: 863 * MHZ, hi: 868 * MHZ, erpMw: 25, label: '863–868 MHz' },
  { lo: 868 * MHZ, hi: 868.6 * MHZ, erpMw: 25, label: '868.0–868.6 MHz' },
  { lo: 868.7 * MHZ, hi: 869.2 * MHZ, erpMw: 25, label: '868.7–869.2 MHz' },
  { lo: 869.4 * MHZ, hi: 869.65 * MHZ, erpMw: 500, label: '869.40–869.65 MHz' },
  { lo: 869.7 * MHZ, hi: 870 * MHZ, erpMw: 25, label: '869.7–870.0 MHz' },
];

export function ofcomBand(hz: number): OfcomBand | null {
  return OFCOM_BANDS.find((b) => hz >= b.lo && hz <= b.hi) ?? null;
}

/** Bands with the tighter spurious limit (EN 300 220-1 V3.1.1 Table 19). */
const ETSI_TIGHT: [number, number][] = [
  [47, 74],
  [87.5, 118],
  [174, 230],
  [470, 790],
];

/** EN 300 220-1 Table 19, TX mode: spurious limit at `hz` (dBm ERP) and why. */
export function ofcomLimit(hz: number): { dbm: number; why: string } {
  const mhz = hz / MHZ;
  const tight = ETSI_TIGHT.find(([a, b]) => mhz >= a && mhz <= b);
  if (tight) return { dbm: -54, why: `−54 dBm in the ${tight[0]}–${tight[1]} MHz broadcast band` };
  if (mhz <= 1000) return { dbm: -36, why: '−36 dBm below 1 GHz' };
  return { dbm: -30, why: '−30 dBm above 1 GHz' };
}

// -------------------------------------------------------------------- FCC

/** §15.205 restricted bands (MHz), 8 MHz to 2.5 GHz. */
const RESTRICTED: [number, number][] = [
  [8.291, 8.294], [8.362, 8.366], [8.37625, 8.38675], [8.41425, 8.41475], [12.29, 12.293],
  [12.51975, 12.52025], [12.57675, 12.57725], [13.36, 13.41], [16.42, 16.423], [16.69475, 16.69525],
  [16.80425, 16.80475], [25.5, 25.67], [37.5, 38.25], [73, 74.6], [74.8, 75.2], [108, 121.94], [123, 138],
  [149.9, 150.05], [156.52475, 156.52525], [156.7, 156.9], [162.0125, 167.17], [167.72, 173.2], [240, 285],
  [322, 335.4], [399.9, 410], [608, 614], [960, 1240], [1300, 1427], [1435, 1626.5], [1645.5, 1646.5],
  [1660, 1710], [1718.8, 1722.2], [2200, 2300], [2310, 2390], [2483.5, 2500],
];

export function restrictedBand(hz: number): [number, number] | null {
  const mhz = hz / MHZ;
  return RESTRICTED.find(([a, b]) => mhz >= a && mhz <= b) ?? null;
}

/** §15.209(a) general limit at 3 m (µV/m), 30 MHz and up. */
export function fcc209(hz: number): number {
  const mhz = hz / MHZ;
  if (mhz <= 88) return 100;
  if (mhz <= 216) return 150;
  if (mhz <= 960) return 200;
  return 500;
}

const lerp = (x: number, x0: number, x1: number, y0: number, y1: number) => y0 + ((x - x0) / (x1 - x0)) * (y1 - y0);

export type FccRule =
  | { kind: 'rule'; section: string; label: string; fundUvm: number; harmonicUvm: (hz: number) => { uvm: number; why: string } }
  | { kind: 'none'; why: string };

/** Which Part 15 section a carrier at `hz` would operate under. */
export function fccRule(hz: number): FccRule {
  const mhz = hz / MHZ;
  const r = restrictedBand(hz);
  if (r) return { kind: 'none', why: `${mhz.toFixed(3)} MHz is in a §15.205 restricted band (${r[0]}–${r[1]} MHz): no intentional emissions allowed` };
  const only209 = (h: number) => {
    const rb = restrictedBand(h);
    return { uvm: fcc209(h), why: rb ? `§15.209 (restricted band ${rb[0]}–${rb[1]} MHz)` : '§15.209 general limit' };
  };
  if (mhz >= 26.96 && mhz <= 27.28) {
    return { kind: 'rule', section: '§15.227', label: '26.96–27.28 MHz', fundUvm: 10_000, harmonicUvm: only209 };
  }
  if (mhz >= 902 && mhz <= 928) {
    return {
      kind: 'rule',
      section: '§15.249',
      label: '902–928 MHz',
      fundUvm: 50_000,
      harmonicUvm: (h) => {
        const rb = restrictedBand(h);
        if (rb) return { uvm: Math.min(500, fcc209(h)), why: `§15.205: restricted band ${rb[0]}–${rb[1]} MHz` };
        return { uvm: 500, why: '§15.249(a) harmonic limit' };
      },
    };
  }
  // §15.231(b): periodic / momentary control transmitters.
  let fund: number | null = null;
  if (mhz >= 40.66 && mhz <= 40.7) fund = 2_250;
  else if (mhz >= 70 && mhz <= 130) fund = 1_250;
  else if (mhz > 130 && mhz <= 174) fund = lerp(mhz, 130, 174, 1_250, 3_750);
  else if (mhz > 174 && mhz <= 260) fund = 3_750;
  else if (mhz > 260 && mhz <= 470) fund = lerp(mhz, 260, 470, 3_750, 12_500);
  else if (mhz > 470) fund = 12_500;
  if (fund === null) return { kind: 'none', why: `no Part 15 band for SRDs at ${mhz.toFixed(3)} MHz` };
  const f = fund;
  return {
    kind: 'rule',
    section: '§15.231(b)',
    label: 'periodic / control transmitters',
    fundUvm: f,
    harmonicUvm: (h) => {
      const rb = restrictedBand(h);
      if (rb) return { uvm: fcc209(h), why: `§15.205: restricted band ${rb[0]}–${rb[1]} MHz, §15.209 applies` };
      const tenth = f / 10;
      const g = fcc209(h);
      return tenth >= g
        ? { uvm: tenth, why: '§15.231(b): 1/10 of the fundamental limit' }
        : { uvm: g, why: '§15.209 general limit (higher than 1/10 of the fundamental)' };
    },
  };
}

// ---------------------------------------------------------- limits in dBc

export interface Limit {
  /** Highest allowed harmonic level relative to the carrier, dBc. */
  dbc: number;
  why: string;
}

export function ofcomDbc(fundHz: number, harmHz: number, backoffDb: number): Limit {
  const erpMw = ofcomBand(fundHz)?.erpMw ?? 10;
  const carrierDbm = 10 * Math.log10(erpMw) - backoffDb;
  const lim = ofcomLimit(harmHz);
  return { dbc: lim.dbm - carrierDbm, why: `${lim.why} (EN 300 220-1 Table 19) vs a ${fmtDbm(carrierDbm)} carrier` };
}

export function fccDbc(fundHz: number, harmHz: number, backoffDb: number): Limit | null {
  const rule = fccRule(fundHz);
  if (rule.kind === 'none') return null;
  const h = rule.harmonicUvm(harmHz);
  return {
    dbc: 20 * Math.log10(h.uvm / rule.fundUvm) + backoffDb,
    why: `${h.why}: ${fmtUvm(h.uvm)} vs the ${fmtUvm(rule.fundUvm)} fundamental limit`,
  };
}

function fmtDbm(dbm: number): string {
  const mw = 10 ** (dbm / 10);
  return `${dbm >= 0 ? '+' : '−'}${Math.abs(dbm).toFixed(1)} dBm (${mw >= 1 ? mw.toFixed(mw < 10 ? 1 : 0) : mw.toFixed(2)} mW ERP)`;
}

function fmtUvm(uvm: number): string {
  return `${Math.round(uvm).toLocaleString('en-GB')} µV/m`;
}

// ------------------------------------------------------------ measurement

/** A waterfall frame as captured (bins copied, they're reused). */
export interface Captured {
  bins: Uint8Array;
  sampleRate: number;
  centerHz: number;
  minDb: number;
  maxDb: number;
}

export interface Measurement {
  /** Window measured (Hz). */
  lo: number;
  hi: number;
  frames: number;
  /** Strongest signal power in the window (noise removed), dBFS; null if not seen. */
  powerDb: number | null;
  /** Noise power in the window, dBFS. */
  noiseDb: number;
  /** If not seen: it's at most this (dBFS). */
  ceilingDb: number;
  /** Highest single bin in the window, dBFS. */
  peakBinDb: number;
  /** Share of frames the signal was on (≥ the detection threshold). */
  duty: number;
}

/** Signal must reach this far below the window's noise power to count as seen. */
const DETECT_DB = -6;
/** Frames averaged when looking for the strongest moment (≈ 0.2–0.3 s). */
const RUN = 5;

function frameDb(f: Captured, i: number): number {
  return f.minDb + (f.bins[i] * (f.maxDb - f.minDb)) / 255;
}

function binOf(f: Captured, hz: number): number {
  return ((hz - (f.centerHz - f.sampleRate / 2)) / f.sampleRate) * f.bins.length;
}

function median(xs: number[]): number {
  if (!xs.length) return NaN;
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}

const lin = (db: number) => 10 ** (db / 10);
const dbOf = (p: number) => 10 * Math.log10(Math.max(p, 1e-30));

/**
 * Power in [lo, hi] for each frame, with the noise judged from bins either
 * side of the window (as wide as the window, a small gap away).
 */
export function measure(frames: Captured[], lo: number, hi: number): Measurement {
  const sig: number[] = [];
  const noise: number[] = [];
  let peakBinDb = -Infinity;
  for (const f of frames) {
    const n = f.bins.length;
    const b0 = Math.max(0, Math.floor(binOf(f, lo)));
    const b1 = Math.min(n - 1, Math.ceil(binOf(f, hi)));
    if (b1 < b0) continue;
    const w = b1 - b0 + 1;
    const gap = Math.max(2, Math.round(w * 0.15));
    const guard: number[] = [];
    for (let i = b0 - gap - w; i < b0 - gap; i++) if (i >= 0) guard.push(frameDb(f, i));
    for (let i = b1 + gap + 1; i <= b1 + gap + w; i++) if (i < n) guard.push(frameDb(f, i));
    // Narrow window: widen the noise sample so the median is steady.
    if (guard.length < 16) for (let i = 0; i < n; i += Math.max(1, Math.floor(n / 64))) guard.push(frameDb(f, i));
    const noisePerBin = lin(median(guard));
    let p = 0;
    for (let i = b0; i <= b1; i++) {
      const db = frameDb(f, i);
      if (db > peakBinDb) peakBinDb = db;
      p += lin(db);
    }
    noise.push(noisePerBin * w);
    sig.push(p - noisePerBin * w);
  }
  if (!sig.length) return { lo, hi, frames: 0, powerDb: null, noiseDb: NaN, ceilingDb: NaN, peakBinDb: NaN, duty: 0 };

  // Strongest moment: best running mean, which steadies the noise.
  const k = Math.min(RUN, sig.length);
  let best = -Infinity;
  let bestNoise = noise[0];
  for (let i = 0; i + k <= sig.length; i++) {
    let s = 0;
    let nn = 0;
    for (let j = i; j < i + k; j++) {
      s += sig[j];
      nn += noise[j];
    }
    if (s / k > best) {
      best = s / k;
      bestNoise = nn / k;
    }
  }
  const noiseP = noise.reduce((a, b) => a + b, 0) / noise.length;
  const thresh = lin(DETECT_DB);
  const seen = best >= bestNoise * thresh;
  const on = sig.filter((s, i) => s >= noise[i] * thresh * 2).length;
  return {
    lo,
    hi,
    frames: sig.length,
    powerDb: seen ? dbOf(best) : null,
    noiseDb: dbOf(noiseP),
    ceilingDb: dbOf(noiseP * thresh),
    peakBinDb,
    duty: on / sig.length,
  };
}

/** The carrier: strongest signal within ±`searchHz` of `hz`, and how wide it is. */
export function findCarrier(
  frames: Captured[],
  hz: number,
  searchHz: number,
): { hz: number; halfWidth: number; peakDb: number } | null {
  if (!frames.length) return null;
  const f0 = frames[0];
  const n = f0.bins.length;
  const hold = new Float32Array(n).fill(-Infinity);
  for (const f of frames) {
    if (f.bins.length !== n || f.centerHz !== f0.centerHz) continue;
    for (let i = 0; i < n; i++) hold[i] = Math.max(hold[i], frameDb(f, i));
  }
  const binHz = f0.sampleRate / n;
  const b0 = Math.max(0, Math.floor(binOf(f0, hz - searchHz)));
  const b1 = Math.min(n - 1, Math.ceil(binOf(f0, hz + searchHz)));
  let pk = -1;
  for (let i = b0; i <= b1; i++) if (pk < 0 || hold[i] > hold[pk]) pk = i;
  if (pk < 0) return null;
  const floor = median(Array.from(hold));
  if (hold[pk] < floor + 15) return null;
  // Occupied: out from the peak while clearly above the floor (2-bin gaps ok).
  const above = (i: number) => i >= 0 && i < n && hold[i] > floor + 10;
  let l = pk;
  while (above(l - 1) || above(l - 2) || above(l - 3)) l--;
  let r = pk;
  while (above(r + 1) || above(r + 2) || above(r + 3)) r++;
  const lo = (l - n / 2) * binHz + f0.centerHz;
  const hi = (r + 1 - n / 2) * binHz + f0.centerHz;
  return {
    hz: (lo + hi) / 2,
    halfWidth: Math.max(5_000, ((hi - lo) / 2) * 1.25 + 2 * binHz),
    peakDb: hold[pk],
  };
}

// ---------------------------------------------------------------- results

export interface Verdict {
  rating: Rating;
  /** Limit minus measured (dB): positive passes. For unseen harmonics, at least this. */
  marginDb: number | null;
  limit: Limit | null;
  text: string;
}

export interface HarmonicResult {
  n: number;
  hz: number;
  /** Measured level, dBc; null if not seen above the noise. */
  dbc: number | null;
  /** If not seen: at most this, dBc. */
  ceilingDbc: number | null;
  /** Why it wasn't measured (out of the receiver's range, etc.). */
  skipped: string | null;
  duty: number;
  verdicts: Partial<Record<Regime, Verdict>>;
}

export function judge(dbc: number | null, ceilingDbc: number | null, limit: Limit | null): Verdict {
  if (!limit) return { rating: 'unknown', marginDb: null, limit, text: 'No limit for this band.' };
  const lim = `limit ${dbStr(limit.dbc)} dBc`;
  if (dbc !== null) {
    const m = limit.dbc - dbc;
    if (m >= GOOD_MARGIN_DB) return { rating: 'good', marginDb: m, limit, text: `Passes by ${m.toFixed(1)} dB (${lim}).` };
    if (m >= 0) return { rating: 'medium', marginDb: m, limit, text: `Passes by only ${m.toFixed(1)} dB (${lim}): within measurement error, test properly.` };
    return { rating: 'bad', marginDb: m, limit, text: `Fails by ${(-m).toFixed(1)} dB (${lim}).` };
  }
  if (ceilingDbc === null) return { rating: 'unknown', marginDb: null, limit, text: 'Not measured.' };
  const m = limit.dbc - ceilingDbc;
  if (m >= GOOD_MARGIN_DB) return { rating: 'good', marginDb: m, limit, text: `Below the noise, at least ${m.toFixed(1)} dB under the ${lim}.` };
  if (m >= 0) return { rating: 'medium', marginDb: m, limit, text: `Below the noise, but the noise is only ${m.toFixed(1)} dB under the ${lim}.` };
  return {
    rating: 'unknown',
    marginDb: null,
    limit,
    text: `Not seen, but the noise (${dbStr(ceilingDbc)} dBc) is above the ${lim}: can't tell. Move closer or raise the gain.`,
  };
}

/** dB value with a proper minus sign. */
export function dbStr(v: number): string {
  return `${v > 0 ? '+' : v < 0 ? '−' : ''}${Math.abs(v).toFixed(1)}`;
}

const ORDER: Rating[] = ['bad', 'medium', 'unknown', 'good'];

/**
 * Overall: the worst harmonic measured; "can't tell" only beats "good".
 * Harmonics the receiver can't reach don't count (`checked` says how many did).
 */
export function overall(results: HarmonicResult[], regime: Regime): { rating: Rating; checked: number; total: number } {
  const vs = results
    .filter((r) => !r.skipped)
    .map((r) => r.verdicts[regime])
    .filter((v): v is Verdict => !!v);
  const known = vs.filter((v) => v.rating !== 'unknown');
  if (!known.length) return { rating: 'unknown', checked: 0, total: results.length };
  const worst = vs.reduce((w, v) => (ORDER.indexOf(v.rating) < ORDER.indexOf(w) ? v.rating : w), 'good' as Rating);
  return { rating: worst, checked: known.length, total: results.length };
}

export function inRange(ranges: FreqRange[], hz: number): boolean {
  return !ranges.length || ranges.some((r) => hz >= r.lo_hz && hz <= r.hi_hz);
}

export function ordinal(n: number): string {
  return n === 2 ? '2nd' : n === 3 ? '3rd' : `${n}th`;
}
