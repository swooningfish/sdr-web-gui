/** UK band plans drawn above the spectrum: amateur allocations with their
 * RSGB sub-bands, plus broadcast, aviation, marine, PMR/CB, ISM/SRD and
 * satellite bands. Simplified for display; check the RSGB band plan before
 * transmitting. */

import type { AudioMode } from './audio';

export type BandGroup = 'amateur' | 'broadcast' | 'aviation' | 'marine' | 'pmr' | 'ism' | 'satellite';

export type SegKind = 'cw' | 'ssb' | 'digital' | 'beacon' | 'fm' | 'repeater' | 'sat' | 'all';

export interface Segment {
  lo: number;
  hi: number;
  label: string;
  kind: SegKind;
  /** Usual listening mode here. */
  mode?: AudioMode;
}

export interface Band {
  group: BandGroup;
  name: string;
  lo: number;
  hi: number;
  mode?: AudioMode;
  segments?: Segment[];
}

export const GROUPS: { id: BandGroup; label: string; colour: string }[] = [
  { id: 'amateur', label: 'Amateur', colour: '#0ea5e9' },
  { id: 'broadcast', label: 'Broadcast', colour: '#d946ef' },
  { id: 'aviation', label: 'Aviation', colour: '#f97316' },
  { id: 'marine', label: 'Marine', colour: '#14b8a6' },
  { id: 'pmr', label: 'PMR / CB', colour: '#eab308' },
  { id: 'ism', label: 'ISM / SRD', colour: '#84cc16' },
  { id: 'satellite', label: 'Satellite / space', colour: '#818cf8' },
];

export const KINDS: { id: SegKind; label: string; colour: string }[] = [
  { id: 'cw', label: 'CW', colour: '#f59e0b' },
  { id: 'ssb', label: 'SSB / narrow', colour: '#a855f7' },
  { id: 'digital', label: 'Digital', colour: '#06b6d4' },
  { id: 'beacon', label: 'Beacons', colour: '#ef4444' },
  { id: 'fm', label: 'FM simplex', colour: '#22c55e' },
  { id: 'repeater', label: 'Repeaters', colour: '#65a30d' },
  { id: 'sat', label: 'Satellites', colour: '#3b82f6' },
  { id: 'all', label: 'All modes', colour: '#64748b' },
];

const KIND_COLOUR = Object.fromEntries(KINDS.map((k) => [k.id, k.colour])) as Record<SegKind, string>;
const GROUP_COLOUR = Object.fromEntries(GROUPS.map((g) => [g.id, g.colour])) as Record<BandGroup, string>;

const M = (mhz: number) => Math.round(mhz * 1e6);

/** [lo MHz, hi MHz, label, kind, mode?] */
type Seg = [number, number, string, SegKind, AudioMode?];

function ham(name: string, lo: number, hi: number, segs: Seg[]): Band {
  return {
    group: 'amateur',
    name,
    lo: M(lo),
    hi: M(hi),
    segments: segs.map(([a, b, label, kind, mode]) => ({ lo: M(a), hi: M(b), label, kind, mode })),
  };
}

/** HF sideband convention: LSB below 10 MHz, USB above. */
const hf = (lo: number, hi: number, cw: number, dig: number, beacon?: [number, number]): Seg[] => {
  const ssb: AudioMode = lo < 10 ? 'lsb' : 'usb';
  const s: Seg[] = [
    [lo, cw, 'CW', 'cw', 'cw'],
    [cw, dig, 'Digital', 'digital', 'usb'],
  ];
  if (beacon) s.push([beacon[0], beacon[1], 'Beacons', 'beacon', 'cw'], [beacon[1], hi, 'SSB', 'ssb', ssb]);
  else s.push([dig, hi, 'SSB', 'ssb', ssb]);
  return s;
};

const other = (group: BandGroup, name: string, lo: number, hi: number, mode?: AudioMode, segments?: Seg[]): Band => ({
  group,
  name,
  lo: M(lo),
  hi: M(hi),
  mode,
  segments: segments?.map(([a, b, label, kind, m]) => ({ lo: M(a), hi: M(b), label, kind, mode: m })),
});

export const BANDS: Band[] = [
  // ---- Amateur (UK, RSGB band plans, simplified)
  ham('160 m', 1.81, 2.0, hf(1.81, 2.0, 1.838, 1.843)),
  ham('80 m', 3.5, 3.8, hf(3.5, 3.8, 3.57, 3.6)),
  ham('60 m', 5.2585, 5.4065, [[5.2585, 5.4065, 'All modes (UK)', 'all', 'usb']]),
  ham('40 m', 7.0, 7.2, hf(7.0, 7.2, 7.04, 7.06)),
  ham('30 m', 10.1, 10.15, [
    [10.1, 10.13, 'CW', 'cw', 'cw'],
    [10.13, 10.15, 'Digital', 'digital', 'usb'],
  ]),
  ham('20 m', 14.0, 14.35, hf(14.0, 14.35, 14.07, 14.099, [14.099, 14.101])),
  ham('17 m', 18.068, 18.168, hf(18.068, 18.168, 18.095, 18.109, [18.109, 18.111])),
  ham('15 m', 21.0, 21.45, hf(21.0, 21.45, 21.07, 21.149, [21.149, 21.151])),
  ham('12 m', 24.89, 24.99, hf(24.89, 24.99, 24.915, 24.929, [24.929, 24.931])),
  ham('10 m', 28.0, 29.7, [
    [28.0, 28.07, 'CW', 'cw', 'cw'],
    [28.07, 28.19, 'Digital', 'digital', 'usb'],
    [28.19, 28.225, 'Beacons', 'beacon', 'cw'],
    [28.225, 29.1, 'SSB', 'ssb', 'usb'],
    [29.1, 29.3, 'FM', 'fm', 'nfm'],
    [29.3, 29.51, 'Satellites', 'sat', 'usb'],
    [29.52, 29.7, 'FM repeaters', 'repeater', 'nfm'],
  ]),
  ham('6 m', 50.0, 52.0, [
    [50.0, 50.03, 'Beacons', 'beacon', 'cw'],
    [50.03, 50.1, 'CW', 'cw', 'cw'],
    [50.1, 50.5, 'SSB', 'ssb', 'usb'],
    [50.5, 50.71, 'All modes', 'all'],
    [50.71, 50.89, 'Repeater out', 'repeater', 'nfm'],
    [50.89, 51.21, 'All modes', 'all'],
    [51.21, 51.39, 'Repeater in', 'repeater', 'nfm'],
    [51.41, 51.59, 'FM simplex', 'fm', 'nfm'],
    [51.59, 52.0, 'All modes', 'all'],
  ]),
  ham('4 m', 70.0, 70.5, [
    [70.0, 70.09, 'Beacons', 'beacon', 'cw'],
    [70.09, 70.25, 'CW / SSB', 'ssb', 'usb'],
    [70.25, 70.5, 'All modes / FM', 'fm', 'nfm'],
  ]),
  ham('2 m', 144.0, 146.0, [
    [144.0, 144.035, 'EME', 'cw', 'cw'],
    [144.035, 144.15, 'CW', 'cw', 'cw'],
    [144.15, 144.4, 'SSB', 'ssb', 'usb'],
    [144.4, 144.49, 'Beacons', 'beacon', 'cw'],
    [144.5, 144.794, 'All modes', 'all'],
    [144.794, 144.99, 'Digital / APRS', 'digital', 'nfm'],
    [144.99, 145.194, 'Repeater in', 'repeater', 'nfm'],
    [145.194, 145.594, 'FM simplex', 'fm', 'nfm'],
    [145.594, 145.794, 'Repeater out', 'repeater', 'nfm'],
    [145.794, 146.0, 'Satellites', 'sat', 'nfm'],
  ]),
  ham('70 cm', 430.0, 440.0, [
    [430.0, 431.975, 'Repeaters / all modes', 'repeater', 'nfm'],
    [432.0, 432.15, 'CW', 'cw', 'cw'],
    [432.15, 432.4, 'SSB', 'ssb', 'usb'],
    [432.4, 432.49, 'Beacons', 'beacon', 'cw'],
    [432.5, 432.994, 'All modes', 'all'],
    [432.994, 433.381, 'Repeater in', 'repeater', 'nfm'],
    [433.394, 434.594, 'FM simplex', 'fm', 'nfm'],
    [434.594, 434.981, 'Repeater out', 'repeater', 'nfm'],
    [435.0, 438.0, 'Satellites', 'sat', 'nfm'],
    [438.0, 440.0, 'All modes / DV', 'all', 'nfm'],
  ]),
  ham('23 cm', 1240.0, 1325.0, [
    [1240.0, 1260.0, 'All modes / ATV', 'all'],
    [1260.0, 1270.0, 'Satellites', 'sat'],
    [1270.0, 1296.0, 'All modes / ATV', 'all'],
    [1296.0, 1296.15, 'CW', 'cw', 'cw'],
    [1296.15, 1296.8, 'SSB', 'ssb', 'usb'],
    [1296.8, 1296.994, 'Beacons', 'beacon', 'cw'],
    [1297.0, 1297.5, 'Repeaters', 'repeater', 'nfm'],
    [1297.5, 1298.0, 'FM simplex', 'fm', 'nfm'],
    [1298.0, 1300.0, 'Digital', 'digital'],
    [1300.0, 1325.0, 'All modes / ATV', 'all'],
  ]),

  // ---- Broadcast
  other('broadcast', 'MW broadcast', 0.5265, 1.6065, 'am'),
  ...(
    [
      ['120 m', 2.3, 2.495],
      ['90 m', 3.2, 3.4],
      ['75 m', 3.9, 4.0],
      ['60 m', 4.75, 5.06],
      ['49 m', 5.9, 6.2],
      ['41 m', 7.2, 7.45],
      ['31 m', 9.4, 9.9],
      ['25 m', 11.6, 12.1],
      ['22 m', 13.57, 13.87],
      ['19 m', 15.1, 15.8],
      ['16 m', 17.48, 17.9],
      ['15 m', 18.9, 19.02],
      ['13 m', 21.45, 21.85],
      ['11 m', 25.67, 26.1],
    ] as [string, number, number][]
  ).map(([n, lo, hi]) => other('broadcast', `${n} SW broadcast`, lo, hi, 'am')),
  other('broadcast', 'FM broadcast', 87.5, 108.0, 'wfm'),
  other('broadcast', 'DAB (band III)', 174.0, 240.0),

  // ---- Aviation
  other('aviation', 'VOR / ILS navaids', 108.0, 117.975, 'am'),
  other('aviation', 'Airband (AM voice)', 118.0, 137.0, 'am'),
  other('aviation', 'Military airband', 225.0, 400.0, 'am'),
  other('aviation', 'ADS-B / Mode S', 1089.0, 1091.0),

  // ---- Marine
  other('marine', 'Marine VHF', 156.0, 162.05, 'nfm', [
    [156.0, 161.9625, 'Channels (Ch 16 = 156.800)', 'fm', 'nfm'],
    [161.9625, 162.0375, 'AIS', 'digital'],
  ]),

  // ---- PMR / CB
  other('pmr', 'CB (EU)', 26.965, 27.405, 'nfm'),
  other('pmr', 'CB (UK)', 27.60125, 27.99125, 'nfm'),
  other('pmr', 'PMR446', 446.0, 446.2, 'nfm', [
    [446.0, 446.1, 'Analogue', 'fm', 'nfm'],
    [446.1, 446.2, 'Digital (dPMR / DMR)', 'digital'],
  ]),

  // ---- ISM / SRD
  other('ism', 'ISM 433 (sensors, TPMS, keyfobs)', 433.05, 434.79),
  other('ism', 'SRD 868 (alarms, LoRa, smart meters)', 863.0, 870.0),

  // ---- Satellite / space
  other('satellite', 'Weather satellites (APT / LRPT)', 137.0, 138.0, 'wfm'),
  other('satellite', 'Hydrogen line', 1420.0, 1421.0),
  other('satellite', 'GPS L1', 1574.4, 1576.4),
  other('satellite', 'Iridium', 1616.0, 1626.5),
];

export const ALL_GROUPS = GROUPS.map((g) => g.id);

/** The band and sub-band at `hz` (first match among the enabled groups). */
export function bandAt(hz: number, enabled: ReadonlySet<BandGroup>): { band: Band; segment: Segment | null } | null {
  for (const band of BANDS) {
    if (!enabled.has(band.group) || hz < band.lo || hz > band.hi) continue;
    const segment = band.segments?.find((s) => hz >= s.lo && hz < s.hi) ?? null;
    return { band, segment };
  }
  return null;
}

/** Usual listening mode at `hz`, from the band plan. */
export function modeAt(hz: number, enabled: ReadonlySet<BandGroup>): AudioMode | null {
  // Look through all matching bands: a sub-band's mode wins over a band's.
  let found: AudioMode | null = null;
  for (const band of BANDS) {
    if (!enabled.has(band.group) || hz < band.lo || hz > band.hi) continue;
    const seg = band.segments?.find((s) => hz >= s.lo && hz < s.hi);
    if (seg?.mode) return seg.mode;
    found ??= band.mode ?? null;
  }
  return found;
}

export function describe(hit: { band: Band; segment: Segment | null } | null): string {
  if (!hit) return '';
  return hit.segment ? `${hit.band.name} · ${hit.segment.label}` : hit.band.name;
}

/** Rows of the strip (CSS pixels). */
export const LANE_H = 13;
export const SEG_H = 14;
export const STRIP_H = 2 * LANE_H + SEG_H;

/** Draw the band strip: two lanes of band names, then sub-bands. */
export function drawStrip(
  ctx: CanvasRenderingContext2D,
  start: number,
  span: number,
  dpr: number,
  enabled: ReadonlySet<BandGroup>,
  hoverHz: number | null,
) {
  const W = ctx.canvas.width;
  const H = ctx.canvas.height;
  ctx.clearRect(0, 0, W, H);
  const xOf = (hz: number) => ((hz - start) / span) * W;
  const end = start + span;
  const visible = BANDS.filter((b) => enabled.has(b.group) && b.hi > start && b.lo < end).sort((a, b) => a.lo - b.lo);
  ctx.font = `${10.5 * dpr}px ui-sans-serif, system-ui, sans-serif`;
  ctx.textBaseline = 'middle';

  const label = (text: string, x0: number, x1: number, y: number, colour: string) => {
    const pad = 3 * dpr;
    const a = Math.max(0, x0) + pad;
    const b = Math.min(W, x1) - pad;
    if (b - a < 12 * dpr) return;
    ctx.fillStyle = colour;
    let t = text;
    while (t.length > 1 && ctx.measureText(t).width > b - a) t = t.slice(0, -2) + '…';
    if (ctx.measureText(t).width > b - a) return;
    ctx.fillText(t, a, y);
  };

  // Bands: up to two lanes, overlapping bands go to the second.
  const laneEnd = [-Infinity, -Infinity];
  for (const b of visible) {
    const lane = b.lo >= laneEnd[0] ? 0 : b.lo >= laneEnd[1] ? 1 : -1;
    if (lane < 0) continue;
    laneEnd[lane] = b.hi;
    const x0 = xOf(b.lo);
    const x1 = xOf(b.hi);
    const y = lane * LANE_H * dpr;
    const c = GROUP_COLOUR[b.group];
    ctx.fillStyle = c + '33';
    ctx.fillRect(x0, y + 1 * dpr, x1 - x0, (LANE_H - 2) * dpr);
    ctx.fillStyle = c;
    ctx.fillRect(x0, y + 1 * dpr, Math.max(1, Math.min(2 * dpr, x1 - x0)), (LANE_H - 2) * dpr);
    label(b.name, x0, x1, y + (LANE_H / 2) * dpr, '#e2e8f0');
  }

  // Sub-bands.
  const y = 2 * LANE_H * dpr;
  for (const b of visible) {
    for (const s of b.segments ?? []) {
      if (s.hi <= start || s.lo >= end) continue;
      const x0 = xOf(s.lo);
      const x1 = xOf(s.hi);
      const c = KIND_COLOUR[s.kind];
      const hovered = hoverHz !== null && hoverHz >= s.lo && hoverHz < s.hi;
      ctx.fillStyle = c + (hovered ? 'aa' : '55');
      ctx.fillRect(x0 + 0.5 * dpr, y + 1 * dpr, Math.max(1, x1 - x0 - 1 * dpr), (SEG_H - 2) * dpr);
      label(s.label, x0, x1, y + (SEG_H / 2) * dpr, '#f8fafc');
    }
  }

  // What's under the cursor, written out in full beside it.
  if (hoverHz !== null) {
    const text = describe(bandAt(hoverHz, enabled));
    if (text) {
      const x = xOf(hoverHz);
      const w = ctx.measureText(text).width + 8 * dpr;
      const left = x + 8 * dpr + w > W ? x - 8 * dpr - w : x + 8 * dpr;
      ctx.fillStyle = 'rgba(2,6,23,0.88)';
      ctx.fillRect(left, 1 * dpr, w, (LANE_H - 1) * dpr);
      ctx.fillStyle = '#f8fafc';
      ctx.fillText(text, left + 4 * dpr, (LANE_H / 2 + 0.5) * dpr);
    }
  }
}
