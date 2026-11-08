<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type RadioState, type User } from './lib/api';
  import { Stream } from './lib/stream';
  import { formatMHz } from './lib/format';
  import Login from './components/Login.svelte';
  import Waterfall from './components/Waterfall.svelte';
  import RadioPanel from './components/RadioPanel.svelte';
  import Account from './components/Account.svelte';
  import Users from './components/Users.svelte';
  import Sensors from './components/Sensors.svelte';
  import DecoderSettings from './components/DecoderSettings.svelte';
  import Aprs from './components/Aprs.svelte';
  import AprsSettings from './components/AprsSettings.svelte';
  import Sstv from './components/Sstv.svelte';
  import SstvSettings from './components/SstvSettings.svelte';

  let user = $state<User | null>(null);
  let checking = $state(true);
  let radio = $state<RadioState | null>(null);
  let wsUp = $state(false);
  let view = $state<'spectrum' | 'sensors' | 'aprs' | 'sstv' | 'settings'>('spectrum');
  let waterfall = $state<ReturnType<typeof Waterfall> | undefined>();
  let sensors = $state<ReturnType<typeof Sensors> | undefined>();
  let aprs = $state<ReturnType<typeof Aprs> | undefined>();
  let sstv = $state<ReturnType<typeof Sstv> | undefined>();

  const isAdmin = $derived(user?.role === 'admin');

  onMount(async () => {
    try {
      user = await api.me();
    } catch {
      user = null;
    }
    checking = false;
  });

  // One WebSocket per signed-in session.
  $effect(() => {
    if (!user) return;
    const s = new Stream({
      onFrame: (f) => waterfall?.push(f),
      onRadio: (st) => (radio = st),
      onEvent: (d, ev) => {
        if (d.decoder === 'aprs') aprs?.push(d, ev);
        else if (d.decoder === 'sstv') sstv?.push(d, ev);
        else sensors?.push(d, ev);
      },
      onSstvRows: (r) => sstv?.rows(r),
      onConnection: (c) => (wsUp = c),
      onAuthLost: () => {
        user = null;
        radio = null;
      },
    });
    s.start();
    return () => s.stop();
  });

  async function logout() {
    try {
      await api.logout();
    } finally {
      user = null;
      radio = null;
      view = 'spectrum';
    }
  }

  async function tuneTo(hz: number) {
    try {
      await api.configure({ center_hz: hz });
    } catch (e) {
      console.warn('tune failed', e);
    }
  }
</script>

{#if checking}
  <main class="center muted">Loading…</main>
{:else if !user}
  <Login onlogin={(u) => (user = u)} />
{:else}
  <header>
    <div class="brand">
      <svg viewBox="0 0 32 32" width="22" height="22" aria-hidden="true"
        ><path
          d="M3 22 L8 20 L11 9 L14 21 L17 15 L20 19 L23 6 L26 20 L29 22"
          fill="none"
          stroke="currentColor"
          stroke-width="2.4"
          stroke-linejoin="round"
          stroke-linecap="round"
        /></svg
      >
      <span>SDR Monitor</span>
    </div>
    <nav>
      <button type="button" class="tab" class:active={view === 'spectrum'} onclick={() => (view = 'spectrum')}>Spectrum</button>
      <button type="button" class="tab" class:active={view === 'sensors'} onclick={() => (view = 'sensors')}>Sensors</button>
      <button type="button" class="tab" class:active={view === 'aprs'} onclick={() => (view = 'aprs')}>APRS</button>
      <button type="button" class="tab" class:active={view === 'sstv'} onclick={() => (view = 'sstv')}>SSTV</button>
      <button type="button" class="tab" class:active={view === 'settings'} onclick={() => (view = 'settings')}>Settings</button>
    </nav>
    <div class="status-bar">
      <span class="pill" title={wsUp ? 'Live stream connected' : 'Reconnecting…'}>
        <span class="dot" class:ok={wsUp} class:bad={!wsUp}></span>
        {wsUp ? 'Live' : 'Offline'}
      </span>
      {#if radio}
        <span class="pill hide-sm" title={radio.device ?? ''}>
          <span class="dot" class:ok={radio.connected} class:bad={!radio.connected}></span>
          {radio.connected ? formatMHz(radio.settings.center_hz, 3) : 'No SDR'}
        </span>
      {/if}
      <span class="user hide-sm">{user.username}</span>
      <button type="button" class="small" onclick={logout}>Sign out</button>
    </div>
  </header>

  <main class="content">
    <div class="spectrum-view" hidden={view !== 'spectrum'}>
      <div class="panel wf-panel">
        <Waterfall bind:this={waterfall} canTune={isAdmin && !!radio?.can_tune} ontune={tuneTo} />
      </div>
      <RadioPanel {radio} {isAdmin} />
    </div>
    <!-- Kept mounted so live events keep arriving while another tab is open. -->
    <div hidden={view !== 'sensors'}>
      <Sensors bind:this={sensors} {isAdmin} {radio} />
    </div>
    <div hidden={view !== 'aprs'}>
      <Aprs bind:this={aprs} {isAdmin} {radio} />
    </div>
    <div hidden={view !== 'sstv'}>
      <Sstv bind:this={sstv} {isAdmin} {radio} />
    </div>
    {#if view === 'settings'}
      <div class="settings-view">
        <Account {user} />
        {#if isAdmin}<DecoderSettings />{/if}
        {#if isAdmin}<AprsSettings />{/if}
        {#if isAdmin}<SstvSettings />{/if}
        {#if isAdmin}<Users me={user} />{/if}
      </div>
    {/if}
  </main>
{/if}

<style>
  header {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
    padding: 10px 16px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    color: var(--accent);
  }
  .brand span {
    color: var(--text);
  }
  nav {
    display: flex;
    gap: 4px;
  }
  .tab {
    background: transparent;
    border-color: transparent;
  }
  .tab.active {
    background: var(--surface-2);
    border-color: var(--border);
  }
  .status-bar {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    font-variant-numeric: tabular-nums;
  }
  .user {
    color: var(--muted);
  }
  .content {
    padding: 16px;
    max-width: 1600px;
    margin: 0 auto;
  }
  .spectrum-view {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 320px;
    gap: 16px;
    align-items: start;
  }
  .spectrum-view[hidden] {
    display: none;
  }
  .wf-panel {
    min-width: 0;
  }
  .settings-view {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 380px), 1fr));
    gap: 16px;
    align-items: start;
  }
  .center {
    min-height: 100vh;
    display: grid;
    place-items: center;
  }
  @media (max-width: 900px) {
    .spectrum-view {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  @media (max-width: 560px) {
    .hide-sm {
      display: none;
    }
    .status-bar {
      margin-left: 0;
    }
    /* Five tabs don't fit a phone at full padding. */
    nav {
      width: 100%;
      justify-content: space-between;
      gap: 0;
    }
    .tab {
      padding: 6px 7px;
      font-size: 14px;
    }
  }
</style>
