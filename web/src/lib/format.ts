export function formatMHz(hz: number, decimals = 4): string {
  return `${(hz / 1e6).toFixed(decimals)} MHz`;
}

export function formatRate(sps: number): string {
  return sps >= 1e6 ? `${(sps / 1e6).toFixed(3).replace(/\.?0+$/, '')} MS/s` : `${sps / 1e3} kS/s`;
}

/** A bandwidth: "2.048 MHz", "512 kHz", "800 Hz". */
export function formatSpan(hz: number): string {
  const trim = (v: number) => v.toFixed(3).replace(/\.?0+$/, '');
  if (hz >= 1e6) return `${trim(hz / 1e6)} MHz`;
  if (hz >= 1e3) return `${trim(hz / 1e3)} kHz`;
  return `${Math.round(hz)} Hz`;
}

/** Parse "145.8", "145.800 MHz", "433920k", "433920000" → Hz. */
export function parseFrequency(input: string): number | null {
  const s = input.trim().toLowerCase().replace(/\s+/g, '').replace(/hz$/, '');
  const m = /^([0-9]*\.?[0-9]+)([kmg]?)$/.exec(s);
  if (!m) return null;
  const v = parseFloat(m[1]);
  const unit = m[2];
  let hz: number;
  if (unit === 'k') hz = v * 1e3;
  else if (unit === 'm') hz = v * 1e6;
  else if (unit === 'g') hz = v * 1e9;
  else hz = v < 10_000 ? v * 1e6 : v; // bare numbers: small = MHz, large = Hz
  return Math.round(hz);
}

/** Known channels drawn as markers on the spectrum (UK defaults). */
export const MARKERS: { hz: number; label: string }[] = [
  { hz: 144_800_000, label: 'APRS' },
  { hz: 145_800_000, label: 'ISS voice/SSTV' },
  { hz: 145_825_000, label: 'ISS APRS' },
  { hz: 433_920_000, label: 'ISM/TPMS' },
  { hz: 437_550_000, label: 'ARISS SSTV' },
];

export const PRESETS: { label: string; hz: number; rate?: number }[] = [
  { label: 'VHF block (APRS + ISS)', hz: 145_300_000, rate: 2_048_000 },
  // Centred just above 433.92 so the dongle's DC spike doesn't sit on the sensors.
  { label: 'ISM 433.92', hz: 434_000_000, rate: 1_024_000 },
  { label: 'ARISS 437.55', hz: 437_550_000, rate: 1_024_000 },
];

/** "12 s ago", "5 min ago", or a date for old timestamps (ms since epoch). */
export function relTime(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return `${s} s ago`;
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return new Date(ms).toLocaleDateString();
}
