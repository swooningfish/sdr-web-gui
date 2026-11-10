/** Geometry for the satellite map: coastlines, ground tracks, footprints and
 * the day/night line. Angles are degrees; longitudes are east-positive. */

import landUrl from 'world-atlas/land-110m.json?url';
import type { GroundPoint, Observer } from './api';

export const RE_KM = 6378.137;
const DEG = Math.PI / 180;

export type LonLat = [number, number];

/** Longitude folded into −180…180. */
export const norm = (lon: number) => ((((lon + 180) % 360) + 360) % 360) - 180;

interface Topology {
  transform: { scale: [number, number]; translate: [number, number] };
  arcs: [number, number][][];
  objects: { land: { geometries: { type: string; arcs: number[][] | number[][][] }[] } };
}

let landPromise: Promise<LonLat[][]> | null = null;

/** Coastline rings (Natural Earth 1:110m, bundled with the app). */
export function loadLand(): Promise<LonLat[][]> {
  landPromise ??= fetch(landUrl)
    .then((r) => r.json())
    .then((topo: Topology) => {
      const { scale, translate } = topo.transform;
      const arcs = topo.arcs.map((arc) => {
        let x = 0;
        let y = 0;
        return arc.map(([dx, dy]): LonLat => {
          x += dx;
          y += dy;
          return [x * scale[0] + translate[0], y * scale[1] + translate[1]];
        });
      });
      const ring = (ids: number[]) =>
        ids.flatMap((i, k) => {
          const a = i < 0 ? arcs[~i].slice().reverse() : arcs[i];
          return k ? a.slice(1) : a;
        });
      const rings: LonLat[][] = [];
      for (const g of topo.objects.land.geometries) {
        const polys = g.type === 'MultiPolygon' ? (g.arcs as number[][][]) : [g.arcs as number[][]];
        for (const poly of polys) for (const r of poly) rings.push(ring(r));
      }
      return rings;
    });
  landPromise.catch(() => (landPromise = null));
  return landPromise;
}

/** Where the sun is overhead. */
export function sunPoint(ms: number): { lat: number; lon: number } {
  const d = ms / 86_400_000 - 10_957.5; // days since J2000
  const g = (357.529 + 0.985_600_28 * d) * DEG;
  const lambda = (280.459 + 0.985_647_36 * d + 1.915 * Math.sin(g) + 0.02 * Math.sin(2 * g)) * DEG;
  const eps = (23.439 - 3.6e-7 * d) * DEG;
  const ra = Math.atan2(Math.cos(eps) * Math.sin(lambda), Math.cos(lambda)) / DEG;
  const gmst = 280.460_618_37 + 360.985_647_366_29 * d;
  return { lat: Math.asin(Math.sin(eps) * Math.sin(lambda)) / DEG, lon: norm(ra - gmst) };
}

/** Latitude of the day/night line at a longitude; night is on the far side
 * of it from the sun's hemisphere. */
export function terminatorLat(lon: number, sun: { lat: number; lon: number }): number {
  const dec = (Math.abs(sun.lat) < 0.05 ? 0.05 * (sun.lat < 0 ? -1 : 1) : sun.lat) * DEG;
  return Math.atan(-Math.cos((lon - sun.lon) * DEG) / Math.tan(dec)) / DEG;
}

/** Angle at the Earth's centre between two ground points. */
export function arcDeg(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const c =
    Math.sin(lat1 * DEG) * Math.sin(lat2 * DEG) + Math.cos(lat1 * DEG) * Math.cos(lat2 * DEG) * Math.cos((lon1 - lon2) * DEG);
  return Math.acos(Math.max(-1, Math.min(1, c))) / DEG;
}

/** How far (as an angle at the Earth's centre) a satellite at this height
 * can be seen from: the radius of its footprint. */
export const footprintDeg = (altKm: number) => Math.acos(RE_KM / (RE_KM + Math.max(0, altKm))) / DEG;

/** Points [lat, lon] on the circle `radiusDeg` around a ground point. */
export function ringAround(lat: number, lon: number, radiusDeg: number, n = 72): [number, number][] {
  const d = radiusDeg * DEG;
  const p1 = lat * DEG;
  const out: [number, number][] = [];
  for (let i = 0; i <= n; i++) {
    const t = (i / n) * 2 * Math.PI;
    const p2 = Math.asin(Math.sin(p1) * Math.cos(d) + Math.cos(p1) * Math.sin(d) * Math.cos(t));
    const dl = Math.atan2(Math.sin(t) * Math.sin(d) * Math.cos(p1), Math.cos(d) - Math.sin(p1) * Math.sin(p2));
    out.push([p2 / DEG, lon + dl / DEG]);
  }
  return out;
}

export interface SatPos {
  lat: number;
  lon: number;
  altKm: number;
}

/** Position at `t`, between the two nearest track points; null outside the track. */
export function posAt(points: GroundPoint[], t: number): SatPos | null {
  if (!points.length || t < points[0][0] || t > points[points.length - 1][0]) return null;
  let lo = 0;
  let hi = points.length - 1;
  while (hi - lo > 1) {
    const m = (lo + hi) >> 1;
    if (points[m][0] <= t) lo = m;
    else hi = m;
  }
  const a = points[lo];
  const b = points[hi];
  const f = b[0] === a[0] ? 0 : (t - a[0]) / (b[0] - a[0]);
  return {
    lat: a[1] + (b[1] - a[1]) * f,
    lon: norm(a[2] + norm(b[2] - a[2]) * f),
    altKm: a[3] + (b[3] - a[3]) * f,
  };
}

/** Is a satellite over this point above the observer's horizon? */
export function inRange(obs: Observer | null, p: SatPos): boolean {
  return !!obs && arcDeg(obs.lat_deg, obs.lon_deg, p.lat, p.lon) < footprintDeg(p.altKm);
}

/** Kilometres over the ground between the observer and a point. */
export const groundKm = (obs: Observer, p: SatPos) => arcDeg(obs.lat_deg, obs.lon_deg, p.lat, p.lon) * DEG * RE_KM;

/** "51.5°N 0.1°W" */
export function latLon(lat: number, lon: number): string {
  return `${Math.abs(lat).toFixed(1)}°${lat >= 0 ? 'N' : 'S'} ${Math.abs(lon).toFixed(1)}°${lon >= 0 ? 'E' : 'W'}`;
}

/** Everything either view draws. */
export interface MapScene {
  now: number;
  observer: Observer | null;
  sun: { lat: number; lon: number };
  sats: { name: string; pos: SatPos; inRange: boolean; focus: boolean }[];
  /** Ground track of the focused satellite around now. */
  track: { pos: SatPos; inRange: boolean }[];
  /** Ground track of the pass picked in the table, when it isn't in `track`. */
  passTrack: { pos: SatPos; inRange: boolean }[];
  /** Where the satellite is at the highest point of that pass. */
  peak: { pos: SatPos; label: string } | null;
}

export interface MapColours {
  ocean: string;
  land: string;
  grid: string;
  text: string;
  muted: string;
  sat: string;
  good: string;
  track: string;
  surface: string;
}

export function mapColours(el: Element): MapColours {
  const s = getComputedStyle(el);
  const v = (n: string) => s.getPropertyValue(n).trim();
  return {
    ocean: v('--bg'),
    land: v('--border'),
    grid: v('--surface-2'),
    text: v('--text'),
    muted: v('--muted'),
    sat: v('--accent'),
    good: v('--ok'),
    track: v('--warn'),
    surface: v('--surface'),
  };
}

/** Paint ocean and land onto an equirectangular canvas spanning −180…180. */
export function paintLand(c: CanvasRenderingContext2D, w: number, h: number, land: LonLat[][], col: MapColours) {
  c.fillStyle = col.ocean;
  c.fillRect(0, 0, w, h);
  c.fillStyle = col.land;
  c.beginPath();
  for (const ring of land) {
    // A coastline that crosses the date line is followed past the edge and
    // drawn again a world away, rather than cutting back across the map.
    let prev = ring[0][0];
    let crosses = false;
    const pts = ring.map(([lon, lat]): LonLat => {
      if (Math.abs(lon - prev) > 180) crosses = true;
      prev += norm(lon - prev);
      return [prev, lat];
    });
    for (const off of crosses ? [-360, 0, 360] : [0]) {
      pts.forEach(([lon, lat], i) => {
        const x = ((lon + off + 180) / 360) * w;
        const y = ((90 - lat) / 180) * h;
        if (i) c.lineTo(x, y);
        else c.moveTo(x, y);
      });
      c.closePath();
    }
  }
  c.fill('evenodd');
}
