/** Spectrum zoom and the mini map's frequency scale. */

import type { FreqRange } from './api';

/** Zoomed in, at least this many FFT bins stay in view. */
export const MIN_BINS = 32;
/** Zoomed out, the view spans at most about this much. */
export const MAX_SPAN_HZ = 100e6;

/**
 * Zoom limits for a sample rate and FFT size, as powers of two: zoom > 1
 * shows part of the sampled band, zoom < 1 shows more than is sampled.
 */
export function zoomLimits(sampleRate: number, bins: number): [number, number] {
  const out = 2 ** Math.floor(Math.log2(Math.max(1, MAX_SPAN_HZ / sampleRate)));
  const inn = 2 ** Math.floor(Math.log2(Math.max(1, bins / MIN_BINS)));
  return [1 / out, inn];
}

/** "×4", "×1", "÷8", "×2.6". */
export function zoomLabel(z: number): string {
  const r = z >= 1 ? z : 1 / z;
  const s = Math.abs(r - Math.round(r)) < 0.05 ? String(Math.round(r)) : r.toFixed(1);
  return (z >= 1 ? '×' : '÷') + s;
}

/**
 * Frequencies the mini map covers: the receiver's ranges (or the usual
 * R820T range when unknown), down to HF when direct sampling is on, and
 * always including the sampled window.
 */
export function fullRange(
  ranges: FreqRange[],
  directSampling: number,
  winLo: number,
  winHi: number,
): { lo: number; hi: number } {
  let lo = ranges.length ? Math.min(...ranges.map((r) => r.lo_hz)) : 24e6;
  const hi = ranges.length ? Math.max(...ranges.map((r) => r.hi_hz)) : 1766e6;
  if (directSampling > 0) lo = Math.min(lo, 500e3);
  return { lo: Math.max(100e3, Math.min(lo, winLo)), hi: Math.max(hi, winHi) };
}

/** Log frequency scale across `width` pixels. */
export function logScale(lo: number, hi: number, width: number) {
  const l0 = Math.log(lo);
  const k = width / (Math.log(hi) - l0);
  return {
    x: (hz: number) => (Math.log(Math.max(hz, lo)) - l0) * k,
    hz: (x: number) => Math.exp(l0 + Math.min(width, Math.max(0, x)) / k),
  };
}
