<script lang="ts">
  // Apps: find an installed app and open it. Type to search; star the ones used most
  // and they stay on top, with the last few opened next to them. The same search finds
  // files and folders by name on the whole computer, from Nimbus's own index.
  import { onMount, tick } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { api, fileUrl, reason, type AppEntry, type FileHit, type FilesStatus } from "../lib/api";
  import type { IconName } from "../lib/icons";
  import Icon from "../lib/Icon.svelte";
  import Kbd from "../lib/Kbd.svelte";
  import { busy, closePanel, typing } from "../lib/ui.svelte";

  const RECENT = 6;
  const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

  let apps = $state<AppEntry[] | null>(null);
  let loadError = $state("");
  let query = $state("");
  let active = $state(0);
  let starred = $state<string[]>([]);
  let recent = $state<string[]>([]);
  let broken = $state<Record<string, boolean>>({});
  let opening = $state<string | null>(null);
  let error = $state("");
  let search: HTMLInputElement | undefined = $state();

  // Files: asked for on every key press; an answer to an older query is dropped.
  let files = $state<FileHit[]>([]);
  let filesFor = $state("");
  let filesState = $state<FilesStatus | null>(null);
  let asked = 0;

  async function load() {
    loadError = "";
    try {
      apps = (await api.appsList()).sort((a, b) => collator.compare(a.name, b.name));
    } catch (e) {
      loadError = reason(e);
      apps ??= [];
    }
  }

  onMount(() => {
    api
      .storeGet<string[]>("apps-starred")
      .then((s) => (starred = s ?? []))
      .catch(() => {});
    api
      .storeGet<string[]>("apps-recent")
      .then((r) => (recent = r ?? []))
      .catch(() => {});
    let retry: ReturnType<typeof setTimeout> | undefined;
    load().then(() => {
      // The very first read can finish before the system has listed anything.
      if (apps && !apps.length && !loadError) retry = setTimeout(load, 2500);
    });
    api
      .filesStatus()
      .then((s) => (filesState = s))
      .catch(() => {});
    const off = listen<FilesStatus>("files", (e) => {
      // A fresh status also asks again (below), as the index changed under the search.
      filesState = e.payload;
    });
    return () => {
      clearTimeout(retry);
      off.then((f) => f());
    };
  });

  async function findFiles(text: string) {
    const mine = ++asked;
    if (!text || !filesState?.on) {
      files = [];
      filesFor = text;
      return;
    }
    try {
      const hits = await api.filesSearch(text, 40);
      if (mine !== asked) return;
      files = hits;
      filesFor = text;
      if (active >= results.length + hits.length) active = 0;
    } catch {
      if (mine === asked) files = [];
    }
  }
  $effect(() => {
    findFiles(q);
  });

  let byId = $derived(new Map((apps ?? []).map((a) => [a.id, a])));
  let starredApps = $derived(starred.map((id) => byId.get(id)).filter((a): a is AppEntry => !!a));
  let recentApps = $derived(
    recent
      .filter((id) => !starred.includes(id))
      .map((id) => byId.get(id))
      .filter((a): a is AppEntry => !!a)
      .slice(0, RECENT),
  );

  // All apps under their first letter; digits and symbols go under "#".
  let groups = $derived.by(() => {
    const out: { letter: string; apps: AppEntry[] }[] = [];
    for (const a of apps ?? []) {
      const first = a.name.trim().charAt(0).toLocaleUpperCase().normalize("NFD").charAt(0);
      const letter = /\p{L}/u.test(first) ? first : "#";
      const last = out.at(-1);
      if (last?.letter === letter) last.apps.push(a);
      else out.push({ letter, apps: [a] });
    }
    return out;
  });

  // Best match first: the name starts with it, then a word does, then the initials
  // ("vsc" finds Visual Studio Code), then anywhere in the name.
  function score(name: string, q: string): number {
    const n = name.toLowerCase();
    if (n.startsWith(q)) return 0;
    const words = n.split(/[\s\-_.()]+/).filter(Boolean);
    if (words.some((w) => w.startsWith(q))) return 1;
    if (q.length > 1 && words.map((w) => w[0]).join("").startsWith(q)) return 2;
    return n.includes(q) ? 3 : -1;
  }
  const rank = (a: AppEntry) => (starred.includes(a.id) ? 2 : recent.includes(a.id) ? 1 : 0);

  let q = $derived(query.trim().toLowerCase());
  let results = $derived.by(() => {
    if (!q || !apps) return [];
    return apps
      .map((a) => ({ a, s: score(a.name, q) }))
      .filter((r) => r.s >= 0)
      .sort((x, y) => x.s - y.s || rank(y.a) - rank(x.a) || collator.compare(x.a.name, y.a.name))
      .slice(0, 40)
      .map((r) => r.a);
  });
  // Apps first, then files: one list for the arrow keys.
  let total = $derived(results.length + files.length);

  const KINDS: [RegExp, IconName][] = [
    [/\.pdf$/, "file-pdf"],
    [/\.(png|jpe?g|gif|webp|bmp|svg|heic|avif|tiff?|ico|raw|cr2|nef|arw|psd)$/, "file-image"],
    [/\.(mp4|mkv|mov|avi|webm|wmv|m4v|flv)$/, "file-video"],
    [/\.(mp3|wav|flac|ogg|m4a|aac|opus|wma)$/, "file-audio"],
    [/\.(zip|7z|rar|tar|gz|bz2|xz|zst|iso|cab|msi|dmg)$/, "file-zip"],
    [/\.(docx?|odt|rtf|pages)$/, "file-doc"],
    [/\.(xlsx?|ods|csv|numbers)$/, "file-xls"],
    [/\.(pptx?|odp|key)$/, "file-ppt"],
    [/\.(rs|ts|tsx|js|jsx|mjs|py|c|cc|cpp|h|hpp|cs|go|java|kt|swift|rb|php|sh|ps1|bat|lua|svelte|vue|html?|css|json|toml|ya?ml|xml|sql|il|scs|ocn|va|sv|v)$/, "file-code"],
    [/\.(txt|md|log|ini|cfg|conf|tex)$/, "file-text"],
  ];
  function kind(f: FileHit): IconName {
    if (f.dir) return "folder-simple";
    const n = f.name.toLowerCase();
    return KINDS.find(([re]) => re.test(n))?.[1] ?? "file";
  }

  const SIZE = ["bytes", "KB", "MB", "GB", "TB"];
  function size(n: number): string {
    let i = 0;
    let v = n;
    while (v >= 1024 && i < SIZE.length - 1) {
      v /= 1024;
      i++;
    }
    return i === 0 ? `${n} ${n === 1 ? "byte" : "bytes"}` : `${v < 10 ? v.toFixed(1) : Math.round(v)} ${SIZE[i]}`;
  }
  const dayFormat = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", year: "numeric" });
  const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
  function when(ms: number): string {
    const d = new Date(ms);
    return d.toDateString() === new Date().toDateString() ? timeFormat.format(d) : dayFormat.format(d);
  }
  // Long folder paths keep their end, where the useful part is.
  function shortFolder(f: string): string {
    return f.length > 64 ? `…${f.slice(-62)}` : f;
  }
  const big = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)} million` : n.toLocaleString());

  async function openFile(f: FileHit, reveal = false) {
    error = "";
    try {
      await busy(reveal ? api.fileReveal(f.path) : api.fileOpen(f.path));
      closePanel();
    } catch (e) {
      error = `${f.name}: ${reason(e)}`;
    }
  }

  async function launch(a: AppEntry) {
    if (opening) return;
    error = "";
    opening = a.id;
    try {
      await busy(api.appLaunch(a.id));
      recent = [a.id, ...recent.filter((id) => id !== a.id)].slice(0, 12);
      api.storeSet("apps-recent", $state.snapshot(recent)).catch(() => {});
      closePanel();
    } catch (e) {
      error = `${a.name} did not open: ${reason(e)}. Try again, or open it the usual way.`;
    } finally {
      opening = null;
    }
  }

  function toggleStar(a: AppEntry) {
    starred = starred.includes(a.id) ? starred.filter((id) => id !== a.id) : [...starred, a.id];
    api.storeSet("apps-starred", $state.snapshot(starred)).catch(() => {});
  }

  function searchKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (!total) return;
      e.preventDefault();
      active = (active + (e.key === "ArrowDown" ? 1 : -1) + total) % total;
      tick().then(() => document.getElementById(`hit-${active}`)?.scrollIntoView({ block: "nearest" }));
    } else if (e.key === "Enter") {
      const a = results[active];
      const f = files[active - results.length];
      if (a) {
        e.preventDefault();
        launch(a);
      } else if (f) {
        e.preventDefault();
        // Ctrl+Enter (Cmd+Enter on a Mac) shows it in its folder instead.
        openFile(f, e.ctrlKey || e.metaKey);
      }
    } else if (e.key === "Escape" && query) {
      // The first Escape clears the search; the next one folds the panel.
      e.preventDefault();
      query = "";
    }
  }

  // With the keyboard anywhere on this page, typing a letter starts a search.
  function typeToSearch(e: KeyboardEvent) {
    if (e.key.length !== 1 || e.key === " " || e.ctrlKey || e.altKey || e.metaKey) return;
    if (e.target instanceof HTMLInputElement || !search) return;
    search.focus();
  }
</script>

<svelte:window onkeydown={typeToSearch} />

{#snippet appIcon(a: AppEntry, size: number)}
  {#if a.icon && !broken[a.id]}
    <img
      class="app-icon"
      style:--size="{size}px"
      src={fileUrl(a.icon)}
      alt=""
      width={size}
      height={size}
      loading="lazy"
      onerror={() => (broken[a.id] = true)}
    />
  {:else}
    <span class="app-icon letter display" style:--size="{size}px" aria-hidden="true">
      {a.name.trim().charAt(0).toUpperCase() || "?"}
    </span>
  {/if}
{/snippet}

{#snippet tile(a: AppEntry, starrable: boolean)}
  <div class="tile-wrap">
    <button class="tile" onclick={() => launch(a)} aria-busy={opening === a.id} title={a.name}>
      {@render appIcon(a, 40)}
      <span class="tile-name ellipsis">{a.name}</span>
    </button>
    {#if starrable}
      <button class="unstar icon-btn" aria-label="Remove {a.name} from starred" onclick={() => toggleStar(a)}>
        <Icon name="x" size={12} />
      </button>
    {/if}
  </div>
{/snippet}

<div class="apps">
  <div class="field search">
    <Icon name="magnifying-glass" size={16} />
    <input
      bind:this={search}
      bind:value={query}
      use:typing
      oninput={() => (active = 0)}
      onkeydown={searchKey}
      placeholder={filesState?.on ? "Search apps and files" : "Search apps"}
      aria-label={filesState?.on ? "Search apps and files" : "Search apps"}
      role="combobox"
      aria-expanded={!!q}
      aria-controls="app-results"
      aria-autocomplete="list"
      aria-activedescendant={q && total ? `hit-${active}` : undefined}
      spellcheck="false"
      autocomplete="off"
    />
    {#if q && (results.length || files.length)}
      <span class="count meta num">{results.length} {results.length === 1 ? "app" : "apps"}{filesState?.on ? ` · ${files.length}${files.length === 40 ? "+" : ""} ${files.length === 1 ? "file" : "files"}` : ""}</span>
    {:else if apps?.length}
      <span class="count meta num">{apps.length} apps{filesState?.on && filesState.files ? ` · ${big(filesState.files)} files` : ""}</span>
    {/if}
  </div>

  <div class="scroll">
    {#if q}
      {#if total}
        <ul class="results" id="app-results" role="listbox" aria-label="Matching apps and files">
          {#each results as a, i (a.id)}
            <!-- The search field handles the keys; rows take clicks. -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <li
              id="hit-{i}"
              role="option"
              aria-selected={i === active}
              class:active={i === active}
              onclick={() => launch(a)}
              onpointermove={() => (active = i)}
            >
              {@render appIcon(a, 28)}
              <span class="name ellipsis">{a.name}</span>
              {#if starred.includes(a.id)}<span class="star-mark"><Icon name="star-fill" size={13} /><span class="sr-only">Starred</span></span>{/if}
              <span class="go meta" aria-hidden="true">{opening === a.id ? "Opening…" : "Open"}<Kbd k="enter" /></span>
            </li>
          {/each}
          {#if files.length}
            <li class="section" role="presentation">
              <h2>Files</h2>
            </li>
          {/if}
          {#each files as f, j (f.path)}
            {@const i = results.length + j}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <li
              id="hit-{i}"
              role="option"
              aria-selected={i === active}
              class="file"
              class:active={i === active}
              onclick={() => openFile(f)}
              onpointermove={() => (active = i)}
              title={f.path}
            >
              <span class="kind" class:dir={f.dir}><Icon name={kind(f)} size={18} /></span>
              <span class="what">
                <span class="name ellipsis">{f.name}</span>
                <span class="where meta ellipsis">{shortFolder(f.folder)}</span>
              </span>
              <span class="facts meta num">
                {#if f.size !== null}<span>{size(f.size)}</span>{/if}
                {#if f.modified}<span>{when(f.modified)}</span>{/if}
              </span>
              <button
                class="reveal icon-btn"
                aria-label="Show {f.name} in its folder"
                title="Show in folder (Ctrl+Enter)"
                onclick={(e) => {
                  e.stopPropagation();
                  openFile(f, true);
                }}
              >
                <Icon name="folder-open" size={15} />
              </button>
            </li>
          {/each}
        </ul>
        {#if filesState?.on && !filesState.files && filesState.scanning}
          <p class="note meta" role="status"><span class="spin"><Icon name="circle-notch" size={14} /></span>Reading the drives for file search. Files show up here in a minute.</p>
        {/if}
      {:else if filesState?.on && !filesState.files && filesState.scanning}
        <div class="empty" role="status">
          <span class="badge"><span class="spin"><Icon name="circle-notch" size={26} /></span></span>
          <strong>Reading the drives</strong>
          <span>The first time takes about a minute. After that, files show up as you type.</span>
        </div>
      {:else if filesFor !== q && filesState?.on}
        <div class="empty" aria-hidden="true"></div>
      {:else}
        <div class="empty">
          <strong>Nothing called “{query.trim()}”</strong>
          <span>{filesState?.on ? "No app, file or folder has that in its name. Check the spelling, or try part of the name." : "Check the spelling, or type the first word of its name."}</span>
        </div>
      {/if}
    {:else if !apps}
      <div class="ghosts" aria-label="Reading the installed apps" role="status">
        <div class="tiles">
          {#each Array(6) as _, i (i)}<span class="ghost tile-ghost" style:--i={i}></span>{/each}
        </div>
        <div class="ghost-rows">
          {#each Array(10) as _, i (i)}<span class="ghost row-ghost" style:--i={i}></span>{/each}
        </div>
      </div>
    {:else if !apps.length}
      <div class="empty">
        <span class="badge"><Icon name="squares-four" size={26} /></span>
        <strong>{loadError ? "Could not read the installed apps" : "No apps found yet"}</strong>
        <span>{loadError ? `${loadError}.` : "The list is still being read. Try again in a moment."}</span>
        <button class="btn small" onclick={load}><Icon name="arrow-clockwise" size={14} />Try again</button>
      </div>
    {:else}
      <section aria-labelledby="starred-title">
        <h2 id="starred-title">Starred</h2>
        {#if starredApps.length}
          <div class="tiles">
            {#each starredApps as a (a.id)}{@render tile(a, true)}{/each}
          </div>
        {:else}
          <p class="hint meta"><Icon name="star" size={14} />Star the apps you use most and they wait here.</p>
        {/if}
      </section>

      {#if recentApps.length}
        <section aria-labelledby="recent-title">
          <h2 id="recent-title">Recent</h2>
          <div class="tiles">
            {#each recentApps as a (a.id)}{@render tile(a, false)}{/each}
          </div>
        </section>
      {/if}

      <section aria-labelledby="all-title">
        <h2 id="all-title">All apps</h2>
        {#each groups as g (g.letter)}
          <div class="group" role="group" aria-label={g.letter === "#" ? "Numbers and symbols" : g.letter}>
            <span class="letter-head display" aria-hidden="true">{g.letter}</span>
            <div class="rows">
              {#each g.apps as a (a.id)}
                {@const on = starred.includes(a.id)}
                <div class="row">
                  <button class="open" onclick={() => launch(a)} aria-busy={opening === a.id}>
                    {@render appIcon(a, 26)}
                    <span class="name ellipsis">{a.name}</span>
                  </button>
                  <button
                    class="star icon-btn"
                    class:on
                    aria-pressed={on}
                    aria-label="Star {a.name}"
                    onclick={() => toggleStar(a)}
                  >
                    <Icon name={on ? "star-fill" : "star"} size={14} />
                  </button>
                </div>
              {/each}
            </div>
          </div>
        {/each}
      </section>
    {/if}
  </div>

  {#if error}<p class="error foot" role="alert">{error}</p>{/if}
</div>

<style>
  .apps {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .count {
    flex: none;
    color: var(--text-3);
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin-right: -8px;
    padding-right: 8px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }

  h2 {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
    margin: 0 4px 8px;
  }

  .app-icon {
    width: var(--size, auto);
    height: var(--size, auto);
    flex: none;
    object-fit: contain;
  }
  .app-icon.letter {
    display: grid;
    place-items: center;
    border-radius: calc(var(--size) * 0.28);
    background: var(--s3);
    color: var(--text-2);
    font-size: calc(var(--size) * 0.46);
    line-height: 1;
  }

  /* ---- starred and recent ---- */

  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(92px, 1fr));
    gap: 8px;
  }
  .tile-wrap {
    position: relative;
    min-width: 0;
  }
  .tile {
    width: 100%;
    height: 86px;
    padding: 12px 8px 10px;
    border-radius: 16px;
    display: grid;
    justify-items: center;
    align-content: center;
    gap: 8px;
    background: var(--s1);
    transition:
      background-color var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }
  .tile:hover {
    background: var(--s2);
  }
  .tile:active {
    scale: 0.95;
  }
  .tile .app-icon {
    transition: translate var(--t-pop) var(--spring-pop);
  }
  .tile:hover .app-icon {
    translate: 0 -2px;
  }
  .tile-name {
    max-width: 100%;
    font-size: 12px;
    color: var(--text-2);
  }
  .tile:hover .tile-name {
    color: var(--text);
  }
  .unstar {
    position: absolute;
    top: 4px;
    right: 4px;
    width: 22px;
    height: 22px;
    background: var(--s3);
    opacity: 0;
    transition: opacity var(--t-fade) var(--out);
  }
  .tile-wrap:hover .unstar,
  .unstar:focus-visible {
    opacity: 1;
  }
  .hint {
    height: 44px;
    padding: 0 14px;
    border-radius: 14px;
    display: flex;
    align-items: center;
    gap: 8px;
    box-shadow: inset 0 0 0 1px var(--line);
  }

  /* ---- all apps ---- */

  .group {
    display: grid;
    grid-template-columns: 28px 1fr;
    gap: 4px;
  }
  .group + .group {
    margin-top: 6px;
  }
  .letter-head {
    padding-top: 9px;
    text-align: center;
    font-size: 13px;
    color: var(--text-3);
  }
  .rows {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 2px 8px;
  }
  .row {
    position: relative;
    min-width: 0;
    border-radius: 12px;
    transition: background-color var(--t-fade) var(--out);
  }
  .row:hover {
    background: var(--s1);
  }
  .open {
    width: 100%;
    height: 40px;
    padding: 0 40px 0 8px;
    border-radius: 12px;
    display: flex;
    align-items: center;
    gap: 10px;
    text-align: left;
  }
  .name {
    min-width: 0;
  }
  .star {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 28px;
    height: 28px;
    opacity: 0;
  }
  .star.on {
    color: var(--accent-text);
    opacity: 1;
  }
  .row:hover .star,
  .star:focus-visible {
    opacity: 1;
  }

  /* ---- search results ---- */

  .results {
    list-style: none;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  .results li {
    height: 44px;
    padding: 0 12px 0 10px;
    border-radius: 12px;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .results li.active {
    background: var(--accent-12);
  }
  .results .name {
    flex: 1;
  }
  .star-mark {
    display: grid;
    color: var(--accent-text);
  }
  .go {
    display: flex;
    align-items: center;
    gap: 6px;
    opacity: 0;
  }
  .active .go {
    opacity: 1;
  }

  /* ---- files ---- */

  .results li.section {
    height: auto;
    padding: 12px 0 0;
  }
  .section h2 {
    margin-bottom: 4px;
  }
  .results li.file {
    height: 50px;
    padding-right: 6px;
  }
  .kind {
    width: 28px;
    height: 28px;
    flex: none;
    display: grid;
    place-items: center;
    border-radius: 8px;
    background: var(--s2);
    color: var(--text-2);
  }
  .kind.dir {
    color: var(--accent-text);
  }
  .what {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 1px;
  }
  .where {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .facts {
    flex: none;
    display: grid;
    justify-items: end;
    gap: 1px;
    font-size: 11.5px;
    color: var(--text-3);
  }
  .reveal {
    width: 30px;
    height: 30px;
    flex: none;
    opacity: 0;
  }
  .file.active .reveal,
  .reveal:focus-visible {
    opacity: 1;
  }
  .note {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 12px;
  }
  .spin {
    display: grid;
    color: var(--accent-text);
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      rotate: 1turn;
    }
  }

  /* ---- loading and empty ---- */

  .ghosts {
    display: grid;
    gap: 26px;
    padding-top: 26px;
  }
  .ghost {
    border-radius: 14px;
    background: var(--s1);
    animation: pulse 1.4s ease-in-out calc(var(--i) * 60ms) infinite;
  }
  .tile-ghost {
    height: 86px;
  }
  .ghost-rows {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    padding-left: 32px;
  }
  .row-ghost {
    height: 32px;
  }
  .empty {
    flex: 1;
  }
  .badge {
    width: 56px;
    height: 56px;
    margin-bottom: 4px;
    border-radius: 18px;
    display: grid;
    place-items: center;
    background: var(--s2);
    color: var(--accent-text);
  }
  .foot {
    padding: 0 4px;
  }

  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ghost,
    .spin {
      animation: none;
    }
  }
  @container page (max-width: 460px) {
    .facts {
      display: none;
    }
  }
  @container page (max-width: 540px) {
    .rows,
    .ghost-rows {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
