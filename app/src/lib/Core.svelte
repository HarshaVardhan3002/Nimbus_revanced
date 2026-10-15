<script lang="ts" module>
  export type Mood = "asleep" | "awake" | "happy" | "music" | "thinking" | "listening";
</script>

<script lang="ts">
  // The core: a small glowing sphere with two eyes that lives in the middle of the
  // notch. It sleeps while nobody is near, wakes and watches the pointer, smiles and
  // bobs to music, looks up while it works, gulps each new copy, and looks away
  // when it leaves a password out of the clipboard history.
  import { onMount, untrack } from "svelte";

  let {
    mood = "awake",
    look = null,
    level = 0,
    gulps = 0,
    shies = 0,
    cheers = 0,
    size = 22,
  }: {
    mood?: Mood;
    /** A point in window CSS px for the eyes to look at. */
    look?: { x: number; y: number } | null;
    /** Music loudness 0..1: the core bobs to it. */
    level?: number;
    /** Counters: each increase plays the reaction once. */
    gulps?: number;
    shies?: number;
    cheers?: number;
    size?: number;
  } = $props();

  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

  let el: HTMLElement;
  let orb: HTMLElement;
  let blink = $state(false);
  let shy = $state(false);
  let cheer = $state(false);

  let face = $derived<Mood>(cheer && mood !== "thinking" ? "happy" : mood);

  let eyes = $derived.by(() => {
    if (shy) return { x: -2.3, y: 0.9 };
    if (face === "thinking") return { x: 1.8, y: -1.6 };
    if (face === "asleep" || !look || !el) return { x: 0, y: 0.4 };
    const r = el.getBoundingClientRect();
    const dx = look.x - (r.left + r.width / 2);
    const dy = look.y - (r.top + r.height / 2);
    const d = Math.hypot(dx, dy);
    if (d < 2) return { x: 0, y: 0 };
    // Close to the core the eyes turn less, so they never cross.
    const reach = Math.min(1, d / 120);
    return { x: (dx / d) * 2.4 * reach, y: (dy / d) * 1.7 * reach };
  });

  onMount(() => {
    let next: ReturnType<typeof setTimeout>;
    const pending: ReturnType<typeof setTimeout>[] = [];
    const once = (at: number) => {
      pending.push(setTimeout(() => (blink = true), at));
      pending.push(setTimeout(() => (blink = false), at + 120));
    };
    const schedule = () => {
      next = setTimeout(
        () => {
          pending.length = 0;
          once(0);
          // Now and then a double blink.
          if (Math.random() < 0.22) once(260);
          schedule();
        },
        2600 + Math.random() * 4200,
      );
    };
    schedule();
    return () => {
      clearTimeout(next);
      pending.forEach(clearTimeout);
    };
  });

  const seen = untrack(() => ({ gulps, shies, cheers }));
  let shyTimer: ReturnType<typeof setTimeout> | undefined;
  let cheerTimer: ReturnType<typeof setTimeout> | undefined;

  $effect(() => {
    if (gulps === seen.gulps) return;
    seen.gulps = gulps;
    if (reduced || !orb) return;
    orb.animate(
      [
        { scale: "1 1", translate: "0 0", filter: "brightness(1)" },
        { scale: "1.18 0.8", translate: "0 1px", filter: "brightness(1.3)", offset: 0.18 },
        { scale: "0.9 1.12", translate: "0 -1px", offset: 0.42 },
        { scale: "1.04 0.97", translate: "0 0", offset: 0.68 },
        { scale: "1 1", translate: "0 0", filter: "brightness(1)" },
      ],
      { duration: 560, easing: "ease-out" },
    );
  });

  $effect(() => {
    if (shies === seen.shies) return;
    seen.shies = shies;
    shy = true;
    clearTimeout(shyTimer);
    shyTimer = setTimeout(() => (shy = false), 1700);
  });

  $effect(() => {
    if (cheers === seen.cheers) return;
    seen.cheers = cheers;
    cheer = true;
    clearTimeout(cheerTimer);
    cheerTimer = setTimeout(() => (cheer = false), 1500);
  });
</script>

<span
  class="core"
  class:blink
  class:shy
  class:still={reduced}
  data-mood={face}
  bind:this={el}
  style:--size="{size}px"
  style:--lv={face === "music" ? level : 0}
  aria-hidden="true"
>
  <span class="halo"></span>
  <span class="body">
    <span class="spark"></span>
    <span class="orb" bind:this={orb}>
      <span class="shine"></span>
      <span class="eyes" style:--ex={eyes.x} style:--ey={eyes.y}>
        <i class="eye"><b class="open"></b><b class="arc"></b></i>
        <i class="eye"><b class="open"></b><b class="arc"></b></i>
      </span>
    </span>
  </span>
</span>

<style>
  .core {
    /* One unit is 1 px at the notch size (22 px); everything scales with --size. */
    --u: calc(var(--size) / 22);
    position: relative;
    display: block;
    flex: none;
    width: var(--size);
    height: var(--size);
  }

  .halo {
    position: absolute;
    inset: calc(var(--u) * -6);
    border-radius: 50%;
    background: radial-gradient(closest-side, color-mix(in oklab, var(--accent) 55%, transparent), transparent);
    opacity: 0.3;
    transition: opacity 0.6s var(--out);
  }
  .core:not([data-mood="asleep"]) .halo {
    opacity: 0.72;
    animation: breathe 3.8s ease-in-out infinite;
  }
  .core[data-mood="listening"] .halo {
    opacity: 0.95;
    animation-duration: 1.3s;
  }
  .core[data-mood="music"] .halo {
    animation: none;
    opacity: calc(0.5 + var(--lv) * 0.5);
    scale: calc(0.92 + var(--lv) * 0.2);
    transition:
      opacity 90ms linear,
      scale 90ms linear;
  }

  .body {
    position: absolute;
    inset: 0;
    translate: 0 calc(var(--lv) * var(--u) * -1.6);
    scale: calc(1 + var(--lv) * 0.08);
    transition:
      translate 90ms linear,
      scale 90ms linear;
  }

  .orb {
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: radial-gradient(
      circle at 35% 30%,
      color-mix(in oklab, var(--accent) 28%, white) 0%,
      var(--accent) 50%,
      color-mix(in oklab, var(--accent) 58%, black) 100%
    );
    box-shadow: inset 0 calc(var(--u) * -1.5) calc(var(--u) * 3) color-mix(in oklab, var(--accent) 45%, black);
    transition: filter 0.6s var(--out);
  }
  .core[data-mood="asleep"] .orb {
    filter: brightness(0.72) saturate(0.85);
  }

  .shine {
    position: absolute;
    left: 20%;
    top: 13%;
    width: 32%;
    height: 20%;
    border-radius: 50%;
    background: rgb(255 255 255 / 0.5);
    rotate: -28deg;
    filter: blur(calc(var(--u) * 0.6));
  }

  /* Thinking: a spark circles the core. */
  .spark {
    position: absolute;
    inset: calc(var(--u) * -3.5);
    border-radius: 50%;
    opacity: 0;
    transition: opacity 0.3s var(--out);
  }
  .spark::before {
    content: "";
    position: absolute;
    top: 0;
    left: 50%;
    width: calc(var(--u) * 3);
    height: calc(var(--u) * 3);
    margin-left: calc(var(--u) * -1.5);
    border-radius: 50%;
    background: color-mix(in oklab, var(--accent) 30%, white);
    box-shadow: 0 0 calc(var(--u) * 4) var(--accent);
  }
  .core[data-mood="thinking"] .spark {
    opacity: 1;
    animation: orbit 1.1s linear infinite;
  }

  .eyes {
    position: absolute;
    inset: 0;
    display: flex;
    justify-content: center;
    align-items: center;
    gap: calc(var(--u) * 4.2);
    padding-bottom: var(--u);
    translate: calc(var(--ex) * var(--u)) calc(var(--ey) * var(--u));
    transition: translate 280ms var(--spring-pop);
  }
  .eye {
    position: relative;
    width: calc(var(--u) * 3);
    height: calc(var(--u) * 6.4);
    transition: scale 110ms var(--out);
  }
  .open {
    position: absolute;
    inset: 0;
    border-radius: calc(var(--u) * 2);
    background: var(--ink);
    transition:
      scale 260ms var(--spring-pop),
      opacity 160ms var(--out);
  }
  /* Smiling eyes: the top of a ring. */
  .arc {
    position: absolute;
    left: 50%;
    top: 50%;
    width: calc(var(--u) * 5.6);
    height: calc(var(--u) * 5.6);
    translate: -50% -30%;
    border-radius: 50%;
    border: calc(var(--u) * 1.7) solid transparent;
    border-top-color: var(--ink);
    opacity: 0;
    scale: 0.5;
    transition:
      scale 260ms var(--spring-pop),
      opacity 160ms var(--out);
  }

  .blink:is([data-mood="awake"], [data-mood="listening"], [data-mood="thinking"]) .eye {
    scale: 1 0.1;
  }
  .core[data-mood="asleep"] .open,
  .shy .open {
    scale: 1.3 0.22;
  }
  .core[data-mood="happy"] .open,
  .core[data-mood="music"] .open {
    opacity: 0;
    scale: 0.5;
  }
  .core[data-mood="happy"] .arc,
  .core[data-mood="music"] .arc {
    opacity: 1;
    scale: 1;
  }
  .core[data-mood="listening"] .open {
    scale: 1.1 1.2;
  }

  .core.still .halo,
  .core.still .spark {
    animation: none;
  }
  .still .body {
    translate: none;
    scale: none;
  }

  @keyframes breathe {
    0%,
    100% {
      scale: 0.9;
    }
    50% {
      scale: 1.06;
    }
  }
  @keyframes orbit {
    to {
      rotate: 1turn;
    }
  }
</style>
