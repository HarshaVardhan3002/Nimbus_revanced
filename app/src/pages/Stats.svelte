<script lang="ts">
  // Stats: processor, graphics and memory as gauges with their recent history, a few
  // plain facts, and the apps using the most. Read once a second while open.
  import { onMount } from "svelte";
  import { bytes, duration, rate } from "../lib/api";
  import Gauge from "../lib/Gauge.svelte";
  import Icon from "../lib/Icon.svelte";
  import { live, refreshStats } from "../lib/live.svelte";
  import Spark from "../lib/Spark.svelte";

  let by = $state<"cpu" | "mem">("cpu");

  onMount(() => {
    refreshStats();
    const t = setInterval(refreshStats, 1000);
    return () => clearInterval(t);
  });

  let s = $derived(live.stats);
  let memPct = $derived(s ? (s.mem_used / Math.max(1, s.mem_total)) * 100 : null);
  let diskPct = $derived(s ? (s.disk_used / Math.max(1, s.disk_total)) * 100 : 0);
  let top = $derived(s ? (by === "cpu" ? s.top_cpu : s.top_mem).slice(0, 4) : []);
  let topMax = $derived(Math.max(1e-6, ...top.map((p) => (by === "cpu" ? p.cpu_percent : p.mem))));
  let cpuDetail = $derived.by(() => {
    if (!s) return "";
    const threads = `${s.cpu_per_core.length} threads`;
    return s.cpu_mhz ? `${threads} · ${(s.cpu_mhz / 1000).toFixed(2)} GHz` : threads;
  });
</script>

<div class="stats">
  <section class="card meter" aria-labelledby="cpu-title">
    <div class="head">
      <Gauge value={s?.cpu_percent ?? null} label="Processor load" size={84} />
      <div class="info">
        <h2 id="cpu-title">Processor</h2>
        <span class="meta ellipsis" title={s?.cpu_name}>{s?.cpu_name ?? "Reading…"}</span>
        <span class="meta faint num">{cpuDetail}</span>
        {#if s}
          <div class="cores" aria-hidden="true">
            {#each s.cpu_per_core as c, i (i)}<i style:--v={Math.min(1, c / 100)}></i>{/each}
          </div>
        {/if}
      </div>
    </div>
    <div class="spark"><Spark values={live.history.cpu} /></div>
  </section>

  <section class="card meter" aria-labelledby="gpu-title">
    <div class="head">
      <Gauge value={s?.gpu_percent ?? null} label="Graphics load" size={84} tone="var(--chest)" />
      <div class="info">
        <h2 id="gpu-title">Graphics</h2>
        <span class="meta ellipsis" title={s?.gpu_name ?? undefined}>{s ? (s.gpu_name ?? "Not reported by this system") : "Reading…"}</span>
        {#if s?.gpu_mem_used != null}
          <span class="meta faint num">{bytes(s.gpu_mem_used)} video memory in use</span>
        {/if}
      </div>
    </div>
    <div class="spark"><Spark values={live.history.gpu} tone="var(--chest)" /></div>
  </section>

  <section class="card meter" aria-labelledby="mem-title">
    <div class="head">
      <Gauge value={memPct} label="Memory in use" size={84} tone="var(--done)" />
      <div class="info">
        <h2 id="mem-title">Memory</h2>
        <span class="meta num">{s ? `${bytes(s.mem_used)} of ${bytes(s.mem_total)}` : "Reading…"}</span>
        {#if s && s.swap_total > 0}
          <span class="meta faint num">{bytes(s.swap_used)} swapped to disk</span>
        {/if}
      </div>
    </div>
    <div class="spark"><Spark values={live.history.mem} tone="var(--done)" /></div>
  </section>

  <section class="card facts" aria-label="System">
    <div class="fact">
      <span class="label"><Icon name="hard-drives" size={14} />Disk</span>
      <span class="value num">{s ? `${bytes(s.disk_used)} of ${bytes(s.disk_total)}` : "–"}</span>
      <span class="bar" aria-hidden="true"><i style:--v={diskPct / 100}></i></span>
    </div>
    <div class="fact">
      <span class="label"><Icon name="globe-simple" size={14} />Network</span>
      <span class="value num net">
        <span><Icon name="arrow-down" size={12} /><span class="sr-only">Down</span>{s ? rate(s.net_rx_per_s) : "–"}</span>
        <span><Icon name="arrow-up" size={12} /><span class="sr-only">Up</span>{s ? rate(s.net_tx_per_s) : "–"}</span>
      </span>
    </div>
    <div class="fact">
      <span class="label"><Icon name="clock-counter-clockwise" size={14} />Up for</span>
      <span class="value num">{s ? duration(s.uptime_s) : "–"}</span>
    </div>
    {#if s?.battery}
      <div class="fact">
        <span class="label">
          <Icon
            name={s.battery.charging ? "battery-charging" : s.battery.percent > 60 ? "battery-full" : s.battery.percent > 25 ? "battery-medium" : "battery-low"}
            size={14}
          />Battery
        </span>
        <span class="value num">{Math.round(s.battery.percent)}%{s.battery.charging ? ", charging" : s.battery.plugged_in ? ", plugged in" : ""}</span>
      </div>
    {:else}
      <div class="fact">
        <span class="label"><Icon name="stack" size={14} />Processes</span>
        <span class="value num">{s ? s.processes : "–"}</span>
      </div>
    {/if}
  </section>

  <section class="card top" aria-labelledby="top-title">
    <header>
      <h2 id="top-title">Busiest apps</h2>
      <div class="by" role="group" aria-label="Sort by">
        <button class="chip" aria-pressed={by === "cpu"} onclick={() => (by = "cpu")}>Processor</button>
        <button class="chip" aria-pressed={by === "mem"} onclick={() => (by = "mem")}>Memory</button>
      </div>
    </header>
    <ol>
      {#each top as p (p.name)}
        <li>
          <span class="name ellipsis">{p.name}{#if p.count > 1}<span class="count num" title="{p.count} processes">{p.count}</span>{/if}</span>
          <span class="amount num">{by === "cpu" ? `${p.cpu_percent.toFixed(1)}%` : bytes(p.mem)}</span>
          <span class="bar" aria-hidden="true"><i style:--v={(by === "cpu" ? p.cpu_percent : p.mem) / topMax}></i></span>
        </li>
      {/each}
    </ol>
  </section>
</div>

<style>
  .stats {
    height: 100%;
    display: grid;
    grid-template-columns: repeat(6, minmax(0, 1fr));
    grid-template-rows: 172px minmax(0, 1fr);
    gap: 12px;
  }
  .meter {
    grid-column: span 2;
    padding: 14px 14px 10px;
    display: grid;
    grid-template-rows: auto 1fr;
    gap: 6px;
    min-width: 0;
  }
  .head {
    display: flex;
    gap: 12px;
    min-width: 0;
  }
  .info {
    flex: 1;
    min-width: 0;
    display: grid;
    align-content: start;
    gap: 1px;
    padding-top: 6px;
  }
  h2 {
    font-size: 14px;
    font-weight: 600;
    margin-bottom: 1px;
  }
  /* One bar per thread, rising from a shared baseline. */
  .cores {
    display: flex;
    gap: 2px;
    height: 16px;
    margin-top: 8px;
    align-items: flex-end;
    box-shadow: 0 1px 0 var(--line);
  }
  .cores i {
    flex: 1;
    height: calc(max(0.1, var(--v)) * 100%);
    border-radius: 1.5px 1.5px 0 0;
    background: var(--accent);
    opacity: calc(0.3 + var(--v) * 0.7);
    transition:
      height 600ms var(--out),
      opacity 600ms var(--out);
  }
  .spark {
    min-height: 0;
    margin: 0 -2px;
  }

  .facts {
    grid-column: span 3;
    padding: 12px 16px;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px 20px;
    align-content: center;
  }
  .fact {
    display: grid;
    gap: 2px;
    align-content: center;
    min-width: 0;
  }
  .label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-2);
  }
  .value {
    font-family: var(--display);
    font-stretch: 108%;
    font-size: 15px;
    font-weight: 600;
    white-space: nowrap;
  }
  .net {
    display: flex;
    gap: 12px;
  }
  .net > span {
    display: inline-flex;
    align-items: center;
    gap: 3px;
  }
  .bar {
    height: 3px;
    border-radius: 2px;
    background: var(--s3);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--accent);
    transform-origin: left;
    scale: var(--v) 1;
    transition: scale 600ms var(--out);
  }

  .top {
    grid-column: span 3;
    padding: 12px 16px;
    display: grid;
    grid-template-rows: auto 1fr;
    gap: 6px;
    min-width: 0;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .by {
    display: flex;
    gap: 4px;
  }
  .by .chip {
    height: 24px;
    padding: 0 10px;
  }
  ol {
    list-style: none;
    padding: 0;
    display: grid;
    align-content: start;
    gap: 3px;
  }
  li {
    display: grid;
    grid-template-columns: 1fr auto;
    column-gap: 12px;
    row-gap: 2px;
    align-items: center;
  }
  li .bar {
    grid-column: 1 / -1;
    height: 2px;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-size: 12.5px;
  }
  .count {
    flex: none;
    min-width: 18px;
    height: 16px;
    padding: 0 5px;
    border-radius: 8px;
    display: inline-grid;
    place-items: center;
    font-size: 10.5px;
    font-weight: 600;
    color: var(--text-2);
    background: var(--s2);
  }
  .amount {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-2);
  }
  @container page (max-width: 760px) {
    .cores {
      display: none;
    }
  }
  @container page (max-width: 620px) {
    .stats {
      height: auto;
      min-height: 100%;
      grid-template-rows: none;
    }
    .meter,
    .facts,
    .top {
      grid-column: 1 / -1;
    }
    .meter {
      height: 150px;
    }
  }
</style>
