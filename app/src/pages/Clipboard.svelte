<script lang="ts">
  // Clipboard: what was copied lately, newest first. It stays on this computer, and
  // copies from password managers are never saved (see clip.rs).
  import { tick } from "svelte";
  import { ago, api, appLabel, fileUrl, reason, type ClipItem, type ClipKind, type ClipView } from "../lib/api";
  import { time } from "../lib/clock.svelte";
  import Icon from "../lib/Icon.svelte";
  import type { IconName } from "../lib/icons";
  import Kbd from "../lib/Kbd.svelte";
  import { live } from "../lib/live.svelte";
  import Switch from "../lib/Switch.svelte";
  import { closePanel, typing, ui } from "../lib/ui.svelte";

  type Filter = "all" | ClipKind;
  const FILTERS: { id: Filter; label: string }[] = [
    { id: "all", label: "All" },
    { id: "text", label: "Text" },
    { id: "link", label: "Links" },
    { id: "image", label: "Images" },
  ];
  const KIND_ICON: Record<ClipKind, IconName> = { text: "text-t", link: "link", image: "image" };
  const KIND_NAME: Record<ClipKind, string> = { text: "Text", link: "Link", image: "Image" };
  const count = new Intl.NumberFormat();

  let view = $state<ClipView | null>(null);
  let loadError = $state("");
  let query = $state("");
  let filter = $state<Filter>("all");
  let selected = $state<string | null>(null);
  let confirmClear = $state(false);
  let copied = $state<string | null>(null);
  let error = $state("");
  let clearTimer: ReturnType<typeof setTimeout> | undefined;
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  async function load() {
    try {
      view = await api.clipboardList();
      loadError = "";
    } catch (e) {
      loadError = reason(e);
    }
  }

  // Read now, and again whenever the history changes.
  $effect(() => {
    void live.clipVersion;
    load();
  });

  let items = $derived.by(() => {
    if (!view) return [];
    const q = query.trim().toLowerCase();
    return view.items.filter(
      (it) =>
        (filter === "all" || it.kind === filter) &&
        (!q || (it.text ?? "").toLowerCase().includes(q) || (it.app ?? "").toLowerCase().includes(q)),
    );
  });

  let sections = $derived.by(() => {
    const today = new Date(time.now);
    today.setHours(0, 0, 0, 0);
    const start = today.getTime();
    const groups: { label: string; items: ClipItem[] }[] = [
      { label: "Pinned", items: [] },
      { label: "Today", items: [] },
      { label: "Yesterday", items: [] },
      { label: "Earlier", items: [] },
    ];
    for (const it of items) {
      groups[it.pinned ? 0 : it.at >= start ? 1 : it.at >= start - 86_400_000 ? 2 : 3].items.push(it);
    }
    return groups.filter((g) => g.items.length);
  });

  let flat = $derived(sections.flatMap((s) => s.items));
  let current = $derived(flat.find((it) => it.id === selected) ?? flat[0] ?? null);
  let unpinned = $derived(view ? view.items.filter((it) => !it.pinned).length : 0);
  let keys = $derived(ui.keyboard || ui.typing > 0);

  function firstLine(it: ClipItem) {
    if (it.kind === "image") return it.size ? `Image, ${it.size[0]} × ${it.size[1]}` : "Image";
    const line = (it.text ?? "").split(/\r?\n/).find((l) => l.trim()) ?? "";
    return line.replace(/\s+/g, " ").trim().slice(0, 160) || "Blank text";
  }

  function host(url: string | undefined) {
    try {
      return new URL(url ?? "").host;
    } catch {
      return "";
    }
  }

  function detail(it: ClipItem) {
    const parts: string[] = [KIND_NAME[it.kind]];
    if (it.kind === "text") parts.push(`${count.format(it.chars)} ${it.chars === 1 ? "character" : "characters"}`);
    if (it.kind === "image" && it.size) parts.push(`${it.size[0]} × ${it.size[1]}`);
    if (it.app) parts.push(`from ${appLabel(it.app)}`);
    parts.push(ago(it.at, time.now.getTime()));
    return parts.join(" · ");
  }

  function select(id: string) {
    selected = id;
    tick().then(() => document.getElementById(`clip-${id}`)?.scrollIntoView({ block: "nearest" }));
  }

  async function paste(it: ClipItem) {
    error = "";
    try {
      await api.clipboardPaste(it.id);
      closePanel();
    } catch (e) {
      error = `Could not paste: ${reason(e)}. Use Copy, then paste it yourself.`;
    }
  }

  async function copy(it: ClipItem) {
    error = "";
    try {
      await api.clipboardCopy(it.id);
      copied = it.id;
      clearTimeout(copiedTimer);
      copiedTimer = setTimeout(() => (copied = null), 1400);
    } catch (e) {
      error = `Could not copy: ${reason(e)}. Try again.`;
    }
  }

  async function pin(it: ClipItem) {
    error = "";
    try {
      await api.clipboardPin(it.id, !it.pinned);
      await load();
    } catch (e) {
      error = `Could not ${it.pinned ? "unpin" : "pin"} it: ${reason(e)}. Try again.`;
    }
  }

  async function remove(it: ClipItem) {
    error = "";
    const i = flat.findIndex((x) => x.id === it.id);
    const next = flat[i + 1] ?? flat[i - 1] ?? null;
    try {
      await api.clipboardDelete(it.id);
      selected = next?.id ?? null;
      await load();
    } catch (e) {
      error = `Could not remove it: ${reason(e)}. Try again.`;
    }
  }

  // Two steps: the first click asks, the second clears. Pinned copies stay.
  async function clear() {
    if (!confirmClear) {
      confirmClear = true;
      clearTimeout(clearTimer);
      clearTimer = setTimeout(() => (confirmClear = false), 4000);
      return;
    }
    confirmClear = false;
    clearTimeout(clearTimer);
    try {
      await api.clipboardClear();
      await load();
    } catch (e) {
      error = `Could not clear the history: ${reason(e)}. Try again.`;
    }
  }

  async function enable(on: boolean) {
    error = "";
    try {
      await api.clipboardEnable(on);
      await load();
    } catch (e) {
      error = `Could not ${on ? "turn on" : "pause"} the history: ${reason(e)}.`;
    }
  }

  function key(e: KeyboardEvent) {
    const inField = e.target instanceof HTMLInputElement;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (!flat.length) return;
      e.preventDefault();
      const i = current ? flat.findIndex((x) => x.id === current.id) : -1;
      select(flat[Math.max(0, Math.min(flat.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))].id);
    } else if (e.key === "Enter") {
      if (current && !(e.target instanceof HTMLButtonElement)) {
        e.preventDefault();
        paste(current);
      }
    } else if (e.key === "Escape") {
      // A search in progress is cleared first; the next Escape folds the panel.
      if (inField && query) {
        e.preventDefault();
        query = "";
      }
    } else if (!inField && current) {
      const k = e.key.toLowerCase();
      if ((e.ctrlKey || e.metaKey) && k === "c" && !window.getSelection()?.toString()) {
        e.preventDefault();
        copy(current);
      } else if (k === "p" && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        pin(current);
      } else if (e.key === "Delete") {
        e.preventDefault();
        remove(current);
      }
    }
  }
</script>

<!-- Keys work anywhere on the page, so they are caught here before the island's own. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="clip" onkeydown={key}>
  <div class="top">
    <div class="field search">
      <Icon name="magnifying-glass" size={16} />
      <input
        bind:value={query}
        use:typing
        placeholder="Search copies"
        aria-label="Search the clipboard history"
        aria-controls="clips"
        spellcheck="false"
        autocomplete="off"
      />
    </div>
    <div class="filters" role="group" aria-label="Show">
      {#each FILTERS as f (f.id)}
        <button class="chip" aria-pressed={filter === f.id} onclick={() => (filter = f.id)}>{f.label}</button>
      {/each}
    </div>
    <span class="spacer"></span>
    <div class="save">
      <span class="meta" aria-hidden="true">Save copies</span>
      <Switch on={!!view?.enabled} label="Save copies to the history" disabled={!view} onchange={enable} />
    </div>
    <button class="btn small" class:danger={confirmClear} onclick={clear} disabled={!unpinned}>
      <Icon name="broom" size={14} />
      {confirmClear ? `Clear ${unpinned} ${unpinned === 1 ? "copy" : "copies"}` : "Clear"}
    </button>
  </div>

  <div class="main" class:split={!!current}>
    {#if !view}
      {#if loadError}
        <div class="empty">
          <strong>Could not read the history</strong>
          <span>{loadError}</span>
          <button class="btn small" onclick={load}><Icon name="arrow-clockwise" size={14} />Try again</button>
        </div>
      {/if}
    {:else if !view.enabled && !view.items.length}
      <div class="empty">
        <span class="badge"><Icon name="clipboard-text" size={26} /></span>
        <strong>Clipboard history is off</strong>
        <span>
          Turn it on to keep the last 200 things you copy. It stays on this computer{view.guards_secrets
            ? ", and passwords from password managers are left out"
            : ""}.
        </span>
        <button class="btn primary small" onclick={() => enable(true)}>Turn on</button>
      </div>
    {:else if !view.items.length}
      <div class="empty">
        <span class="badge"><Icon name="copy" size={26} /></span>
        <strong>Nothing copied yet</strong>
        <span>Copy some text, a link or an image, and it shows up here.</span>
      </div>
    {:else if !current}
      <div class="empty">
        <strong>Nothing matches</strong>
        <span>{query.trim() ? `No copy contains “${query.trim()}”.` : "No copies of this kind yet."}</span>
        <button
          class="btn small"
          onclick={() => {
            query = "";
            filter = "all";
          }}>Show everything</button
        >
      </div>
    {:else}
      <div
        class="list"
        id="clips"
        role="listbox"
        tabindex="0"
        aria-label="Copies"
        aria-activedescendant="clip-{current.id}"
      >
        {#each sections as s (s.label)}
          <div class="group" role="group" aria-labelledby="sec-{s.label}">
            <div class="section" id="sec-{s.label}">{s.label}</div>
            {#each s.items as it (it.id)}
              <!-- Keys are handled by the list; rows only take clicks. -->
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <div
                class="row"
                class:sel={current.id === it.id}
                id="clip-{it.id}"
                role="option"
                tabindex="-1"
                aria-selected={current.id === it.id}
                onclick={() => select(it.id)}
                ondblclick={() => paste(it)}
              >
                <span class="lead" class:thumb={it.kind === "image" && it.thumb}>
                  {#if it.kind === "image" && it.thumb}
                    <img src={fileUrl(it.thumb)} alt="" loading="lazy" />
                  {:else}
                    <Icon name={KIND_ICON[it.kind]} size={16} />
                  {/if}
                </span>
                <span class="row-text">
                  <span class="first ellipsis">{firstLine(it)}</span>
                  <span class="row-meta ellipsis">
                    {it.app ? `${appLabel(it.app)} · ` : ""}{ago(it.at, time.now.getTime())}
                  </span>
                </span>
                {#if it.pinned}<span class="pinned"><Icon name="push-pin-fill" size={12} /><span class="sr-only">Pinned</span></span>{/if}
              </div>
            {/each}
          </div>
        {/each}
      </div>

      <section class="preview card" aria-label="Selected copy">
        <p class="detail meta">{detail(current)}</p>
        <div class="content">
          {#if current.kind === "image"}
            <div class="image">
              <img
                src={fileUrl(current.image ?? current.thumb ?? "")}
                alt={current.size ? `Copied image, ${current.size[0]} by ${current.size[1]} pixels` : "Copied image"}
              />
            </div>
          {:else if current.kind === "link"}
            <div class="link">
              <span class="url">{current.text}</span>
              {#if host(current.text)}<span class="meta"><Icon name="link" size={13} />{host(current.text)}</span>{/if}
            </div>
          {:else}
            <pre class="text">{current.text}</pre>
            {#if current.chars > (current.text?.length ?? 0)}
              <p class="meta more">
                Showing the first {count.format(current.text?.length ?? 0)} of {count.format(current.chars)} characters. Paste and
                Copy use all of it.
              </p>
            {/if}
          {/if}
        </div>
        <div class="actions">
          <button class="btn primary" onclick={() => current && paste(current)}>
            <Icon name="arrow-elbow-down-left" size={15} />Paste
          </button>
          <button class="btn" onclick={() => current && copy(current)}>
            {#if copied === current.id}<Icon name="check" size={15} />Copied{:else}<Icon name="copy" size={15} />Copy{/if}
          </button>
          <button class="btn" aria-pressed={current.pinned} onclick={() => current && pin(current)}>
            <Icon name={current.pinned ? "push-pin-fill" : "push-pin"} size={15} />{current.pinned ? "Unpin" : "Pin"}
          </button>
          <span class="spacer"></span>
          <button class="btn quiet danger" onclick={() => current && remove(current)}>
            <Icon name="trash" size={15} />Remove
          </button>
        </div>
      </section>
    {/if}
  </div>

  <div class="foot">
    {#if error}
      <span class="error" role="alert">{error}</span>
    {:else if keys}
      <span class="hints meta">
        <Kbd k="up" /><Kbd k="down" /> Choose <Kbd k="enter" /> Paste <Kbd k="P" /> Pin <Kbd k="Del" /> Remove
      </span>
    {:else}
      <span class="hints meta">Double-click a copy to paste it where you were typing.</span>
    {/if}
    <span class="privacy meta" class:paused={view && !view.enabled}>
      {#if view && !view.enabled}
        <Icon name="warning" size={14} />Paused. New copies are not saved.
      {:else}
        <Icon name="shield-check" size={14} />Stays on this computer.{view?.guards_secrets
          ? ` Passwords are left out${view.skipped ? ` (${view.skipped} so far)` : ""}.`
          : ""}
      {/if}
    </span>
  </div>
</div>

<style>
  .clip {
    height: 100%;
    display: grid;
    grid-template-rows: 36px 1fr 20px;
    gap: 12px;
  }

  .top {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .search {
    flex: 0 1 260px;
    min-width: 150px;
  }
  .filters {
    display: flex;
    gap: 4px;
  }
  .spacer {
    flex: 1;
  }
  .save {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .main {
    min-height: 0;
    display: grid;
  }
  .main.split {
    grid-template-columns: minmax(0, 400px) minmax(0, 1fr);
    gap: 12px;
  }

  /* ---- list ---- */

  .list {
    min-height: 0;
    overflow-y: auto;
    padding-right: 4px;
    border-radius: 14px;
  }
  .list:focus-visible {
    outline-offset: -2px;
  }
  .section {
    position: sticky;
    top: 0;
    z-index: 1;
    padding: 6px 10px 4px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
    background: var(--panel);
  }
  .row {
    height: 46px;
    padding: 0 10px;
    border-radius: 12px;
    display: flex;
    align-items: center;
    gap: 10px;
    transition: background-color var(--t-fade) var(--out);
  }
  .row:hover {
    background: var(--s1);
  }
  .row.sel {
    background: var(--accent-12);
  }
  .lead {
    width: 30px;
    height: 30px;
    flex: none;
    border-radius: 9px;
    display: grid;
    place-items: center;
    background: var(--s2);
    color: var(--text-2);
    overflow: hidden;
  }
  .sel .lead {
    color: var(--accent-text);
  }
  .lead img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .row-text {
    flex: 1;
    min-width: 0;
    display: grid;
  }
  .first {
    font-size: 13px;
  }
  .row-meta {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .pinned {
    color: var(--accent-text);
    display: grid;
  }

  /* ---- preview ---- */

  .preview {
    min-width: 0;
    min-height: 0;
    display: grid;
    grid-template-rows: auto 1fr auto;
    gap: 10px;
    padding: 14px 16px 14px;
  }
  .content {
    min-height: 0;
    overflow: auto;
    display: grid;
    align-content: start;
    gap: 8px;
  }
  .text {
    margin: 0;
    font: inherit;
    font-size: 13px;
    line-height: 1.55;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .more {
    color: var(--text-3);
  }
  .link {
    display: grid;
    gap: 6px;
    align-content: start;
  }
  .url {
    font-size: 15px;
    font-weight: 560;
    line-height: 1.45;
    overflow-wrap: anywhere;
    color: var(--accent-text);
    user-select: text;
  }
  .link .meta {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .image {
    height: 100%;
    min-height: 0;
    display: grid;
    place-items: center;
  }
  .image img {
    max-width: 100%;
    max-height: 220px;
    border-radius: 10px;
    object-fit: contain;
    background: repeating-conic-gradient(var(--s2) 0 25%, transparent 0 50%) 0 0 / 16px 16px;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  /* ---- empty states ---- */

  .empty {
    padding: 0 160px;
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

  /* ---- footer ---- */

  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 0 4px;
  }
  .hints {
    display: flex;
    align-items: center;
    gap: 5px;
    white-space: nowrap;
  }
  .hints :global(kbd + kbd) {
    margin-left: -2px;
  }
  .privacy {
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }
  .privacy.paused {
    color: var(--ember);
  }
  .foot .error {
    flex: 1;
    min-width: 0;
  }
  @container page (max-width: 760px) {
    .save .meta {
      display: none;
    }
    .main.split {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  @container page (max-width: 580px) {
    .clip {
      grid-template-rows: auto minmax(0, 1fr) 20px;
    }
    .top {
      flex-wrap: wrap;
    }
    .search {
      flex: 1 1 100%;
    }
    .main.split {
      grid-template-columns: minmax(0, 1fr);
    }
    .preview {
      display: none;
    }
  }
</style>
