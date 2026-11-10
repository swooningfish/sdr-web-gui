/** ARISS SSTV events: which passes fall in one, and a calendar file of them. */

import type { ArissEvent, Pass } from './api';
import { formatMHz } from './format';
import { compass } from './passes';

export const ISS_NORAD = 25544;
export const ARISS_GALLERY_URL = 'https://ariss-usa.org/ARISS_SSTV/';
export const ARISS_EVENTS_URL = 'https://www.ariss.org/upcoming-sstv-events.html';

/** The event a time span overlaps (for ISS passes and pictures). */
export function eventDuring(events: ArissEvent[], from_ms: number, to_ms: number): ArissEvent | null {
  return events.find((e) => to_ms > e.start_ms && from_ms < e.end_ms) ?? null;
}

/** "Mon, 9 Nov 2026 17:45 UTC" */
export function utc(ms: number): string {
  return (
    new Date(ms).toLocaleString('en-GB', {
      weekday: 'short',
      day: 'numeric',
      month: 'short',
      year: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      timeZone: 'UTC',
    }) + ' UTC'
  );
}

const stamp = (ms: number) => new Date(ms).toISOString().replace(/[-:]/g, '').replace(/\.\d+/, '');
/** RFC 5545 text: escape, then fold lines at 75 octets. */
const esc = (s: string) => s.replace(/\\/g, '\\\\').replace(/\n/g, '\\n').replace(/[,;]/g, (c) => `\\${c}`);
function fold(line: string): string {
  const out: string[] = [];
  let rest = line;
  while (new TextEncoder().encode(rest).length > 73) {
    let n = 73;
    while (new TextEncoder().encode(rest.slice(0, n)).length > 73) n--;
    out.push(rest.slice(0, n));
    rest = ' ' + rest.slice(n);
  }
  out.push(rest);
  return out.join('\r\n');
}

const summary = (p: Pass) => `ISS SSTV pass, max ${Math.round(p.max_el_deg)}°`;
function details(event: ArissEvent, p: Pass): string {
  const what = [event.freq_hz ? `${formatMHz(event.freq_hz, 3)} FM` : null, event.mode, event.cadence].filter(Boolean).join(', ');
  return (
    `ARISS Series ${event.series} "${event.name}". ${what ? what + '. ' : ''}` +
    `Highest ${Math.round(p.max_el_deg)}° at ${new Date(p.max_ms).toISOString().slice(11, 16)} UTC, ` +
    `${compass(p.aos_az_deg)} to ${compass(p.los_az_deg)}.\nGallery: ${ARISS_GALLERY_URL}`
  );
}

/** Link that opens Google Calendar with one pass filled in (Google has no link for several at once). */
export function googleUrl(event: ArissEvent, p: Pass): string {
  const q = new URLSearchParams({
    action: 'TEMPLATE',
    text: summary(p),
    dates: `${stamp(p.aos_ms)}/${stamp(p.los_ms)}`,
    details: details(event, p),
  });
  return `https://calendar.google.com/calendar/render?${q}`;
}

/** The same for Outlook.com. */
export function outlookUrl(event: ArissEvent, p: Pass): string {
  const q = new URLSearchParams({
    rru: 'addevent',
    subject: summary(p),
    startdt: new Date(p.aos_ms).toISOString(),
    enddt: new Date(p.los_ms).toISOString(),
    body: details(event, p),
  });
  return `https://outlook.live.com/calendar/0/action/compose?${q}`;
}

/** An iCalendar file with one entry per pass during the event. */
export function passesIcs(event: ArissEvent, passes: Pass[], now = Date.now()): string {
  const lines = ['BEGIN:VCALENDAR', 'VERSION:2.0', 'PRODID:-//sdr-web-gui//ARISS SSTV passes//EN', 'CALSCALE:GREGORIAN'];
  for (const p of passes) {
    lines.push(
      'BEGIN:VEVENT',
      `UID:ariss-${event.series}-${p.aos_ms}@sdr-web-gui`,
      `DTSTAMP:${stamp(now)}`,
      `DTSTART:${stamp(p.aos_ms)}`,
      `DTEND:${stamp(p.los_ms)}`,
      `SUMMARY:${esc(summary(p))}`,
      `DESCRIPTION:${esc(details(event, p))}`,
      'END:VEVENT',
    );
  }
  lines.push('END:VCALENDAR');
  return lines.map(fold).join('\r\n') + '\r\n';
}

/** Hand a text file to the browser to save. */
export function download(name: string, type: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
