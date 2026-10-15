<script lang="ts">
  // Power: lock with a click; sleep, restart and shut down only after a one-second
  // hold, so a stray click never ends the session.
  import { api, reason, type PowerAction } from "../lib/api";
  import HoldButton from "../lib/HoldButton.svelte";
  import type { IconName } from "../lib/icons";
  import { closePanel } from "../lib/ui.svelte";

  type Item = { action: PowerAction; label: string; icon: IconName; hint: string; doing: string; hold: number };

  const ITEMS: Item[] = [
    { action: "lock", label: "Lock", icon: "lock", hint: "Click to lock", doing: "Locking…", hold: 0 },
    { action: "sleep", label: "Sleep", icon: "moon-stars", hint: "Hold to sleep", doing: "Going to sleep…", hold: 1000 },
    { action: "restart", label: "Restart", icon: "arrow-clockwise", hint: "Hold to restart", doing: "Restarting…", hold: 1000 },
    { action: "shut_down", label: "Shut down", icon: "power", hint: "Hold to shut down", doing: "Shutting down…", hold: 1000 },
  ];

  let error = $state("");

  async function run(it: Item) {
    error = "";
    // Lock and sleep leave the desktop; fold the panel so it is not open on return.
    if (it.action === "lock" || it.action === "sleep") closePanel();
    try {
      await api.powerRun(it.action);
    } catch (e) {
      error = `${it.label} did not start: ${reason(e)}. Use the system menu instead.`;
    }
  }
</script>

<div class="power">
  <div class="row">
    {#each ITEMS as it (it.action)}
      <HoldButton label={it.label} icon={it.icon} hint={it.hint} doing={it.doing} hold={it.hold} onconfirm={() => run(it)} />
    {/each}
  </div>
  <p class="foot" aria-live="polite">
    {#if error}<span class="error">{error}</span>{/if}
  </p>
</div>

<style>
  .power {
    height: 100%;
    display: grid;
    grid-template-rows: 1fr 18px;
    gap: 6px;
  }
  .row {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 12px;
  }
  .foot {
    padding: 0 4px;
  }
  @container page (max-width: 560px) {
    .power {
      height: auto;
      min-height: 100%;
    }
    .row {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
</style>
