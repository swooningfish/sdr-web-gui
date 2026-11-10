/**
 * In-page replacement for `window.confirm()`. The native box freezes the page's
 * main thread while it's up, which starves the audio worklet feed and the
 * waterfall; this one is an ordinary element, so everything keeps running.
 *
 *     if (!(await confirmAsk('Delete this image?', { danger: true }))) return;
 *
 * Rendered by `ConfirmHost.svelte`, mounted once in `App.svelte`.
 */

export interface ConfirmOptions {
  /** Label for the button that goes ahead. Default "OK". */
  ok?: string;
  /** Label for the button that backs out. Default "Cancel". */
  cancel?: string;
  /** Destructive: the go-ahead button is red and Cancel takes the focus. */
  danger?: boolean;
}

export interface ConfirmRequest extends ConfirmOptions {
  message: string;
  resolve: (ok: boolean) => void;
}

class Confirms {
  /** Waiting questions; the host shows the first. */
  queue = $state<ConfirmRequest[]>([]);

  get current(): ConfirmRequest | undefined {
    return this.queue[0];
  }

  ask(message: string, opts: ConfirmOptions = {}): Promise<boolean> {
    return new Promise((resolve) => {
      this.queue.push({ message, ...opts, resolve });
    });
  }

  /** Answer the question on show and move to the next. */
  answer(ok: boolean) {
    const req = this.queue.shift();
    req?.resolve(ok);
  }
}

export const confirms = new Confirms();

/** Ask a yes/no question without blocking the page. Resolves true to go ahead. */
export function confirmAsk(message: string, opts?: ConfirmOptions): Promise<boolean> {
  return confirms.ask(message, opts);
}
