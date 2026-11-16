<script lang="ts">
  import { onMount } from 'svelte';
  import L from 'leaflet';
  import 'leaflet/dist/leaflet.css';
  import type { MeshNode } from '../lib/api';
  import { nodeName } from '../lib/mesh';

  let {
    nodes,
    selected,
    tileUrl,
    onselect,
  }: {
    nodes: MeshNode[];
    selected: string | null;
    tileUrl: string;
    onselect: (key: string) => void;
  } = $props();

  let el: HTMLDivElement;
  let map = $state.raw<L.Map | null>(null);
  let layer: L.LayerGroup;
  let fitted = false;
  let lastSelected: string | null = null;
  const markers = new Map<string, { marker: L.Marker; look: string }>();

  const esc = (s: string) =>
    s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);

  const placed = $derived(nodes.filter((n) => n.lat !== null && n.lon !== null));

  /** Frame the nodes the first time the map is visible with some. */
  function fitOnce(m: L.Map) {
    if (fitted || m.getSize().x === 0 || !placed.length) return;
    fitted = true;
    const b = L.latLngBounds(placed.map((n) => [n.lat!, n.lon!] as L.LatLngTuple));
    if (placed.length === 1) m.setView(b.getCenter(), 12);
    else m.fitBounds(b.pad(0.2), { maxZoom: 13 });
  }

  onMount(() => {
    const m = L.map(el, { worldCopyJump: true }).setView([54, -2], 6);
    const osm = tileUrl.includes('openstreetmap.org');
    L.tileLayer(tileUrl, {
      maxZoom: 18,
      attribution: osm ? '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors' : '',
      referrerPolicy: 'strict-origin-when-cross-origin',
    } as L.TileLayerOptions).addTo(m);
    layer = L.layerGroup().addTo(m);
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

  // Markers updated in place, so a hover or click isn't interrupted by a poll.
  $effect(() => {
    if (!map) return;
    const seen = new Set<string>();
    for (const n of placed) {
      seen.add(n.key);
      const sel = n.key === selected;
      const name = nodeName(n);
      const look = `${sel}|${name}`;
      const icon = () =>
        L.divIcon({
          className: 'mesh-marker',
          html: `<span class="dot ${n.proto}${sel ? ' sel' : ''}"></span><span class="label">${esc(name)}</span>`,
          iconSize: [14, 14],
          iconAnchor: [7, 7],
        });
      const cur = markers.get(n.key);
      if (!cur) {
        const marker = L.marker([n.lat!, n.lon!], {
          icon: icon(),
          title: `${name} · ${n.proto === 'meshtastic' ? 'Meshtastic' : 'MeshCore'}`,
          zIndexOffset: sel ? 1000 : 0,
        })
          .on('click', () => onselect(n.key))
          .addTo(layer);
        markers.set(n.key, { marker, look });
        continue;
      }
      cur.marker.setLatLng([n.lat!, n.lon!]);
      if (cur.look !== look) {
        cur.marker.setIcon(icon());
        cur.marker.setZIndexOffset(sel ? 1000 : 0);
        cur.look = look;
      }
    }
    for (const [k, { marker }] of markers) {
      if (!seen.has(k)) {
        marker.remove();
        markers.delete(k);
      }
    }
    fitOnce(map);
    if (selected !== lastSelected) {
      lastSelected = selected;
      const s = placed.find((n) => n.key === selected);
      if (s) map.panTo([s.lat!, s.lon!]);
    }
  });
</script>

<div class="map" bind:this={el} role="region" aria-label="Map of mesh nodes"></div>

<style>
  .map {
    flex: 1;
    min-height: 420px;
    border-radius: 6px;
  }
  .map :global(.mesh-marker) {
    overflow: visible;
  }
  .map :global(.mesh-marker .dot) {
    display: block;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid #fff;
    box-shadow: 0 0 2px rgb(0 0 0 / 0.6);
    box-sizing: border-box;
  }
  .map :global(.mesh-marker .dot.meshtastic) {
    background: #2e9d5b;
  }
  .map :global(.mesh-marker .dot.meshcore) {
    background: #7a4fd0;
  }
  .map :global(.mesh-marker .dot.sel) {
    outline: 3px solid var(--accent);
  }
  .map :global(.mesh-marker .label) {
    position: absolute;
    left: 17px;
    top: -2px;
    white-space: nowrap;
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
    text-shadow:
      0 0 3px var(--surface),
      0 0 3px var(--surface),
      0 0 3px var(--surface);
  }
</style>
