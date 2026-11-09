<script lang="ts">
  import { onMount } from 'svelte';
  import L from 'leaflet';
  import 'leaflet/dist/leaflet.css';
  import type { AdsbRange, AdsbTrailPoint, Aircraft } from '../lib/api';
  import { aircraftName, altColour, altHue, emergency, iconKind, iconSize, iconSvg, shortAlt } from '../lib/adsb';

  export type TrailMode = 'all' | 'selected' | 'off';
  export type RangeMode = 'off' | 'rings' | '24h' | 'all';

  let {
    aircraft,
    trails,
    selected,
    tileUrl,
    home = null,
    trailMode = 'all',
    labels = true,
    range = null,
    rangeMode = 'rings',
    follow = false,
    onselect,
  }: {
    aircraft: Aircraft[];
    /** Trail points per ICAO address. */
    trails: Map<string, AdsbTrailPoint[]>;
    selected: string | null;
    tileUrl: string;
    home?: [number, number] | null;
    trailMode?: TrailMode;
    labels?: boolean;
    range?: AdsbRange | null;
    rangeMode?: RangeMode;
    /** Keep the selected aircraft in the middle. */
    follow?: boolean;
    onselect: (icao: string) => void;
  } = $props();

  const HOUSE =
    '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M3 11.2 12 3.5l9 7.7V21h-6.2v-6H9.2v6H3z" fill="currentColor"/></svg>';

  let el: HTMLDivElement;
  let map = $state.raw<L.Map | null>(null);
  let markerLayer: L.LayerGroup;
  let trailLayer: L.LayerGroup;
  let rangeLayer: L.LayerGroup;
  let renderer: L.Canvas;
  let homeMarker: L.Marker | null = null;
  let homeBtn: HTMLButtonElement | null = null;
  let fitted = false;
  const markers = new Map<string, { marker: L.Marker; look: string }>();
  const trailCache = new Map<string, { layer: L.LayerGroup; key: string }>();

  const esc = (s: string) =>
    s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);

  function centreHome() {
    if (map && home) map.setView(home, Math.max(map.getZoom(), 9));
  }

  /** First view: around home, or around whatever is flying. */
  function fitOnce(m: L.Map) {
    if (fitted || m.getSize().x === 0) return;
    if (home) {
      fitted = true;
      m.setView(home, 8);
      return;
    }
    const pts = aircraft.filter((a) => a.lat !== null).map((a) => [a.lat!, a.lon!] as L.LatLngTuple);
    if (!pts.length) return;
    fitted = true;
    m.fitBounds(L.latLngBounds(pts).pad(0.2), { maxZoom: 10 });
  }

  onMount(() => {
    const m = L.map(el, { worldCopyJump: true }).setView([54, -2], 6);
    const osm = tileUrl.includes('openstreetmap.org');
    L.tileLayer(tileUrl, {
      maxZoom: 18,
      attribution: osm ? '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors' : '',
      referrerPolicy: 'strict-origin-when-cross-origin',
    } as L.TileLayerOptions).addTo(m);
    renderer = L.canvas({ padding: 0.3 });
    rangeLayer = L.layerGroup().addTo(m);
    trailLayer = L.layerGroup().addTo(m);
    markerLayer = L.layerGroup().addTo(m);
    const HomeControl = L.Control.extend({
      onAdd() {
        const wrap = L.DomUtil.create('div', 'leaflet-bar');
        const b = L.DomUtil.create('button', 'adsb-home-btn', wrap) as HTMLButtonElement;
        b.type = 'button';
        b.innerHTML = HOUSE;
        L.DomEvent.disableClickPropagation(wrap);
        L.DomEvent.on(b, 'click', (e) => {
          L.DomEvent.preventDefault(e);
          centreHome();
        });
        homeBtn = b;
        return wrap;
      },
    });
    new HomeControl({ position: 'topleft' }).addTo(m);
    const ro = new ResizeObserver(() => {
      m.invalidateSize();
      fitOnce(m);
    });
    ro.observe(el);
    map = m;
    return () => {
      ro.disconnect();
      m.remove();
      map = null;
    };
  });

  // Home marker and its button.
  $effect(() => {
    if (!map) return;
    if (homeBtn) {
      homeBtn.disabled = !home;
      const t = home ? 'Centre on home' : 'Set the station location in Settings → Passes to show home';
      homeBtn.title = t;
      homeBtn.setAttribute('aria-label', t);
    }
    if (!home) {
      homeMarker?.remove();
      homeMarker = null;
      return;
    }
    if (homeMarker) homeMarker.setLatLng(home);
    else
      homeMarker = L.marker(home, {
        icon: L.divIcon({ className: 'adsb-home', html: `<span class="house">${HOUSE}</span>`, iconSize: [22, 22], iconAnchor: [11, 11] }),
        title: 'Home: station location',
        zIndexOffset: -100,
        keyboard: false,
      }).addTo(map);
    fitOnce(map);
  });

  // Aircraft markers, updated in place.
  $effect(() => {
    if (!map) return;
    const seen = new Set<string>();
    for (const a of aircraft) {
      if (a.lat === null || a.lon === null) continue;
      seen.add(a.icao);
      const sel = a.icao === selected;
      const kind = iconKind(a);
      const alt = a.alt_baro_ft ?? a.alt_gnss_ft;
      const colour = altColour(alt, a.on_ground);
      const track = a.track_deg ?? a.heading_deg;
      const em = emergency(a);
      const name = aircraftName(a);
      const label = labels || sel ? `${esc(name)}<small>${esc(shortAlt(a))}</small>` : '';
      const look = `${kind}|${colour}|${track === null ? '' : Math.round(track / 3)}|${sel}|${label}|${em}`;
      const size = iconSize(kind);
      const icon = () =>
        L.divIcon({
          className: `adsb-marker${sel ? ' sel' : ''}${em ? ' emergency' : ''}`,
          html: `${iconSvg(kind, colour, track, sel)}${label ? `<span class="label" style="left:${size / 2 + 6}px">${label}</span>` : ''}`,
          iconSize: [size, size],
          iconAnchor: [size / 2, size / 2],
        });
      const title = `${name}${a.type_code ? ` · ${a.type_code}` : ''} · ${shortAlt(a) || 'altitude unknown'}`;
      const cur = markers.get(a.icao);
      if (!cur) {
        const marker = L.marker([a.lat, a.lon], { icon: icon(), title, zIndexOffset: sel ? 1000 : 0, keyboard: true })
          .on('click', () => onselect(a.icao))
          .addTo(markerLayer);
        markers.set(a.icao, { marker, look });
        continue;
      }
      cur.marker.setLatLng([a.lat, a.lon]);
      if (cur.look !== look) {
        cur.marker.setIcon(icon());
        cur.marker.setZIndexOffset(sel ? 1000 : em ? 900 : 0);
        cur.marker.options.title = title;
        cur.look = look;
      }
    }
    for (const [id, { marker }] of markers) {
      if (!seen.has(id)) {
        marker.remove();
        markers.delete(id);
      }
    }
    fitOnce(map);
    if (follow && selected) {
      const s = aircraft.find((a) => a.icao === selected);
      if (s && s.lat !== null && s.lon !== null) map.panTo([s.lat, s.lon], { animate: true });
    }
  });

  /** Trail as runs of one colour; a gap of over 5 min breaks the line. */
  function trailLayers(points: AdsbTrailPoint[], sel: boolean): L.LayerGroup {
    const g = L.layerGroup();
    let run: L.LatLngTuple[] = [];
    let bucket = '';
    const flush = () => {
      if (run.length > 1) {
        L.polyline(run, {
          renderer,
          color: bucket,
          weight: sel ? 3.5 : 2,
          opacity: sel ? 1 : 0.8,
          interactive: false,
        }).addTo(g);
      }
    };
    points.forEach((p, i) => {
      const prev = points[i - 1];
      const b = p.ground ? altColour(null, true) : p.alt === null ? altColour(null) : `hsl(${Math.round(altHue(p.alt) / 8) * 8} 85% 55%)`;
      if (prev && (p.ts - prev.ts > 300_000 || Math.abs(prev.lon - p.lon) > 180)) {
        flush();
        run = [];
      } else if (b !== bucket && run.length) {
        flush();
        run = [run[run.length - 1]];
      }
      bucket = b;
      run.push([p.lat, p.lon]);
    });
    flush();
    return g;
  }

  $effect(() => {
    if (!map) return;
    const want = new Set<string>();
    if (trailMode !== 'off') {
      for (const a of aircraft) {
        if (trailMode === 'selected' && a.icao !== selected) continue;
        const pts = trails.get(a.icao);
        if (!pts || pts.length < 2) continue;
        want.add(a.icao);
        const sel = a.icao === selected;
        const key = `${pts.length}|${pts[pts.length - 1].ts}|${sel}`;
        const cur = trailCache.get(a.icao);
        if (cur?.key === key) continue;
        cur?.layer.remove();
        trailCache.set(a.icao, { layer: trailLayers(pts, sel).addTo(trailLayer), key });
      }
    }
    for (const [id, t] of trailCache) {
      if (!want.has(id)) {
        t.layer.remove();
        trailCache.delete(id);
      }
    }
  });

  /** A point `km` from `from` on bearing `deg`. */
  function dest(from: [number, number], deg: number, km: number): L.LatLngTuple {
    const R = 6371;
    const d = km / R;
    const b = (deg * Math.PI) / 180;
    const la1 = (from[0] * Math.PI) / 180;
    const lo1 = (from[1] * Math.PI) / 180;
    const la2 = Math.asin(Math.sin(la1) * Math.cos(d) + Math.cos(la1) * Math.sin(d) * Math.cos(b));
    const lo2 = lo1 + Math.atan2(Math.sin(b) * Math.sin(d) * Math.cos(la1), Math.cos(d) - Math.sin(la1) * Math.sin(la2));
    return [(la2 * 180) / Math.PI, (((lo2 * 180) / Math.PI + 540) % 360) - 180];
  }

  // Range rings every 50 km, and the furthest range per 10° sector.
  $effect(() => {
    if (!map) return;
    rangeLayer.clearLayers();
    if (!home || rangeMode === 'off') return;
    // Canvas paths can't take CSS classes: read the theme's colours.
    const css = getComputedStyle(el);
    const muted = css.getPropertyValue('--muted').trim() || '#94a3b8';
    const accent = css.getPropertyValue('--accent').trim() || '#38bdf8';
    for (const km of [50, 100, 150, 200, 300]) {
      L.circle(home, {
        renderer,
        radius: km * 1000,
        color: muted,
        weight: 1,
        opacity: 0.6,
        dashArray: '4 4',
        interactive: false,
        fill: false,
      }).addTo(rangeLayer);
      L.marker(dest(home, 0, km), {
        icon: L.divIcon({ className: 'adsb-ring-label', html: `${km} km`, iconSize: [50, 14], iconAnchor: [25, 7] }),
        interactive: false,
        keyboard: false,
      }).addTo(rangeLayer);
    }
    if (range && (rangeMode === '24h' || rangeMode === 'all')) {
      const v = rangeMode === '24h' ? range.last_24h : range.all_time;
      const step = 360 / v.length;
      // Directions nothing has been heard from yet are left out, rather
      // than pulling the outline back to home.
      const pts = v.flatMap((km, i) => (km > 0 ? [dest(home, i * step + step / 2, km)] : []));
      if (pts.length >= 3) {
        L.polygon(pts, { renderer, color: accent, weight: 1.5, fillColor: accent, fillOpacity: 0.08, interactive: false }).addTo(
          rangeLayer,
        );
      }
    }
  });
</script>

<div class="map" bind:this={el} role="region" aria-label="Aircraft map"></div>

<style>
  .map {
    width: 100%;
    height: 100%;
    min-height: 420px;
    border-radius: 8px;
    background: var(--surface-2);
  }
  .map :global(.leaflet-tile-pane) {
    filter: var(--tile-filter);
  }
  .map :global(.leaflet-container),
  .map :global(.leaflet-control-attribution) {
    font: inherit;
    font-size: 11px;
  }
  .map :global(.adsb-marker) {
    overflow: visible;
    cursor: pointer;
  }
  .map :global(.adsb-marker svg) {
    display: block;
    filter: drop-shadow(0 0 1px rgba(0, 0, 0, 0.6));
  }
  .map :global(.adsb-marker.sel svg) {
    filter: drop-shadow(0 0 4px var(--accent));
  }
  .map :global(.adsb-marker .label) {
    position: absolute;
    top: -2px;
    display: grid;
    line-height: 1.15;
    white-space: nowrap;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text);
    text-shadow:
      0 0 3px var(--surface),
      0 0 3px var(--surface),
      0 0 3px var(--surface);
    pointer-events: none;
  }
  .map :global(.adsb-marker .label small) {
    font-size: 10.5px;
    font-weight: 500;
    color: var(--muted);
  }
  .map :global(.adsb-marker.emergency .label) {
    color: #ef4444;
  }
  .map :global(.adsb-home .house) {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: var(--surface);
    color: var(--text);
    border: 2px solid var(--text);
    box-sizing: border-box;
  }
  .map :global(.adsb-home-btn) {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    padding: 0;
    border: none;
    border-radius: 2px;
    background: #fff;
    color: #333;
    cursor: pointer;
  }
  .map :global(.adsb-home-btn:disabled) {
    color: #bbb;
    cursor: not-allowed;
  }
  .map :global(.adsb-ring-label) {
    font-size: 10.5px;
    color: var(--muted);
    text-align: center;
    text-shadow:
      0 0 3px var(--surface),
      0 0 3px var(--surface);
  }
</style>
