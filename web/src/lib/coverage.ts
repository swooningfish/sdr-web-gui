import type { FreqRange, RadioState } from './api';

/** Same rule as `radio::covers`: no ranges = unknown device, nothing ruled out. */
export function covers(ranges: FreqRange[] | null | undefined, hz: number): boolean {
  return !ranges || ranges.length === 0 || ranges.some((r) => hz >= r.lo_hz && hz <= r.hi_hz);
}

/** "24–1766 MHz" or "146–308, 438–924 MHz". */
export function describeRanges(ranges: FreqRange[]): string {
  if (ranges.length === 0) return 'unknown range';
  const mhz = (hz: number) => String(Math.round(hz / 1e5) / 10);
  return `${ranges.map((r) => `${mhz(r.lo_hz)}–${mhz(r.hi_hz)}`).join(', ')} MHz`;
}

/**
 * Why the selected SDR can't receive `hz`, or null if it can. Used to show
 * features as disabled.
 */
export function unsupported(radio: RadioState | null | undefined, hz: number): string | null {
  if (!radio || covers(radio.ranges, hz)) return null;
  return `Outside this SDR's range (${describeRanges(radio.ranges)})`;
}
