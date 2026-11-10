<script lang="ts">
  /**
   * World map (2D) or globe (3D) of the scheduled satellites: where each is
   * now, the ground track ahead, the area that can hear it, and the night side.
   */
  import { onMount, untrack } from 'svelte';
  import { api, type GroundTrack, type Observer, type Pass } from '../lib/api';
  import type { Globe } from '../lib/globe';
  import {
    footprintDeg,
    groundKm,
    inRange,
    latLon,
    loadLand,
    mapColours,
    norm,
    paintLand,
    posAt,
    ringAround,
    sunPoint,
    terminatorLat,
    type LonLat,
    type MapColours,
    type MapScene,
  } from '../lib/satmap';

  let {
    observer,
    focus = null,
    pass = null,
  }: {
    observer: Observer | null;
    /** Satellite whose track is drawn (default: the first one). */
    focus?: string | null;
    /** Pass picked on the page: its ground track and highest point are shown. */
    pass?: Pass | null;
  } = $props();

  const VIEW_KEY = 'sdr.satmap.view';
  const ALT_KEY = 'sdr.satmap.alt';
  /** How often to fetch the track around now (it covers the next 100 minutes). */
  const REFRESH_MS = 4 * 60_000;
  const read = (k: string) => {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  };
  const save = (k: string, v: string) => {
    try {
      localStorage.setItem(k, v);
    } catch {
      /* storage unavailable */
    }
  };

  let view = $state<'2d' | '3d'>(read(VIEW_KEY) === '3d' ? '3d' : '2d');
  let exaggerate = $state(read(ALT_KEY) === '1');
  let full = $state(false);
  let now = $state(Date.now());
  let live = $state<GroundTrack[]>([]);
  let passPoints = $state<{ key: string; track: GroundTrack } | null>(null);
  let land = $state<LonLat[][] | null>(null);
  let err = $state<string | null>(null);
  let globeErr = $state<string | null>(null);
  let globeReady = $state(false);
  let width = $state(600);

  let card: HTMLElement;
  let wrap: HTMLDivElement;
  let canvas = $state<HTMLCanvasElement>();
  let globeEl = $state<HTMLDivElement>();
  let globe: Globe | null = null;
  let landCanvas: HTMLCanvasElement | null = null;
  let landKey = '';

  const focusName = $derived(focus && live.some((t) => t.satellite === focus) ? focus : (live[0]?.satellite ?? null));
  const focusTrack = $derived(live.find((t) => t.satellite === focusName) ?? null);
  /** The picked pass, when the track around now doesn't already cover it. */
  const farPass = $derived.by(() => {
    if (!pass || pass.satellite !== focusName) return null;
    const pts = focusTrack?.points;
    return pts?.length && pass.aos_ms >= pts[0][0] && pass.los_ms <= pts[pts.length - 1][0] ? null : pass;
  });
  const farKey = $derived(farPass ? `${farPass.satellite}/${farPass.aos_ms}` : null);

  const scene = $derived.by((): MapScene => {
    const flag = (t: GroundTrack | null | undefined) =>
      (t?.points ?? []).map(([, lat, lon, altKm]) => {
        const pos = { lat, lon, altKm };
        return { pos, inRange: inRange(observer, pos) };
      });
    const sats = live.flatMap((t) => {
      const pos = posAt(t.points, now);
      return pos ? [{ name: t.satellite, pos, inRange: inRange(observer, pos), focus: t.satellite === focusName }] : [];
    });
    const far = passPoints && passPoints.key === farKey ? passPoints.track : null;
    let peak: MapScene['peak'] = null;
    if (pass && pass.satellite === focusName && pass.aos_ms > now) {
      const pos = posAt((far ?? focusTrack)?.points ?? [], pass.max_ms);
      if (pos) peak = { pos, label: `${clock(pass.max_ms)} · ${Math.round(pass.max_el_deg)}° up` };
    }
    return { now, observer, sun: sunPoint(now), sats, track: flag(focusTrack), passTrack: flag(far), peak };
  });

  const status = $derived.by(() => {
    const s = scene.sats.find((x) => x.focus);
    if (!s) return err ?? (live.length ? 'Waiting for orbit data.' : 'No satellite positions yet.');
    const parts = [`${s.name} is over ${latLon(s.pos.lat, s.pos.lon)}`, `${Math.round(s.pos.altKm)} km up`];
    if (observer) parts.push(s.inRange ? 'in range of the station now' : `${Math.round(groundKm(observer, s.pos)).toLocaleString()} km away`);
    else parts.push('set the station location to see when it is in range');
    return parts.join(' · ');
  });

  function clock(ms: number): string {
    return new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  }

  async function loadLive() {
    try {
      live = (await api.groundTracks()).tracks;
      err = null;
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }

  // The picked pass is further ahead than the live track: fetch its own stretch.
  $effect(() => {
    const key = farKey;
    const p = untrack(() => farPass);
    if (!key || !p || untrack(() => passPoints?.key) === key) return;
    const minutes = Math.ceil((p.los_ms - p.aos_ms) / 60_000) + 8;
    api.groundTracks(p.aos_ms - 4 * 60_000, minutes).then(
      (r) => {
        const track = r.tracks.find((t) => t.satellite === p.satellite);
        if (track) passPoints = { key, track };
      },
      () => {},
    );
  });

  // ---------- 2D ----------
  const H = $derived(Math.round(width / 2));
  const centre = $derived(observer?.lon_deg ?? 0);

  function draw2d(s: MapScene) {
    const c = canvas?.getContext('2d');
    if (!c || !canvas) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const W = width;
    if (canvas.width !== Math.round(W * dpr) || canvas.height !== Math.round(H * dpr)) {
      canvas.width = Math.round(W * dpr);
      canvas.height = Math.round(H * dpr);
    }
    c.setTransform(dpr, 0, 0, dpr, 0, 0);
    const col = mapColours(canvas);
    /** x of a longitude that may run past ±180 (unwrapped shapes). */
    const X = (lon: number) => ((lon - centre + 180) / 360) * W;
    const Y = (lat: number) => ((90 - lat) / 180) * H;
    /** Draw a shape on every copy of the world it might fall on. */
    const copies = (fn: () => void) => {
      for (const k of [-2, -1, 0, 1, 2]) {
        c.save();
        c.translate(k * W, 0);
        fn();
        c.restore();
      }
    };
    /** Longitudes made continuous, starting within half a world of the centre. */
    const unwrap = (pts: [number, number][]) => {
      let prev: number | null = null;
      return pts.map(([lat, lon]): [number, number] => {
        prev = prev === null ? centre + norm(lon - centre) : prev + norm(lon - prev);
        return [lat, prev];
      });
    };
    const trace = (pts: [number, number][]) => {
      c.beginPath();
      pts.forEach(([lat, lon], i) => (i ? c.lineTo(X(lon), Y(lat)) : c.moveTo(X(lon), Y(lat))));
    };

    // Coastlines, painted once per size and theme, then slid to the centre.
    const key = `${canvas.width}/${col.ocean}/${col.land}`;
    if (land && key !== landKey) {
      landCanvas ??= document.createElement('canvas');
      landCanvas.width = canvas.width;
      landCanvas.height = canvas.height;
      paintLand(landCanvas.getContext('2d')!, landCanvas.width, landCanvas.height, land, col);
      landKey = key;
    }
    c.fillStyle = col.ocean;
    c.fillRect(0, 0, W, H);
    if (land && landCanvas) {
      const shift = (norm(-centre) / 360) * W;
      for (const k of [-1, 0, 1]) c.drawImage(landCanvas, shift + k * W, 0, W, H);
    }

    c.strokeStyle = col.grid;
    c.lineWidth = 1;
    c.beginPath();
    for (let lat = -60; lat <= 60; lat += 30) {
      c.moveTo(0, Y(lat));
      c.lineTo(W, Y(lat));
    }
    for (let k = 1; k < 12; k++) {
      c.moveTo((k / 12) * W, 0);
      c.lineTo((k / 12) * W, H);
    }
    c.stroke();

    // Night side.
    const edge = s.sun.lat >= 0 ? H : 0;
    c.beginPath();
    c.moveTo(0, edge);
    for (let x = 0; x <= W; x += 4) c.lineTo(x, Y(terminatorLat(centre + (x / W) * 360 - 180, s.sun)));
    c.lineTo(W, Y(terminatorLat(centre + 180, s.sun)));
    c.lineTo(W, edge);
    c.closePath();
    c.fillStyle = 'rgba(0, 0, 0, 0.32)';
    c.fill();

    const drawTrack = (pts: MapScene['track'], alpha: number) => {
      if (pts.length < 2) return;
      const line = unwrap(pts.map((p) => [p.pos.lat, p.pos.lon]));
      copies(() => {
        c.globalAlpha = alpha;
        c.strokeStyle = col.track;
        c.lineWidth = 1.5;
        trace(line);
        c.stroke();
        c.globalAlpha = 1;
        c.strokeStyle = col.good;
        c.lineWidth = 3.5;
        c.lineCap = 'round';
        let run: [number, number][] = [];
        const flush = () => {
          if (run.length > 1) {
            trace(run);
            c.stroke();
          }
          run = [];
        };
        pts.forEach((p, i) => (p.inRange ? run.push(line[i]) : flush()));
        flush();
      });
    };
    drawTrack(s.passTrack, 0.55);
    drawTrack(s.track, 1);

    const label = (text: string, lat: number, lon: number, colour: string) => {
      c.font = '600 12px system-ui, sans-serif';
      const w = c.measureText(text).width;
      let x = X(centre + norm(lon - centre)) + 10;
      if (x + w + 6 > W) x -= w + 26;
      const y = Math.min(H - 8, Math.max(14, Y(lat) - 8));
      c.fillStyle = col.surface;
      c.globalAlpha = 0.85;
      c.fillRect(x - 4, y - 12, w + 8, 17);
      c.globalAlpha = 1;
      c.fillStyle = colour;
      c.fillText(text, x, y);
    };

    if (s.observer) {
      const { lat_deg, lon_deg } = s.observer;
      const alt = (s.sats.find((x) => x.focus) ?? s.sats[0])?.pos.altKm ?? 420;
      const ring = unwrap(ringAround(lat_deg, lon_deg, footprintDeg(alt)));
      copies(() => {
        c.setLineDash([4, 4]);
        c.strokeStyle = col.muted;
        c.lineWidth = 1.2;
        trace(ring);
        c.stroke();
        c.setLineDash([]);
        c.fillStyle = col.text;
        c.fillRect(X(lon_deg) - 3.5, Y(lat_deg) - 3.5, 7, 7);
      });
    }

    for (const sat of s.sats) {
      const colour = sat.inRange ? col.good : sat.focus ? col.track : col.sat;
      const radius = footprintDeg(sat.pos.altKm);
      const ring = unwrap(ringAround(sat.pos.lat, sat.pos.lon, radius));
      // A footprint over a pole isn't a closed shape on this map: outline only.
      const closed = Math.abs(sat.pos.lat) + radius < 89;
      copies(() => {
        trace(ring);
        if (closed && sat.focus) {
          c.globalAlpha = 0.16;
          c.fillStyle = colour;
          c.fill();
          c.globalAlpha = 1;
        }
        c.strokeStyle = colour;
        c.lineWidth = sat.focus ? 1.5 : 1;
        c.globalAlpha = sat.focus ? 1 : 0.6;
        c.stroke();
        c.globalAlpha = 1;
        c.beginPath();
        c.arc(X(centre + norm(sat.pos.lon - centre)), Y(sat.pos.lat), sat.focus ? 6 : 4.5, 0, 2 * Math.PI);
        c.fillStyle = colour;
        c.fill();
        c.strokeStyle = col.surface;
        c.lineWidth = 1.5;
        c.stroke();
      });
    }
    if (s.peak) {
      const { lat, lon } = s.peak.pos;
      c.beginPath();
      c.arc(X(centre + norm(lon - centre)), Y(lat), 5, 0, 2 * Math.PI);
      c.strokeStyle = col.good;
      c.lineWidth = 2;
      c.stroke();
      label(s.peak.label, lat, lon, col.good);
    }
    for (const sat of s.sats) label(sat.name, sat.pos.lat, sat.pos.lon, col.text);
  }

  $effect(() => {
    if (view !== '2d') return;
    void [width, land, centre];
    draw2d(scene);
  });

  // ---------- 3D ----------
  $effect(() => {
    if (view !== '3d' || !globeEl || !land) return;
    const el = globeEl;
    const coast = land;
    let dead = false;
    globeErr = null;
    import('../lib/globe').then(
      ({ createGlobe }) => {
        if (dead) return;
        try {
          globe = createGlobe(el, coast);
          const s = untrack(() => scene);
          const at = s.observer
            ? { lat: s.observer.lat_deg, lon: s.observer.lon_deg }
            : (s.sats.find((x) => x.focus)?.pos ?? { lat: 20, lon: 0 });
          globe.aim(Math.max(-60, Math.min(60, at.lat)), 'lon' in at ? at.lon : 0);
          globeReady = true;
        } catch {
          globeErr = "This browser can't show the 3D globe (WebGL is unavailable). The 2D map still works.";
        }
      },
      () => (globeErr = "Couldn't load the 3D globe."),
    );
    return () => {
      dead = true;
      globe?.destroy();
      globe = null;
      globeReady = false;
    };
  });

  $effect(() => {
    if (!globeReady || !globe || !globeEl) return;
    globe.update(scene, mapColours(globeEl) as MapColours, exaggerate ? 3 : 1);
  });

  $effect(() => {
    void [width, full, view];
    if (globeReady) globe?.resize();
  });

  function setView(v: '2d' | '3d') {
    view = v;
    save(VIEW_KEY, v);
  }

  async function toggleFull() {
    try {
      if (document.fullscreenElement) await document.exitFullscreen();
      else await card.requestFullscreen();
    } catch {
      /* not allowed here */
    }
  }

  onMount(() => {
    loadLive();
    loadLand().then(
      (l) => (land = l),
      () => (err = "Couldn't load the coastlines."),
    );
    const tick = setInterval(() => (now = Date.now()), 1000);
    const poll = setInterval(loadLive, REFRESH_MS);
    const ro = new ResizeObserver(() => (width = Math.max(200, Math.round(wrap.clientWidth))));
    ro.observe(wrap);
    const onfull = () => (full = document.fullscreenElement === card);
    document.addEventListener('fullscreenchange', onfull);
    // Themes change the palette: redraw when the OS or the page switches.
    const scheme = matchMedia('(prefers-color-scheme: dark)');
    const retheme = () => (now = Date.now());
    scheme.addEventListener('change', retheme);
    return () => {
      clearInterval(tick);
      clearInterval(poll);
      ro.disconnect();
      document.removeEventListener('fullscreenchange', onfull);
      scheme.removeEventListener('change', retheme);
    };
  });
</script>

<section class="panel satmap" class:full bind:this={card}>
  <div class="head">
    <h2>Satellite map</h2>
    <div class="controls">
      <button type="button" class="small" aria-pressed={full} onclick={toggleFull}>{full ? 'Exit full screen' : 'Full screen'}</button>
      <div class="seg" role="group" aria-label="Map view">
        <button type="button" class="small" class:primary={view === '2d'} aria-pressed={view === '2d'} onclick={() => setView('2d')}
          >2D map</button
        ><button type="button" class="small" class:primary={view === '3d'} aria-pressed={view === '3d'} onclick={() => setView('3d')}
          >3D globe</button
        >
      </div>
    </div>
  </div>

  <div class="wrap" bind:this={wrap}>
    {#if view === '2d'}
      <div role="img" aria-label="World map showing where each satellite is, centred on the station. {status}">
        <canvas bind:this={canvas} style:height="{H}px" aria-hidden="true"></canvas>
      </div>
    {:else}
      <div
        class="globe"
        bind:this={globeEl}
        role="img"
        aria-label="Globe showing each satellite at its height with the area that can hear it. {status}"
      >
        {#if globeErr}<p class="muted small-text note">{globeErr}</p>{:else if !globeReady}<p class="muted small-text note">Loading the globe…</p>{/if}
      </div>
    {/if}
  </div>

  <p class="status small-text"><strong>{new Date(now).toLocaleTimeString()}</strong> {status}</p>
  <div class="legend small-text muted">
    <span><i class="dot" style:background="var(--warn)"></i>Satellite</span>
    <span><i class="dot" style:background="var(--ok)"></i>In range of the station</span>
    <span><i class="line" style:background="var(--warn)"></i>Next 100 min of track</span>
    <span><i class="line thick" style:background="var(--ok)"></i>Track while in range</span>
    <span><i class="ring" style:border-color="var(--warn)"></i>Area that can hear it</span>
    <span><i class="ring dashed"></i>Station's range</span>
    <span><i class="night"></i>Night side</span>
  </div>
  {#if view === '3d'}
    <div class="row3d small-text muted">
      <span>Drag to turn the globe, scroll or pinch to zoom. The cone shows where on the ground the satellite can be heard.</span>
      <label
        ><input
          type="checkbox"
          bind:checked={exaggerate}
          onchange={() => save(ALT_KEY, exaggerate ? '1' : '0')}
        /> Exaggerate height ×3</label
      >
    </div>
  {/if}
</section>

<style>
  .satmap.full {
    display: flex;
    flex-direction: column;
    overflow: auto;
    border-radius: 0;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 6px 12px;
    margin-bottom: 10px;
  }
  .head h2 {
    margin: 0;
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .seg {
    display: inline-flex;
  }
  .seg button:first-child {
    border-radius: 6px 0 0 6px;
  }
  .seg button:last-child {
    border-radius: 0 6px 6px 0;
    margin-left: -1px;
  }
  .wrap {
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg);
    min-width: 0;
  }
  canvas {
    display: block;
    width: 100%;
  }
  .globe {
    position: relative;
    height: clamp(280px, 60vw, 560px);
    cursor: grab;
    touch-action: none;
  }
  .full .wrap {
    flex: 1;
    min-height: 0;
  }
  .full .globe {
    height: 100%;
    min-height: 300px;
  }
  .full canvas {
    max-height: 100%;
  }
  .note {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    margin: 0;
    padding: 16px;
    text-align: center;
  }
  .globe :global(.globe-labels) {
    position: absolute;
    inset: 0;
    overflow: hidden;
    pointer-events: none;
  }
  .globe :global(.globe-labels span) {
    position: absolute;
    left: 0;
    top: 0;
    padding: 0 5px;
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    color: var(--text);
    background: color-mix(in srgb, var(--surface) 85%, transparent);
    border-radius: 3px;
  }
  .globe :global(.globe-labels span.good) {
    color: var(--ok);
  }
  .status {
    margin: 8px 0 6px;
    font-variant-numeric: tabular-nums;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
  }
  .legend span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .legend i {
    display: inline-block;
    flex: none;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
  }
  .line {
    width: 18px;
    height: 2px;
  }
  .line.thick {
    height: 4px;
    border-radius: 2px;
  }
  .ring {
    width: 11px;
    height: 11px;
    border: 1.5px solid var(--muted);
    border-radius: 50%;
  }
  .ring.dashed {
    border-style: dashed;
  }
  .night {
    width: 16px;
    height: 10px;
    border: 1px solid var(--border);
    background: linear-gradient(90deg, var(--border) 50%, color-mix(in srgb, var(--border) 60%, #000) 50%);
  }
  .row3d {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 6px 16px;
    margin-top: 8px;
  }
  .row3d label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }
</style>
