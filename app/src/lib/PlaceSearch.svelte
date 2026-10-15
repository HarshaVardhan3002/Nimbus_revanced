<script lang="ts">
  // Find a city by name and pick it for the weather.
  import { onMount } from "svelte";
  import Icon from "./Icon.svelte";
  import { typing } from "./ui.svelte";
  import { findPlaces, setPlace, type Place } from "./weather.svelte";

  let { ondone }: { ondone: () => void } = $props();

  let input: HTMLInputElement;
  let query = $state("");
  let results = $state<Place[]>([]);
  let active = $state(0);
  let status = $state<"idle" | "searching" | "none" | "error">("idle");
  let error = $state("");
  let seq = 0;
  let wait: ReturnType<typeof setTimeout> | undefined;

  onMount(() => {
    // Only grab the keyboard if the island already has it; otherwise the first
    // click into the field asks for it.
    if (document.hasFocus()) input.focus();
  });

  function search() {
    clearTimeout(wait);
    const q = query.trim();
    if (q.length < 2) {
      results = [];
      status = "idle";
      return;
    }
    wait = setTimeout(async () => {
      const mine = ++seq;
      status = "searching";
      try {
        const found = await findPlaces(q);
        if (mine !== seq) return;
        results = found;
        active = 0;
        status = found.length ? "idle" : "none";
      } catch (e) {
        if (mine !== seq) return;
        status = "error";
        error = e instanceof Error ? e.message : String(e);
      }
    }, 250);
  }

  async function choose(p: Place) {
    await setPlace(p);
    ondone();
  }

  function key(e: KeyboardEvent) {
    if (e.key === "ArrowDown" && results.length) {
      active = (active + 1) % results.length;
      e.preventDefault();
    } else if (e.key === "ArrowUp" && results.length) {
      active = (active - 1 + results.length) % results.length;
      e.preventDefault();
    } else if (e.key === "Enter" && results[active]) {
      choose(results[active]);
      e.preventDefault();
    } else if (e.key === "Escape") {
      // Close the search, not the whole panel.
      e.preventDefault();
      ondone();
    }
  }
</script>

<div class="search">
  <div class="field">
    <Icon name="magnifying-glass" size={16} />
    <input
      bind:this={input}
      bind:value={query}
      use:typing
      oninput={search}
      onkeydown={key}
      placeholder="City, for example Munich"
      aria-label="City"
      role="combobox"
      aria-expanded={results.length > 0}
      aria-controls="places"
      aria-activedescendant={results.length ? `place-${active}` : undefined}
      autocomplete="off"
      spellcheck="false"
    />
    <button class="icon-btn small" aria-label="Cancel" onclick={ondone}><Icon name="x" size={14} /></button>
  </div>
  <ul id="places" role="listbox" aria-label="Places">
    {#each results as p, i (`${p.lat},${p.lon}`)}
      <!-- Keys are handled by the field (a combobox); the options only take clicks. -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <li
        id="place-{i}"
        role="option"
        aria-selected={i === active}
        class:active={i === active}
        onclick={() => choose(p)}
        onpointerenter={() => (active = i)}
      >
        <Icon name="map-pin" size={15} />
        <span class="ellipsis"><strong>{p.name}</strong> <span class="faint">{p.region}</span></span>
      </li>
    {/each}
  </ul>
  {#if status === "searching"}
    <p class="meta">Searching…</p>
  {:else if status === "none"}
    <p class="meta">No place called “{query.trim()}”. Check the spelling, or try a bigger city nearby.</p>
  {:else if status === "error"}
    <p class="error">{error}</p>
  {/if}
</div>

<style>
  .search {
    display: grid;
    gap: 6px;
  }
  .icon-btn.small {
    width: 24px;
    height: 24px;
    margin-right: -8px;
  }
  ul {
    list-style: none;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  li {
    height: 32px;
    padding: 0 12px;
    border-radius: 10px;
    display: flex;
    align-items: center;
    gap: 8px;
    text-align: left;
    color: var(--text-2);
  }
  li.active {
    background: var(--s2);
    color: var(--text);
  }
  strong {
    font-weight: 600;
    color: var(--text);
  }
  p {
    padding: 0 12px;
  }
</style>
