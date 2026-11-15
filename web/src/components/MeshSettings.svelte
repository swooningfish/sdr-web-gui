<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type MeshChannelKey, type MeshInfo } from '../lib/api';
  import { formatMHz } from '../lib/format';

  let info = $state<MeshInfo | null>(null);
  let enabled = $state(true);
  let presets = $state<string[]>([]);
  let mt = $state<MeshChannelKey[]>([]);
  let mc = $state<MeshChannelKey[]>([]);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let saved = $state(false);

  function fill(i: MeshInfo) {
    info = i;
    enabled = i.config.enabled;
    presets = [...i.config.presets];
    mt = i.config.meshtastic_channels.map((c) => ({ name: c.name, key: c.key ?? '' }));
    mc = i.config.meshcore_channels.map((c) => ({ name: c.name, key: c.key ?? '' }));
  }

  onMount(() => {
    api.mesh().then(fill, (e) => (err = e instanceof Error ? e.message : String(e)));
  });

  function toggle(id: string, on: boolean) {
    presets = on ? [...presets.filter((p) => p !== id), id] : presets.filter((p) => p !== id);
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    err = null;
    saved = false;
    busy = true;
    const clean = (l: MeshChannelKey[]) => l.filter((c) => c.name.trim()).map((c) => ({ name: c.name.trim(), key: (c.key ?? '').trim() }));
    try {
      await api.setMesh({ enabled, presets, meshtastic_channels: clean(mt), meshcore_channels: clean(mc) });
      fill(await api.mesh());
      saved = true;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<section class="panel">
  <h2>Meshtastic and MeshCore</h2>
  {#if !info}
    <p class="muted">{err ?? 'Loading…'}</p>
  {:else}
    <form onsubmit={save}>
      <label class="check"><input type="checkbox" bind:checked={enabled} /> Enabled</label>
      <p class="muted small-text hint">
        LoRa packets are decoded whenever the receiver is tuned near {formatMHz(info.status.tune_hz, 3)} MHz (a 1 MS/s
        or wider capture there covers every preset). Each preset costs some CPU while it's in the band.
      </p>
      <fieldset>
        <legend>Presets</legend>
        {#each info.presets as p (p.id)}
          <label class="check small-text">
            <input type="checkbox" checked={presets.includes(p.id)} onchange={(e) => toggle(p.id, e.currentTarget.checked)} />
            {p.name} · {formatMHz(p.freq_hz, 3)} MHz · SF{p.sf}, {p.bw / 1000} kHz, CR 4/{4 + p.cr}
          </label>
        {/each}
      </fieldset>

      <fieldset>
        <legend>Meshtastic channels</legend>
        <p class="muted small-text hint">
          The default channels (LongFast, MediumFast and the other preset names on the default key) are always read.
          Add a private channel with its name and PSK in base64, as the Meshtastic app shows it. Messages on private
          channels are shown to admins only; keys never leave the server except to admins.
        </p>
        {#each mt as c, i (i)}
          <div class="row">
            <input class="name" bind:value={c.name} maxlength="11" placeholder="Channel name" aria-label="Meshtastic channel {i + 1} name" />
            <input class="key" bind:value={c.key} placeholder="PSK (base64)" autocomplete="off" spellcheck="false" aria-label="Meshtastic channel {i + 1} key" />
            <button type="button" class="small danger" onclick={() => mt.splice(i, 1)} aria-label="Remove Meshtastic channel {i + 1}">✕</button>
          </div>
        {/each}
        <button type="button" class="small" disabled={mt.length >= 16} onclick={() => mt.push({ name: '', key: '' })}>Add channel</button>
      </fieldset>

      <fieldset>
        <legend>MeshCore channels</legend>
        <p class="muted small-text hint">
          Public is always read. Add a private channel with its secret (hex or base64), or a hashtag channel such as
          <span class="mono">#uk</span> with no key: its key comes from the name.
        </p>
        {#each mc as c, i (i)}
          <div class="row">
            <input class="name" bind:value={c.name} maxlength="32" placeholder="Name or #hashtag" aria-label="MeshCore channel {i + 1} name" />
            <input class="key" bind:value={c.key} placeholder={c.name.startsWith('#') ? 'No key needed' : 'Secret (hex or base64)'} autocomplete="off" spellcheck="false" aria-label="MeshCore channel {i + 1} key" />
            <button type="button" class="small danger" onclick={() => mc.splice(i, 1)} aria-label="Remove MeshCore channel {i + 1}">✕</button>
          </div>
        {/each}
        <button type="button" class="small" disabled={mc.length >= 16} onclick={() => mc.push({ name: '', key: '' })}>Add channel</button>
      </fieldset>

      <div class="save"><button type="submit" class="primary" disabled={busy}>Save</button></div>
      {#if err}<p class="error">{err}</p>{/if}
      {#if saved}<p class="success">Saved</p>{/if}
    </form>
  {/if}
</section>

<style>
  form {
    display: grid;
    gap: 8px;
  }
  fieldset {
    display: grid;
    gap: 6px;
    margin: 6px 0 0;
    padding: 8px 10px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    min-width: 0;
  }
  legend {
    padding: 0 4px;
    font-weight: 600;
    font-size: 14px;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
  }
  .check.small-text {
    font-size: 13px;
  }
  .hint {
    margin: 0 0 4px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .row .name {
    flex: 0 1 11em;
    min-width: 0;
  }
  .row .key {
    flex: 1 1 14em;
    min-width: 0;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }
  fieldset > button.small {
    justify-self: start;
  }
  .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }
  .save {
    margin-top: 6px;
  }
</style>
