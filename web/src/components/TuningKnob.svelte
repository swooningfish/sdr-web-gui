<script lang="ts">
  /** The main tuning knob: drag it round, scroll over it, or focus it and use
   * the arrow keys (Page Up/Down for ten steps). Each detent calls `onstep`. */
  let {
    onstep,
    label,
    value,
    valuetext,
    disabled = false,
    size = 132,
  }: {
    onstep: (n: number) => void;
    label: string;
    /** The frequency it tunes (Hz), for assistive tech. */
    value: number;
    valuetext: string;
    disabled?: boolean;
    size?: number;
  } = $props();

  /** Degrees of turn per step. */
  const DETENT = 9;
  let angle = $state(0);
  let el: HTMLDivElement;
  let drag: { last: number; acc: number } | null = null;

  function pointerAngle(e: PointerEvent): number {
    const r = el.getBoundingClientRect();
    return (Math.atan2(e.clientY - (r.top + r.height / 2), e.clientX - (r.left + r.width / 2)) * 180) / Math.PI;
  }

  function turn(deg: number) {
    angle = (angle + deg) % 360;
  }

  function down(e: PointerEvent) {
    if (disabled || e.button !== 0) return;
    el.setPointerCapture(e.pointerId);
    drag = { last: pointerAngle(e), acc: 0 };
    e.preventDefault();
  }
  function move(e: PointerEvent) {
    if (!drag) return;
    const a = pointerAngle(e);
    let d = a - drag.last;
    if (d > 180) d -= 360;
    if (d < -180) d += 360;
    drag.last = a;
    turn(d);
    drag.acc += d;
    const n = Math.trunc(drag.acc / DETENT);
    if (n) {
      drag.acc -= n * DETENT;
      onstep(n);
    }
  }
  function up() {
    drag = null;
  }

  let wheelAcc = 0;
  function wheel(e: WheelEvent) {
    if (disabled) return;
    e.preventDefault();
    // Mice send ±100 a notch; trackpads send many small deltas.
    wheelAcc += -e.deltaY;
    const n = Math.trunc(wheelAcc / 40);
    if (n) {
      wheelAcc -= n * 40;
      turn(n * DETENT);
      onstep(n);
    }
  }

  function key(e: KeyboardEvent) {
    if (disabled) return;
    const k: Record<string, number> = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1, PageUp: 10, PageDown: -10 };
    const n = k[e.key];
    if (!n) return;
    e.preventDefault();
    turn(n * DETENT);
    onstep(n);
  }

  // Wheel listeners must be non-passive to stop the page scrolling.
  $effect(() => {
    el.addEventListener('wheel', wheel, { passive: false });
    return () => el.removeEventListener('wheel', wheel);
  });
</script>

<div
  class="knob"
  class:disabled
  bind:this={el}
  style:width="{size}px"
  style:height="{size}px"
  role="slider"
  tabindex={disabled ? -1 : 0}
  aria-label={label}
  aria-valuenow={value}
  aria-valuetext={valuetext}
  aria-disabled={disabled}
  title="Drag round, scroll, or use the arrow keys to tune"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  onkeydown={key}
>
  <div class="skirt"></div>
  <div class="cap" style:transform="rotate({angle}deg)">
    <span class="dimple"></span>
  </div>
</div>

<style>
  .knob {
    position: relative;
    flex: none;
    border-radius: 50%;
    cursor: grab;
    touch-action: none;
    user-select: none;
  }
  .knob:active {
    cursor: grabbing;
  }
  .knob.disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .knob:focus-visible {
    outline-offset: 4px;
  }
  /* Knurled rim. */
  .skirt {
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background:
      repeating-conic-gradient(from 0deg, #2a2d33 0deg 3deg, #3b3f47 3deg 6deg),
      #30333a;
    box-shadow:
      0 6px 14px rgba(0, 0, 0, 0.45),
      inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }
  /* Spun-metal cap. */
  .cap {
    position: absolute;
    inset: 9%;
    border-radius: 50%;
    background:
      radial-gradient(circle at 50% 50%, transparent 0 58%, rgba(0, 0, 0, 0.25) 59% 60%, transparent 61%),
      conic-gradient(#1c1e22, #3c4048, #1c1e22, #2f3238, #1c1e22, #3c4048, #1c1e22);
    box-shadow:
      inset 0 2px 3px rgba(255, 255, 255, 0.12),
      inset 0 -3px 6px rgba(0, 0, 0, 0.5);
  }
  .dimple {
    position: absolute;
    left: 50%;
    top: 14%;
    width: 18%;
    height: 18%;
    transform: translateX(-50%);
    border-radius: 50%;
    background: radial-gradient(circle at 40% 35%, #15171a, #2b2e34 70%);
    box-shadow: inset 0 2px 3px rgba(0, 0, 0, 0.7), 0 1px 0 rgba(255, 255, 255, 0.08);
  }
</style>
