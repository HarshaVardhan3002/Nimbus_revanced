<script lang="ts">
  // Over the main screen while the agent acts: a light that leaves the notch and runs
  // along the screen's edges both ways, a chip above the taskbar saying what is being
  // done, and the agent's own cursor, which glides out of the island to each spot.
  // Nothing here takes a click; Escape is caught by the app, not by this page.
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import Kbd from "../lib/Kbd.svelte";
  import { rise } from "../lib/motion";
  import { startPalette } from "../lib/palette.svelte";

  type Show =
    | { kind: "on"; label: string; bottom: number }
    | { kind: "off" }
    | { kind: "cursor"; x: number; y: number; click: boolean; label: string | null };

  /** How long the cursor takes to reach a spot (matches the CSS). */
  const GLIDE = 640;
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

  let on = $state(false);
  let label = $state("");
  let bottom = $state(0);
  let cursor = $state({ x: 0, y: 0, tag: null as string | null, seen: false });
  let glide = $state(false);
  let pressing = $state(false);
  let ripples = $state<{ id: number; x: number; y: number }[]>([]);
  let rid = 0;
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  let goneTimer: ReturnType<typeof setTimeout> | undefined;

  function press(x: number, y: number) {
    pressing = true;
    const id = ++rid;
    ripples = [...ripples, { id, x, y }];
    setTimeout(() => (pressing = false), 150);
    setTimeout(() => (ripples = ripples.filter((r) => r.id !== id)), 900);
  }

  function move(x: number, y: number, click: boolean, tag: string | null) {
    clearTimeout(pressTimer);
    if (!cursor.seen && !reduced) {
      // The first time, the cursor comes out of the island at the top.
      glide = false;
      cursor = { x: innerWidth / 2, y: 20, tag: null, seen: true };
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          glide = true;
          cursor = { x, y, tag, seen: true };
        }),
      );
    } else {
      glide = !reduced;
      cursor = { x, y, tag, seen: true };
    }
    if (click) pressTimer = setTimeout(() => press(x, y), reduced ? 0 : GLIDE + 40);
  }

  onMount(() => {
    const stopPalette = startPalette();
    const off = listen<Show>("overlay", ({ payload: s }) => {
      const dpr = devicePixelRatio || 1;
      if (s.kind === "on") {
        clearTimeout(goneTimer);
        on = true;
        label = s.label;
        bottom = s.bottom / dpr;
      } else if (s.kind === "off") {
        on = false;
        goneTimer = setTimeout(() => (cursor = { ...cursor, seen: false, tag: null }), 500);
      } else {
        move(s.x / dpr, s.y / dpr, s.click, s.label);
      }
    });
    return () => {
      stopPalette();
      off.then((f) => f());
    };
  });
</script>

<div class="stage" class:on aria-hidden={!on}>
  <div class="glow"></div>
  {#if on && !reduced}
    <!-- Remade each time, so the light always sets out from the notch. -->
    <div class="sweep"></div>
  {/if}

  {#each ripples as r (r.id)}
    <span class="ripple" style:translate="{r.x}px {r.y}px"></span>
  {/each}

  {#if cursor.seen}
    <div class="cursor" class:glide class:pressing style:translate="{cursor.x}px {cursor.y}px">
      <svg class="pointer" viewBox="0 0 26 26" width="26" height="26" aria-hidden="true">
        <defs>
          <linearGradient id="fill" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stop-color="var(--accent)" />
            <stop offset="1" stop-color="var(--chest)" />
          </linearGradient>
        </defs>
        <path
          d="M3.2 2.6 L22.4 11.6 Q23.6 12.2 22.3 12.7 L14.3 15.2 Q13.6 15.4 13.4 16.1 L10.9 24.1 Q10.4 25.4 9.8 24.2 L3 3.4 Q2.8 2.4 3.2 2.6 Z"
          fill="url(#fill)"
          stroke="rgb(255 255 255 / 0.95)"
          stroke-width="1.6"
          stroke-linejoin="round"
        />
      </svg>
      {#if cursor.tag}
        {#key cursor.tag}<span class="tag" in:rise={{ y: 4, duration: 220 }}>{cursor.tag}</span>{/key}
      {/if}
    </div>
  {/if}

  <div class="chip" style:bottom="{bottom + 24}px" role="status" aria-live="polite">
    <span class="core"></span>
    {#key label}<span class="label" in:rise={{ y: 6, duration: 240 }}>{label}</span>{/key}
    <span class="sep"></span>
    <span class="stop"><Kbd k="Esc" /> to stop</span>
  </div>
</div>

<style>
  .stage {
    position: fixed;
    inset: 0;
    pointer-events: none;
    opacity: 0;
    transition: opacity 360ms var(--out);
  }
  .stage.on {
    opacity: 1;
    transition-duration: 480ms;
  }

  /* A soft light pooled along every edge, breathing slowly. */
  .glow {
    position: fixed;
    inset: 0;
    box-shadow:
      inset 0 0 0 1px color-mix(in oklab, var(--accent) 55%, transparent),
      inset 0 0 26px 2px color-mix(in oklab, var(--accent) 42%, transparent),
      inset 0 0 110px 10px color-mix(in oklab, var(--accent) 14%, transparent);
    animation: breathe 3.2s ease-in-out infinite alternate;
  }
  @keyframes breathe {
    from {
      opacity: 0.7;
    }
    to {
      opacity: 1;
    }
  }

  /* Two bright arcs leave the notch, run down either side and meet at the foot, then
     come back up: two cones turning opposite ways, seen only through a soft frame.
     Turning the cones' angle repaints one screen-sized layer, nothing larger. */
  @property --turn {
    syntax: "<angle>";
    inherits: false;
    initial-value: 0deg;
  }
  .sweep {
    position: fixed;
    inset: 0;
    background:
      conic-gradient(
        from var(--turn),
        transparent 0deg 310deg,
        color-mix(in oklab, var(--chest) 45%, transparent) 336deg,
        var(--accent) 350deg,
        color-mix(in oklab, var(--accent) 72%, white) 355deg,
        transparent 360deg
      ),
      conic-gradient(
        from calc(-1 * var(--turn)),
        transparent 0deg,
        color-mix(in oklab, var(--accent) 72%, white) 5deg,
        var(--accent) 10deg,
        color-mix(in oklab, var(--chest) 45%, transparent) 24deg,
        transparent 50deg 360deg
      );
    mask:
      linear-gradient(90deg, #000, rgb(0 0 0 / 0.3) 7px, transparent 16px calc(100% - 16px), rgb(0 0 0 / 0.3) calc(100% - 7px), #000),
      linear-gradient(180deg, #000, rgb(0 0 0 / 0.3) 7px, transparent 16px calc(100% - 16px), rgb(0 0 0 / 0.3) calc(100% - 7px), #000);
    mask-composite: add;
    animation:
      arrive 700ms var(--out) both,
      turn 5.6s linear infinite;
  }
  @keyframes arrive {
    from {
      opacity: 0;
    }
  }
  @keyframes turn {
    to {
      --turn: 360deg;
    }
  }

  /* What is being done, above the taskbar, in the island's own black. */
  .chip {
    position: fixed;
    left: 50%;
    display: flex;
    align-items: center;
    gap: 10px;
    height: 40px;
    max-width: min(640px, calc(100vw - 32px));
    padding: 0 8px 0 14px;
    border-radius: 999px;
    background: var(--bezel);
    box-shadow:
      0 0 0 1px rgb(255 255 255 / 0.07),
      0 12px 32px rgb(0 0 0 / 0.5),
      0 0 44px var(--accent-20);
    white-space: nowrap;
    translate: -50% 14px;
    scale: 0.96;
    opacity: 0;
    transition:
      translate var(--t-move) var(--spring),
      scale var(--t-move) var(--spring),
      opacity 220ms var(--out);
  }
  .on .chip {
    translate: -50% 0;
    scale: 1;
    opacity: 1;
  }
  .core {
    width: 10px;
    height: 10px;
    flex: none;
    border-radius: 50%;
    background: radial-gradient(circle at 35% 35%, #fff, var(--accent) 55%, var(--chest));
    box-shadow: 0 0 10px var(--accent);
    animation: pulse 1.6s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      box-shadow: 0 0 18px 2px var(--accent);
      scale: 0.86;
    }
  }
  .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 560;
  }
  .sep {
    width: 1px;
    height: 16px;
    flex: none;
    background: var(--line);
  }
  .stop {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
    padding-right: 6px;
    font-size: 12px;
    color: var(--text-3);
  }

  /* The agent's cursor: its own shape and colours, so it is never mistaken for yours. */
  .cursor {
    position: fixed;
    left: 0;
    top: 0;
  }
  .cursor.glide {
    transition: translate 640ms var(--spring);
  }
  .pointer {
    position: absolute;
    left: -3px;
    top: -2.6px;
    overflow: visible;
    filter: drop-shadow(0 2px 5px rgb(0 0 0 / 0.45)) drop-shadow(0 0 10px var(--accent-35));
    transform-origin: 3px 2.6px;
    transition: scale 140ms var(--out);
  }
  .pressing .pointer {
    scale: 0.84;
  }
  .tag {
    position: absolute;
    left: 20px;
    top: 24px;
    padding: 4px 10px;
    border-radius: 999px;
    background: var(--accent);
    color: var(--ink);
    font: 600 12px/1.25 var(--ui);
    white-space: nowrap;
    box-shadow: 0 6px 16px rgb(0 0 0 / 0.35);
  }

  .ripple {
    position: fixed;
    left: 0;
    top: 0;
  }
  .ripple::before,
  .ripple::after {
    content: "";
    position: absolute;
    left: -22px;
    top: -22px;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    border: 2px solid var(--accent);
    animation: ring 700ms var(--out) both;
  }
  .ripple::after {
    animation-delay: 110ms;
  }
  @keyframes ring {
    from {
      scale: 0.2;
      opacity: 0.9;
    }
    to {
      scale: 1.5;
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .glow,
    .core {
      animation: none;
    }
  }
</style>
