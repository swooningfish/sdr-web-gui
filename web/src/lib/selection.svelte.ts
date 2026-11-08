import { SvelteSet } from 'svelte/reactivity';

/** Multi-select state for a list: click toggles, Shift+click selects a range. */
export class Selection {
  readonly ids = new SvelteSet<number>();
  private anchor: number | null = null;

  get size() {
    return this.ids.size;
  }

  has(id: number) {
    return this.ids.has(id);
  }

  /** `order` is the list as currently shown, for Shift+click ranges. */
  toggle(id: number, shift = false, order: number[] = []) {
    if (shift && this.anchor !== null) {
      const a = order.indexOf(this.anchor);
      const b = order.indexOf(id);
      if (a >= 0 && b >= 0) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        for (const x of order.slice(lo, hi + 1)) this.ids.add(x);
        this.anchor = id;
        return;
      }
    }
    if (this.ids.has(id)) this.ids.delete(id);
    else this.ids.add(id);
    this.anchor = id;
  }

  /** Select all of `ids`, or clear them if they're all selected already. */
  toggleAll(ids: number[]) {
    if (ids.length && ids.every((i) => this.ids.has(i))) ids.forEach((i) => this.ids.delete(i));
    else ids.forEach((i) => this.ids.add(i));
  }

  allOf(ids: number[]) {
    return ids.length > 0 && ids.every((i) => this.ids.has(i));
  }

  someOf(ids: number[]) {
    return ids.some((i) => this.ids.has(i));
  }

  clear() {
    this.ids.clear();
    this.anchor = null;
  }

  /** Drop ids that are no longer listed. */
  keepOnly(ids: number[]) {
    const keep = new Set(ids);
    for (const i of [...this.ids]) if (!keep.has(i)) this.ids.delete(i);
  }

  list() {
    return [...this.ids];
  }
}
