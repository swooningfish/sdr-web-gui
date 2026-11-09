/** Standard calling and centre-of-activity frequencies (IARU Region 1 / UK
 * band plans), plus a few well-known channels worth a listen. */

import type { AudioMode } from './audio';

export interface Channel {
  name: string;
  freq_hz: number;
  mode: AudioMode;
  group: string;
}

const ch = (group: string, name: string, mhz: number, mode: AudioMode): Channel => ({
  group,
  name,
  freq_hz: Math.round(mhz * 1e6),
  mode,
});

export const CALLING: Channel[] = [
  ch('160 m', 'CW QRP', 1.836, 'cw'),
  ch('80 m', 'CW QRP', 3.56, 'cw'),
  ch('80 m', 'SSB QRP', 3.69, 'lsb'),
  ch('40 m', 'CW QRP', 7.03, 'cw'),
  ch('40 m', 'SSB QRP', 7.09, 'lsb'),
  ch('30 m', 'CW QRP', 10.116, 'cw'),
  ch('20 m', 'CW QRP', 14.06, 'cw'),
  ch('20 m', 'SSB QRP', 14.285, 'usb'),
  ch('17 m', 'CW QRP', 18.086, 'cw'),
  ch('15 m', 'CW QRP', 21.06, 'cw'),
  ch('15 m', 'SSB QRP', 21.285, 'usb'),
  ch('12 m', 'CW QRP', 24.906, 'cw'),
  ch('10 m', 'CW QRP', 28.06, 'cw'),
  ch('10 m', 'SSB QRP', 28.36, 'usb'),
  ch('10 m', 'FM calling', 29.6, 'nfm'),
  ch('6 m', 'SSB calling', 50.15, 'usb'),
  ch('6 m', 'FM calling', 51.51, 'nfm'),
  ch('4 m', 'SSB calling', 70.2, 'usb'),
  ch('4 m', 'FM calling', 70.45, 'nfm'),
  ch('2 m', 'CW calling', 144.05, 'cw'),
  ch('2 m', 'SSB calling', 144.3, 'usb'),
  ch('2 m', 'APRS', 144.8, 'nfm'),
  ch('2 m', 'FM calling', 145.5, 'nfm'),
  ch('70 cm', 'CW calling', 432.05, 'cw'),
  ch('70 cm', 'SSB calling', 432.2, 'usb'),
  ch('70 cm', 'FM calling', 433.5, 'nfm'),
  ch('23 cm', 'SSB calling', 1296.2, 'usb'),
  ch('23 cm', 'FM calling', 1297.5, 'nfm'),
  ch('Satellite', 'ISS voice / SSTV', 145.8, 'nfm'),
  ch('Satellite', 'ISS APRS', 145.825, 'nfm'),
  ch('Other', 'CB ch 9 (CEPT, emergency)', 27.065, 'nfm'),
  ch('Other', 'CB ch 19 (CEPT)', 27.185, 'nfm'),
  ch('Other', 'Air distress', 121.5, 'am'),
  ch('Other', 'Marine ch 16', 156.8, 'nfm'),
  ch('Other', 'PMR446 ch 1', 446.00625, 'nfm'),
];
