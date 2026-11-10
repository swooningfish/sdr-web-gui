<script lang="ts">
  /** ARISS SSTV events: the one in progress or coming up, and how the station will receive it. */
  import type { ArissEvent, Pass, ScheduleInfo } from '../lib/api';
  import { ARISS_EVENTS_URL, ARISS_GALLERY_URL, ISS_NORAD, download, eventDuring, googleUrl, outlookUrl, passesIcs, utc } from '../lib/ariss';
  import { formatMHz, relTime } from '../lib/format';
  import { countdown } from '../lib/passes';

  let { info, now, isAdmin, onsettings }: { info: ScheduleInfo; now: number; isAdmin: boolean; onsettings: () => void } = $props();

  // An sdrd from before ARISS events were added sends none.
  const ariss = $derived(info.status.ariss ?? { checked_ms: null, error: null, events: [] });
  const iss = $derived(info.config.satellites.find((s) => s.norad_id === ISS_NORAD) ?? null);
  /** In progress, else the next to start. */
  const event = $derived(
    ariss.events.filter((e) => e.end_ms > now).sort((a, b) => a.start_ms - b.start_ms)[0] ?? null,
  );
  const past = $derived(ariss.events.filter((e) => e.end_ms <= now).sort((a, b) => b.start_ms - a.start_ms));
  const on = $derived(!!event && event.start_ms <= now);
  const passes = $derived.by((): Pass[] => {
    if (!event || !iss) return [];
    return info.status.passes.filter((p) => p.satellite === iss.name && eventDuring([event], p.aos_ms, p.los_ms));
  });
  /** Why the station won't hear the event as things stand. */
  const problem = $derived.by(() => {
    if (!event) return null;
    if (!iss || !iss.enabled) return 'The ISS is not in the list of satellites to follow, so its passes are not received.';
    if (!info.status.location) return 'Set the station location to predict the passes.';
    if (!info.config.auto_switch) return 'Auto-switch for passes is off, so the receiver will not tune to these passes.';
    if (event.freq_hz && !info.config.ariss_auto && iss.record_hz !== event.freq_hz)
      return `ISS passes are set to record ${formatMHz(iss.record_hz, 3)}, not the event's ${formatMHz(event.freq_hz, 3)}.`;
    return null;
  });
  const local = (ms: number) =>
    new Date(ms).toLocaleString(undefined, { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });

  const CAL_KEY = 'sdr.ariss.calendar';
  type Cal = 'ics' | 'google' | 'outlook';
  const CALS: { id: Cal; label: string }[] = [
    { id: 'ics', label: 'Calendar file (.ics): Apple, Outlook, Thunderbird' },
    { id: 'google', label: 'Google Calendar' },
    { id: 'outlook', label: 'Outlook.com' },
  ];
  function stored(): Cal {
    try {
      const v = localStorage.getItem(CAL_KEY);
      return v === 'google' || v === 'outlook' ? v : 'ics';
    } catch {
      return 'ics';
    }
  }
  let cal = $state<Cal>(stored());
  function pick() {
    try {
      localStorage.setItem(CAL_KEY, cal);
    } catch {
      /* storage unavailable */
    }
  }

  function calendar(e: ArissEvent) {
    download(`ariss-series-${e.series}-passes.ics`, 'text/calendar', passesIcs(e, passes));
  }
</script>

{#if event || past.length}
  <section class="panel">
    <div class="head">
      <h2>ARISS SSTV events</h2>
      {#if ariss.checked_ms}<span class="muted small-text">Checked {relTime(ariss.checked_ms, now)}</span>{/if}
    </div>
    {#if event}
      <div class="event" class:on>
        <div class="title">
          <span class="series small-text">ARISS Series {event.series}</span>
          <strong>{event.name}</strong>
          <span class="badge small-text" class:on>{on ? 'On now' : `Starts ${countdown(event.start_ms, now)}`}</span>
        </div>
        <dl>
          <dt>Start</dt><dd>{utc(event.start_ms)} <span class="muted">({local(event.start_ms)} your time)</span></dd>
          <dt>End</dt><dd>{utc(event.end_ms)} <span class="muted">({local(event.end_ms)} your time)</span></dd>
          {#if event.freq_hz}
            <dt>Frequency</dt><dd>{formatMHz(event.freq_hz, 3)} FM <span class="muted">Doppler is followed for you</span></dd>
          {/if}
          {#if event.mode}<dt>SSTV mode</dt><dd>{event.mode}</dd>{/if}
          {#if event.cadence}<dt>Schedule</dt><dd>{event.cadence}</dd>{/if}
          <dt>This station</dt>
          <dd>
            {#if !info.status.location}
              no location set
            {:else if passes.length}
              {passes.length} {passes.length === 1 ? 'pass' : 'passes'} predicted during the event{#if event.freq_hz && info.config.ariss_auto && info.config.auto_switch},
                received on {formatMHz(event.freq_hz, 3)}{/if}
            {:else}
              no passes predicted during the event yet
            {/if}
          </dd>
        </dl>
        {#if problem}
          <p class="callout small-text">
            {problem}
            {#if isAdmin}<button type="button" class="small" onclick={onsettings}>Open settings</button>{/if}
          </p>
        {/if}
        <p class="small-text note">
          Send your decoded pictures to the <a href={ARISS_GALLERY_URL} target="_blank" rel="noopener">ARISS SSTV Gallery</a>.
          Original decodes qualify for the event certificate.
        </p>
        {#if passes.length}
          <div class="cal">
            <label class="small-text"
              >Add the passes to
              <select bind:value={cal} onchange={pick}>
                {#each CALS as c (c.id)}<option value={c.id}>{c.label}</option>{/each}
              </select></label
            >
            <button type="button" class="small" class:primary={cal === 'ics'} onclick={() => calendar(event)}
              >Download all {passes.length} {passes.length === 1 ? 'pass' : 'passes'} in one file (.ics)</button
            >
          </div>
          {#if cal !== 'ics'}
            <p class="muted small-text hint">
              {#if cal === 'google'}
                Google Calendar's links add one pass at a time. For all of them at once, download the file and import it at
                Google Calendar → Settings → Import &amp; export.
              {:else}
                Outlook.com's links add one pass at a time. For all of them at once, download the file and use Add calendar →
                Upload from file.
              {/if}
            </p>
            <ul class="cal-list small-text">
              {#each passes as p (p.aos_ms)}
                <li>
                  <span>{local(p.aos_ms)} <span class="muted">· max {Math.round(p.max_el_deg)}°</span></span>
                  <a href={cal === 'google' ? googleUrl(event, p) : outlookUrl(event, p)} target="_blank" rel="noopener"
                    >Add to {cal === 'google' ? 'Google Calendar' : 'Outlook.com'}</a
                  >
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      </div>
    {:else}
      <p class="muted small-text">No event is scheduled at the moment.</p>
    {/if}
    {#if past.length}
      <details class="small-text">
        <summary>Recent events <span class="muted">last {past.length}</span></summary>
        <ul>
          {#each past as e (e.series)}
            <li>
              <strong>Series {e.series}</strong> {e.name} <span class="muted">· {utc(e.start_ms)} to {utc(e.end_ms)}</span>
            </li>
          {/each}
        </ul>
      </details>
    {/if}
    <p class="muted small-text source">
      Read from <a href={ARISS_EVENTS_URL} target="_blank" rel="noopener">ARISS Upcoming SSTV events</a> every 6 hours.
      Times can change at short notice.{#if ariss.error}
        <span class="error-text">Last check failed: {ariss.error}</span>{/if}
    </p>
  </section>
{/if}

<style>
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 6px 12px;
  }
  .event {
    border: 1px solid var(--border);
    border-left: 4px solid var(--muted);
    border-radius: 8px;
    padding: 10px 12px;
    margin: 8px 0;
  }
  .event.on {
    border-left-color: var(--ok);
  }
  .title {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 10px;
    margin-bottom: 8px;
    font-size: 15px;
  }
  .series {
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .badge {
    padding: 0 8px;
    border: 1px solid var(--border);
    border-radius: 999px;
  }
  .badge.on {
    border-color: var(--ok);
    color: var(--ok);
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 14px;
    margin: 0;
    font-size: 14px;
    font-variant-numeric: tabular-nums;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
  }
  .callout {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin: 10px 0 0;
    padding: 8px 10px;
    border: 1px solid var(--warn);
    border-radius: 8px;
  }
  .note {
    margin: 10px 0 8px;
  }
  .cal {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
  }
  .cal select {
    padding: 4px 8px;
    max-width: 100%;
  }
  .hint {
    margin: 8px 0 4px;
  }
  .cal-list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 220px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
    font-variant-numeric: tabular-nums;
  }
  .cal-list li {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 2px 12px;
    padding: 5px 10px;
    border-bottom: 1px solid var(--border);
  }
  .cal-list li:last-child {
    border-bottom: none;
  }
  details ul {
    margin: 6px 0 0;
    padding-left: 18px;
  }
  summary {
    cursor: pointer;
  }
  .source {
    margin: 8px 0 0;
  }
  .error-text {
    color: var(--bad);
  }
  a {
    color: var(--accent);
  }
</style>
