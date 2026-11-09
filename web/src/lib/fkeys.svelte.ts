/** Programmable function keys (F1–F8) on the receiver's front panel: each
 * user's own, stored on the server. A key recalls a frequency and mode,
 * usually set from a bookmark. */

import { api } from './api';
import type { AudioMode } from './audio';
import { bookmarks, type Bookmark } from './bookmarks.svelte';

export const SLOTS = 8;

export interface FKey {
  slot: number;
  name: string;
  freq_hz: number;
  mode: AudioMode;
  /** The saved bookmark it was set from (none for calling channels, repeaters). */
  bookmark_id: number | null;
  /** DAB: the service (station) in the multiplex. */
  service_id?: number | null;
}

export type FKeyInput = Omit<FKey, 'slot'>;

class FKeyStore {
  keys = $state<(FKey | null)[]>(Array(SLOTS).fill(null));
  error = $state<string | null>(null);

  async load() {
    try {
      const list = await api.fkeys();
      const k: (FKey | null)[] = Array(SLOTS).fill(null);
      for (const f of list) if (f.slot >= 1 && f.slot <= SLOTS) k[f.slot - 1] = f;
      this.keys = k;
      this.error = null;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** What a key recalls: the bookmark as it is now (with its sample rate),
   * else the key's own copy. */
  recall(slot: number): (FKeyInput & { sample_rate?: number | null }) | null {
    const k = this.keys[slot - 1];
    if (!k) return null;
    const b = k.bookmark_id ? bookmarks.list.find((x) => x.id === k.bookmark_id) : undefined;
    return b
      ? {
          name: b.name,
          freq_hz: b.freq_hz,
          mode: b.mode,
          bookmark_id: b.id,
          sample_rate: b.sample_rate,
          service_id: b.service_id,
        }
      : k;
  }

  async assign(slot: number, b: Pick<Bookmark, 'id' | 'name' | 'freq_hz' | 'mode'> & { service_id?: number | null }) {
    const input: FKeyInput = {
      name: b.name,
      freq_hz: b.freq_hz,
      mode: b.mode,
      bookmark_id: b.id > 0 ? b.id : null,
      service_id: b.mode === 'dab' ? (b.service_id ?? null) : null,
    };
    await api.setFkey(slot, input);
    this.keys[slot - 1] = { slot, ...input };
  }

  async clear(slot: number) {
    await api.clearFkey(slot);
    this.keys[slot - 1] = null;
  }

  /** Keys holding this bookmark. */
  slotsOf(id: number): number[] {
    return this.keys.filter((k): k is FKey => !!k && k.bookmark_id === id).map((k) => k.slot);
  }

  reset() {
    this.keys = Array(SLOTS).fill(null);
  }
}

export const fkeys = new FKeyStore();
