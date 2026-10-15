<script lang="ts">
  // The last readings as a soft area line, oldest on the left. Until there are `slots`
  // readings the line grows from the left edge; after that it scrolls. Values are 0..100.
  let { values, slots = 48, tone = "var(--accent)" }: { values: number[]; slots?: number; tone?: string } = $props();

  const W = 200;
  const H = 40;
  const id = `spark-${Math.random().toString(36).slice(2, 8)}`;

  let points = $derived.by(() => {
    const step = W / (slots - 1);
    return values.slice(-slots).map((v, i) => [i * step, H - 2 - (Math.max(0, Math.min(100, v)) / 100) * (H - 4)] as const);
  });
  let line = $derived(points.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`).join(" "));
  let area = $derived(points.length > 1 ? `${line} L${points.at(-1)![0].toFixed(1)} ${H} L0 ${H} Z` : "");
</script>

<svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" aria-hidden="true" style:--tone={tone}>
  <line class="base" x1="0" y1={H - 0.5} x2={W} y2={H - 0.5} />
  <defs>
    <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color={tone} stop-opacity="0.24" />
      <stop offset="1" stop-color={tone} stop-opacity="0" />
    </linearGradient>
  </defs>
  {#if points.length > 1}
    <path class="area" d={area} fill="url(#{id})" />
    <path class="line" d={line} />
  {/if}
</svg>

<style>
  svg {
    display: block;
    width: 100%;
    height: 100%;
  }
  .base {
    stroke: var(--line);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }
  .line {
    fill: none;
    stroke: var(--tone);
    stroke-width: 1.5;
    vector-effect: non-scaling-stroke;
    stroke-linejoin: round;
  }
</style>
