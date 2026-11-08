import type { Device, EventRow, Json } from './api';

/** Fields that identify the device or describe reception rather than data. */
const META = new Set([
  'model', 'id', 'channel', 'type', 'subtype', 'protocol', 'mic', 'mod',
  'freq', 'freq1', 'freq2', 'rssi', 'snr', 'noise', 'count', 'num_rows',
]);

const num = (v: unknown, d = 1) => (typeof v === 'number' ? v.toFixed(d).replace(/\.0+$/, '') : String(v));

/** Raw bit strings can run to thousands of hex digits. */
const clip = (s: string, max = 32) => (s.length > max ? `${s.slice(0, max)}…` : s);

export function deviceLabel(d: Device): string {
  if (d.name) return d.name;
  return [d.model, d.dev_id, d.channel ? `ch${d.channel}` : null].filter(Boolean).join(' ');
}

/** Name for a card or heading: the given name, else just the model. */
export function deviceTitle(d: Device): string {
  return d.name || d.model;
}

/** Short human-readable readings, most useful first. */
export function readings(data: Json): string[] {
  const out: string[] = [];
  const used = new Set<string>();
  const take = (k: string, f: (v: unknown) => string) => {
    if (k in data) {
      out.push(f(data[k]));
      used.add(k);
    }
  };
  take('pressure_kPa', (v) => `${num(v)} kPa (${num((v as number) * 0.145038)} psi)`);
  take('pressure_PSI', (v) => `${num(v)} psi`);
  take('pressure_hPa', (v) => `${num(v)} hPa`);
  take('temperature_C', (v) => `${num(v)} °C`);
  take('temperature_F', (v) => `${num(v)} °F`);
  take('humidity', (v) => `${num(v, 0)} %RH`);
  take('wind_avg_km_h', (v) => `wind ${num(v)} km/h`);
  take('rain_mm', (v) => `rain ${num(v)} mm`);
  take('battery_ok', (v) => (v === 0 ? 'battery low' : 'battery ok'));
  // Flex decoders report raw bits, as `rows` and again as `codes`; show one, shortened.
  if (Array.isArray(data.rows)) {
    used.add('rows');
    used.add('codes');
    for (const r of data.rows.slice(0, 2) as Json[]) {
      if (typeof r?.data === 'string') out.push(`{${r.len}} ${clip(r.data)}`);
    }
    if (data.rows.length > 2) out.push(`+${data.rows.length - 2} rows`);
  } else if (Array.isArray(data.codes)) {
    used.add('codes');
    out.push(...(data.codes as string[]).slice(0, 2).map(clip));
  }
  for (const [k, v] of Object.entries(data)) {
    if (used.has(k) || META.has(k) || typeof v === 'object') continue;
    out.push(`${k}: ${typeof v === 'number' ? num(v, 2) : v}`);
  }
  return out;
}

export interface Point {
  t: number;
  v: number;
}

/** Numeric fields worth plotting, in display order. */
export const CHART_FIELDS: { key: string; label: string; unit: string; digits: number }[] = [
  { key: 'temperature_C', label: 'Temperature', unit: '°C', digits: 1 },
  { key: 'temperature_F', label: 'Temperature', unit: '°F', digits: 1 },
  { key: 'humidity', label: 'Humidity', unit: '%', digits: 0 },
  { key: 'pressure_kPa', label: 'Pressure', unit: 'kPa', digits: 1 },
  { key: 'pressure_PSI', label: 'Pressure', unit: 'psi', digits: 1 },
  { key: 'pressure_hPa', label: 'Pressure', unit: 'hPa', digits: 1 },
  { key: 'moisture', label: 'Moisture', unit: '%', digits: 0 },
  { key: 'wind_avg_km_h', label: 'Wind', unit: 'km/h', digits: 1 },
  { key: 'rain_mm', label: 'Rain', unit: 'mm', digits: 1 },
  { key: 'light_lux', label: 'Light', unit: 'lx', digits: 0 },
  { key: 'rssi', label: 'Signal', unit: 'dB', digits: 1 },
];

/** Points for one field, oldest first (events arrive newest first). */
export function series(events: EventRow[], key: string): Point[] {
  const out: Point[] = [];
  for (let i = events.length - 1; i >= 0; i--) {
    const v = events[i].data[key];
    if (typeof v === 'number' && Number.isFinite(v)) out.push({ t: events[i].ts, v });
  }
  return out;
}
