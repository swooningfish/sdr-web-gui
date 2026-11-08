/** Google "Turbo" colormap (polynomial approximation), 256 entries of RGB. */
export function turbo(): Uint8ClampedArray {
  const out = new Uint8ClampedArray(256 * 3);
  for (let i = 0; i < 256; i++) {
    const x = i / 255;
    const r =
      0.13572138 + x * (4.6153926 + x * (-42.66032258 + x * (132.13108234 + x * (-152.94239396 + x * 59.28637943))));
    const g =
      0.09140261 + x * (2.19418839 + x * (4.84296658 + x * (-14.18503333 + x * (4.27729857 + x * 2.82956604))));
    const b =
      0.1066733 + x * (12.64194608 + x * (-60.58204836 + x * (110.36276771 + x * (-89.90310912 + x * 27.34824973))));
    out[i * 3] = r * 255;
    out[i * 3 + 1] = g * 255;
    out[i * 3 + 2] = b * 255;
  }
  return out;
}

/** Linear gradient through evenly spaced RGB stops, 256 entries. */
function gradient(stops: [number, number, number][]): Uint8ClampedArray {
  const out = new Uint8ClampedArray(256 * 3);
  const n = stops.length - 1;
  for (let i = 0; i < 256; i++) {
    const t = (i / 255) * n;
    const k = Math.min(n - 1, Math.floor(t));
    const f = t - k;
    for (let c = 0; c < 3; c++) out[i * 3 + c] = stops[k][c] + (stops[k + 1][c] - stops[k][c]) * f;
  }
  return out;
}

export const PALETTES: { id: string; label: string; make: () => Uint8ClampedArray }[] = [
  { id: 'turbo', label: 'Turbo', make: turbo },
  {
    id: 'classic',
    label: 'Classic SDR',
    make: () => gradient([[0, 0, 0], [0, 0, 140], [0, 120, 255], [0, 255, 255], [255, 255, 0], [255, 60, 0], [255, 255, 255]]),
  },
  {
    id: 'viridis',
    label: 'Viridis',
    make: () => gradient([[68, 1, 84], [59, 82, 139], [33, 145, 140], [94, 201, 98], [253, 231, 37]]),
  },
  {
    id: 'inferno',
    label: 'Inferno',
    make: () => gradient([[0, 0, 4], [87, 16, 110], [188, 55, 84], [249, 142, 9], [252, 255, 164]]),
  },
  { id: 'green', label: 'Green screen', make: () => gradient([[0, 0, 0], [0, 70, 20], [0, 190, 60], [180, 255, 180]]) },
  { id: 'grey', label: 'Greyscale', make: () => gradient([[0, 0, 0], [255, 255, 255]]) },
];
