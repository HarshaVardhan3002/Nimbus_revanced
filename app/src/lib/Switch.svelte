<script lang="ts">
  let {
    on,
    label,
    disabled = false,
    onchange,
  }: { on: boolean; label: string; disabled?: boolean; onchange: (on: boolean) => void } = $props();
</script>

<button class="switch" role="switch" aria-checked={on} aria-label={label} {disabled} onclick={() => onchange(!on)}>
  <span class="knob"></span>
</button>

<style>
  .switch {
    position: relative;
    flex: none;
    width: 40px;
    height: 24px;
    border-radius: 12px;
    background: var(--s4);
    transition: background-color var(--t-fade) var(--out);
  }
  .switch:hover:not(:disabled) {
    background: rgb(255 255 255 / 0.2);
  }
  .switch[aria-checked="true"] {
    background: var(--accent);
  }
  .switch[aria-checked="true"]:hover:not(:disabled) {
    background: color-mix(in oklab, var(--accent) 86%, white);
  }
  .switch:disabled {
    opacity: 0.4;
  }
  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--text-2);
    box-shadow: 0 1px 3px rgb(0 0 0 / 0.4);
    transition:
      translate var(--t-pop) var(--spring-pop),
      background-color var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }
  .switch:active:not(:disabled) .knob {
    scale: 1.12 0.92;
  }
  [aria-checked="true"] .knob {
    translate: 16px 0;
    background: var(--ink);
  }
</style>
