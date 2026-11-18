<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type ObserverConfig, type ObserverInfo } from '../lib/api';
  import { confirmAsk } from '../lib/confirm.svelte';
  import { relTime } from '../lib/format';

  /** MeshCore observer: shares the MeshCore packets we hear with LetsMesh. */
  let info = $state<ObserverInfo | null>(null);
  let cfg = $state<ObserverConfig | null>(null);
  let busy = $state(false);
  let msg = $state<{ ok: boolean; text: string } | null>(null);
  let copied = $state(false);
  const canCopy = typeof navigator !== 'undefined' && !!navigator.clipboard;

  async function load(form: boolean) {
    try {
      const i = await api.meshObserver();
      info = i;
      if (form) cfg = { ...i.config };
    } catch (e) {
      msg = { ok: false, text: e instanceof Error ? e.message : String(e) };
    }
  }

  onMount(() => {
    load(true);
    // Connection state only; the form keeps what's being typed.
    const t = setInterval(() => load(false), 5000);
    return () => clearInterval(t);
  });

  async function save(e: SubmitEvent) {
    e.preventDefault();
    if (!cfg || !info) return;
    if (
      cfg.enabled &&
      !info.config.enabled &&
      !(await confirmAsk(
        'Share the MeshCore packets this receiver hears on LetsMesh? They appear publicly on its map and analyzer, with the observer name and area code.',
        { ok: 'Share packets' },
      ))
    )
      return;
    busy = true;
    msg = null;
    try {
      info = await api.setMeshObserver({ ...cfg });
      cfg = { ...info.config };
      msg = { ok: true, text: cfg.enabled && !info.paused ? 'Saved. Connecting…' : 'Saved' };
    } catch (e) {
      msg = { ok: false, text: e instanceof Error ? e.message : String(e) };
    } finally {
      busy = false;
    }
  }

  async function copyKey() {
    if (!info) return;
    try {
      await navigator.clipboard.writeText(info.public_key);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      /* clipboard blocked; the key is still selectable */
    }
  }
</script>

<section class="panel">
  <h2>MeshCore observer (LetsMesh)</h2>
  {#if !info || !cfg}
    <p class="muted">{msg?.text ?? 'Loading…'}</p>
  {:else}
    <p class="muted small-text hint">
      Sends every MeshCore packet the Mesh receiver hears to the
      <a href="https://analyzer.letsmesh.net/" target="_blank" rel="noopener noreferrer">LetsMesh analyzer</a>, which maps
      coverage and the paths packets take. Packets go as they were on air: private channels and direct messages stay
      encrypted. Damaged packets are skipped, and nothing is sent while the receiver is simulated or replayed.
    </p>

    {#if info.config.enabled}
      <p class="status small-text">
        <span class="dot" class:ok={info.status.connected && !info.paused} class:bad={!info.status.connected || !!info.paused}
        ></span>
        {#if info.status.connected}
          Connected to {info.status.server}
        {:else}
          Not connected{info.status.server ? ` to ${info.status.server}` : ''}{info.status.error ? `: ${info.status.error}` : ''}
        {/if}
      </p>
      {#if info.paused}<p class="error small-text">{info.paused}</p>{/if}
      <p class="muted small-text">
        {info.status.sent} packet{info.status.sent === 1 ? '' : 's'} sent since sdrd started{#if info.status.last_sent_ms}, the
          last {relTime(info.status.last_sent_ms)}{/if}.
      </p>
    {/if}

    <form onsubmit={save}>
      <label class="check"><input type="checkbox" bind:checked={cfg.enabled} /> Share MeshCore packets</label>
      <div class="grid">
        <label for="obs-server">Server</label>
        <select id="obs-server" bind:value={cfg.broker}>
          {#each info.brokers as b (b.id)}<option value={b.id}>{b.name}</option>{/each}
        </select>

        <label for="obs-iata">Area code</label>
        <span class="tok">
          <input
            id="obs-iata"
            class="n"
            bind:value={cfg.iata}
            maxlength="3"
            placeholder="LHR"
            autocapitalize="characters"
            spellcheck="false"
          />
          <span class="muted small-text">IATA code of the nearest airport</span>
        </span>

        <label for="obs-name">Observer name</label>
        <input id="obs-name" bind:value={cfg.name} maxlength="32" />

        <label for="obs-owner">Owner key</label>
        <input
          id="obs-owner"
          bind:value={cfg.owner}
          maxlength="64"
          placeholder="Optional: your companion's public key"
          spellcheck="false"
        />

        <label for="obs-email">Email</label>
        <input id="obs-email" type="email" bind:value={cfg.email} maxlength="120" placeholder="Optional" />

        <span class="lbl">Observer ID</span>
        <span class="tok">
          <code class="key">{info.public_key}</code>
          {#if canCopy}<button type="button" class="small" onclick={copyKey}>{copied ? 'Copied' : 'Copy'}</button>{/if}
        </span>
      </div>
      <p class="muted small-text hint">
        The observer ID is this receiver's own MeshCore public key, made by sdrd; its private key never leaves the server.
        The owner key and email are optional: they go in the login so LetsMesh can link this observer to you.
      </p>
      <div class="buttons">
        <button type="submit" class="primary" disabled={busy}>Save</button>
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
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    overflow-wrap: anywhere;
  }
  .grid {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 8px 12px;
    align-items: center;
    font-size: 14px;
  }
  .grid label,
  .lbl {
    color: var(--muted);
  }
  .grid input,
  .grid select {
    min-width: 0;
  }
  .tok {
    display: flex;
    gap: 6px;
    align-items: center;
    min-width: 0;
  }
  .n {
    width: 4.5em;
    text-transform: uppercase;
  }
  .key {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12px;
    overflow-wrap: anywhere;
    min-width: 0;
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
