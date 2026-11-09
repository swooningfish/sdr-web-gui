/** Frequency bookmarks: the station's shared list plus the user's own,
 * loaded once and shared by the spectrum, the side panel and the
 * Bookmarks page. */

import { api } from './api';
import type { AudioMode } from './audio';

export interface Bookmark {
  id: number;
  name: string;
  freq_hz: number;
  mode: AudioMode;
  group: string | null;
  colour: string | null;
  notes: string | null;
  /** Receiver sample rate to switch to when tuning it (null = leave as is). */
  sample_rate: number | null;
  /** DAB: the service (station) in the multiplex. */
  service_id: number | null;
  /** In the station's shared list (else the user's own). */
  shared: boolean;
  can_edit: boolean;
  updated_at: number;
}

export interface BookmarkInput {
  name: string;
  freq_hz: number;
  mode: AudioMode;
  group: string | null;
  colour: string | null;
  notes: string | null;
  sample_rate: number | null;
  /** DAB: the service (station) in the multiplex. */
  service_id?: number | null;
  shared: boolean;
}

/** The fields a bookmark is saved from. */
export function inputOf(b: Bookmark): BookmarkInput {
  const { name, freq_hz, mode, group, colour, notes, sample_rate, service_id, shared } = b;
  return { name, freq_hz, mode, group, colour, notes, sample_rate, service_id, shared };
}

export type Scope = 'all' | 'shared' | 'mine';

export interface ImportResult {
  added: number;
  duplicates: number;
  errors: string[];
}

/** Marker colour when none is set: station list blue, your own pink. */
export function colourOf(b: Pick<Bookmark, 'colour' | 'shared'>): string {
  return b.colour ?? (b.shared ? '#38bdf8' : '#f472b6');
}

export function matches(b: Bookmark, q: string, scope: Scope, group: string): boolean {
  if (scope === 'shared' && !b.shared) return false;
  if (scope === 'mine' && b.shared) return false;
  if (group && (b.group ?? '') !== group) return false;
  if (!q) return true;
  const s = q.toLowerCase();
  return (
    b.name.toLowerCase().includes(s) ||
    (b.group ?? '').toLowerCase().includes(s) ||
    (b.notes ?? '').toLowerCase().includes(s) ||
    (b.freq_hz / 1e6).toFixed(4).includes(s)
  );
}

class BookmarkStore {
  list = $state<Bookmark[]>([]);
  loaded = $state(false);
  error = $state<string | null>(null);

  /** Group names in use, sorted. */
  get groups(): string[] {
    return [...new Set(this.list.map((b) => b.group).filter((g): g is string => !!g))].sort((a, b) =>
      a.localeCompare(b),
    );
  }

  async load() {
    try {
      this.list = await api.bookmarks();
      this.loaded = true;
      this.error = null;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  async add(b: BookmarkInput) {
    const r = await api.addBookmark(b);
    await this.load();
    return r.id;
  }

  async update(id: number, b: BookmarkInput) {
    await api.updateBookmark(id, b);
    await this.load();
  }

  async remove(ids: number[]) {
    const r = await api.deleteBookmarks(ids);
    await this.load();
    return r;
  }

  async import(csv: string, shared: boolean): Promise<ImportResult> {
    const r = await api.importBookmarks(csv, shared);
    await this.load();
    return r;
  }

  clear() {
    this.list = [];
    this.loaded = false;
  }
}

export const bookmarks = new BookmarkStore();
