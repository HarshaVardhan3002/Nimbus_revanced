<script lang="ts">
  // The island. Folded, it is a black notch at the top of the screen with the core in
  // the middle; opened, it unfolds out of that notch into a panel, the pages spreading
  // out to either side of the core as tabs. The window never moves or resizes: only
  // the island's shape paints, and Rust makes everything outside it click-through
  // (we send it the shape's rectangle whenever the size changes).
  import { onMount, tick, untrack } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { api, clock } from "./lib/api";
  import { BRAND } from "./brand";
  import { startClock, time } from "./lib/clock.svelte";
  import Core, { type Mood } from "./lib/Core.svelte";
  import Icon from "./lib/Icon.svelte";
  import { live, startLive } from "./lib/live.svelte";
  import { leave, rise, spread } from "./lib/motion";
  import { colors, startPalette } from "./lib/palette.svelte";
  import { loadTimer, remaining, running, timer } from "./lib/timer.svelte";
  import { closePanel, fitted, focusActiveTab, NAMED, openPanel, selectTab, stepTab, TABS, ui, type Peek } from "./lib/ui.svelte";
  import { startWeather } from "./lib/weather.svelte";
  import { chat, startChat, words } from "./lib/chat.svelte";
  import { chime } from "./lib/chime";
  import { book, startTodos } from "./lib/todos.svelte";
  import type { Due } from "./lib/api";
  import { startSpeech } from "./lib/speech.svelte";
  import { startVoice, voice } from "./lib/voice.svelte";
  import Apps from "./pages/Apps.svelte";
  import Chat from "./pages/Chat.svelte";
  import Clipboard from "./pages/Clipboard.svelte";
  import Controls from "./pages/Controls.svelte";
  import Power from "./pages/Power.svelte";
  import Settings from "./pages/Settings.svelte";
  import Stats from "./pages/Stats.svelte";
  import Today from "./pages/Today.svelte";
  import Wallpaper from "./pages/Wallpaper.svelte";

  const PAGES = {
    controls: Controls,
    today: Today,
    stats: Stats,
    clipboard: Clipboard,
    chat: Chat,
    apps: Apps,
    wallpaper: Wallpaper,
    power: Power,
    settings: Settings,
  };
  const BAR = 34;
  const LEFT = TABS.slice(0, 4);
  const RIGHT = TABS.slice(5);
  /** The tab bar's width, and below what width its tabs show only their icons. */
  const BAR_W = 696;
  const COMPACT = 600;

  const hhmm = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
  const dayShort = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric" });

  let page = $derived(TABS.find((t) => t.id === ui.tab) ?? TABS[0]);
  let Page = $derived(PAGES[ui.tab]);
  let fit = $derived(fitted(page));
  // The tab bar follows the screen, not the page: it keeps one width and one look
  // from page to page, so the pill only ever slides along it.
  let barW = $derived(Math.min(BAR_W, Math.max(320, ui.vw - 28) - 24));
  let compact = $derived(barW < COMPACT);
  let playing = $derived(!!live.np?.playing);
  let timing = $derived(running());
  let coreHover = $state(false);

  // ---- shape ---------------------------------------------------------------------

  let size = $derived.by(() => {
    if (ui.open) return { w: fit.w, h: fit.h, r: 30 };
    if (ui.peek) return { w: Math.min(ui.peek.kind === "reminder" || ui.peek.kind === "ask" || ui.peek.kind === "notice" ? 440 : 380, ui.vw - 28), h: 84, r: 26 };
    // Something live (music, a timer) widens the notch a little, like a live activity.
    const w = playing || timing ? 232 : 200;
    return ui.hover ? { w: w + 16, h: BAR + 4, r: 16 } : { w, h: BAR, r: 13 };
  });

  // Rust lets clicks through everywhere outside this rectangle.
  $effect(() => {
    const { w, h } = size;
    api.setHitRect({ x: (window.innerWidth - w) / 2, y: 0, w, h }).catch(() => {});
  });

  let mood = $derived.by((): Mood => {
    if (ui.busy > 0) return "thinking";
    // "Hey Nimbus" heard: the core listens for the request.
    if (voice.now?.awake) return "listening";
    // Typing to it: the core listens.
    if (ui.open && ui.tab === "chat" && chat.draft.trim() && chat.replying == null) return "listening";
    if (ui.open && coreHover) return "happy";
    if (playing) return "music";
    if (ui.open || ui.hover || ui.peek || ui.pointer) return "awake";
    return "asleep";
  });

  let bars = $derived.by(() => {
    const [a, b, c] = live.bands;
    return [0.25 + b * 0.75, 0.3 + a * 0.7, 0.2 + c * 0.8, 0.25 + ((a + c) / 2) * 0.75];
  });

  // ---- open and close --------------------------------------------------------------

  // With the mouse, the panel folds shortly after the pointer leaves it, unless the
  // keyboard is in it, a field is being typed in, or something is being held.
  let leaveTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const away = ui.open && !ui.hover && !ui.keyboard && ui.typing === 0 && ui.stay === 0 && !ui.sticky;
    clearTimeout(leaveTimer);
    if (away) leaveTimer = setTimeout(closePanel, 480);
  });

  // Opened from the tray, the panel waits for the pointer to arrive (or 8 seconds).
  let stickyTimer: ReturnType<typeof setTimeout> | undefined;
  function openFromTray() {
    openPanel({ sticky: true });
    clearTimeout(stickyTimer);
    stickyTimer = setTimeout(() => (ui.sticky = false), 8000);
  }

  function onHover(inside: boolean) {
    ui.hover = inside;
    if (inside) ui.sticky = false;
  }

  // A wallpaper previewed but not applied gives its colours back when the panel folds.
  $effect(() => {
    if (!ui.open) colors.preview = null;
  });

  function onKey(e: KeyboardEvent) {
    if (!ui.open) return;
    if (e.key === "Escape") {
      // Pages that use Escape themselves (clearing a search) mark it handled.
      if (!e.defaultPrevented) closePanel();
    } else if (e.ctrlKey && e.key === "Tab") {
      e.preventDefault();
      stepTab(e.shiftKey ? -1 : 1);
      tick().then(focusActiveTab);
    } else if (e.ctrlKey && !e.altKey && !e.shiftKey && /^[1-8]$/.test(e.key)) {
      e.preventDefault();
      selectTab(NAMED[Number(e.key) - 1].id);
      tick().then(focusActiveTab);
    }
  }

  function tabKey(e: KeyboardEvent) {
    const i = TABS.findIndex((t) => t.id === ui.tab);
    const to: Record<string, number> = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: TABS.length - 1 };
    if (!(e.key in to)) return;
    e.preventDefault();
    selectTab(TABS[(to[e.key] + TABS.length) % TABS.length].id);
    tick().then(focusActiveTab);
  }

  // Scroll down on the notch to open it; scroll up on the tabs to fold it.
  function shapeWheel(e: WheelEvent) {
    if (!ui.open && e.deltaY > 0) openPanel(ui.peek?.kind === "timer" ? { tab: "today" } : {});
  }
  function tabsWheel(e: WheelEvent) {
    if (e.deltaY < 0) closePanel();
  }

  // ---- tabs ------------------------------------------------------------------------

  let tablist = $state<HTMLElement | null>(null);
  let pill = $state<{ x: number; w: number } | null>(null);
  let pillMoves = $state(false);

  $effect(() => {
    const id = ui.tab;
    const list = tablist;
    // Measured again when the bar itself changes (a new screen).
    void barW;
    void compact;
    if (!list) {
      pill = null;
      pillMoves = false;
      return;
    }
    const el = list.querySelector<HTMLElement>(`[data-tab="${id}"]`);
    // Chat is the core, outside the bar: the pill shrinks into the middle and fades.
    pill = el ? { x: el.offsetLeft, w: el.offsetWidth } : { x: list.offsetWidth / 2 - 17, w: 34 };
    // Placed without animation the first time, then it slides.
    if (!untrack(() => pillMoves)) requestAnimationFrame(() => requestAnimationFrame(() => (pillMoves = true)));
  });

  /** The core looks at the tab under the pointer. */
  function gazeAt(e: PointerEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    ui.gaze = { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  }

  // ---- peeks: the notch grows for a moment to say something --------------------------

  let peekTimer: ReturnType<typeof setTimeout> | undefined;
  function showPeek(p: Peek, ms: number) {
    ui.peek = p;
    clearTimeout(peekTimer);
    peekTimer = setTimeout(() => (ui.peek = null), ms);
  }
  function dismissPeek() {
    clearTimeout(peekTimer);
    stopRinging();
    ui.peek = null;
  }

  // Reminders. A plain one chimes once and stays a minute; an alarm rings every few
  // seconds until it is answered (for ten minutes at most).
  let ring: ReturnType<typeof setInterval> | undefined;
  function stopRinging() {
    clearInterval(ring);
    ring = undefined;
  }
  function remind(d: Due) {
    const alarm = d.kind === "alarm";
    stopRinging();
    chime();
    if (alarm) {
      const until = Date.now() + 10 * 60_000;
      ring = setInterval(() => (Date.now() > until ? stopRinging() : chime()), 4000);
    }
    ui.cheers++;
    showPeek({ kind: "reminder", id: d.id, text: d.text, alarm, task: d.kind === "task" }, alarm ? 10 * 60_000 : 60_000);
  }
  function reminderDone(id: string) {
    const t = book.todos.find((t) => t.id === id);
    // A repeating one has already moved on to its next time; only a one-off is ticked.
    if (t && t.repeat === "never") api.todoEdit({ ...$state.snapshot(t), done: true }).catch(() => {});
    dismissPeek();
  }
  function snooze(id: string) {
    api.todoSnooze(id, 10).catch(() => {});
    dismissPeek();
  }

  // An OK the assistant waits for, while the panel is folded.
  let seenAsking = 0;
  $effect(() => {
    const n = chat.asking;
    untrack(() => {
      if (n > seenAsking && !(ui.open && ui.tab === "chat")) {
        let title = "Needs your OK";
        for (const t of chat.turns) {
          if (t.role !== "assistant") continue;
          for (const p of t.parts) if (p.kind === "ask" && p.answer == null) title = p.title;
        }
        if (ui.peek?.kind !== "reminder") showPeek({ kind: "ask", title }, 120_000);
      }
      if (n === 0 && ui.peek?.kind === "ask") dismissPeek();
      seenAsking = n;
    });
  });

  // A new track while the panel is folded.
  let lastTrack = "";
  $effect(() => {
    const np = live.np;
    if (!np) return;
    const key = `${np.app}|${np.title}|${np.artist}`;
    untrack(() => {
      if (key === lastTrack) return;
      const first = lastTrack === "";
      lastTrack = key;
      if (first || !np.playing || ui.open || ui.peek?.kind === "timer") return;
      showPeek({ kind: "track", title: np.title, artist: np.artist, art: np.artwork }, 3200);
    });
  });

  // A timer finishing: the chime plays (timer.svelte.ts) and the notch says so.
  let seenDone = 0;
  $effect(() => {
    const n = timer.finished;
    untrack(() => {
      if (n === seenDone) return;
      seenDone = n;
      ui.cheers++;
      if (!ui.open) showPeek({ kind: "timer", label: `${Math.round(timer.total / 60_000)} min timer` }, 30_000);
    });
  });

  // A plugin's notice: with buttons it waits five minutes for an answer, else a moment.
  type Notice = { id: string; from: string; text: string; buttons: string[] };
  function notice(n: Notice) {
    if (ui.peek?.kind === "reminder" || ui.peek?.kind === "ask") return;
    chime();
    showPeek({ kind: "notice", ...n }, n.buttons.length ? 5 * 60_000 : 12_000);
  }
  function noticePressed(id: string, button: string) {
    api.noticeAnswer(id, button).catch(() => {});
    dismissPeek();
  }

  function openFromPeek() {
    const k = ui.peek?.kind;
    if (k === "ask" || k === "ran") {
      dismissPeek();
      openPanel({ tab: "chat" });
    } else {
      openPanel(k === "timer" || k === "reminder" ? { tab: "today" } : {});
    }
  }

  // ---- start -------------------------------------------------------------------------

  onMount(() => {
    const stops = [
      startLive(),
      startPalette(),
      startClock(),
      startSpeech(),
      startVoice(),
      startTodos(remind),
      (() => {
        const off = listen<Notice>("notice", ({ payload }) => notice(payload));
        return () => void off.then((f) => f());
      })(),
      // A routine finishing on its own says so from the notch.
      startChat((t) => {
        if (ui.open && ui.tab === "chat") return;
        if (ui.peek?.kind === "reminder") return;
        const said = words(t);
        showPeek(
          { kind: "ran", name: t.from ?? "Routine", result: t.state === "failed" ? (t.error ?? "It did not finish.") : said || "Done.", failed: t.state === "failed" },
          8000,
        );
      }),
    ];
    loadTimer();
    let stopWeather = () => {};
    startWeather().then((f) => (stopWeather = f));

    const unlisten = [
      listen<{ keyboard: boolean; tab: "chat" | null }>("toggle-panel", ({ payload }) => {
        if (payload.tab === "chat") {
          // Straight into the chat box, whether or not the panel was open.
          ui.wantInput = true;
          if (ui.open && ui.tab !== "chat") selectTab("chat");
          else if (ui.open) document.getElementById("chat-input")?.focus();
          else openPanel({ keyboard: true, tab: "chat" });
        } else if (payload.keyboard) {
          if (ui.open) closePanel();
          else openPanel({ keyboard: true });
        } else if (!ui.open) {
          openFromTray();
        }
      }),
      listen<boolean>("island-hover", ({ payload }) => onHover(payload)),
      listen<{ x: number; y: number } | null>("cursor", ({ payload }) => {
        ui.pointer = payload;
      }),
    ];

    // Clicking into another app takes the keyboard away: fold, as menus do.
    const blur = () => {
      if (ui.open && (ui.keyboard || ui.typing) && ui.stay === 0) closePanel();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("blur", blur);

    return () => {
      stops.forEach((stop) => stop());
      stopWeather();
      unlisten.forEach((p) => p.then((f) => f()));
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", blur);
      clearTimeout(leaveTimer);
      clearTimeout(stickyTimer);
      clearTimeout(peekTimer);
      stopRinging();
    };
  });
</script>

<div class="stage">
  <div
    class="frame"
    class:open={ui.open}
    style:width="{size.w}px"
    style:height="{size.h}px"
    style:--r="{size.r}px"
  >
    <i class="shoulder left" aria-hidden="true"></i>
    <i class="shoulder right" aria-hidden="true"></i>

    <div class="shape" onwheel={shapeWheel}>
      <div class="tint" aria-hidden="true"></div>

      {#if !ui.open}
        <button
          class="opener"
          aria-label="Open {BRAND.name}"
          onclick={() => openFromPeek()}
        ></button>

        <div class="slot left" aria-hidden="true" in:rise|global={{ delay: 220 }} out:leave|global={{ duration: 80 }}>
          {#if playing && live.np?.artwork}
            <img class="cover" src={live.np.artwork} alt="" in:rise />
          {:else}
            <span class="display clock" in:rise>{hhmm.format(time.now)}</span>
          {/if}
        </div>

        <div class="slot right" aria-hidden="true" in:rise|global={{ delay: 220 }} out:leave|global={{ duration: 80 }}>
          {#if timing}
            <span class="countdown num" in:rise>
              <span class="mini-ring" style:--p={remaining() / Math.max(1, timer.total)}></span>
              {clock(remaining())}
            </span>
          {:else if playing}
            <span class="eq" in:rise>
              {#each bars as b, i (i)}<i style:--b={b}></i>{/each}
            </span>
          {:else}
            <span class="date" in:rise>{dayShort.format(time.now)}</span>
          {/if}
        </div>

        {#if ui.peek}
          <div class="peek" role="status" in:rise={{ delay: 140 }} out:leave>
            {#if ui.peek.kind === "track"}
              {#if ui.peek.art}
                <img class="peek-art" src={ui.peek.art} alt="" />
              {:else}
                <span class="peek-icon"><Icon name="music-notes" size={18} /></span>
              {/if}
              <span class="peek-text">
                <strong class="ellipsis">{ui.peek.title || "Unknown title"}</strong>
                <span class="meta ellipsis">{ui.peek.artist}</span>
              </span>
            {:else if ui.peek.kind === "reminder"}
              {@const p = ui.peek}
              <span class="peek-icon on" class:ringing={p.alarm}><Icon name={p.alarm ? "alarm" : p.task ? "lightning" : "bell-ringing"} size={18} /></span>
              <span class="peek-text">
                <strong class="ellipsis">{p.text}</strong>
                <span class="meta">{p.alarm ? "Alarm" : p.task ? `${BRAND.name} is on it` : "Reminder"}</span>
              </span>
              {#if !p.task}
                <button class="btn small quiet" onclick={() => snooze(p.id)}>Snooze</button>
              {/if}
              <button class="btn small" onclick={() => reminderDone(p.id)}>{p.task ? "OK" : "Done"}</button>
            {:else if ui.peek.kind === "notice"}
              {@const p = ui.peek}
              <span class="peek-icon on"><Icon name="bell-ringing" size={18} /></span>
              <span class="peek-text">
                <strong class="ellipsis" title={p.text}>{p.text}</strong>
                <span class="meta ellipsis">{p.from}</span>
              </span>
              {#each p.buttons as b, i (b)}
                <button class="btn small" class:quiet={i > 0} onclick={() => noticePressed(p.id, b)}>{b}</button>
              {/each}
            {:else if ui.peek.kind === "ran"}
              <span class="peek-icon" class:on={!ui.peek.failed}><Icon name={ui.peek.failed ? "x" : "lightning"} size={18} /></span>
              <span class="peek-text">
                <strong class="ellipsis">{ui.peek.name}</strong>
                <span class="meta ellipsis">{ui.peek.result}</span>
              </span>
            {:else if ui.peek.kind === "ask"}
              <span class="peek-icon on"><Icon name="shield-check" size={18} /></span>
              <span class="peek-text">
                <strong class="ellipsis">{ui.peek.title}</strong>
                <span class="meta">{BRAND.name} is waiting for your OK</span>
              </span>
              <button class="btn small" onclick={() => openFromPeek()}>Review</button>
            {:else}
              <span class="peek-icon on"><Icon name="timer" size={18} /></span>
              <span class="peek-text">
                <strong>Time is up</strong>
                <span class="meta">{ui.peek.label}</span>
              </span>
              <button class="btn small" onclick={dismissPeek}>Dismiss</button>
            {/if}
          </div>
        {/if}
      {:else}
        <!-- svelte-ignore a11y_interactive_supports_focus -->
        <div
          class="tabs"
          class:compact
          role="tablist"
          aria-label="{BRAND.name} pages"
          aria-owns="tab-chat"
          style:width="{compact ? 344 : barW}px"
          bind:this={tablist}
          onkeydown={tabKey}
          onwheel={tabsWheel}
        >
          {#if pill}
            <span class="pill" class:moves={pillMoves} class:away={ui.tab === "chat"} style:translate="{pill.x}px 0" style:width="{pill.w}px"></span>
          {/if}
          <div class="wing left">
            {#each LEFT as t, i (t.id)}
              {@const k = LEFT.length - 1 - i}
              <button
                class="tab"
                role="tab"
                id="tab-{t.id}"
                data-tab={t.id}
                aria-selected={ui.tab === t.id}
                aria-controls="page"
                tabindex={ui.tab === t.id ? 0 : -1}
                onclick={() => selectTab(t.id)}
                onpointerenter={gazeAt}
                onpointerleave={() => (ui.gaze = null)}
                aria-label={compact ? t.label : undefined}
                title={compact ? t.label : undefined}
                in:spread|global={{ delay: 40 + k * 34, dx: 18 + k * 10 }}
                out:leave|global={{ duration: 90 }}
                >{#if compact}<Icon name={t.icon} size={17} />{:else}{t.label}{/if}</button
              >
            {/each}
          </div>
          <div aria-hidden="true"></div>
          <div class="wing right">
            {#each RIGHT as t, k (t.id)}
              <button
                class="tab"
                role="tab"
                id="tab-{t.id}"
                data-tab={t.id}
                aria-selected={ui.tab === t.id}
                aria-controls="page"
                tabindex={ui.tab === t.id ? 0 : -1}
                onclick={() => selectTab(t.id)}
                onpointerenter={gazeAt}
                onpointerleave={() => (ui.gaze = null)}
                aria-label={compact ? t.label : undefined}
                title={compact ? t.label : undefined}
                in:spread|global={{ delay: 40 + k * 34, dx: -(18 + k * 10) }}
                out:leave|global={{ duration: 90 }}
                >{#if compact}<Icon name={t.icon} size={17} />{:else}{t.label}{/if}</button
              >
            {/each}
          </div>
        </div>

        <div class="body">
          {#key ui.tab}
            <div
              class="page"
              id="page"
              role="tabpanel"
              aria-labelledby="tab-{ui.tab}"
              style:width="{fit.w}px"
              style:margin-left="{-fit.w / 2}px"
              in:rise|global={{ delay: 90 }}
              out:leave|global
            >
              <Page />
            </div>
          {/key}
        </div>
      {/if}

      <!-- Open, the core is the chat tab: it is the one to talk to. -->
      <button
        class="core"
        class:live={ui.open}
        class:selected={ui.open && ui.tab === "chat"}
        inert={!ui.open}
        id="tab-chat"
        data-tab="chat"
        role={ui.open ? "tab" : undefined}
        aria-selected={ui.open ? ui.tab === "chat" : undefined}
        aria-controls={ui.open ? "page" : undefined}
        tabindex={ui.open && ui.tab === "chat" ? 0 : -1}
        aria-label="Chat"
        title="Chat"
        onclick={() => {
          ui.wantInput = true;
          selectTab("chat");
        }}
        onkeydown={tabKey}
        onpointerenter={() => (coreHover = true)}
        onpointerleave={() => (coreHover = false)}
      >
        <Core
          {mood}
          look={ui.gaze ?? ui.pointer}
          level={live.bands[0]}
          gulps={ui.gulps}
          shies={ui.shies}
          cheers={ui.cheers}
        />
      </button>
    </div>
  </div>
</div>

<style>
  .stage {
    position: fixed;
    inset: 0;
    pointer-events: none;
  }

  .frame {
    position: absolute;
    top: 0;
    left: 50%;
    translate: -50% 0;
    pointer-events: auto;
    transition:
      width var(--t-move) var(--spring),
      height var(--t-move) var(--spring);
  }

  /* Concave corners where the notch meets the top edge of the screen. */
  .shoulder {
    position: absolute;
    top: 0;
    width: 12px;
    height: 12px;
    pointer-events: none;
  }
  .shoulder.left {
    right: 100%;
    background: radial-gradient(circle at 0 100%, transparent 11.5px, var(--bezel) 12px);
  }
  .shoulder.right {
    left: 100%;
    background: radial-gradient(circle at 100% 100%, transparent 11.5px, var(--bezel) 12px);
  }

  .shape {
    position: absolute;
    inset: 0;
    overflow: hidden;
    border-radius: 0 0 var(--r) var(--r);
    background: var(--bezel);
    box-shadow:
      0 18px 40px -14px rgb(0 0 0 / 0.75),
      0 4px 10px -4px rgb(0 0 0 / 0.5);
    transition: border-radius var(--t-move) var(--spring);
  }

  /* Open, the panel takes the wallpaper's darkest tone below the black bar, lit
     faintly from the core. */
  .tint {
    position: absolute;
    inset: 0;
    opacity: 0;
    pointer-events: none;
    background:
      radial-gradient(ellipse 46% 130px at 50% 0, color-mix(in oklab, var(--accent) 9%, transparent), transparent),
      linear-gradient(180deg, var(--bezel) 30px, var(--panel) 190px);
    transition: opacity 0.5s var(--out);
  }
  .open .tint {
    opacity: 1;
  }
  /* A hairline rim on the open panel's sides and bottom, fading out towards the top
     edge so the notch stays seamless with the bezel. */
  .shape::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: inherit;
    pointer-events: none;
    box-shadow: inset 0 0 0 1px rgb(255 255 255 / 0.08);
    mask: linear-gradient(180deg, transparent 34px, #000 130px);
    opacity: 0;
    transition: opacity 0.4s var(--out);
  }
  .open .shape::after {
    opacity: 1;
  }

  /* ---- folded ---- */

  .opener {
    position: absolute;
    inset: 0;
    border-radius: inherit;
  }
  .opener:focus-visible {
    outline-offset: -3px;
  }

  .slot {
    position: absolute;
    top: 0;
    height: 34px;
    display: flex;
    align-items: center;
    pointer-events: none;
  }
  .slot.left {
    left: 15px;
  }
  .slot.right {
    right: 15px;
  }
  .clock {
    font-size: 13px;
  }
  .date {
    font-size: 12.5px;
    font-weight: 560;
    color: var(--text-2);
  }
  .cover {
    width: 20px;
    height: 20px;
    border-radius: 6px;
    object-fit: cover;
  }
  .eq {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 14px;
  }
  .eq i {
    width: 3px;
    height: 14px;
    border-radius: 2px;
    background: var(--accent);
    scale: 1 var(--b);
    transition: scale 90ms linear;
  }
  .countdown {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--accent-text);
  }
  .mini-ring {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: conic-gradient(var(--accent) calc(var(--p) * 1turn), var(--s3) 0);
    mask: radial-gradient(closest-side, transparent 56%, #000 60%);
  }

  .peek {
    position: absolute;
    left: 0;
    right: 0;
    top: 34px;
    height: 50px;
    padding: 0 14px 10px 16px;
    display: flex;
    align-items: center;
    gap: 12px;
    pointer-events: none;
  }
  .peek .btn {
    pointer-events: auto;
  }
  .peek-art,
  .peek-icon {
    width: 36px;
    height: 36px;
    border-radius: 10px;
    flex: none;
  }
  .peek-art {
    object-fit: cover;
  }
  .peek-icon {
    display: grid;
    place-items: center;
    background: var(--s2);
    color: var(--text-2);
  }
  .peek-icon.ringing {
    animation: ring 1.2s var(--out) infinite;
  }
  @keyframes ring {
    0%,
    40%,
    100% {
      rotate: 0deg;
    }
    8%,
    24% {
      rotate: -14deg;
    }
    16%,
    32% {
      rotate: 14deg;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .peek-icon.ringing {
      animation: none;
    }
  }
  .peek-icon.on {
    background: var(--accent);
    color: var(--ink);
  }
  .peek-text {
    flex: 1;
    min-width: 0;
    display: grid;
  }
  .peek-text strong {
    font-weight: 600;
  }

  /* ---- open ---- */

  .tabs {
    position: absolute;
    top: 4px;
    left: 50%;
    translate: -50% 0;
    height: 26px;
    display: grid;
    grid-template-columns: 1fr 52px 1fr;
  }
  .wing {
    display: flex;
    gap: 2px;
  }
  .wing.left {
    justify-content: flex-end;
  }
  .tab {
    position: relative;
    height: 26px;
    padding: 0 11px;
    border-radius: 13px;
    font-size: 12.5px;
    font-weight: 560;
    color: var(--text-2);
    transition: color 160ms var(--out);
  }
  .tab:hover {
    color: var(--text);
  }
  .compact .tab {
    width: 34px;
    padding: 0;
    display: grid;
    place-items: center;
  }
  /* The label darkens once the pill has arrived under it. */
  .tab[aria-selected="true"] {
    color: var(--ink);
    transition: color 180ms var(--out) 110ms;
  }
  .pill {
    position: absolute;
    top: 0;
    left: 0;
    height: 26px;
    border-radius: 13px;
    background: var(--accent);
    box-shadow: 0 4px 14px -4px color-mix(in oklab, var(--accent) 60%, transparent);
    animation: pill-in 320ms var(--out) 180ms both;
  }
  .pill.away {
    opacity: 0;
    scale: 0.6;
  }
  .pill.moves {
    transition:
      translate var(--t-pop) var(--spring-pop),
      width var(--t-pop) var(--spring-pop),
      opacity var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }

  .body {
    position: absolute;
    top: 34px;
    left: 0;
    right: 0;
    bottom: 0;
  }
  /* Pages reflow by their own width (container queries), and scroll when the screen
     is too short for them. */
  .page {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 50%;
    padding: 12px 16px 16px;
    container: page / inline-size;
    overflow: hidden auto;
  }

  /* ---- the core ---- */

  .core {
    position: absolute;
    top: 6px;
    left: 50%;
    width: 22px;
    height: 22px;
    margin-left: -11px;
    border-radius: 50%;
    pointer-events: none;
    transition: scale var(--t-pop) var(--spring-pop);
  }
  .core.live {
    pointer-events: auto;
  }
  .core.live:hover {
    scale: 1.1;
  }
  .core.live:active {
    scale: 0.9;
  }
  /* On the chat page the core wears the accent ring the pill wears elsewhere. */
  .core::before {
    content: "";
    position: absolute;
    inset: -4px;
    border-radius: 50%;
    box-shadow:
      0 0 0 2px var(--accent),
      0 0 16px -2px var(--accent);
    opacity: 0;
    scale: 0.7;
    pointer-events: none;
    transition:
      opacity var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }
  .core.selected::before {
    opacity: 1;
    scale: 1;
  }

  @keyframes pill-in {
    from {
      opacity: 0;
      scale: 0.7;
    }
  }
</style>
