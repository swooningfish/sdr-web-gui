/** ADS-B: aircraft icons, altitude colours and labels. */

import type { Aircraft } from './api';

export type IconKind =
  | 'light'
  | 'jet'
  | 'airliner'
  | 'heavy'
  | 'fast'
  | 'heli'
  | 'glider'
  | 'balloon'
  | 'drone'
  | 'vehicle'
  | 'unknown';

/** Silhouettes in a 32×32 box, nose up (drawn for this project). */
const PATHS: Record<IconKind, string> = {
  airliner:
    'M16 1.5c1.1 0 1.9 1.2 1.9 3v7.2l11.6 6.6v2.6l-11.6-3.3v6.4l3.2 2.4v2.1l-5.1-1.4-5.1 1.4v-2.1l3.2-2.4v-6.4L2.5 20.9v-2.6l11.6-6.6V4.5c0-1.8.8-3 1.9-3zM8.3 15.4h2.2v3.3H8.3zM21.5 15.4h2.2v3.3h-2.2z',
  heavy:
    'M16 1c1.2 0 2 1.3 2 3.2v7.3l12.3 6.9v2.8L18 17.8v6.6l3.4 2.5v2.2L16 27.6l-5.4 1.5v-2.2l3.4-2.5v-6.6L1.7 21.2v-2.8L14 11.5V4.2C14 2.3 14.8 1 16 1zM9.6 14.9h2v3.1h-2zM5.3 17.3h2v3.1h-2zM20.4 14.9h2v3.1h-2zM24.7 17.3h2v3.1h-2z',
  jet: 'M16 2.5c.9 0 1.6 1 1.6 2.6v7.4l9.4 6v2.1l-9.4-2.9v5.6l2.8 2.1v1.8L16 26l-4.4 1.2v-1.8l2.8-2.1v-5.6L5 20.6v-2.1l9.4-6V5.1c0-1.6.7-2.6 1.6-2.6z',
  light:
    'M14.9 3.2h2.2l.4 6.8H29v3.2H17.4l-.5 9.2h4.3v2.6h-4.4l-.8 2.2-.8-2.2h-4.4v-2.6h4.3l-.5-9.2H3V10h11.5zM12.4 2h7.2v1.2h-7.2z',
  fast: 'M16 1.5l1.6 5.5v6l11 10.5v2.4l-11-3.6v3.8l3 2.3v1.6L16 29l-4.6 1v-1.6l3-2.3v-3.8l-11 3.6v-2.4l11-10.5V7z',
  heli: 'M16 7.2c2.2 0 4 2.7 4 6s-1.8 5.8-4 5.8-4-2.5-4-5.8 1.8-6 4-6zM15.2 18.5h1.6v8.6h-1.6zM12.4 26.2h7.2v1.5h-7.2zM5.2 3.4l1-1 20.6 20.6-1 1zM26.8 3.4l-1-1L5.2 23l1 1z',
  glider:
    'M15.4 4h1.2l.5 7.4H31v1.6H17l-.5 13.2h3.6v1.4h-3.6l-.9.8-.9-.8h-3.6v-1.4h3.6L15 13H1v-1.6h13.9z',
  balloon:
    'M16 2a8 8 0 0 1 8 8c0 4.4-4 7.8-6 10.6h-4C12 17.8 8 14.4 8 10a8 8 0 0 1 8-8zM13.6 23h4.8v4h-4.8zM14 20.6h.9l.3 2.4h-.9zM17.1 20.6h.9l-.3 2.4h-.9z',
  drone:
    'M13 13h6v6h-6zM10.6 9.6l1-1 11.8 11.8-1 1zM21.4 8.6l1 1-11.8 11.8-1-1zM9 5.6a3.4 3.4 0 1 1 0 6.8 3.4 3.4 0 0 1 0-6.8zM23 5.6a3.4 3.4 0 1 1 0 6.8 3.4 3.4 0 0 1 0-6.8zM9 19.6a3.4 3.4 0 1 1 0 6.8 3.4 3.4 0 0 1 0-6.8zM23 19.6a3.4 3.4 0 1 1 0 6.8 3.4 3.4 0 0 1 0-6.8z',
  vehicle: 'M11 5h10a2 2 0 0 1 2 2v18a2 2 0 0 1-2 2H11a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2zM11.5 8v4h9V8z',
  unknown: 'M16 3l9 24-9-5-9 5z',
};

/** On-screen size (px) for each kind. */
const SIZES: Record<IconKind, number> = {
  light: 22,
  jet: 24,
  airliner: 28,
  heavy: 34,
  fast: 26,
  heli: 26,
  glider: 28,
  balloon: 22,
  drone: 20,
  vehicle: 16,
  unknown: 22,
};

/** Icons that keep pointing up (they have no nose). */
const UPRIGHT = new Set<IconKind>(['balloon', 'drone']);

/** What the emitter categories mean. */
export const CATEGORIES: Record<string, string> = {
  A1: 'Light (< 7 t)',
  A2: 'Small (7–34 t)',
  A3: 'Large (34–136 t)',
  A4: 'High-vortex large (e.g. B757)',
  A5: 'Heavy (> 136 t)',
  A6: 'High performance',
  A7: 'Rotorcraft',
  B1: 'Glider / sailplane',
  B2: 'Balloon / airship',
  B3: 'Parachutist / skydiver',
  B4: 'Ultralight / hang glider',
  B6: 'Unmanned aircraft (drone)',
  B7: 'Space vehicle',
  C1: 'Emergency vehicle (surface)',
  C2: 'Service vehicle (surface)',
  C3: 'Obstacle',
};

const HELI_TYPES = /^(EC\d\d|H\d{3}|AS\d\d|A1(09|19|29|39|49|69|89)|AW\d|B(06|07|105|212|222|230|407|412|427|429|430|47G|505)|R22|R44|R66|S(61|76|92)|NH90|EH10|PUMA|LYNX|MRLX|UH1|H60|CH47|EXPL|G2CA|BK17|ALO3|GAZL)$/;

export function iconKind(a: Pick<Aircraft, 'category' | 'type_code' | 'desc' | 'on_ground'>): IconKind {
  switch (a.category) {
    case 'A1':
    case 'B4':
      return 'light';
    case 'A2':
      return 'jet';
    case 'A3':
    case 'A4':
      return 'airliner';
    case 'A5':
      return 'heavy';
    case 'A6':
    case 'B7':
      return 'fast';
    case 'A7':
      return 'heli';
    case 'B1':
      return 'glider';
    case 'B2':
      return 'balloon';
    case 'B6':
      return 'drone';
    case 'C1':
    case 'C2':
      return 'vehicle';
  }
  // No category: guess from the database's type.
  if (a.type_code && HELI_TYPES.test(a.type_code)) return 'heli';
  if (a.desc && /HELICOPTER|COPTER|ROTORCRAFT/i.test(a.desc)) return 'heli';
  if (a.desc && /GLIDER|SAILPLANE/i.test(a.desc)) return 'glider';
  return 'unknown';
}

/** Altitude → colour: warm low down, cool high up; grey on the ground. */
const STOPS: [number, number][] = [
  [0, 22],
  [2000, 42],
  [5000, 68],
  [10000, 125],
  [20000, 190],
  [30000, 235],
  [40000, 285],
];

export function altHue(ft: number): number {
  if (ft <= STOPS[0][0]) return STOPS[0][1];
  for (let i = 1; i < STOPS.length; i++) {
    const [a, ha] = STOPS[i - 1];
    const [b, hb] = STOPS[i];
    if (ft <= b) return ha + ((ft - a) / (b - a)) * (hb - ha);
  }
  return STOPS[STOPS.length - 1][1];
}

export function altColour(ft: number | null, ground = false): string {
  if (ground) return 'hsl(220 8% 62%)';
  if (ft === null) return 'hsl(220 10% 75%)';
  return `hsl(${altHue(ft).toFixed(0)} 85% 55%)`;
}

/** Legend stops (feet). */
export const LEGEND_FT = [0, 2000, 5000, 10000, 20000, 30000, 40000];

export function legendGradient(): string {
  const max = 40000;
  return `linear-gradient(to right, ${STOPS.map(([ft, h]) => `hsl(${h} 85% 55%) ${((ft / max) * 100).toFixed(1)}%`).join(', ')})`;
}

/** SVG markup for an aircraft, rotated to its track. */
export function iconSvg(kind: IconKind, colour: string, track: number | null, selected: boolean): string {
  const size = SIZES[kind];
  const rot = UPRIGHT.has(kind) || track === null ? 0 : track;
  const stroke = selected ? '#fff' : 'rgba(0,0,0,0.75)';
  return `<svg viewBox="0 0 32 32" width="${size}" height="${size}" style="transform: rotate(${rot.toFixed(0)}deg)" aria-hidden="true"><path d="${PATHS[kind]}" fill="${colour}" stroke="${stroke}" stroke-width="${selected ? 2 : 1}" stroke-linejoin="round" paint-order="stroke"/></svg>`;
}

export function iconSize(kind: IconKind): number {
  return SIZES[kind];
}

/** Callsign, else registration, else the ICAO address. */
export function aircraftName(a: Pick<Aircraft, 'callsign' | 'reg' | 'icao'>): string {
  return a.callsign || a.reg || a.icao.toUpperCase();
}

export function fmtAlt(a: Pick<Aircraft, 'alt_baro_ft' | 'alt_gnss_ft' | 'on_ground'>): string {
  if (a.on_ground) return 'ground';
  const ft = a.alt_baro_ft ?? a.alt_gnss_ft;
  return ft === null ? '–' : `${ft.toLocaleString('en-GB')} ft`;
}

/** Flight level for map labels: "FL370", or feet below the transition. */
export function shortAlt(a: Pick<Aircraft, 'alt_baro_ft' | 'alt_gnss_ft' | 'on_ground'>): string {
  if (a.on_ground) return 'GND';
  const ft = a.alt_baro_ft ?? a.alt_gnss_ft;
  if (ft === null) return '';
  return ft >= 6000 ? `FL${Math.round(ft / 100)}` : `${Math.round(ft / 100) * 100}`;
}

export function fmtVrate(fpm: number | null): string {
  if (fpm === null) return '–';
  if (Math.abs(fpm) < 100) return 'level';
  return `${fpm > 0 ? '↑' : '↓'} ${Math.abs(fpm).toLocaleString('en-GB')} ft/min`;
}

export const EMERGENCY_SQUAWKS: Record<string, string> = {
  '7500': 'hijack',
  '7600': 'radio failure',
  '7700': 'emergency',
};

/** Emergency text from the status message or a special squawk. */
export function emergency(a: Pick<Aircraft, 'emergency' | 'squawk'>): string | null {
  return a.emergency ?? (a.squawk ? (EMERGENCY_SQUAWKS[a.squawk] ?? null) : null);
}

export const compass = (deg: number) =>
  ['N', 'NNE', 'NE', 'ENE', 'E', 'ESE', 'SE', 'SSE', 'S', 'SSW', 'SW', 'WSW', 'W', 'WNW', 'NW', 'NNW'][
    Math.round(deg / 22.5) % 16
  ];
