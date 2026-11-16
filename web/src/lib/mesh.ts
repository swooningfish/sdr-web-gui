import type { MeshNode, MeshProto } from './api';

export const PROTO_NAMES: Record<MeshProto, string> = { meshtastic: 'Meshtastic', meshcore: 'MeshCore' };

export function nodeName(n: MeshNode): string {
  return n.long_name ?? n.short_name ?? n.id;
}

/** Battery as Meshtastic reports it: over 100 means on external power. */
export function battery(n: MeshNode): string {
  if (n.battery === null) return '';
  return n.battery > 100 ? 'powered' : `${n.battery}%`;
}

export function uptime(s: number | null): string {
  if (s === null) return '–';
  const d = Math.floor(s / 86_400);
  const h = Math.floor((s % 86_400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
}

export function clock(ms: number): string {
  const d = new Date(ms);
  const today = new Date().toDateString() === d.toDateString();
  return today
    ? d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' })
    : d.toLocaleString('en-GB', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
}
