<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type Profile, type RadioState } from '../lib/api';
  import { unsupported } from '../lib/coverage';
  import { formatRate, parseFrequency } from '../lib/format';
  import { schedule } from '../lib/schedule.svelte';

  /** Rotation: the profiles the radio cycles through when no pass is on.
   * They also show as quick-tune buttons under the receiver's presets. */
  let { radio }: { radio: RadioState | null } = $props();

  const RATES = [250_000, 1_024_000, 1_536_000, 1_800_000, 1_920_000, 2_048_000, 2_400_000];
  const mhz = (hz: number) => (hz / 1e6).toFixed(3);
  const why = (text: string | undefined) => unsupported(radio, parseFrequency(text ?? '') ?? 0);

  let profiles = $state<Profile[] | null>(null);
  let freq = $state<string[]>([]);
  let rotation = $state(false);
  let busy = $state(false);
  let msg = $state<{ ok: boolean; text: string } | null>(null);

  function fill(p: Profile[], on: boolean) {
    profiles = structuredClone(p);
    freq = p.map((x) => mhz(x.center_hz));
    rotation = on;
  }

  onMount(() => {
    api.schedule().then(
      (i) => fill(i.config.profiles, i.config.rotation),
      (e) => (msg = { ok: false, text: String(e.message ?? e) }),
    );
  });

  async function act(f: () => Promise<unknown>, ok: string) {
    busy = true;
    msg = null;
    try {
      await f();
      msg = { ok: true, text: ok };
      schedule.load();
    } catch (e) {
      msg = { ok: false, text: e instanceof Error ? e.message : String(e) };
    } finally {
      busy = false;
    }
  }

  function save(e: SubmitEvent) {
    e.preventDefault();
    if (!profiles) return;
    const out = $state.snapshot(profiles) as Profile[];
    for (const [i, p] of out.entries()) {
      const hz = parseFrequency(freq[i]);
      if (hz === null) return void (msg = { ok: false, text: `"${freq[i]}" is not a frequency` });
      p.center_hz = hz;
    }
    act(async () => {
      // Only the profiles are ours; keep everything else as the server has it.
      const cfg = (await api.schedule()).config;
      cfg.profiles = out;
      if (!out.length) cfg.rotation = false;
      const saved = (await api.setSchedule(cfg)).config;
      fill(saved.profiles, saved.rotation);
    }, 'Rotation saved');
  }

  function add() {
    profiles!.push({ name: 'Profile', center_hz: 145_300_000, sample_rate: 2_048_000, dwell_s: 60 });
    freq.push('145.300');
  }

  function move(i: number, d: number) {
    const j = i + d;
    if (!profiles || j < 0 || j >= profiles.length) return;
    [profiles[i], profiles[j]] = [profiles[j], profiles[i]];
    [freq[i], freq[j]] = [freq[j], freq[i]];
  }
</script>

<section class="panel">
  <h2>Rotation</h2>
  {#if !profiles}
    <p class="muted">{msg?.text ?? 'Loading…'}</p>
  {:else}
    <label class="check"
      ><input
        type="checkbox"
        checked={rotation}
        disabled={busy || profiles.length === 0}
        onchange={(e) => {
          const on = (e.currentTarget as HTMLInputElement).checked;
          act(async () => {
            await api.setRotation(on);
            rotation = on;
          }, on ? 'Rotation on' : 'Rotation off');
        }}
      /> Cycle through these profiles when no pass is on (takes effect straight away)</label
    >
    <p class="muted small-text hint">
      Holds while an SSTV image is being received (up to 5 minutes). Satellite passes take over when they're due. Every
      profile is also a quick-tune button under the receiver's <strong>Presets</strong>.
    </p>

    <form onsubmit={save}>
      {#each profiles as p, i (i)}
        {@const na = why(freq[i])}
        <div class="row" class:na-row={!!na}>
          <span class="order">
            <button type="button" class="small" aria-label="Move {p.name} up" disabled={i === 0} onclick={() => move(i, -1)}>↑</button>
            <button
              type="button"
              class="small"
              aria-label="Move {p.name} down"
              disabled={i === profiles.length - 1}
              onclick={() => move(i, 1)}>↓</button
            >
          </span>
          <input class="grow" bind:value={p.name} maxlength="32" aria-label="Profile name" />
          <input class="f" bind:value={freq[i]} inputmode="decimal" aria-label="{p.name} centre frequency in MHz" />
          <span class="muted small-text">MHz</span>
          {#if na}<span class="na-tag" title="{na}. Rotation skips this profile.">Not supported</span>{/if}
          <select bind:value={p.sample_rate} aria-label="{p.name} sample rate">
            {#each RATES as r (r)}<option value={r}>{formatRate(r)}</option>{/each}
          </select>
          <input class="n" type="number" bind:value={p.dwell_s} min="10" max="3600" aria-label="{p.name} dwell in seconds" />
          <span class="muted small-text">s</span>
          <button
            type="button"
            class="small danger"
            aria-label="Remove {p.name}"
            onclick={() => {
              profiles!.splice(i, 1);
              freq.splice(i, 1);
            }}>✕</button
          >
        </div>
      {:else}
        <p class="muted small-text">No profiles yet.</p>
      {/each}
      <button type="button" class="small" disabled={profiles.length >= 8} onclick={add}>Add profile</button>
      <div class="buttons"><button type="submit" class="primary" disabled={busy}>Save rotation</button></div>
    </form>
    {#if msg}<p class:error={!msg.ok} class:success={msg.ok}>{msg.text}</p>{/if}
  {/if}
</section>

<style>
  form {
    display: grid;
    gap: 6px;
  }
  form > button.small {
    justify-self: start;
  }
  .buttons {
    margin-top: 6px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }
  .order {
    display: inline-flex;
    gap: 2px;
  }
  .order button {
    padding: 2px 6px;
  }
  .grow {
    flex: 1 1 120px;
    min-width: 0;
  }
  .n {
    width: 5.5em;
  }
  .f {
    width: 6.5em;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
  }
  .hint {
    margin: 4px 0 10px;
  }
</style>
