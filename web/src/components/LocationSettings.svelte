<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { maidenhead } from '../lib/geo';

  let lat = $state('');
  let lon = $state('');
  let alt = $state('0');
  let query = $state('');
  let places = $state<{ name: string; lat_deg: number; lon_deg: number }[]>([]);
  let msg = $state<{ ok: boolean; text: string } | null>(null);
  let busy = $state(false);
  let locating = $state(false);

  // Browsers only share position with secure pages (HTTPS, or localhost).
  const secure = typeof window !== 'undefined' && window.isSecureContext;

  onMount(async () => {
    try {
      const l = (await api.schedule()).status.location;
      if (l) {
        lat = String(l.lat_deg);
        lon = String(l.lon_deg);
        alt = String(l.alt_m);
      }
    } catch (e) {
      msg = { ok: false, text: e instanceof Error ? e.message : String(e) };
    }
  });

  function fill(la: number, lo: number, how: string) {
    lat = la.toFixed(4);
    lon = lo.toFixed(4);
    places = [];
    msg = { ok: true, text: `${how}. Check the altitude, then press Save location.` };
  }

  async function search(e: SubmitEvent) {
    e.preventDefault();
    msg = null;
    places = [];
    const q = query.trim();
    if (!q) return;
    const m = maidenhead(q);
    if (m) return fill(m.lat, m.lon, `Centre of locator ${q.toUpperCase()}`);
    busy = true;
    try {
      const r = await api.geocode(q);
      if (r.places.length === 0) msg = { ok: false, text: `Nothing found for "${q}"` };
      else if (r.places.length === 1) fill(r.places[0].lat_deg, r.places[0].lon_deg, `Found ${r.places[0].name}`);
      else places = r.places;
    } catch (e) {
      msg = { ok: false, text: e instanceof Error ? e.message : String(e) };
    } finally {
      busy = false;
    }
  }

  function useDevice() {
    msg = null;
    if (!secure) {
      msg = {
        ok: false,
        text: `Browsers only share location with secure pages, and this one is ${location.protocol}//${location.host}. Use the search instead, or open the site over HTTPS.`,
      };
      return;
    }
    if (!navigator.geolocation) {
      msg = { ok: false, text: 'This browser has no location service. Use the search instead.' };
      return;
    }
    locating = true;
    navigator.geolocation.getCurrentPosition(
      (p) => {
        locating = false;
        if (p.coords.altitude !== null) alt = String(Math.round(p.coords.altitude));
        fill(p.coords.latitude, p.coords.longitude, `From this device (±${Math.round(p.coords.accuracy)} m)`);
      },
      (e) => {
        locating = false;
        const why =
          e.code === e.PERMISSION_DENIED
            ? 'Location permission was refused. Allow it for this site in the browser, or use the search.'
            : e.code === e.POSITION_UNAVAILABLE
              ? "This device couldn't work out where it is (on a PC, the operating system's location services may be off). Use the search instead."
              : 'Getting the location timed out. Try again, or use the search.';
        msg = { ok: false, text: why };
      },
      { enableHighAccuracy: false, timeout: 20_000, maximumAge: 600_000 },
    );
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    const obs = { lat_deg: Number(lat), lon_deg: Number(lon), alt_m: Number(alt || 0) };
    if (lat === '' || lon === '' || ![obs.lat_deg, obs.lon_deg, obs.alt_m].every(Number.isFinite)) {
      msg = { ok: false, text: 'Enter latitude and longitude in decimal degrees' };
      return;
    }
    busy = true;
    try {
      await api.setLocation(obs);
      msg = { ok: true, text: 'Location saved; passes recalculated' };
    } catch (e) {
      msg = { ok: false, text: e instanceof Error ? e.message : String(e) };
    } finally {
      busy = false;
    }
  }
</script>

<section class="panel">
  <h2>Station location</h2>
  <p class="muted small-text hint">Used to predict satellite passes and Doppler. A few hundred metres out makes no difference.</p>

  <form class="find" onsubmit={search}>
    <input
      bind:value={query}
      placeholder="Town, postcode or locator (e.g. IO91wm)"
      aria-label="Find a place, postcode or Maidenhead locator"
    />
    <button type="submit" class="small" disabled={busy}>Find</button>
  </form>
  {#if places.length}
    <ul class="places">
      {#each places as p (p.name)}
        <li>
          <button type="button" class="link" onclick={() => fill(p.lat_deg, p.lon_deg, `Found ${p.name}`)}>{p.name}</button>
        </li>
      {/each}
    </ul>
  {/if}
  <div class="buttons">
    <button type="button" class="small" onclick={useDevice} disabled={locating}
      >{locating ? 'Locating…' : "Use this device's location"}</button
    >
    {#if !secure}
      <span class="muted small-text">Needs HTTPS (or localhost)</span>
    {/if}
  </div>

  <form class="loc" onsubmit={save}>
    <label>Latitude <input bind:value={lat} inputmode="decimal" placeholder="51.5074" /></label>
    <label>Longitude <input bind:value={lon} inputmode="decimal" placeholder="-0.1278" /></label>
    <label>Altitude (m) <input bind:value={alt} inputmode="decimal" /></label>
    <div class="buttons">
      <button type="submit" class="small primary" disabled={busy}>Save location</button>
    </div>
  </form>
  {#if msg}<p class:error={!msg.ok} class:success={msg.ok}>{msg.text}</p>{/if}
  <p class="muted small-text hint">Place search uses OpenStreetMap Nominatim; the search text is sent to it from the server.</p>
</section>

<style>
  .hint {
    margin: 0 0 10px;
  }
  .find {
    display: flex;
    gap: 6px;
  }
  .find input {
    flex: 1;
    min-width: 0;
  }
  .places {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: grid;
    gap: 4px;
  }
  button.link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    text-align: left;
    font-size: 13px;
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin: 8px 0;
  }
  .loc {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
    gap: 6px;
    align-items: end;
    margin-top: 6px;
  }
  .loc label {
    display: grid;
    gap: 2px;
    font-size: 13px;
  }
  .loc input {
    min-width: 0;
  }
  .loc .buttons {
    grid-column: 1 / -1;
    margin: 0;
  }
</style>
