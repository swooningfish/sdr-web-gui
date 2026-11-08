<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type FlexDecoder, type RadioState, type Rtl433Info } from '../lib/api';
  import { unsupported } from '../lib/coverage';
  import { parseFrequency } from '../lib/format';

  let { radio }: { radio: RadioState | null } = $props();

  let info = $state<Rtl433Info | null>(null);
  let enabled = $state(true);
  let freqInput = $state('');
  const freqWhy = $derived(unsupported(radio, parseFrequency(freqInput) ?? 0));
  let protocolsInput = $state('');
  let flex = $state<(FlexDecoder & { check?: { ok: boolean; message?: string } })[]>([]);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let saved = $state(false);

  function fill(i: Rtl433Info) {
    info = i;
    enabled = i.config.enabled;
    freqInput = (i.config.frequency_hz / 1e6).toFixed(3);
    protocolsInput = i.config.protocols.join(', ');
    flex = i.config.flex.map((f) => ({ ...f }));
  }

  async function load() {
    try {
      fill(await api.rtl433());
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  async function refreshStatus() {
    try {
      const i = await api.rtl433();
      if (info) info = { ...info, status: i.status, mqtt: i.mqtt };
    } catch {
      /* ignore */
    }
  }

  onMount(() => {
    load();
    const t = setInterval(refreshStatus, 5000);
    return () => clearInterval(t);
  });

  function parseProtocols(s: string): number[] | null {
    const parts = s.split(/[\s,]+/).filter(Boolean);
    const nums = parts.map(Number);
    return nums.every((n) => Number.isInteger(n)) ? nums : null;
  }

  async function check(i: number) {
    const f = flex[i];
    f.check = undefined;
    try {
      f.check = await api.checkSpec(f.spec.trim());
    } catch (e) {
      f.check = { ok: false, message: e instanceof Error ? e.message : String(e) };
    }
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    err = null;
    saved = false;
    const hz = parseFrequency(freqInput);
    if (hz === null) {
      err = 'Enter a frequency like 433.92';
      return;
    }
    const protocols = parseProtocols(protocolsInput);
    if (protocols === null) {
      err = 'Protocols must be whole numbers, e.g. 19, -59';
      return;
    }
    busy = true;
    try {
      const r = await api.setRtl433({
        enabled,
        frequency_hz: hz,
        protocols,
        flex: flex.filter((f) => f.spec.trim()).map((f) => ({ spec: f.spec.trim(), enabled: f.enabled })),
      });
      if (info) fill({ ...info, config: r.config });
      saved = true;
      setTimeout(refreshStatus, 1000);
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<section class="panel">
  <h2>ISM decoder (rtl_433)</h2>
  {#if !info}
    <p class="muted">{err ?? 'Loading…'}</p>
  {:else}
    <form onsubmit={save}>
      <label class="check"><input type="checkbox" bind:checked={enabled} /> Enabled</label>

      <label for="r433-freq">Frequency (MHz)</label>
      <input id="r433-freq" bind:value={freqInput} inputmode="decimal" />
      {#if freqWhy}<p class="small-text hint"><span class="na-tag">Not supported by this SDR</span> {freqWhy}</p>{/if}
      <p class="muted small-text hint">
        Decodes about ±100 kHz around this frequency whenever it is inside the tuned band.
      </p>

      <label for="r433-proto">Protocols (optional)</label>
      <input id="r433-proto" placeholder="e.g. 19, -59" bind:value={protocolsInput} />
      <p class="muted small-text hint">
        Passed as <code>-R</code>. Positive numbers enable only those decoders, negative ones disable a decoder,
        0 disables all built-in decoders. Leave empty for rtl_433's defaults.
      </p>

      <span class="label">Flex decoders</span>
      <p class="muted small-text hint">
        Passed as <code>-X</code>; they report raw bits, for exploring unknown signals. TPMS is
        already covered by the built-in decoders. e.g.
        <code>n=probe,m=OOK_PWM,s=400,l=800,r=2000,preamble=&#123;8&#125;0xaa</code>.
      </p>
      {#each flex as f, i (i)}
        <div class="flex-row">
          <input type="checkbox" bind:checked={f.enabled} aria-label="Enable flex decoder {i + 1}" />
          <input
            class="mono"
            bind:value={f.spec}
            oninput={() => (f.check = undefined)}
            spellcheck="false"
            autocapitalize="off"
            aria-label="Flex decoder {i + 1}"
          />
          <button type="button" class="small" onclick={() => check(i)} disabled={!f.spec.trim()}>Check</button>
          <button type="button" class="small danger" onclick={() => flex.splice(i, 1)} aria-label="Remove flex decoder {i + 1}"
            >✕</button
          >
        </div>
        {#if f.check}
          <p class="small-text" class:success={f.check.ok} class:error={!f.check.ok}>
            {f.check.ok ? 'rtl_433 accepts this spec' : f.check.message}
          </p>
        {/if}
      {/each}
      <button type="button" class="small" onclick={() => flex.push({ spec: '', enabled: true })} disabled={flex.length >= 32}
        >Add flex decoder</button
      >

      <div class="save">
        <button type="submit" class="primary" disabled={busy}>Save and restart rtl_433</button>
      </div>
      {#if err}<p class="error">{err}</p>{/if}
      {#if saved}<p class="success">Saved</p>{/if}
    </form>

    <h3>Status</h3>
    <p class="small-text">
      <span class="dot" class:ok={info.status.running} class:bad={!!info.status.error}></span>
      {#if info.status.error}
        {info.status.error}
      {:else if info.status.running}
        Running{info.status.in_band ? '' : ' (idle: frequency outside the tuned band)'}
      {:else}
        Not running{info.status.enabled && !info.status.in_band ? ': frequency outside the tuned band' : ''}
      {/if}
      {#if info.status.restarts}<span class="muted"> · {info.status.restarts} restarts</span>{/if}
    </p>
    {#if info.status.log.length}
      <pre class="log">{info.status.log.join('\n')}</pre>
    {/if}

  {/if}
</section>

<style>
  form {
    display: grid;
    gap: 6px;
  }
  label,
  .label {
    font-size: 14px;
    margin-top: 6px;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .hint {
    margin: 0 0 4px;
  }
  .flex-row {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .flex-row .mono {
    flex: 1;
    min-width: 0;
  }
  .mono,
  code,
  .log {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 13px;
  }
  form > button.small {
    justify-self: start;
  }
  .save {
    margin-top: 10px;
  }
  h3 {
    margin: 18px 0 6px;
    font-size: 15px;
  }
  .log {
    max-height: 200px;
    overflow: auto;
    margin: 6px 0 0;
    padding: 8px;
    background: var(--surface-2);
    border-radius: 6px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  p.small-text {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 4px 0;
  }
</style>
