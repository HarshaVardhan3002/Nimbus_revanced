<script lang="ts">
  // A 240 degree arc gauge with the figure in the middle.
  let {
    value,
    label,
    size = 96,
    tone = "var(--accent)",
  }: { value: number | null; label: string; size?: number; tone?: string } = $props();

  // An arc from 150 to 390 degrees around (50, 50), clockwise through the top.
  const ARC = "M 15.36 70 A 40 40 0 1 1 84.64 70";
  let v = $derived(value == null ? 0 : Math.max(0, Math.min(100, value)));
</script>

<div class="gauge" style:--size="{size}px" role="img" aria-label="{label}: {value == null ? 'not available' : `${Math.round(v)} percent`}">
  <svg viewBox="0 0 100 100" aria-hidden="true">
    <path class="bed" d={ARC} pathLength="100" />
    <path class="val" d={ARC} pathLength="100" style:stroke={tone} style:stroke-dasharray="{v} 100" />
  </svg>
  <span class="figure display">
    {#if value == null}–{:else}<span class="n">{Math.round(v)}<small>%</small></span>{/if}
  </span>
</div>

<style>
  .gauge {
    position: relative;
    width: var(--size);
    height: var(--size);
    flex: none;
  }
  svg {
    width: 100%;
    height: 100%;
    overflow: visible;
  }
  path {
    fill: none;
    stroke-width: 8;
    stroke-linecap: round;
  }
  .bed {
    stroke: var(--s3);
  }
  .val {
    transition:
      stroke-dasharray 600ms var(--out),
      stroke 0.8s var(--out);
  }
  .figure {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: calc(var(--size) * 0.27);
    line-height: 1;
    white-space: nowrap;
  }
  /* Optically centred: the percent sign hangs off the right, so nudge the pair left. */
  .n {
    display: inline-flex;
    align-items: baseline;
    margin-left: 0.18em;
  }
  small {
    font-size: 0.42em;
    font-weight: 600;
    color: var(--text-2);
    margin-left: 0.08em;
  }
</style>
