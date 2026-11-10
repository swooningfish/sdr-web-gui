<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type RepeaterConfig } from '../lib/api';
  import { confirmAsk } from '../lib/confirm.svelte';

  /** RepeaterBook lookup for the "Repeaters" list under Bookmarks. */
  let cfg = $state<RepeaterConfig | null>(null);
  let token = $state('');
  let busy = $state(false);
  let msg = $state<{ ok: boolean; text: string } | null>(null);

  onMount(() => {
    api.repeaterConfig().then(
      (c) => (cfg = c),
      (e) => (msg = { ok: false, text: String(e.message ?? e) }),
    );
  });

  async function act(f: () => Promise<string>) {
    busy = true;
    msg = null;
    try {
      msg = { ok: true, text: await f() };
    } catch (e) {
      msg = { ok: false, text: e instanceof Error ? e.message : String(e) };
    } finally {
      busy = false;
    }
  }

  function save(e: SubmitEvent) {
    e.preventDefault();
    if (!cfg) return;
    const c = cfg;
    act(async () => {
      cfg = await api.setRepeaterConfig({
        enabled: c.enabled,
        email: c.email,
        country: c.country,
        radius_miles: Number(c.radius_miles),
        ...(token.trim() ? { token: token.trim() } : {}),
      });
      token = '';
      return 'Saved. The list refreshes the next time someone opens it.';
    });
  }

  async function forgetToken() {
    if (!cfg || !(await confirmAsk('Forget the stored RepeaterBook token? The lookup will be switched off.', { ok: 'Forget token', danger: true }))) return;
    const c = cfg;
    act(async () => {
      cfg = await api.setRepeaterConfig({ enabled: false, email: c.email, country: c.country, radius_miles: c.radius_miles, token: '' });
      return 'Token forgotten';
    });
  }
</script>

<section class="panel">
  <h2>RepeaterBook</h2>
  {#if !cfg}
    <p class="muted">{msg?.text ?? 'Loading…'}</p>
  {:else}
    <p class="muted small-text hint">
      Lists repeaters near the station location under <strong>Bookmarks → Repeaters</strong>. RepeaterBook only answers
      approved clients: request API access at
      <a href="https://www.repeaterbook.com/api/token_request.php" target="_blank" rel="noopener noreferrer">repeaterbook.com</a>
      and paste your token here. It stays on this server and is never shown again. Personal use only, per RepeaterBook's terms;
      the list is fetched at most once a day.
    </p>
    <form onsubmit={save}>
      <label class="check"><input type="checkbox" bind:checked={cfg.enabled} /> Look up nearby repeaters</label>
      <div class="grid">
        <label for="rb-token">API token</label>
        <span class="tok">
          <input
            id="rb-token"
            type="password"
            autocomplete="off"
            bind:value={token}
            placeholder={cfg.has_token ? 'Stored (type to replace)' : 'app_… or rbuapp_…'}
          />
          {#if cfg.has_token}<button type="button" class="small" disabled={busy} onclick={forgetToken}>Forget</button>{/if}
        </span>

        <label for="rb-email">Contact email</label>
        <input id="rb-email" type="email" bind:value={cfg.email} placeholder="you@example.com" maxlength="120" />

        <label for="rb-country">Country</label>
        <input id="rb-country" bind:value={cfg.country} maxlength="64" />

        <label for="rb-radius">Radius</label>
        <span class="tok">
          <input id="rb-radius" class="n" type="number" min="0.1" max="200" step="0.1" bind:value={cfg.radius_miles} />
          <span class="muted small-text">miles from the station</span>
        </span>
      </div>
      <p class="muted small-text hint">
        RepeaterBook requires a contact email in each request's User-Agent. A 1-mile radius often finds nothing; 10–25 miles
        is typical for local repeaters.
      </p>
      <div class="buttons">
        <button type="submit" class="primary" disabled={busy}>Save</button>
        <button
          type="button"
          class="small"
          disabled={busy || !cfg.enabled}
          onclick={() =>
            act(async () => {
              const r = await api.refreshRepeaters();
              return `Found ${r.count} repeater${r.count === 1 ? '' : 's'}`;
            })}>Fetch now</button
        >
      </div>
    </form>
    {#if msg}<p class:error={!msg.ok} class:success={msg.ok}>{msg.text}</p>{/if}
  {/if}
</section>

<style>
  form {
    display: grid;
    gap: 10px;
  }
  .hint {
    margin: 0 0 6px;
  }
  .grid {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 8px 12px;
    align-items: center;
    font-size: 14px;
  }
  .grid label {
    color: var(--muted);
  }
  .grid input {
    min-width: 0;
  }
  .tok {
    display: flex;
    gap: 6px;
    align-items: center;
    min-width: 0;
  }
  .tok input:not(.n) {
    flex: 1;
  }
  .n {
    width: 6em;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
</style>
