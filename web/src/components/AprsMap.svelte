<script lang="ts">
  import { onMount } from 'svelte';
  import L from 'leaflet';
  import 'leaflet/dist/leaflet.css';
  import type { Device, Track } from '../lib/api';
  import { isSatellite, stationName, symbolName } from '../lib/aprs';

  let {
    stations,
    tracks,
    selectedId,
    tileUrl,
    onselect,
  }: {
    stations: Device[];
    tracks: Track[];
    selectedId: number | null;
    tileUrl: string;
    onselect: (id: number) => void;
  } = $props();

  let el: HTMLDivElement;
  let map = $state.raw<L.Map | null>(null);
  let markerLayer: L.LayerGroup;
  let trackLayer: L.LayerGroup;
  let fitted = false;
  let lastSelected: number | null = null;
  /** Live markers by station id, with what their icon was drawn from. */
  const markers = new Map<number, { marker: L.Marker; look: string }>();

  const esc = (s: string) =>
    s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);

  /** First time the map is visible with positions: frame the ground stations
   * (a satellite on the other side of the world would zoom right out). */
  function fitOnce(m: L.Map) {
    if (fitted || m.getSize().x === 0) return;
    const ground = stations.filter((d) => d.lat !== null && d.lon !== null && !isSatellite(d));
    if (!ground.length) return;
    fitted = true;
    const b = L.latLngBounds(ground.map((d) => [d.lat!, d.lon!] as L.LatLngTuple));
    if (ground.length === 1) m.setView(b.getCenter(), 12);
    else m.fitBounds(b.pad(0.2), { maxZoom: 13 });
  }

  onMount(() => {
    const m = L.map(el, { worldCopyJump: true }).setView([54, -2], 6);
    const osm = tileUrl.includes('openstreetmap.org');
    L.tileLayer(tileUrl, {
      maxZoom: 18,
      attribution: osm ? '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors' : '',
      // The page sends no Referer; tile servers like OSM's ask for one.
      referrerPolicy: 'strict-origin-when-cross-origin',
    } as L.TileLayerOptions).addTo(m);
    trackLayer = L.layerGroup().addTo(m);
    markerLayer = L.layerGroup().addTo(m);
    // The tab may be hidden when the map is created; fix its size (and frame
    // the stations, which needs a real size) once it's shown.
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

  // Station markers: a dot plus the callsign as a label. Updated in place so
  // a hover or click isn't interrupted every time a packet arrives.
  $effect(() => {
    if (!map) return;
    const seen = new Set<number>();
    for (const d of stations) {
      if (d.lat === null || d.lon === null) continue;
      seen.add(d.id);
      const sel = d.id === selectedId;
      const obj = d.model !== 'APRS';
      const name = stationName(d);
      const look = `${sel}|${obj}|${name}`;
      const icon = () =>
        L.divIcon({
          className: 'aprs-marker',
          html: `<span class="dot${sel ? ' sel' : ''}${obj ? ' obj' : ''}"></span><span class="label">${esc(name)}</span>`,
          iconSize: [12, 12],
          iconAnchor: [6, 6],
        });
      const cur = markers.get(d.id);
      if (!cur) {
        const marker = L.marker([d.lat, d.lon], {
          icon: icon(),
          title: `${name} · ${symbolName(d.symbol)}`,
          zIndexOffset: sel ? 1000 : 0,
          keyboard: true,
        })
          .on('click', () => onselect(d.id))
          .addTo(markerLayer);
        markers.set(d.id, { marker, look });
        continue;
      }
      cur.marker.setLatLng([d.lat, d.lon]);
      if (cur.look !== look) {
        cur.marker.setIcon(icon());
        cur.marker.setZIndexOffset(sel ? 1000 : 0);
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

    if (selectedId !== lastSelected) {
      lastSelected = selectedId;
      const s = stations.find((d) => d.id === selectedId);
      if (s && s.lat !== null && s.lon !== null) map.panTo([s.lat, s.lon]);
    }
  });

  // Tracks: split where they cross the antimeridian.
  $effect(() => {
    if (!map) return;
    trackLayer.clearLayers();
    for (const t of tracks) {
      const segs: L.LatLngTuple[][] = [[]];
      t.points.forEach(([, lat, lon], i) => {
        const prev = t.points[i - 1];
        if (prev && Math.abs(prev[2] - lon) > 180) segs.push([]);
        segs[segs.length - 1].push([lat, lon]);
      });
      L.polyline(segs, {
        className: t.device_id === selectedId ? 'aprs-track sel' : 'aprs-track',
        interactive: false,
      }).addTo(trackLayer);
    }
  });
</script>

<div class="map" bind:this={el} role="region" aria-label="APRS station map"></div>

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
  .map :global(.aprs-marker) {
    overflow: visible;
  }
  .map :global(.aprs-marker .dot) {
    display: block;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--accent);
    border: 2px solid var(--surface);
    box-sizing: border-box;
  }
  .map :global(.aprs-marker .dot.obj) {
    background: var(--surface);
    border-color: var(--accent);
  }
  .map :global(.aprs-marker .dot.sel) {
    transform: scale(1.5);
    box-shadow: 0 0 0 3px var(--accent);
  }
  .map :global(.aprs-marker .label) {
    position: absolute;
    left: 15px;
    top: -4px;
    white-space: nowrap;
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
    text-shadow:
      0 0 3px var(--surface),
      0 0 3px var(--surface),
      0 0 3px var(--surface);
  }
  .map :global(.aprs-track) {
    stroke: var(--accent);
    stroke-width: 2;
    stroke-opacity: 0.55;
    fill: none;
  }
  .map :global(.aprs-track.sel) {
    stroke-opacity: 1;
    stroke-width: 3;
  }
</style>
