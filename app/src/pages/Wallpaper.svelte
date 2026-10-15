<script lang="ts">
  // Wallpaper: pictures from a folder, or Wallpaper Engine's wallpapers, on a reel.
  // The centred one lends the island its colours as a preview; choosing it applies it.
  import { onMount } from "svelte";
  import {
    api,
    fileUrl,
    reason,
    type EngineItem,
    type EngineStatus,
    type Palette,
    type Settings,
    type WallpaperFolder,
  } from "../lib/api";
  import Icon from "../lib/Icon.svelte";
  import { colors } from "../lib/palette.svelte";
  import { busy, ui } from "../lib/ui.svelte";

  type Source = "folder" | "engine";
  type Card = { id: string; title: string; thumb: string | null; palette: Palette | null; detail: string };

  /** Cards drawn on each side of the centre one. */
  const SIDE = 5;
  const KIND: Record<string, string> = { scene: "Scene", video: "Video", web: "Web page", application: "App" };
  const ORIGIN: Record<string, string> = { workshop: "Workshop", mine: "Made by you", default: "Built in" };

  let source = $state<Source>("folder");
  let folder = $state<WallpaperFolder | null>(null);
  let engineItems = $state<EngineItem[] | null>(null);
  let engine = $state<EngineStatus | null>(null);
  let settings = $state<Settings | null>(null);
  /** The picture set as the desktop wallpaper, normalised. */
  let current = $state<string | null>(null);
  let centre = $state(0);
  let loadError = $state("");
  let applying = $state<string | null>(null);
  let error = $state("");

  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

  let cards = $derived.by<Card[]>(() => {
    if (source === "engine") {
      return (engineItems ?? []).map((it) => ({
        id: it.id,
        title: it.title || "Untitled",
        thumb: it.thumb,
        palette: it.palette,
        detail: [KIND[it.kind] ?? it.kind, ORIGIN[it.source]].filter(Boolean).join(" · "),
      }));
    }
    return (folder?.thumbs ?? []).map((t) => {
      const file = baseName(t.path);
      const dot = file.lastIndexOf(".");
      return {
        id: t.path,
        title: dot > 0 ? file.slice(0, dot) : file,
        thumb: t.thumb,
        palette: t.palette,
        detail: dot > 0 ? `${file.slice(dot + 1).toUpperCase()} picture` : "Picture",
      };
    });
  });

  let loaded = $derived(source === "engine" ? engineItems != null : folder != null);
  let shown = $derived(cards.map((c, i) => ({ c, i })).filter(({ i }) => Math.abs(i - centre) <= SIDE));
  let focus = $derived(cards[centre] ?? null);

  function inUse(c: Card) {
    if (source === "engine") return !!engine?.running && engine.current === norm(c.id);
    return current === norm(c.id);
  }

  function centreOn(id: string | null) {
    const i = id ? cards.findIndex((c) => norm(c.id) === id) : -1;
    centre = Math.max(0, i);
  }

  async function loadFolder() {
    loadError = "";
    try {
      const [f, cur] = await Promise.all([api.wallpaperThumbs(), api.wallpaperGet().catch(() => null)]);
      folder = f;
      current = cur ? norm(cur) : null;
      if (source === "folder") centreOn(current);
    } catch (e) {
      loadError = reason(e);
      folder = { dir: "", thumbs: [] };
    }
  }

  async function loadEngine() {
    loadError = "";
    try {
      const [items, st] = await Promise.all([api.engineList(), api.engineStatus()]);
      engineItems = items;
      engine = st;
      if (source === "engine") centreOn(st.current);
    } catch (e) {
      loadError = reason(e);
      engineItems = [];
    }
  }

  async function setSource(s: Source) {
    if (s === source) return;
    source = s;
    error = "";
    api.storeSet("wallpaper-source", s).catch(() => {});
    if (s === "engine") {
      if (engineItems) centreOn(engine?.current ?? null);
      else await loadEngine();
    } else {
      if (folder) centreOn(current);
      else await loadFolder();
    }
  }

  let previewTimer: ReturnType<typeof setTimeout> | undefined;

  onMount(() => {
    (async () => {
      const [saved, st, s] = await Promise.all([
        api.storeGet<Source>("wallpaper-source").catch(() => null),
        api.engineStatus().catch(() => null),
        api.settingsGet().catch(() => null),
      ]);
      engine = st;
      settings = s;
      source = saved === "engine" && st?.installed ? "engine" : "folder";
      await (source === "engine" ? loadEngine() : loadFolder());
    })();
    return () => {
      clearTimeout(previewTimer);
      clearTimeout(wheelTimer);
      colors.preview = null;
    };
  });

  // The centred wallpaper lends the island its colours for a moment, unless the
  // user picked their own accent colour.
  $effect(() => {
    const p = focus?.palette ?? null;
    const follow = settings?.accent == null;
    clearTimeout(previewTimer);
    if (!settings || !follow) return;
    previewTimer = setTimeout(() => {
      colors.preview = p;
    }, 120);
  });

  function step(by: number) {
    if (!cards.length) return;
    centre = Math.max(0, Math.min(cards.length - 1, centre + by));
  }

  async function apply(c: Card) {
    if (applying || inUse(c)) return;
    error = "";
    applying = c.id;
    try {
      if (source === "engine") {
        const id = await busy(api.engineOpen(c.id));
        engine = { installed: true, running: true, current: norm(id) };
      } else {
        current = norm(await busy(api.wallpaperSet(c.id)));
      }
      ui.cheers++;
    } catch (e) {
      error = `“${c.title}” was not applied: ${reason(e)}. Try again, or choose another.`;
    } finally {
      applying = null;
    }
  }

  async function pickFolder() {
    error = "";
    // The system's folder picker takes the pointer and focus away; keep the panel open.
    ui.stay++;
    try {
      const dir = await api.wallpaperPickFolder();
      if (dir) {
        folder = null;
        await loadFolder();
      }
    } catch (e) {
      error = `The folder picker did not open: ${reason(e)}.`;
    } finally {
      ui.stay = Math.max(0, ui.stay - 1);
    }
  }

  // ---- reel input ----

  let wheelSum = 0;
  let wheelTimer: ReturnType<typeof setTimeout> | undefined;

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const d = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    wheelSum += e.deltaMode === 1 ? d * 33 : d;
    clearTimeout(wheelTimer);
    wheelTimer = setTimeout(() => (wheelSum = 0), 180);
    while (Math.abs(wheelSum) >= 60) {
      const s = Math.sign(wheelSum);
      step(s);
      wheelSum -= s * 60;
    }
  }

  // A wheel listener that may cancel the page's own scrolling.
  function wheel(node: HTMLElement) {
    node.addEventListener("wheel", onWheel, { passive: false });
    return { destroy: () => node.removeEventListener("wheel", onWheel) };
  }

  function reelKey(e: KeyboardEvent) {
    const moves: Record<string, number> = {
      ArrowLeft: -1,
      ArrowRight: 1,
      PageUp: -5,
      PageDown: 5,
      Home: -cards.length,
      End: cards.length,
    };
    if (e.key in moves) {
      e.preventDefault();
      step(moves[e.key]);
    } else if ((e.key === "Enter" || e.key === " ") && focus) {
      e.preventDefault();
      apply(focus);
    }
  }

  /** Cover-flow placement for a card `d` steps from the centre. */
  function place(d: number) {
    if (d === 0) return "none";
    const a = Math.abs(d);
    const s = Math.sign(d);
    return `translateX(${s * (230 + (a - 1) * 84)}px) translateZ(${-120 - (a - 1) * 20}px) rotateY(${s * 55}deg)`;
  }
</script>

<div class="wallpaper">
  <div class="top">
    <div class="sources" role="group" aria-label="Show wallpapers from">
      <button class="chip" aria-pressed={source === "folder"} onclick={() => setSource("folder")}>
        <Icon name="images" size={15} />{folder?.dir ? baseName(folder.dir) : "Folder"}
      </button>
      {#if engine?.installed}
        <button class="chip" aria-pressed={source === "engine"} onclick={() => setSource("engine")}>
          <Icon name="monitor-play" size={15} />Wallpaper Engine
        </button>
      {/if}
    </div>

    <p class="note meta">
      {#if source === "engine" && engine && !engine.running}
        <Icon name="info" size={14} />Wallpaper Engine is closed. Choosing a wallpaper starts it.
      {:else if source === "folder" && engine?.running}
        <Icon name="info" size={14} />Wallpaper Engine is running and covers pictures set here.
      {/if}
    </p>

    {#if loaded && cards.length}
      <span class="meta num count">{cards.length} {source === "engine" ? "wallpapers" : "pictures"}</span>
    {/if}
    {#if source === "folder"}
      <button class="btn small" onclick={pickFolder}><Icon name="folder-open" size={14} />Change folder</button>
    {/if}
  </div>

  {#if !loaded}
    <div class="reel" aria-busy="true" aria-label="Reading wallpapers" role="status">
      {#each [-2, -1, 0, 1, 2] as d (d)}
        <span class="card ghost" style:transform={place(d)} style:z-index={50 - Math.abs(d)}></span>
      {/each}
    </div>
  {:else if !cards.length}
    <div class="empty">
      {#if loadError}
        <strong>Could not read {source === "engine" ? "Wallpaper Engine's wallpapers" : "the folder"}</strong>
        <span>{loadError}.</span>
        {#if source === "folder"}<button class="btn small" onclick={pickFolder}>Choose another folder</button>{/if}
      {:else if source === "engine"}
        <strong>No Wallpaper Engine wallpapers yet</strong>
        <span>Subscribe to a few in its Workshop and they show up here.</span>
      {:else}
        <strong>No pictures in {folder?.dir ? baseName(folder.dir) : "this folder"}</strong>
        <span>Choose a folder with JPG, PNG, BMP or WebP pictures.</span>
        <button class="btn primary small" onclick={pickFolder}><Icon name="folder-open" size={14} />Choose folder</button>
      {/if}
    </div>
  {:else}
    <div
      class="reel"
      use:wheel
      role="listbox"
      tabindex="0"
      aria-label={source === "engine" ? "Wallpaper Engine wallpapers" : "Pictures"}
      aria-orientation="horizontal"
      aria-activedescendant="wp-{centre}"
      onkeydown={reelKey}
    >
      {#each shown as { c, i } (c.id)}
        {@const d = i - centre}
        <!-- The reel takes the keys; cards take clicks. -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="card"
          class:centre={d === 0}
          class:busy={applying === c.id}
          id="wp-{i}"
          role="option"
          tabindex="-1"
          aria-selected={d === 0}
          aria-label="{c.title}{inUse(c) ? ', in use' : ''}"
          style:transform={place(d)}
          style:z-index={50 - Math.abs(d)}
          onclick={() => (d === 0 ? apply(c) : (centre = i))}
        >
          {#if c.thumb}
            <img src={fileUrl(c.thumb)} alt="" draggable="false" />
          {:else}
            <span class="blank"><Icon name="image" size={28} /></span>
          {/if}
          {#if inUse(c)}
            <span class="in-use"><Icon name="check" size={12} />In use</span>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  <div class="caption">
    {#if error}
      <p class="error" role="alert">{error}</p>
    {:else if focus && loaded}
      <div class="what">
        <strong class="ellipsis">{focus.title}</strong>
        <span class="meta ellipsis">{focus.detail}</span>
      </div>
    {/if}
    {#if focus && loaded}
      <div class="nav">
        <button class="icon-btn" aria-label="Previous" onclick={() => step(-1)} disabled={centre === 0}>
          <Icon name="caret-left" size={16} />
        </button>
        <span class="meta num" aria-hidden="true">{centre + 1} / {cards.length}</span>
        <button class="icon-btn" aria-label="Next" onclick={() => step(1)} disabled={centre >= cards.length - 1}>
          <Icon name="caret-right" size={16} />
        </button>
      </div>
      {#if inUse(focus)}
        <span class="btn using" aria-live="polite"><Icon name="check-circle-fill" size={16} />In use</span>
      {:else}
        <button class="btn primary" onclick={() => focus && apply(focus)} disabled={!!applying}>
          {#if applying === focus.id}
            <span class="spin"><Icon name="circle-notch" size={16} /></span>Applying…
          {:else}
            <Icon name="image" size={16} />Set as wallpaper
          {/if}
        </button>
      {/if}
    {/if}
  </div>
</div>

<style>
  .wallpaper {
    height: 100%;
    display: grid;
    grid-template-rows: 32px 1fr 40px;
    gap: 8px;
  }

  .top {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .sources {
    display: flex;
    gap: 6px;
  }
  .note {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .count {
    color: var(--text-3);
  }

  /* ---- the reel ---- */

  .reel {
    position: relative;
    min-height: 0;
    perspective: 1100px;
    border-radius: 18px;
    /* The far cards fade into the panel instead of being cut. */
    mask-image: linear-gradient(90deg, transparent, #000 13%, #000 87%, transparent);
  }
  .reel:focus-visible {
    outline: none;
  }
  .reel:focus-visible .card.centre {
    outline: 2px solid var(--accent);
    outline-offset: 3px;
  }
  .card {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 288px;
    height: 162px;
    margin: -81px 0 0 -144px;
    border-radius: 14px;
    overflow: hidden;
    background: var(--s2);
    box-shadow:
      0 14px 30px -14px rgb(0 0 0 / 0.75),
      inset 0 0 0 1px var(--line);
    transition:
      transform var(--t-move) var(--spring),
      scale var(--t-pop) var(--spring-pop);
  }
  /* Side cards sit in shade; the centre one is lit. */
  .card::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: inherit;
    background: rgb(0 0 0 / 0.42);
    transition: opacity var(--t-move) var(--out);
    pointer-events: none;
  }
  .card.centre::after {
    opacity: 0;
  }
  .card.centre:hover {
    scale: 1.025;
  }
  .card.centre:active {
    scale: 0.985;
  }
  .card.busy {
    animation: breathe 1.1s ease-in-out infinite;
  }
  .card img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .blank {
    width: 100%;
    height: 100%;
    display: grid;
    place-items: center;
    color: var(--text-3);
  }
  .in-use {
    position: absolute;
    top: 8px;
    left: 8px;
    z-index: 1;
    height: 22px;
    padding: 0 8px 0 6px;
    border-radius: 11px;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11.5px;
    font-weight: 600;
    background: var(--accent);
    color: var(--ink);
    box-shadow: 0 2px 8px -2px rgb(0 0 0 / 0.5);
  }
  .ghost {
    animation: pulse 1.4s ease-in-out infinite;
    box-shadow: none;
  }
  .ghost::after {
    display: none;
  }

  /* ---- caption ---- */

  .caption {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 0 4px;
  }
  .what {
    flex: 1;
    min-width: 0;
    display: grid;
  }
  .what strong {
    font-weight: 600;
  }
  .caption .error {
    flex: 1;
    min-width: 0;
  }
  .nav {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .nav .meta {
    min-width: 64px;
    text-align: center;
  }
  .using {
    background: none;
    color: var(--accent-text);
    box-shadow: inset 0 0 0 1px var(--accent-35);
  }
  .spin {
    display: grid;
    animation: spin 0.8s linear infinite;
  }

  .empty {
    min-height: 0;
  }

  @keyframes pulse {
    50% {
      opacity: 0.5;
    }
  }
  @keyframes breathe {
    50% {
      opacity: 0.7;
    }
  }
  @keyframes spin {
    to {
      rotate: 1turn;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ghost,
    .card.busy,
    .spin {
      animation: none;
    }
  }
  @container page (max-width: 820px) {
    .wallpaper {
      grid-template-rows: auto minmax(0, 1fr) 40px;
    }
    .top {
      flex-wrap: wrap;
      row-gap: 6px;
    }
    .note {
      order: 3;
      flex-basis: 100%;
    }
  }
</style>
