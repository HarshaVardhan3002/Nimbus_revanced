<script lang="ts">
  // A big button for actions that end the session. With `hold` set it acts only after
  // being held that long (letting go early cancels); with `hold = 0` a click is enough.
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";
  import { ui } from "./ui.svelte";

  let {
    label,
    icon,
    hint,
    doing,
    onconfirm,
    hold = 1000,
  }: { label: string; icon: IconName; hint: string; doing: string; onconfirm: () => void; hold?: number } = $props();

  let progress = $state(0);
  let phase = $state<"idle" | "holding" | "done">("idle");
  let start = 0;
  let frame = 0;
  let back: ReturnType<typeof setTimeout> | undefined;

  function confirm() {
    phase = "done";
    progress = 1;
    onconfirm();
    // Still here after a few seconds: the action did not happen (or was cancelled).
    clearTimeout(back);
    back = setTimeout(() => {
      phase = "idle";
      progress = 0;
    }, 4000);
  }

  function begin() {
    if (phase !== "idle" || hold <= 0) return;
    phase = "holding";
    ui.stay++;
    start = performance.now();
    const step = (now: number) => {
      progress = Math.min(1, (now - start) / hold);
      if (progress >= 1) {
        ui.stay = Math.max(0, ui.stay - 1);
        confirm();
        return;
      }
      frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
  }

  function cancel() {
    if (phase !== "holding") return;
    cancelAnimationFrame(frame);
    phase = "idle";
    progress = 0;
    ui.stay = Math.max(0, ui.stay - 1);
  }

  function click() {
    if (hold <= 0 && phase === "idle") confirm();
  }

  function key(e: KeyboardEvent) {
    if (hold <= 0 || (e.key !== " " && e.key !== "Enter")) return;
    e.preventDefault();
    if (e.type === "keydown" && !e.repeat) begin();
    if (e.type === "keyup") cancel();
  }
</script>

<button
  class="hold"
  class:holding={phase === "holding"}
  class:done={phase === "done"}
  aria-label="{label}. {hint}"
  onclick={click}
  onpointerdown={(e) => e.button === 0 && begin()}
  onpointerup={cancel}
  onpointerleave={cancel}
  onpointercancel={cancel}
  onkeydown={key}
  onkeyup={key}
  oncontextmenu={(e) => e.preventDefault()}
>
  <span class="ring" style:--p={progress}>
    <Icon name={icon} size={22} />
  </span>
  <span class="label">{label}</span>
  <span class="hint meta" aria-live="polite">
    {phase === "done" ? doing : phase === "holding" ? "Keep holding" : hint}
  </span>
</button>

<style>
  .hold {
    width: 100%;
    height: 100%;
    display: grid;
    justify-items: center;
    align-content: center;
    gap: 5px;
    border-radius: 20px;
    background: var(--s1);
    box-shadow: inset 0 0 0 1px var(--line);
    transition:
      background-color var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }
  .hold:hover {
    background: var(--s2);
  }
  .hold.holding {
    scale: 0.97;
  }
  .ring {
    position: relative;
    width: 48px;
    height: 48px;
    margin-bottom: 5px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--s2);
    color: var(--text);
    transition:
      background-color var(--t-fade) var(--out),
      color var(--t-fade) var(--out);
  }
  /* The ring fills clockwise while held. */
  .ring::before {
    content: "";
    position: absolute;
    inset: -4px;
    border-radius: 50%;
    background: conic-gradient(var(--accent) calc(var(--p) * 1turn), transparent 0);
    mask: radial-gradient(closest-side, transparent calc(100% - 3px), #000 calc(100% - 2.5px));
  }
  .done .ring {
    background: var(--accent);
    color: var(--ink);
  }
  .label {
    font-weight: 600;
  }
  .hint {
    min-height: 16px;
  }
  .holding .hint {
    color: var(--accent-text);
  }
</style>
