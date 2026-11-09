<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import { api, type RadioState, type User } from './lib/api';
  import { Stream } from './lib/stream';
  import { formatMHz } from './lib/format';
  import Login from './components/Login.svelte';
  import Settings, { type SettingsTab } from './components/Settings.svelte';
  import Profile from './components/Profile.svelte';
  import Waterfall from './components/Waterfall.svelte';
  import ReceiverBar from './components/ReceiverBar.svelte';
  import PassNotice from './components/PassNotice.svelte';
  import { schedule } from './lib/schedule.svelte';
  import HarmonicsPanel from './components/HarmonicsPanel.svelte';
  import DecoderPanel from './components/DecoderPanel.svelte';
  import BookmarkDrawer from './components/BookmarkDrawer.svelte';
  import { DAB_RATE, MODES, snap, type AudioMode, type Listen } from './lib/audio';
  import { Listener } from './lib/listener.svelte';
  import { fkeys } from './lib/fkeys.svelte';
  import { ALL_GROUPS, modeAt } from './lib/bandplan';
  import { bookmarks, type Bookmark } from './lib/bookmarks.svelte';
  import Sensors from './components/Sensors.svelte';
  import Dashboard from './components/Dashboard.svelte';
  import Aprs from './components/Aprs.svelte';
  import Aircraft from './components/Aircraft.svelte';
  import Sstv from './components/Sstv.svelte';
  import Passes from './components/Passes.svelte';

  let user = $state<User | null>(null);
  let checking = $state(true);
  let radio = $state<RadioState | null>(null);
  let wsUp = $state(false);
  let view = $state<
    'spectrum' | 'dashboard' | 'sensors' | 'aprs' | 'aircraft' | 'sstv' | 'passes' | 'settings' | 'profile'
  >('spectrum');
  let settingsTab = $state<SettingsTab>('receiver');
  let waterfall = $state<ReturnType<typeof Waterfall> | undefined>();
  let harmonics = $state<ReturnType<typeof HarmonicsPanel> | undefined>();
  let listen = $state<Listen>({ on: false, hz: 145_800_000, mode: 'nfm' });
  const ALL_BANDS = new Set(ALL_GROUPS);
  let stream: Stream | null = null;
  /** Audio, the two VFOs and decoders: driven by the front panel. */
  const rx = new Listener(listen, (m) => stream?.send(m));
  onDestroy(rx.attach());
  /** The bookmark manager (slides out over the page). */
  let bmOpen = $state(false);
  let bmAssign = $state<number | null>(null);
  let bmAdding = $state(false);
  let sensors = $state<ReturnType<typeof Sensors> | undefined>();
  let dashboard = $state<ReturnType<typeof Dashboard> | undefined>();
  let aprs = $state<ReturnType<typeof Aprs> | undefined>();
  let sstv = $state<ReturnType<typeof Sstv> | undefined>();
  let passes = $state<ReturnType<typeof Passes> | undefined>();
  let receiver = $state<ReturnType<typeof ReceiverBar> | undefined>();
  /** AOS of a pass this browser opted not to jump to the Passes page for. */
  let keepPage = $state<number | null>(null);
  let switchedFor: number | null = null;

  const isAdmin = $derived(user?.role === 'admin');

  onMount(async () => {
    try {
      user = await api.me();
    } catch {
      user = null;
    }
    checking = false;
  });

  // Bookmarks and F-keys follow whoever is signed in.
  $effect(() => {
    if (user) {
      bookmarks.load();
      fkeys.load();
    } else {
      bookmarks.clear();
      fkeys.reset();
      untrack(() => rx.stop());
      bmOpen = false;
    }
  });

  // One WebSocket per signed-in session.
  $effect(() => {
    if (!user) return;
    const s = new Stream({
      onFrame: (f) => {
        waterfall?.push(f);
        harmonics?.push(f);
        receiver?.push(f);
      },
      onRadio: (st) => (radio = st),
      onEvent: (d, ev) => {
        if (d.decoder === 'aprs') aprs?.push(d, ev);
        else if (d.decoder === 'sstv') {
          sstv?.push(d, ev);
          passes?.push(d, ev);
        } else if (d.decoder === 'passes') passes?.push(d, ev);
        else {
          sensors?.push(d, ev);
          dashboard?.push(d, ev);
        }
      },
      onSstvRows: (r) => sstv?.rows(r),
      onAudio: (f) => rx.feed(f),
      onAudioStatus: (kind, message) => rx.status(kind, message),
      onDecoded: (m) => rx.decoded(m),
      onConnection: (c) => {
        wsUp = c;
        if (c) rx.reconnected();
      },
      onAuthLost: () => {
        user = null;
        radio = null;
      },
    });
    s.start();
    stream = s;
    return () => {
      s.stop();
      stream = null;
    };
  });

  // Scheduler status for the whole app while signed in.
  $effect(() => {
    if (user) return schedule.use();
  });

  // A real pass started with auto-switch on: show the Passes page (once per
  // pass, unless this browser said to keep its page).
  $effect(() => {
    const a = schedule.info?.status.active;
    if (!a || a.test || !schedule.info?.config.auto_switch || switchedFor === a.aos_ms) return;
    switchedFor = a.aos_ms;
    if (keepPage !== a.aos_ms) view = 'passes';
  });

  // While a pass is recording, listen to its downlink (following Doppler) so
  // the S-meter and audio read the satellite rather than whatever channel was
  // picked before. Afterwards go back, unless the listener moved off it.
  let passListen: { aos: number; before: { hz: number; mode: AudioMode }; hz: number } | null = null;
  $effect(() => {
    const a = schedule.info?.status.active;
    if (a?.recording) {
      const hz = Math.round((a.record_hz + (a.doppler_hz ?? 0)) / 100) * 100;
      if (passListen?.aos !== a.aos_ms) {
        passListen = { aos: a.aos_ms, before: untrack(() => ({ hz: listen.hz, mode: listen.mode })), hz };
        listen.mode = 'nfm';
        listen.hz = hz;
      } else if (untrack(() => listen.hz) === passListen.hz && hz !== passListen.hz) {
        passListen.hz = hz;
        listen.hz = hz;
      }
      return;
    }
    if (!passListen) return;
    const { before, hz } = passListen;
    passListen = null;
    if (untrack(() => listen.hz) === hz) {
      listen.mode = before.mode;
      listen.hz = before.hz;
    }
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

  function openSensor(id: number) {
    view = 'sensors';
    sensors?.open(id);
  }

  /** Listen at `hz` (clicked on the spectrum), in the band plan's mode if wanted. */
  function pick(hz: number) {
    if (rx.autoMode) {
      const m = modeAt(hz, ALL_BANDS);
      if (m) listen.mode = m;
    }
    const snapped = snap(hz, listen.mode);
    // Another DAB multiplex: start with its first station.
    if (listen.mode === 'dab' && snapped !== listen.hz) listen.service = null;
    listen.hz = snapped;
    // Picked outside the sampled band (zoomed out): admins bring it in.
    bringIntoBand(listen.hz, listen.mode, 0.5);
  }

  /** Listen to a bookmark; admins also switch to its sample rate and retune
   * when it's outside the band. */
  function selectBookmark(b: Bookmark) {
    tuneListen(b.freq_hz, b.mode, b.sample_rate, b.service_id);
  }

  /** Front panel (knob, typed frequency, F-keys): listen at `hz`, at
   * `sampleRate` if one is given (a bookmark's), and for DAB the service. */
  function tuneListen(hz: number, mode?: AudioMode, sampleRate?: number | null, service?: number | null) {
    if (mode) listen.mode = mode;
    if (listen.mode === 'dab') {
      // DAB stations live on Band III channels; keep the service when it's given.
      hz = snap(hz, 'dab');
      if (service !== undefined) listen.service = service;
      else if (hz !== listen.hz) listen.service = null;
    }
    listen.hz = hz;
    bringIntoBand(hz, listen.mode, 0.45, sampleRate);
  }

  /**
   * Admins: retune so a channel at `hz` sits within `margin` × the sample
   * rate of the centre, a little off the dongle's DC spike in the middle.
   * With `rate`, switch to that sample rate too.
   */
  function bringIntoBand(hz: number, mode: AudioMode, margin: number, rate?: number | null) {
    if (!radio || !isAdmin || !radio.can_tune) return;
    const { center_hz } = radio.settings;
    if (mode === 'dab') {
      // The whole multiplex, centred, at the rate it's sent at.
      if (center_hz !== hz || radio.settings.sample_rate !== DAB_RATE) configure({ center_hz: hz, sample_rate: DAB_RATE });
      return;
    }
    const newRate = rate && rate !== radio.settings.sample_rate && radio.sample_rates.includes(rate) ? rate : null;
    const sample_rate = newRate ?? radio.settings.sample_rate;
    const bw = MODES.find((m) => m.value === mode)?.bw ?? 12_500;
    const inBand = Math.abs(hz - center_hz) + bw / 2 <= sample_rate * margin;
    if (inBand && !newRate) return;
    const offset = Math.min(150_000, sample_rate * 0.1);
    configure({
      ...(inBand ? {} : { center_hz: Math.round((hz + offset) / 1000) * 1000 }),
      ...(newRate ? { sample_rate: newRate } : {}),
    });
  }

  async function configure(patch: { center_hz?: number; sample_rate?: number }) {
    try {
      await api.configure(patch);
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
      <button type="button" class="tab" class:active={view === 'dashboard'} onclick={() => (view = 'dashboard')}>Dashboard</button>
      <button type="button" class="tab" class:active={view === 'sensors'} onclick={() => (view = 'sensors')}>Sensors</button>
      <button type="button" class="tab" class:active={view === 'aprs'} onclick={() => (view = 'aprs')}>APRS</button>
      <button type="button" class="tab" class:active={view === 'aircraft'} onclick={() => (view = 'aircraft')}>Aircraft</button>
      <button type="button" class="tab" class:active={view === 'sstv'} onclick={() => (view = 'sstv')}>SSTV</button>
      <button type="button" class="tab" class:active={view === 'passes'} onclick={() => (view = 'passes')}>Passes</button>
      {#if isAdmin}
        <button type="button" class="tab" class:active={view === 'settings'} onclick={() => (view = 'settings')}>Settings</button>
      {/if}
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
      <button
        type="button"
        class="user"
        class:active={view === 'profile'}
        title="Your profile and password"
        onclick={() => (view = 'profile')}>{user.username}</button
      >
      <button type="button" class="small" onclick={logout}>Sign out</button>
    </div>
  </header>

  <main class="content">
    <div class="rx-area">
      <ReceiverBar
        bind:this={receiver}
        {radio}
        {isAdmin}
        {rx}
        ontune={tuneListen}
        onbookmarks={(slot) => {
          bmAssign = slot ?? null;
          bmOpen = true;
        }}
        onsavebookmark={() => {
          bmAssign = null;
          bmAdding = true;
          bmOpen = true;
        }}
        onsettings={() => {
          settingsTab = 'rotation';
          view = 'settings';
        }}
      />
      <PassNotice {isAdmin} {radio} {keepPage} onkeeppage={(a) => (keepPage = a)} onopen={() => (view = 'passes')} />
    </div>
    <div class="spectrum-view" hidden={view !== 'spectrum'}>
      <div class="panel wf-panel">
        <Waterfall
          bind:this={waterfall}
          canTune={isAdmin && !!radio?.can_tune}
          ontune={(hz) => configure({ center_hz: hz })}
          {listen}
          sub={rx.other}
          onpick={pick}
          bookmarks={bookmarks.list}
          onbookmark={selectBookmark}
          ranges={radio?.ranges ?? []}
          directSampling={radio?.settings.direct_sampling ?? 0}
          sampleRates={radio?.sample_rates ?? []}
          onrate={(r) => api.configure({ sample_rate: r }).catch((e) => console.warn('rate change failed', e))}
        />
      </div>
      <DecoderPanel {rx} />
      <HarmonicsPanel bind:this={harmonics} {radio} {listen} {isAdmin} />
    </div>
    <!-- Kept mounted so live events keep arriving while another tab is open. -->
    <div hidden={view !== 'dashboard'}>
      <Dashboard bind:this={dashboard} {isAdmin} active={view === 'dashboard'} onopen={openSensor} />
    </div>
    <div hidden={view !== 'sensors'}>
      <Sensors bind:this={sensors} {isAdmin} {radio} />
    </div>
    <div hidden={view !== 'aprs'}>
      <Aprs bind:this={aprs} {isAdmin} {radio} />
    </div>
    <div hidden={view !== 'aircraft'}>
      <Aircraft
        {isAdmin}
        {radio}
        active={view === 'aircraft'}
        onsettings={(t) => {
          settingsTab = t;
          view = 'settings';
        }}
      />
    </div>
    <div hidden={view !== 'sstv'}>
      <Sstv bind:this={sstv} {isAdmin} {radio} />
    </div>
    <div hidden={view !== 'passes'}>
      <Passes bind:this={passes} {isAdmin} {radio} onsettings={(t = 'passes') => {
          settingsTab = t;
          view = 'settings';
        }} />
    </div>
    {#if view === 'settings' && isAdmin}
      <Settings {radio} me={user} bind:tab={settingsTab} />
    {:else if view === 'profile'}
      <Profile {user} onlogout={logout} />
    {/if}
  </main>
  <BookmarkDrawer
    bind:open={bmOpen}
    bind:assign={bmAssign}
    bind:adding={bmAdding}
    {listen}
    {isAdmin}
    {radio}
    onselect={selectBookmark}
  />
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
    background: transparent;
    border-color: transparent;
    padding: 4px 8px;
    max-width: 12em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .user:hover,
  .user.active {
    color: var(--text);
    border-color: var(--border);
  }
  .content {
    padding: 16px;
    max-width: 1600px;
    margin: 0 auto;
  }
  .rx-area {
    display: grid;
    gap: 10px;
    margin-bottom: 16px;
    min-width: 0;
  }
  .spectrum-view {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 16px;
    align-items: start;
  }
  .spectrum-view[hidden] {
    display: none;
  }
  .wf-panel {
    min-width: 0;
  }
  .center {
    min-height: 100vh;
    display: grid;
    place-items: center;
  }
  @media (max-width: 560px) {
    .hide-sm {
      display: none;
    }
    .status-bar {
      margin-left: 0;
    }
    /* The tabs don't all fit a phone: let the bar scroll sideways. */
    nav {
      width: 100%;
      gap: 0;
      overflow-x: auto;
      scrollbar-width: none;
    }
    nav::-webkit-scrollbar {
      display: none;
    }
    .tab {
      flex: none;
    }
    .tab {
      padding: 6px 7px;
      font-size: 14px;
    }
  }
</style>
