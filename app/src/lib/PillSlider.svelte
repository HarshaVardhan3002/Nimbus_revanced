<script lang="ts">
  // A 40 px pill: the fill is the value. Drag anywhere on it, or use the arrow keys.
  // The icon at the left end is its own button (volume uses it to mute).
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  let {
    label,
    value,
    icon,
    onset,
    onicon,
    iconLabel = "",
    muted = false,
    disabled = false,
    note = "",
  }: {
    label: string;
    /** 0..100, or null while unknown. */
    value: number | null;
    icon: IconName;
    onset: (v: number) => void;
    onicon?: () => void;
    iconLabel?: string;
    muted?: boolean;
    disabled?: boolean;
    /** Shown inside the pill instead of the value (why it is disabled). */
    note?: string;
  } = $props();

  let track: HTMLDivElement;
  let dragging = $state(false);
  /** The value the user is setting; wins over `value` while dragging and just after. */
  let local = $state<number | null>(null);
  let settle: ReturnType<typeof setTimeout> | undefined;
  let lastSent = 0;
  let pending: ReturnType<typeof setTimeout> | undefined;

  let shown = $derived(local ?? value ?? 0);
  let ratio = $derived(Math.max(0, Math.min(100, shown)) / 100);

  function send(v: number, now = false) {
    clearTimeout(pending);
    const wait = 60 - (performance.now() - lastSent);
    if (now || wait <= 0) {
      lastSent = performance.now();
      onset(v);
    } else {
      pending = setTimeout(() => send(v, true), wait);
    }
  }

  function set(v: number, now = false) {
    v = Math.round(Math.max(0, Math.min(100, v)));
    local = v;
    send(v, now);
    clearTimeout(settle);
    // Polling may still report the old level for a moment; hold ours a little longer.
    settle = setTimeout(() => !dragging && (local = null), 900);
  }

  function fromPointer(e: PointerEvent) {
    const r = track.getBoundingClientRect();
    // The first 40 px hold the icon: value 0 sits at their right edge.
    return ((e.clientX - r.left - 40) / Math.max(1, r.width - 52)) * 100;
  }

  function down(e: PointerEvent) {
    if (disabled || e.button !== 0) return;
    track.setPointerCapture(e.pointerId);
    dragging = true;
    set(fromPointer(e));
  }
  function move(e: PointerEvent) {
    if (dragging) set(fromPointer(e));
  }
  function up(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    set(fromPointer(e), true);
  }

  function key(e: KeyboardEvent) {
    if (disabled) return;
    const steps: Record<string, number> = { ArrowRight: 2, ArrowUp: 2, ArrowLeft: -2, ArrowDown: -2, PageUp: 10, PageDown: -10 };
    if (e.key in steps) set(shown + steps[e.key], true);
    else if (e.key === "Home") set(0, true);
    else if (e.key === "End") set(100, true);
    else return;
    e.preventDefault();
  }
</script>

<div class="pill" class:disabled class:muted class:dragging>
  <div
    class="track"
    bind:this={track}
    role="slider"
    tabindex={disabled ? -1 : 0}
    aria-label={label}
    aria-valuemin={0}
    aria-valuemax={100}
    aria-valuenow={value == null ? undefined : Math.round(shown)}
    aria-disabled={disabled}
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={up}
    onkeydown={key}
  >
    <span class="fill" style:--r={ratio}></span>
    {#if note}
      <span class="note ellipsis">{note}</span>
    {:else}
      <span class="value num" class:over={ratio > 0.88}>{value == null ? "–" : Math.round(shown)}</span>
    {/if}
  </div>
  <button
    class="end"
    tabindex={onicon ? 0 : -1}
    aria-label={iconLabel || label}
    aria-hidden={!onicon}
    disabled={disabled || !onicon}
    onclick={onicon}
  >
    <Icon name={icon} size={18} />
  </button>
</div>

<style>
  .pill {
    position: relative;
    height: 40px;
  }
  .track {
    position: absolute;
    inset: 0;
    border-radius: 20px;
    background: var(--s2);
    overflow: hidden;
    touch-action: none;
  }
  .track:focus-visible {
    outline-offset: 3px;
  }
  .fill {
    position: absolute;
    inset: 0 auto 0 0;
    /* At 0 the fill is just the icon's circle; at 100 it is the whole pill. */
    width: calc(40px + (100% - 40px) * var(--r));
    border-radius: 20px;
    background: var(--accent);
    transition:
      width 140ms var(--out),
      background-color var(--t-fade) var(--out);
  }
  .dragging .fill {
    transition: background-color var(--t-fade) var(--out);
  }
  .muted .fill {
    background: var(--s4);
  }
  .value {
    position: absolute;
    right: 15px;
    top: 50%;
    translate: 0 -50%;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
    transition: color var(--t-fade) var(--out);
  }
  .value.over {
    color: var(--ink);
  }
  .muted .value.over {
    color: var(--text);
  }
  .note {
    position: absolute;
    left: 52px;
    right: 14px;
    top: 50%;
    translate: 0 -50%;
    font-size: 12px;
    color: var(--text-2);
  }
  .end {
    position: absolute;
    left: 0;
    top: 0;
    width: 40px;
    height: 40px;
    border-radius: 20px;
    display: grid;
    place-items: center;
    color: var(--ink);
    transition:
      color var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }
  .end[aria-hidden="true"] {
    pointer-events: none;
  }
  .end:active:not(:disabled) {
    scale: 0.9;
  }
  .muted .end,
  .disabled .end {
    color: var(--text);
  }
  .disabled .track {
    background: var(--s1);
  }
  .disabled .fill {
    background: var(--s2);
  }
  .disabled .end {
    color: var(--text-3);
    opacity: 1;
  }
</style>
