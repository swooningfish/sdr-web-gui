/** SSTV mode names by VIS code (same table as sstv/modes.rs). */
export const SSTV_MODES: Record<number, string> = {
  8: 'Robot 36', 12: 'Robot 72', 44: 'Martin M1', 40: 'Martin M2', 60: 'Scottie S1', 56: 'Scottie S2',
  76: 'Scottie DX', 93: 'PD 50', 99: 'PD 90', 95: 'PD 120', 98: 'PD 160', 96: 'PD 180', 97: 'PD 240',
};

export const imageUrl = (id: number, thumb = false) => `/api/sstv/image/${id}${thumb ? '?thumb=true' : ''}`;
