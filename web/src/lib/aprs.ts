import type { Device } from './api';

/** Icon shapes used for APRS symbols. Several symbols share a shape. */
export type SymbolKind =
  | 'car' | 'van' | 'truck' | 'bus' | 'ambulance' | 'motorcycle' | 'bicycle' | 'train'
  | 'aircraft' | 'helicopter' | 'balloon' | 'shuttle' | 'satellite' | 'boat'
  | 'person' | 'dog' | 'house' | 'tent' | 'digipeater' | 'repeater' | 'gateway' | 'node'
  | 'weather' | 'police' | 'fire' | 'medical' | 'flag' | 'computer' | 'laptop' | 'dish'
  | 'yagi' | 'eye' | 'dot';

/** Names and icon shapes for the common APRS symbols (table + code). */
const SYMBOLS: Record<string, [string, SymbolKind]> = {
  '/!': ['Police', 'police'], '/#': ['Digipeater', 'digipeater'], '/&': ['Gateway', 'gateway'],
  "/'": ['Small aircraft', 'aircraft'], '/(': ['Mobile satellite station', 'dish'],
  '/)': ['Wheelchair', 'person'], '/*': ['Snowmobile', 'car'], '/+': ['Red Cross', 'medical'],
  '/,': ['Scouts', 'person'], '/-': ['House', 'house'], '/:': ['Fire', 'fire'],
  '/;': ['Campground', 'tent'], '/<': ['Motorcycle', 'motorcycle'], '/=': ['Train', 'train'],
  '/>': ['Car', 'car'], '/?': ['Server', 'computer'], '/A': ['Aid station', 'medical'],
  '/B': ['BBS', 'computer'], '/C': ['Canoe', 'boat'], '/E': ['Eyeball', 'eye'],
  '/F': ['Farm vehicle', 'truck'], '/H': ['Hotel', 'house'], '/I': ['TCP/IP', 'computer'],
  '/K': ['School', 'house'], '/L': ['PC user', 'computer'], '/N': ['NTS station', 'flag'],
  '/O': ['Balloon', 'balloon'], '/P': ['Police', 'police'], '/R': ['RV', 'van'],
  '/S': ['Shuttle', 'shuttle'], '/T': ['SSTV', 'computer'], '/U': ['Bus', 'bus'], '/V': ['ATV', 'car'],
  '/W': ['Weather service', 'weather'], '/X': ['Helicopter', 'helicopter'], '/Y': ['Yacht', 'boat'],
  '/[': ['Person', 'person'], '/^': ['Aircraft', 'aircraft'], '/_': ['Weather station', 'weather'],
  '/`': ['Dish antenna', 'dish'], '/a': ['Ambulance', 'ambulance'], '/b': ['Bicycle', 'bicycle'],
  '/c': ['Incident command post', 'flag'], '/d': ['Fire station', 'fire'], '/f': ['Fire engine', 'fire'],
  '/g': ['Glider', 'aircraft'], '/h': ['Hospital', 'medical'], '/j': ['Jeep', 'car'], '/k': ['Truck', 'truck'],
  '/l': ['Laptop', 'laptop'], '/m': ['Mic-E repeater', 'repeater'], '/n': ['Node', 'node'],
  '/o': ['Emergency operations centre', 'flag'], '/p': ['Dog', 'dog'], '/r': ['Repeater', 'repeater'],
  '/s': ['Boat', 'boat'], '/t': ['Truck stop', 'truck'], '/u': ['Lorry', 'truck'], '/v': ['Van', 'van'],
  '/x': ['Xastir', 'computer'], '/y': ['Yagi at QTH', 'yagi'],
  // Alternate table (backslash).
  '\\#': ['Digipeater', 'digipeater'], '\\&': ['Gateway', 'gateway'], '\\-': ['House', 'house'],
  '\\>': ['Car', 'car'], '\\S': ['Satellite', 'satellite'], '\\^': ['Aircraft', 'aircraft'],
  '\\_': ['Weather station', 'weather'], '\\n': ['Node', 'node'], '\\k': ['SUV', 'car'], '\\j': ['Jeep', 'car'],
  '\\s': ['Boat', 'boat'], '\\u': ['Lorry', 'truck'],
};

function lookup(sym: string): [string, SymbolKind] | undefined {
  // Overlay characters replace the table byte; fall back to the alternate table.
  return SYMBOLS[sym] ?? SYMBOLS['\\' + sym[1]];
}

export function symbolName(sym: string | null | undefined): string {
  if (!sym) return '';
  return lookup(sym)?.[0] ?? `Symbol ${sym}`;
}

export function symbolKind(sym: string | null | undefined): SymbolKind {
  return (sym && lookup(sym)?.[1]) || 'dot';
}

// Wheels sit on y=17; body lines stop at their edges.
const WHEELS = (a: number, b: number) => `<circle cx="${a}" cy="17" r="2"/><circle cx="${b}" cy="17" r="2"/>`;

/** 24×24 line drawings in currentColor, one per kind. */
const ICONS: Record<SymbolKind, string> = {
  car: `<path d="M4.5 17H3v-4.5L5 12l2.2-3.6a1 1 0 0 1 .9-.4h7.4a1 1 0 0 1 .8.4L19 12l2 .5V17h-1.5M9 17h6M5 12h14"/>${WHEELS(7, 17)}`,
  van: `<path d="M4.5 17H2V7h12l4 4.5 3 1V17h-2M8.5 17H15M14 7v4.5h4"/>${WHEELS(6.5, 17)}`,
  truck: `<path d="M4.5 17H2V6h12v11M8.5 17H15M14 9h4l3 4v4h-2"/>${WHEELS(6.5, 17)}`,
  bus: `<rect x="2" y="4" width="20" height="12" rx="2"/><path d="M2 10h20M8 4v6M14 4v6M6 16v3M18 16v3"/>`,
  ambulance: `<path d="M4.5 17H2V7h12l4 4.5 3 1V17h-2M8.5 17H15M14 7v4.5h4M8 9.5v4M6 11.5h4"/>${WHEELS(6.5, 17)}`,
  motorcycle: `<circle cx="5" cy="16" r="3"/><circle cx="19" cy="16" r="3"/><path d="M5 16l3.5-5H14l5 5M14 11l-1.5-4H15M8.5 11 7 8.5h3"/>`,
  bicycle: `<circle cx="5.5" cy="16" r="3.5"/><circle cx="18.5" cy="16" r="3.5"/><path d="M5.5 16H12l4-7H9.5l2.5 7M16 9l2.5 7M9.5 9 8.8 6.5H11M16 9l-.8-3h2"/>`,
  train: `<rect x="5" y="3" width="14" height="14" rx="3"/><path d="M5 10h14M9 21l1.5-4M15 21l-1.5-4"/><circle cx="9" cy="13.5" r="1" fill="currentColor" stroke="none"/><circle cx="15" cy="13.5" r="1" fill="currentColor" stroke="none"/>`,
  aircraft: `<path fill="currentColor" stroke="none" d="M12 2c.9 0 1.4 1 1.4 2.2v4.6l7.6 4.4v2.1l-7.6-2.2v4.5l2.1 1.6V21L12 20.1 8.5 21v-1.8l2.1-1.6v-4.5L3 15.3v-2.1l7.6-4.4V4.2C10.6 3 11.1 2 12 2z"/>`,
  helicopter: `<path d="M4 5h16M14 5v3M9 12a4 4 0 0 1 4-4h2a4 4 0 0 1 0 8h-2a4 4 0 0 1-4-4zM9 12H3l-1-2M10 20h10M12 16v4M18 16v4"/>`,
  balloon: `<path d="M12 3a6 6 0 0 0-6 6c0 3.5 3.5 6 5 8h2c1.5-2 5-4.5 5-8a6 6 0 0 0-6-6zM11 17v2M13 17v2"/><rect x="10.5" y="19" width="3" height="2.5" rx=".5"/>`,
  shuttle: `<path d="M12 2c3 2.5 4 6 4 10v4H8v-4c0-4 1-7.5 4-10zM8 13l-3 3v3l3-1.5M16 13l3 3v3l-3-1.5M10.5 19.5 12 22l1.5-2.5"/>`,
  satellite: `<path d="M9 12l3-3 3 3-3 3zM3 7.5 7.5 3l3 3L6 10.5zM13.5 18l4.5-4.5 3 3-4.5 4.5zM8.25 8.25l1.5 1.5M14.25 14.25l1.5 1.5"/>`,
  boat: `<path d="M3 15h18l-2.5 4.5h-13zM12 3v12M12 4l6 9h-6"/>`,
  person: `<circle cx="12" cy="5" r="2"/><path d="M12 8v6M7.5 10.5h9M12 14l-3 6.5M12 14l3 6.5"/>`,
  dog: `<g fill="currentColor" stroke="none"><ellipse cx="5.5" cy="11" rx="1.8" ry="2.2"/><ellipse cx="9.3" cy="6.3" rx="1.8" ry="2.3"/><ellipse cx="14.7" cy="6.3" rx="1.8" ry="2.3"/><ellipse cx="18.5" cy="11" rx="1.8" ry="2.2"/><path d="M12 11.5c-2.5 0-5 3.5-5 6 0 1.5 1 2.5 2.5 2.5 1 0 1.5-.5 2.5-.5s1.5.5 2.5.5c1.5 0 2.5-1 2.5-2.5 0-2.5-2.5-6-5-6z"/></g>`,
  house: `<path d="M3 11 12 3.5l9 7.5M5 9.5V20h14V9.5M10 20v-5h4v5"/>`,
  tent: `<path d="M3 20 12 4l9 16zM12 20l-2.5-5h5z"/>`,
  digipeater: `<path d="M12 10v11M8.5 21 12 10l3.5 11M8.5 4.5a5 5 0 0 0 0 7M15.5 4.5a5 5 0 0 1 0 7M5.5 2a9 9 0 0 0 0 12M18.5 2a9 9 0 0 1 0 12"/><circle cx="12" cy="8" r="1.5" fill="currentColor" stroke="none"/>`,
  repeater: `<path d="M4 8h15l-3.5-3.5M20 16H5l3.5 3.5"/>`,
  gateway: `<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>`,
  node: `<circle cx="12" cy="5" r="2.5"/><circle cx="5" cy="18" r="2.5"/><circle cx="19" cy="18" r="2.5"/><path d="M10.8 7.2 6.2 15.8M13.2 7.2l4.6 8.6M7.5 18h9"/>`,
  weather: `<path d="M7 19a4.5 4.5 0 0 1-.6-9A6 6 0 0 1 17.6 9 5 5 0 0 1 17 19z"/>`,
  police: `<path d="M12 3 4.5 6v5.5c0 4.5 3.2 8 7.5 9.5 4.3-1.5 7.5-5 7.5-9.5V6z"/><path d="m12 8 1.2 2.4 2.6.4-1.9 1.8.5 2.6L12 14l-2.4 1.2.5-2.6-1.9-1.8 2.6-.4z" fill="currentColor" stroke="none"/>`,
  fire: `<path d="M12 21a6 6 0 0 0 6-6c0-4-3-6-4-10-1.5 2-2 3.5-2 5-1-1-2-1.5-2.5-3C7.5 9 6 11.5 6 15a6 6 0 0 0 6 6z"/>`,
  medical: `<path d="M9 3h6v6h6v6h-6v6H9v-6H3V9h6z"/>`,
  flag: `<path d="M5 21V4h12l-2.5 4L17 12H5"/>`,
  computer: `<rect x="3" y="4" width="18" height="12" rx="1.5"/><path d="M8 20h8M12 16v4"/>`,
  laptop: `<rect x="5" y="5" width="14" height="10" rx="1"/><path d="M2.5 19h19l-2-4h-15z"/>`,
  dish: `<path d="M5.5 8.5a8 8 0 0 0 10 10zM10.5 13.5 15 9M8 21h6"/><circle cx="16" cy="8" r="1.3"/>`,
  yagi: `<path d="M3 9h18M6 4.5v9M10 5.5v7M14 6.5v5M18 7.5v3M12 9v12"/>`,
  eye: `<path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12z"/><circle cx="12" cy="12" r="2.5"/>`,
  dot: `<circle cx="12" cy="12" r="4.5" fill="currentColor" stroke="none"/>`,
};

/** Inline SVG for a symbol's icon. The markup is all constants, so it's safe for {@html}. */
export function symbolIcon(sym: string | null | undefined, size = 16): string {
  return (
    `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" ` +
    `stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[symbolKind(sym)]}</svg>`
  );
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
