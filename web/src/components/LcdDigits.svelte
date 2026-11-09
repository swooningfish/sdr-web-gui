<script lang="ts">
  /** Seven-segment digits for the receiver's display. `text` holds digits,
   * spaces (blank cells), '-' and '.' (a decimal point between cells). Unlit
   * segments show faintly, like a real LCD. */
  let { text, size = 0 }: { text: string; /** px; 0 = set by the parent's CSS */ size?: number } = $props();

  // Segments in a 10 × 18 cell.
  const SEG: Record<string, string> = {
    a: 'M1.6 0.4 H8.4 L7.2 1.9 H2.8 Z',
    b: 'M8.9 0.9 V8.3 L8.1 9 L7.4 8.3 V2.3 Z',
    c: 'M8.9 9.7 V17.1 L7.4 15.7 V9.7 L8.1 9 Z',
    d: 'M1.6 17.6 H8.4 L7.2 16.1 H2.8 Z',
    e: 'M1.1 9.7 V17.1 L2.6 15.7 V9.7 L1.9 9 Z',
    f: 'M1.1 0.9 V8.3 L1.9 9 L2.6 8.3 V2.3 Z',
    g: 'M1.9 9 L2.7 8.25 H7.3 L8.1 9 L7.3 9.75 H2.7 Z',
  };
  const ALL = Object.keys(SEG);
  const ON: Record<string, string> = {
    '0': 'abcdef',
    '1': 'bc',
    '2': 'abged',
    '3': 'abgcd',
    '4': 'fgbc',
    '5': 'afgcd',
    '6': 'afgedc',
    '7': 'abc',
    '8': 'abcdefg',
    '9': 'abcdfg',
    '-': 'g',
    ' ': '',
  };
  const CELL = 12;
  const DOT = 3.5;

  const cells = $derived.by(() => {
    const out: { x: number; ch: string; dot: boolean }[] = [];
    let x = 0;
    for (const ch of text) {
      if (ch === '.') {
        out.push({ x, ch, dot: true });
        x += DOT;
      } else {
        out.push({ x, ch, dot: false });
        x += CELL;
      }
    }
    return { out, width: x };
  });
</script>

<svg
  class="lcd-digits"
  viewBox="-1 -0.5 {cells.width + 1} 19"
  style:height={size ? `${size}px` : null}
  aria-hidden="true"
>
  <g transform="skewX(-6) translate(1.5 0)">
    {#each cells.out as c, i (i)}
      {#if c.dot}
        <rect x={c.x - 0.3} y="16.2" width="1.7" height="1.7" class="on" />
      {:else}
        <g transform="translate({c.x} 0)">
          {#each ALL as s (s)}
            <path d={SEG[s]} class={ON[c.ch]?.includes(s) ? 'on' : 'off'} />
          {/each}
        </g>
      {/if}
    {/each}
  </g>
</svg>

<style>
  .lcd-digits {
    display: block;
    flex: none;
    width: auto;
    height: 1em;
    overflow: visible;
  }
  .on {
    fill: currentColor;
    filter: drop-shadow(0 0 0.6px currentColor);
  }
  .off {
    fill: currentColor;
    opacity: 0.07;
  }
</style>
