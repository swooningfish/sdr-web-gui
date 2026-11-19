// Signal labels (M9): what the server's classifier (crates/sdrd/src/dsp/
// classify.rs) found in the capture, plus context only the browser has:
// the band plan and bookmarks near each signal. The context nudges the
// scores (a narrow keyed line in a CW sub-band is more likely Morse) but
// never names a signal the classifier couldn't.

import type { AudioMode } from './audio';
import { ALL_GROUPS, BANDS, describe, type Band, type BandGroup, type Segment, type SegKind } from './bandplan';
import type { Bookmark } from './bookmarks.svelte';
import { formatMHz, MARKERS } from './format';

export type SignalClass = 'carrier' | 'cw' | 'voice' | 'digital_voice' | 'data' | 'broadcast_fm' | 'ofdm';

export interface SignalFeatures {
  bw_hz: number;
  snr_db: number;
  duty: number;
  keying: number;
  level_sd_db: number;
  ripple_db: number | null;
  shape_var: number;
  drift_hz: number;
  burst_s: number | null;
  longest_s: number;
  keyed_with_hz: number | null;
}

export interface Signal {
  id: number;
  hz: number;
  lo_hz: number;
  hi_hz: number;
  active: boolean;
  first_ms: number;
  last_ms: number;
  class: SignalClass | null;
  confidence: number;
  /** Shares in `SignalsMsg.classes` order. */
  scores: number[];
  features: SignalFeatures;
}

export interface SignalsMsg {
  type: 'signals';
  center_hz: number;
  sample_rate: number;
  bin_hz: number;
  ts: number;
  classes: SignalClass[];
  signals: Signal[];
}

export const CLASS_INFO: Record<SignalClass, { label: string; short: string; colour: string; about: string }> = {
  carrier: { label: 'Carrier', short: 'Carrier', colour: '#94a3b8', about: 'an unmodulated line: a beacon, birdie, spur or idle transmitter' },
  cw: { label: 'CW (Morse)', short: 'CW', colour: '#f59e0b', about: 'a carrier keyed on and off: Morse' },
  voice: { label: 'Voice', short: 'Voice', colour: '#22c55e', about: 'analogue speech: NFM, AM or SSB' },
  digital_voice: {
    label: 'Digital voice',
    short: 'DV',
    colour: '#06b6d4',
    about: 'flat-topped, steady while on: DMR, D-STAR, M17, Fusion, dPMR and the like',
  },
  data: { label: 'Data', short: 'Data', colour: '#a855f7', about: 'short bursts: packet/APRS, telemetry, ISM sensors, pagers, LoRa' },
  broadcast_fm: { label: 'Broadcast FM', short: 'WFM', colour: '#d946ef', about: 'a wide FM broadcast station' },
  ofdm: { label: 'OFDM (DAB / DVB)', short: 'OFDM', colour: '#3b82f6', about: 'wide, flat and always on: DAB, DVB-T, LTE and similar' },
};

/** Listening modes that suit each class (the first is the usual one). */
export const CLASS_MODES: Record<SignalClass, AudioMode[]> = {
  carrier: ['cw', 'usb'],
  cw: ['cw'],
  voice: ['nfm', 'am', 'usb', 'lsb'],
  digital_voice: ['nfm'],
  data: ['nfm', 'usb'],
  broadcast_fm: ['wfm'],
  ofdm: ['dab'],
};

const KIND_PRIOR: Record<SegKind, SignalClass[]> = {
  cw: ['cw', 'carrier'],
  ssb: ['voice'],
  digital: ['data', 'digital_voice'],
  beacon: ['cw', 'carrier'],
  fm: ['voice', 'digital_voice'],
  repeater: ['voice', 'digital_voice'],
  sat: ['data', 'voice', 'carrier', 'cw'],
  all: [],
};

const MODE_PRIOR: Record<AudioMode, SignalClass[]> = {
  nfm: ['voice', 'digital_voice', 'data'],
  wfm: ['broadcast_fm'],
  am: ['voice', 'carrier'],
  usb: ['voice', 'data', 'cw'],
  lsb: ['voice'],
  cw: ['cw', 'carrier'],
  dab: ['ofdm'],
};

const GROUP_PRIOR: Partial<Record<BandGroup, SignalClass[]>> = {
  ism: ['data', 'carrier'],
  aviation: ['voice', 'data'],
  marine: ['voice', 'data'],
  pmr: ['voice', 'digital_voice'],
};

/** Context around a signal: the band plan and nearby bookmarks. */
export interface SignalContext {
  band: Band | null;
  segment: Segment | null;
  /** Band plan text, e.g. "2 m · FM simplex". */
  plan: string;
  /** Bookmarks inside the signal (or within 2 kHz of it), nearest first. */
  bookmarks: Bookmark[];
  /** Known channel markers (APRS, ISS ...) inside it. */
  markers: string[];
}

export interface Labelled extends Signal {
  /** Class after context, or null when nothing fits. */
  label: SignalClass | null;
  /** Share of the best class after context (0–1). */
  share: number;
  /** All classes, best first, after context. */
  ranked: { cls: SignalClass; share: number }[];
  context: SignalContext;
}

const ALL = new Set<BandGroup>(ALL_GROUPS);

export function contextOf(s: Signal, bookmarks: readonly Bookmark[]): SignalContext {
  const slack = 2_000;
  const inside = (hz: number) => hz >= s.lo_hz - slack && hz <= s.hi_hz + slack;
  // The innermost band with a segment at the centre wins; else the first band.
  let band: Band | null = null;
  let segment: Segment | null = null;
  for (const b of BANDS) {
    if (!ALL.has(b.group) || s.hz < b.lo || s.hz > b.hi) continue;
    const seg = b.segments?.find((x) => s.hz >= x.lo && s.hz < x.hi) ?? null;
    if (!band || (seg && !segment)) {
      band = b;
      segment = seg;
    }
  }
  return {
    band,
    segment,
    plan: describe(band ? { band, segment } : null),
    bookmarks: bookmarks.filter((b) => inside(b.freq_hz)).sort((a, b) => Math.abs(a.freq_hz - s.hz) - Math.abs(b.freq_hz - s.hz)),
    markers: MARKERS.filter((m) => inside(m.hz)).map((m) => m.label),
  };
}

/** Weight per class from the context: 1 = no opinion. */
function priors(ctx: SignalContext): Record<SignalClass, number> {
  const w = Object.fromEntries(Object.keys(CLASS_INFO).map((c) => [c, 1])) as Record<SignalClass, number>;
  const boost = (classes: SignalClass[] | undefined, k: number) => classes?.forEach((c) => (w[c] *= k));
  if (ctx.band) {
    boost(GROUP_PRIOR[ctx.band.group], 1.3);
    if (ctx.band.mode) boost(MODE_PRIOR[ctx.band.mode], 1.3);
  }
  if (ctx.segment) {
    boost(KIND_PRIOR[ctx.segment.kind], 1.5);
    if (ctx.segment.mode) boost(MODE_PRIOR[ctx.segment.mode], 1.2);
  }
  // A bookmark says what someone found there: a stronger hint.
  const bm = ctx.bookmarks[0];
  if (bm) boost(MODE_PRIOR[bm.mode], 2);
  return w;
}

/** The classifier's verdict with the context folded in. */
export function label(s: Signal, classes: readonly SignalClass[], bookmarks: readonly Bookmark[]): Labelled {
  const context = contextOf(s, bookmarks);
  const w = priors(context);
  const raw = classes.map((c, i) => ({ cls: c, share: (s.scores[i] ?? 0) * w[c] }));
  const sum = raw.reduce((a, r) => a + r.share, 0) || 1;
  const ranked = raw.map((r) => ({ cls: r.cls, share: r.share / sum })).sort((a, b) => b.share - a.share);
  // Context re-ranks what the classifier found plausible; it can't name a
  // signal the classifier gave up on.
  const top = ranked[0];
  const named = s.class !== null && top.share >= 0.4;
  return { ...s, label: named ? top.cls : null, share: named ? top.share : s.confidence, ranked, context };
}

export function labelAll(msg: SignalsMsg | null, bookmarks: readonly Bookmark[]): Labelled[] {
  if (!msg) return [];
  return msg.signals.map((s) => label(s, msg.classes, bookmarks));
}

/** The signal under `hz`, if any (the narrowest that covers it, with a
 * little slack so narrow ones can be hovered). */
export function signalAt(list: readonly Labelled[], hz: number, hzPerPx: number): Labelled | null {
  const slack = Math.max(500, 4 * hzPerPx);
  let best: Labelled | null = null;
  for (const s of list) {
    if (hz < s.lo_hz - slack || hz > s.hi_hz + slack) continue;
    if (!best || s.hi_hz - s.lo_hz < best.hi_hz - best.lo_hz) best = s;
  }
  return best;
}

function bw(hz: number): string {
  return hz >= 1e6 ? `${(hz / 1e6).toFixed(2)} MHz` : hz >= 10e3 ? `${Math.round(hz / 1e3)} kHz` : `${(hz / 1e3).toFixed(1)} kHz`;
}

/** Short reasons, from the measurements, for the readout. */
export function evidence(s: Signal): string[] {
  const f = s.features;
  const out = [`${bw(f.bw_hz)} wide`, `${Math.round(f.snr_db)} dB above the noise`];
  if (f.keyed_with_hz !== null) out.push(`keys on and off with ${formatMHz(f.keyed_with_hz, 4)}: likely a spur or harmonic of it`);
  if (f.duty >= 0.95) out.push('on all the time');
  else if (f.burst_s !== null && f.longest_s < 4) out.push(`bursts of about ${f.burst_s < 1 ? `${Math.round(f.burst_s * 1000)} ms` : `${f.burst_s.toFixed(1)} s`}`);
  else if (f.longest_s >= 3) out.push('long transmissions');
  if (f.keying >= 2 && f.bw_hz < 2500) out.push(`keyed ${f.keying.toFixed(0)} times a second`);
  if (f.ripple_db !== null) out.push(f.ripple_db < 2.5 ? 'flat top' : f.ripple_db > 5 ? 'peaky spectrum' : 'uneven top');
  if (f.shape_var > 0.25) out.push('shape changes like speech');
  if (f.drift_hz > 200 && f.bw_hz < 3000) out.push(`drifting (±${Math.round(f.drift_hz)} Hz)`);
  return out;
}

/** "12 s ago" style age of the last time it was heard. */
export function heardAgo(s: Signal, nowMs: number): string {
  if (s.active) return 'on now';
  const sec = Math.max(0, Math.round((nowMs - s.last_ms) / 1000));
  return sec < 60 ? `heard ${sec} s ago` : `heard ${Math.round(sec / 60)} min ago`;
}
