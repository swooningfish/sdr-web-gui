/** The scheduler's config and status, polled once for the whole app (the
 * receiver bar, the pass notice and the rotation switch all read it). */

import { api, type ScheduleInfo } from './api';

class ScheduleStore {
  info = $state<ScheduleInfo | null>(null);
  /** Wall clock, ticking every second for countdowns. */
  now = $state(Date.now());
  private users = 0;
  private poll: ReturnType<typeof setInterval> | null = null;
  private tick: ReturnType<typeof setInterval> | null = null;

  async load() {
    try {
      this.info = await api.schedule();
    } catch {
      /* keep the last state */
    }
  }

  /** Start polling while anything needs it; returns a function to stop. */
  use(): () => void {
    if (this.users++ === 0) {
      this.load();
      this.poll = setInterval(() => this.load(), 3000);
      this.tick = setInterval(() => (this.now = Date.now()), 1000);
    }
    return () => {
      if (--this.users > 0) return;
      if (this.poll) clearInterval(this.poll);
      if (this.tick) clearInterval(this.tick);
      this.poll = this.tick = null;
    };
  }
}

export const schedule = new ScheduleStore();
