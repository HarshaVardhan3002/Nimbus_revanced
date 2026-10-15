<script lang="ts">
  import { ICONS, type IconName } from "./icons";

  let { name, size = 18 }: { name: IconName; size?: number } = $props();
</script>

<span class="icon" style:--size="{size}px" aria-hidden="true">{@html ICONS[name]}</span>

<style>
  .icon {
    display: inline-grid;
    flex: none;
    width: var(--size);
    height: var(--size);
  }
  .icon :global(svg) {
    width: 100%;
    height: 100%;
  }
</style>
