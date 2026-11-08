const DIRS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'];

/** 8-point compass direction for an azimuth. */
export function compass(az: number): string {
  return DIRS[Math.round((((az % 360) + 360) % 360) / 45) % 8];
}

/** "in 2 h 13 min", "in 45 s", "now". */
export function countdown(ms: number, now = Date.now()): string {
  const s = Math.round((ms - now) / 1000);
  if (s <= 0) return 'now';
  if (s < 60) return `in ${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `in ${m} min`;
  const h = Math.floor(m / 60);
  return `in ${h} h ${m % 60} min`;
}

/** "10 min 51 s" */
export function duration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  const m = Math.floor(s / 60);
  return m ? `${m} min ${s % 60} s` : `${s} s`;
}
