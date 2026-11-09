<script lang="ts">
  /** A small stepped knob, like a radio's RF gain or selector knob: turn it
   * (drag up/right, scroll, or arrow keys) through `options`; the choice is
   * applied once the knob has settled. A `pushable` knob also pushes in: a
   * click (or Enter/Space) toggles `pushed`, and while pushed in it doesn't
   * turn. */
  let {
    label,
    options,
    index,
    onchange,
    pushable = false,
    pushed = false,
    onpush,
    pushedText = 'AUTO',
    disabled = false,
    size = 58,
    title = '',
  }: {
    label: string;
    options: string[];
    /** The applied choice (−1 when it isn't one of the options). */
    index: number;
    onchange: (i: number) => void;
    pushable?: boolean;
    pushed?: boolean;
    onpush?: () => void;
    pushedText?: string;
    disabled?: boolean;
    size?: number;
    title?: string;
  } = $props();

  const SETTLE_MS = 450;
  const PX_PER_STEP = 14;

  let pending = $state<number | null>(null);
  let timer: ReturnType<typeof setTimeout> | null = null;
  const n = $derived(options.length);
  const shown = $derived(pending ?? index);
  const angle = $derived(n > 1 && shown >= 0 ? -135 + (270 * shown) / (n - 1) : -135);
  const text = $derived(pushed ? pushedText : shown >= 0 ? options[shown] : '—');
  const turnable = $derived(!disabled && !pushed && n > 1);
  const ticks = $derived.by(() => {
    if (n < 2) return [] as number[];
    const every = Math.ceil(n / 16);
    return Array.from({ length: n }, (_, i) => i).filter((i) => i % every === 0 || i === n - 1);
  });

  function set(i: number) {
    const v = Math.max(0, Math.min(n - 1, i));
    if (v === shown) return;
    pending = v;
    if (timer) clearTimeout(timer);
    timer = setTimeout(commit, SETTLE_MS);
  }
  function commit() {
    timer = null;
    const v = pending;
    if (v !== null && v !== index) onchange(v);
    // Keep showing the choice until the radio reports it (or a moment passes).
    setTimeout(() => {
      if (!timer) pending = null;
    }, 1500);
  }
  // The radio caught up: drop the local choice.
  $effect(() => {
    if (pending !== null && !timer && pending === index) pending = null;
  });

  let el: HTMLDivElement;
  let drag: { x: number; y: number; from: number; moved: boolean } | null = null;
  function down(e: PointerEvent) {
    if (disabled || e.button !== 0) return;
    el.setPointerCapture(e.pointerId);
    drag = { x: e.clientX, y: e.clientY, from: shown < 0 ? 0 : shown, moved: false };
    e.preventDefault();
    el.focus();
  }
  function move(e: PointerEvent) {
    if (!drag) return;
    const d = drag.y - e.clientY + (e.clientX - drag.x);
    if (Math.abs(d) > 4) drag.moved = true;
    if (drag.moved && turnable) set(drag.from + Math.round(d / PX_PER_STEP));
  }
  function up() {
    if (drag && !drag.moved && pushable && !disabled) onpush?.();
    drag = null;
  }

  let wheelAcc = 0;
  function wheel(e: WheelEvent) {
    if (!turnable) return;
    e.preventDefault();
    wheelAcc += -e.deltaY;
    const k = Math.trunc(wheelAcc / 50);
    if (k) {
      wheelAcc -= k * 50;
      set((shown < 0 ? 0 : shown) + k);
    }
  }
  $effect(() => {
    el.addEventListener('wheel', wheel, { passive: false });
    return () => el.removeEventListener('wheel', wheel);
  });

  function key(e: KeyboardEvent) {
    if (disabled) return;
    if (pushable && (e.key === 'Enter' || e.key === ' ')) {
      e.preventDefault();
      onpush?.();
      return;
    }
    if (!turnable) return;
    const cur = shown < 0 ? 0 : shown;
    const k: Record<string, number> = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1, PageUp: 3, PageDown: -3 };
    if (e.key === 'Home') set(0);
    else if (e.key === 'End') set(n - 1);
    else if (k[e.key]) set(cur + k[e.key]);
    else return;
    e.preventDefault();
  }

  function tickAt(i: number, r: number): [number, number] {
    const a = ((-135 + (270 * i) / (n - 1) - 90) * Math.PI) / 180;
    return [50 + r * Math.cos(a), 50 + r * Math.sin(a)];
  }
</script>

<div class="sel" class:disabled style:--size="{size}px">
  <span class="lbl">{label}</span>
  <div
    class="knob"
    class:pushed
    bind:this={el}
    role="slider"
    tabindex={disabled ? -1 : 0}
    aria-label={pushable ? `${label} (press Enter for ${pushedText.toLowerCase()})` : label}
    aria-valuemin={0}
    aria-valuemax={Math.max(0, n - 1)}
    aria-valuenow={shown < 0 ? undefined : shown}
    aria-valuetext={text}
    aria-disabled={disabled}
    title={title || (pushable ? 'Turn to set; click to push in for automatic' : 'Turn to set')}
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={() => (drag = null)}
    onkeydown={key}
  >
    <svg class="scale" viewBox="0 0 100 100" aria-hidden="true">
      {#each ticks as i (i)}
        {@const [x1, y1] = tickAt(i, 44)}
        {@const [x2, y2] = tickAt(i, 49)}
        <line {x1} {y1} {x2} {y2} class:on={!pushed && i <= shown} />
      {/each}
    </svg>
    <div class="cap" style:transform="rotate({angle}deg)"><span class="mark"></span></div>
  </div>
  <span class="value" class:auto={pushed} class:pending={pending !== null}>{text}</span>
</div>

<style>
  .sel {
    display: grid;
    justify-items: center;
    gap: 3px;
    min-width: calc(var(--size) + 16px);
  }
  .sel.disabled {
    opacity: 0.55;
  }
  .lbl {
    font: 600 10px/1 ui-sans-serif, system-ui, sans-serif;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--face-label);
  }
  .knob {
    position: relative;
    width: var(--size);
    height: var(--size);
    border-radius: 50%;
    cursor: ns-resize;
    touch-action: none;
    user-select: none;
  }
  .disabled .knob {
    cursor: not-allowed;
  }
  .knob:focus-visible {
    outline-offset: 3px;
  }
  .scale {
    position: absolute;
    inset: -7px;
    width: calc(100% + 14px);
    height: calc(100% + 14px);
    overflow: visible;
  }
  .scale line {
    stroke: var(--face-label);
    stroke-width: 2;
    opacity: 0.45;
  }
  .scale line.on {
    stroke: var(--led);
    opacity: 1;
  }
  .cap {
    position: absolute;
    inset: 4px;
    border-radius: 50%;
    background:
      radial-gradient(circle at 50% 50%, transparent 0 55%, rgba(0, 0, 0, 0.25) 56% 58%, transparent 59%),
      conic-gradient(#1c1e22, #43474f, #1c1e22, #34373e, #1c1e22, #43474f, #1c1e22);
    box-shadow:
      0 3px 6px rgba(0, 0, 0, 0.45),
      inset 0 1px 2px rgba(255, 255, 255, 0.15),
      0 0 0 2px #0c0d10;
    transition:
      transform 0.08s ease,
      inset 0.08s ease,
      box-shadow 0.08s ease;
  }
  /* Pushed in: sits lower in the panel, with a lit ring. */
  .pushed .cap {
    inset: 7px;
    box-shadow:
      inset 0 2px 5px rgba(0, 0, 0, 0.6),
      0 0 0 2px #0c0d10,
      0 0 0 4px var(--led),
      0 0 10px var(--led);
  }
  .mark {
    position: absolute;
    left: 50%;
    top: 7%;
    width: 3px;
    height: 30%;
    margin-left: -1.5px;
    border-radius: 2px;
    background: #e5e7eb;
  }
  .value {
    font: 600 11px/1.2 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color: var(--key-text);
    white-space: nowrap;
  }
  .value.auto {
    color: var(--led);
  }
  .value.pending {
    color: var(--accent);
  }
</style>
