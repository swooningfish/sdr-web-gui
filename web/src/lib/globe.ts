/** 3D globe for the satellite map: each satellite at its height, with a cone
 * down to the edge of the area that can hear it. Loaded on demand (three.js
 * is large), so only import this dynamically. */

import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { RE_KM, footprintDeg, paintLand, ringAround, type LonLat, type MapColours, type MapScene, type SatPos } from './satmap';

const DEG = Math.PI / 180;
const SEG = 48;
/** Height of lines drawn on the ground, so they don't z-fight the sphere. */
const SKIN = 1.004;

/** Geographic position → world space, matching SphereGeometry's texture. */
function vec(lat: number, lon: number, r: number, out = new THREE.Vector3()): THREE.Vector3 {
  const phi = (lon + 180) * DEG;
  const theta = (90 - lat) * DEG;
  return out.set(-r * Math.cos(phi) * Math.sin(theta), r * Math.cos(theta), r * Math.sin(phi) * Math.sin(theta));
}

export interface Globe {
  update(scene: MapScene, col: MapColours, altScale: number): void;
  /** Turn the globe to look down on this point. */
  aim(lat: number, lon: number): void;
  resize(): void;
  destroy(): void;
}

interface SatObj {
  dot: THREE.Mesh;
  cone: THREE.Mesh;
  ring: THREE.Line;
  label: HTMLSpanElement;
}

export function createGlobe(container: HTMLElement, land: LonLat[][]): Globe {
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true }); // throws without WebGL
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  container.appendChild(renderer.domElement);
  const labels = document.createElement('div');
  labels.className = 'globe-labels';
  container.appendChild(labels);

  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(35, 1, 0.01, 100);
  camera.position.set(0, 0, 3.4);
  const controls = new OrbitControls(camera, renderer.domElement);
  Object.assign(controls, {
    enableDamping: true,
    dampingFactor: 0.08,
    enablePan: false,
    minDistance: 1.3,
    maxDistance: 9,
    rotateSpeed: 0.5,
    zoomSpeed: 0.7,
  });

  const texCanvas = document.createElement('canvas');
  texCanvas.width = 2048;
  texCanvas.height = 1024;
  const texture = new THREE.CanvasTexture(texCanvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.anisotropy = renderer.capabilities.getMaxAnisotropy();
  scene.add(new THREE.Mesh(new THREE.SphereGeometry(1, 96, 48), new THREE.MeshLambertMaterial({ map: texture })));
  // The sun lights the day side; the ambient light keeps the night side readable.
  const sun = new THREE.DirectionalLight(0xffffff, 2);
  scene.add(sun, new THREE.AmbientLight(0xffffff, 1.8));

  const line = (dashed = false) =>
    new THREE.Line(
      new THREE.BufferGeometry(),
      dashed
        ? new THREE.LineDashedMaterial({ dashSize: 0.02, gapSize: 0.015, transparent: true })
        : new THREE.LineBasicMaterial({ vertexColors: true, transparent: true }),
    );
  const track = line();
  const passTrack = line();
  const home = new THREE.Mesh(new THREE.SphereGeometry(0.011, 12, 8), new THREE.MeshBasicMaterial());
  const homeRing = line(true);
  const peak = new THREE.Mesh(new THREE.SphereGeometry(0.012, 12, 8), new THREE.MeshBasicMaterial({ wireframe: true }));
  scene.add(track, passTrack, home, homeRing, peak);
  const homeLabel = document.createElement('span');
  const peakLabel = document.createElement('span');
  homeLabel.textContent = 'Station';
  labels.append(homeLabel, peakLabel);

  const sats = new Map<string, SatObj>();
  let painted = '';
  let raf = 0;
  let alive = true;

  function satObj(name: string): SatObj {
    let o = sats.get(name);
    if (!o) {
      const coneGeo = new THREE.BufferGeometry();
      coneGeo.setAttribute('position', new THREE.BufferAttribute(new Float32Array((SEG + 2) * 3), 3));
      coneGeo.setIndex(Array.from({ length: SEG }, (_, i) => [0, i + 1, i + 2]).flat());
      const ringGeo = new THREE.BufferGeometry();
      ringGeo.setAttribute('position', new THREE.BufferAttribute(new Float32Array((SEG + 1) * 3), 3));
      o = {
        dot: new THREE.Mesh(new THREE.SphereGeometry(0.016, 16, 12), new THREE.MeshBasicMaterial()),
        cone: new THREE.Mesh(
          coneGeo,
          new THREE.MeshBasicMaterial({ transparent: true, opacity: 0.16, side: THREE.DoubleSide, depthWrite: false }),
        ),
        ring: new THREE.Line(ringGeo, new THREE.LineBasicMaterial({ transparent: true, opacity: 0.9 })),
        label: document.createElement('span'),
      };
      o.label.textContent = name;
      labels.appendChild(o.label);
      scene.add(o.dot, o.cone, o.ring);
      sats.set(name, o);
    }
    return o;
  }

  const height = (p: SatPos, altScale: number) => 1 + (p.altKm / RE_KM) * altScale;

  function setTrack(l: THREE.Line, pts: MapScene['track'], col: MapColours, altScale: number, dim: number) {
    const pos = new Float32Array(pts.length * 3);
    const rgb = new Float32Array(pts.length * 3);
    const far = new THREE.Color(col.track).multiplyScalar(dim);
    const near = new THREE.Color(col.good);
    const v = new THREE.Vector3();
    pts.forEach((p, i) => {
      vec(p.pos.lat, p.pos.lon, height(p.pos, altScale), v).toArray(pos, i * 3);
      (p.inRange ? near : far).toArray(rgb, i * 3);
    });
    l.geometry.dispose();
    l.geometry = new THREE.BufferGeometry();
    l.geometry.setAttribute('position', new THREE.BufferAttribute(pos, 3));
    l.geometry.setAttribute('color', new THREE.BufferAttribute(rgb, 3));
    l.visible = pts.length > 1;
  }

  /** Things to label: where they are, and the element to move there. */
  let anchors: { at: THREE.Vector3; el: HTMLSpanElement; show: boolean }[] = [];

  function update(s: MapScene, col: MapColours, altScale: number) {
    const key = col.ocean + col.land;
    if (key !== painted) {
      painted = key;
      paintLand(texCanvas.getContext('2d')!, texCanvas.width, texCanvas.height, land, col);
      texture.needsUpdate = true;
    }
    vec(s.sun.lat, s.sun.lon, 10, sun.position);
    anchors = [];

    const seen = new Set<string>();
    const v = new THREE.Vector3();
    for (const sat of s.sats) {
      seen.add(sat.name);
      const o = satObj(sat.name);
      const colour = new THREE.Color(sat.inRange ? col.good : sat.focus ? col.track : col.sat);
      const r = height(sat.pos, altScale);
      vec(sat.pos.lat, sat.pos.lon, r, o.dot.position);
      (o.dot.material as THREE.MeshBasicMaterial).color = colour;
      (o.cone.material as THREE.MeshBasicMaterial).color = colour;
      (o.ring.material as THREE.LineBasicMaterial).color = colour;
      const cone = o.cone.geometry.getAttribute('position') as THREE.BufferAttribute;
      const ring = o.ring.geometry.getAttribute('position') as THREE.BufferAttribute;
      cone.setXYZ(0, o.dot.position.x, o.dot.position.y, o.dot.position.z);
      ringAround(sat.pos.lat, sat.pos.lon, footprintDeg(sat.pos.altKm), SEG).forEach(([lat, lon], i) => {
        vec(lat, lon, SKIN, v);
        cone.setXYZ(i + 1, v.x, v.y, v.z);
        ring.setXYZ(i, v.x, v.y, v.z);
      });
      cone.needsUpdate = ring.needsUpdate = true;
      o.cone.geometry.computeBoundingSphere();
      o.ring.geometry.computeBoundingSphere();
      o.label.className = sat.inRange ? 'good' : sat.focus ? 'focus' : '';
      anchors.push({ at: o.dot.position, el: o.label, show: true });
    }
    for (const [name, o] of sats) {
      if (seen.has(name)) continue;
      scene.remove(o.dot, o.cone, o.ring);
      o.label.remove();
      sats.delete(name);
    }

    setTrack(track, s.track, col, altScale, 1);
    setTrack(passTrack, s.passTrack, col, altScale, 0.6);

    home.visible = homeRing.visible = !!s.observer;
    if (s.observer) {
      vec(s.observer.lat_deg, s.observer.lon_deg, SKIN, home.position);
      (home.material as THREE.MeshBasicMaterial).color.set(col.text);
      const focus = s.sats.find((x) => x.focus) ?? s.sats[0];
      const pts = ringAround(s.observer.lat_deg, s.observer.lon_deg, footprintDeg(focus?.pos.altKm ?? 420), SEG).map(
        ([lat, lon]) => vec(lat, lon, SKIN, new THREE.Vector3()),
      );
      homeRing.geometry.dispose();
      homeRing.geometry = new THREE.BufferGeometry().setFromPoints(pts);
      homeRing.computeLineDistances();
      (homeRing.material as THREE.LineDashedMaterial).color.set(col.muted);
    }
    anchors.push({ at: home.position, el: homeLabel, show: !!s.observer });

    peak.visible = !!s.peak;
    if (s.peak) {
      vec(s.peak.pos.lat, s.peak.pos.lon, height(s.peak.pos, altScale), peak.position);
      (peak.material as THREE.MeshBasicMaterial).color.set(col.good);
      peakLabel.textContent = s.peak.label;
      peakLabel.className = 'good';
    }
    anchors.push({ at: peak.position, el: peakLabel, show: !!s.peak });
  }

  const p = new THREE.Vector3();
  const d = new THREE.Vector3();
  /** Is the Earth between the camera and this point? */
  function hidden(at: THREE.Vector3): boolean {
    d.copy(at).sub(camera.position);
    const t = -camera.position.dot(d) / d.lengthSq();
    return t > 0 && t < 1 && p.copy(camera.position).addScaledVector(d, t).length() < 0.995;
  }

  function placeLabels() {
    const w = container.clientWidth;
    const h = container.clientHeight;
    for (const a of anchors) {
      const show = a.show && !hidden(a.at);
      a.el.hidden = !show;
      if (!show) continue;
      p.copy(a.at).project(camera);
      a.el.style.transform = `translate(${((p.x + 1) / 2) * w + 10}px, ${((1 - p.y) / 2) * h - 9}px)`;
    }
  }

  function frame() {
    if (!alive) return;
    raf = requestAnimationFrame(frame);
    controls.update();
    renderer.render(scene, camera);
    placeLabels();
  }

  function resize() {
    const w = container.clientWidth;
    const h = container.clientHeight;
    if (!w || !h) return;
    renderer.setSize(w, h);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  }

  resize();
  frame();

  return {
    update,
    resize,
    aim(lat, lon) {
      vec(lat, lon, camera.position.length(), camera.position);
      controls.update();
    },
    destroy() {
      alive = false;
      cancelAnimationFrame(raf);
      controls.dispose();
      scene.traverse((o) => {
        const m = o as THREE.Mesh;
        m.geometry?.dispose();
        (m.material as THREE.Material | undefined)?.dispose();
      });
      texture.dispose();
      renderer.dispose();
      renderer.domElement.remove();
      labels.remove();
    },
  };
}
