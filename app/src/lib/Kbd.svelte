<script lang="ts">
  // A key, as printed on the keyboard. Arrows and Enter are drawn, not typed glyphs.
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  let { k }: { k: string } = $props();

  const DRAWN: Record<string, { icon: IconName; name: string }> = {
    up: { icon: "arrow-up", name: "Up arrow" },
    down: { icon: "arrow-down", name: "Down arrow" },
    enter: { icon: "arrow-elbow-down-left", name: "Enter" },
  };
  let drawn = $derived(DRAWN[k]);
</script>

<kbd class:drawn>
  {#if drawn}
    <Icon name={drawn.icon} size={12} /><span class="sr-only">{drawn.name}</span>
  {:else}
    {k}
  {/if}
</kbd>

<style>
  kbd {
    display: inline-grid;
    place-items: center;
    min-width: 20px;
    height: 20px;
    padding: 0 6px;
    border-radius: 6px;
    font: 600 11px/1 var(--ui);
    color: var(--text-2);
    background: var(--s2);
    box-shadow:
      inset 0 -1px 0 rgb(0 0 0 / 0.35),
      inset 0 0 0 1px var(--line);
  }
  .drawn {
    padding: 0;
  }
</style>
