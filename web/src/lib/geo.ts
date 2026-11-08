/** Maidenhead locator (2, 4, 6 or 8 characters, e.g. "IO91wm") → centre of
 * the square, or null if it isn't a locator. */
export function maidenhead(loc: string): { lat: number; lon: number } | null {
  const s = loc.trim();
  if (!/^[A-Ra-r]{2}(\d{2}([A-Xa-x]{2}(\d{2})?)?)?$/.test(s)) return null;
  const u = s.toUpperCase();
  const A = 'A'.charCodeAt(0);
  let lon = -180 + (u.charCodeAt(0) - A) * 20;
  let lat = -90 + (u.charCodeAt(1) - A) * 10;
  let dlon = 20;
  let dlat = 10;
  if (u.length >= 4) {
    dlon = 2;
    dlat = 1;
    lon += Number(u[2]) * dlon;
    lat += Number(u[3]) * dlat;
  }
  if (u.length >= 6) {
    dlon = 2 / 24;
    dlat = 1 / 24;
    lon += (u.charCodeAt(4) - A) * dlon;
    lat += (u.charCodeAt(5) - A) * dlat;
  }
  if (u.length >= 8) {
    dlon /= 10;
    dlat /= 10;
    lon += Number(u[6]) * dlon;
    lat += Number(u[7]) * dlat;
  }
  return { lat: lat + dlat / 2, lon: lon + dlon / 2 };
}
