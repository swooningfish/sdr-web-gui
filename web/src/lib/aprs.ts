import type { Device } from './api';

/** Names for the most common APRS symbols (table + code). */
const SYMBOLS: Record<string, string> = {
  '/!': 'Police', '/#': 'Digipeater', '/&': 'Gateway', "/'": 'Small aircraft', '/-': 'House',
  '/<': 'Motorcycle', '/=': 'Train', '/>': 'Car', '/O': 'Balloon', '/R': 'RV', '/S': 'Shuttle',
  '/U': 'Bus', '/X': 'Helicopter', '/Y': 'Yacht', '/[': 'Person', '/^': 'Aircraft', '/_': 'Weather station',
  '/`': 'Dish antenna', '/a': 'Ambulance', '/b': 'Bicycle', '/f': 'Fire engine', '/j': 'Jeep', '/k': 'Truck',
  '/l': 'Laptop', '/n': 'Node', '/p': 'Dog', '/r': 'Repeater', '/s': 'Boat', '/u': 'Lorry', '/v': 'Van',
  '/y': 'Yagi at QTH',
  // Alternate table (backslash).
  '\\#': 'Digipeater', '\\&': 'Gateway', '\\-': 'House', '\\>': 'Car', '\\S': 'Satellite',
  '\\_': 'Weather station', '\\n': 'Node', '\\k': 'SUV', '\\j': 'Jeep',
};

export function symbolName(sym: string | null | undefined): string {
  if (!sym) return '';
  // Overlay characters replace the table byte; fall back to the alternate table.
  return SYMBOLS[sym] ?? SYMBOLS['\\' + sym[1]] ?? `Symbol ${sym}`;
}

export function isSatellite(d: Device): boolean {
  return d.symbol === '\\S' || d.symbol === '/S';
}

export function stationName(d: Device): string {
  return d.name ?? d.dev_id ?? d.model;
}

/** Line under a station's name: comment / status / message summary. */
export function stationText(data: Record<string, unknown>): string {
  const msg = data.message as { to?: string; text?: string } | undefined;
  if (msg?.text) return `to ${msg.to}: ${msg.text}`;
  return String(data.status ?? data.comment ?? '');
}

export function motion(data: Record<string, unknown>): string {
  const parts: string[] = [];
  if (typeof data.speed_kmh === 'number') parts.push(`${Math.round(data.speed_kmh)} km/h`);
  if (typeof data.course_deg === 'number') parts.push(`${data.course_deg}°`);
  if (typeof data.altitude_m === 'number') parts.push(`${Math.round(data.altitude_m)} m alt`);
  return parts.join(' · ');
}
