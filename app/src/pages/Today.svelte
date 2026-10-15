<script lang="ts">
  // Today: the time and date, the week, the weather, a timer, and a short to-do list.
  // A to-do can have a time: it speaks up from the notch then (an alarm rings until
  // dismissed). The assistant adds to the same list.
  import { BRAND } from "../brand";
  import { api, clock, type Repeat, type Todo } from "../lib/api";
  import { time } from "../lib/clock.svelte";
  import Icon from "../lib/Icon.svelte";
  import PlaceSearch from "../lib/PlaceSearch.svelte";
  import { pause, remaining, reset, running, setTotal, start, timer } from "../lib/timer.svelte";
  import { typing } from "../lib/ui.svelte";
  import { book, fromLocalInput, REPEAT_LABEL, toLocalInput, when } from "../lib/todos.svelte";
  import { describe, weather } from "../lib/weather.svelte";

  const hm = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
  const longDate = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long" });
  const narrowDay = new Intl.DateTimeFormat(undefined, { weekday: "narrow" });
  const longDay = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long" });
  const PRESETS = [1, 5, 10, 25, 45];

  let searching = $state(false);
  let draft = $state("");
  /** The time row under the field: open, and what it holds. */
  let timing = $state(false);
  let at = $state("");
  let repeat = $state<Repeat>("never");
  let alarm = $state(false);
  let todoError = $state("");
  let todos = $derived(book.todos);

  // The week around today, Monday first.
  let week = $derived.by(() => {
    const today = new Date(time.now);
    today.setHours(0, 0, 0, 0);
    const monday = new Date(today);
    monday.setDate(today.getDate() - ((today.getDay() + 6) % 7));
    return Array.from({ length: 7 }, (_, i) => {
      const d = new Date(monday);
      d.setDate(monday.getDate() + i);
      return { d, today: d.getTime() === today.getTime(), past: d < today };
    });
  });

  // ---- timer ----

  let left = $derived(remaining());
  let isRunning = $derived(running());
  let fraction = $derived(Math.max(0, Math.min(1, left / Math.max(1, timer.total))));
  let fresh = $derived(!isRunning && left === timer.total);

  // ---- to-dos ----

  const KIND_ICON = { todo: "bell-ringing", alarm: "alarm", task: "lightning" } as const;

  function openTiming() {
    timing = !timing;
    // An hour from now, on the hour, is a good first guess.
    if (timing && !at) at = toLocalInput(Math.ceil((Date.now() + 3_600_000) / 3_600_000) * 3_600_000);
  }
  async function add(e: SubmitEvent) {
    e.preventDefault();
    const text = draft.trim();
    if (!text) return;
    const due = timing ? fromLocalInput(at) : null;
    todoError = "";
    try {
      await api.todoAdd(text, due, due ? repeat : "never", due && alarm ? "alarm" : "todo");
      draft = "";
      timing = false;
      at = "";
      repeat = "never";
      alarm = false;
    } catch (err) {
      todoError = String(err);
    }
  }
  function toggle(t: Todo) {
    api.todoEdit({ ...$state.snapshot(t), done: !t.done }).catch(() => {});
  }
  function remove(t: Todo) {
    api.todoRemove(t.id).catch(() => {});
  }
  function clearDone() {
    api.todosClearDone().catch(() => {});
  }
  let open = $derived(todos.filter((t) => !t.done).length);
  let done = $derived(todos.length - open);
</script>

<div class="today-page">
  <section class="day" aria-label="Date and weather">
    <div class="now">
      <span class="time display">
        {#each hm.formatToParts(time.now) as p, i (i)}{#if p.type === "dayPeriod"}<small>{p.value}</small>{:else if p.value.trim()}{p.value}{/if}{/each}
      </span>
      <span class="date">{longDate.format(time.now)}</span>
    </div>

    {#if !searching}
    <ol class="week" aria-label="This week">
      {#each week as w (w.d.getTime())}
        <li class:current={w.today} class:past={w.past} aria-current={w.today ? "date" : undefined}>
          <span class="dow" aria-hidden="true">{narrowDay.format(w.d)}</span>
          <span class="dom num" aria-hidden="true">{w.d.getDate()}</span>
          <span class="sr-only">{longDay.format(w.d)}</span>
        </li>
      {/each}
    </ol>
    {/if}

    <div class="weather card">
      {#if searching}
        <PlaceSearch ondone={() => (searching = false)} />
      {:else if weather.place && weather.now}
        {@const w = describe(weather.now.code, weather.now.day)}
        <div class="wx">
          <span class="wx-icon"><Icon name={w.icon} size={30} /></span>
          <span class="temp display">{Math.round(weather.now.temp)}°</span>
          <span class="wx-text">
            <strong>{w.label}</strong>
            <span class="meta num">High {Math.round(weather.now.high)}° · Low {Math.round(weather.now.low)}°</span>
          </span>
        </div>
        <button class="btn quiet small place" onclick={() => (searching = true)} aria-label="Change city, now {weather.place.name}">
          <Icon name="map-pin" size={14} />{weather.place.name}
        </button>
      {:else if weather.place}
        <div class="wx-wait">
          {#if weather.error}
            <span class="error">{weather.error}</span>
          {:else}
            <span class="meta">Getting the weather for {weather.place.name}…</span>
          {/if}
          <button class="btn quiet small" onclick={() => (searching = true)}>Change city</button>
        </div>
      {:else}
        <div class="wx-add">
          <span class="wx-icon"><Icon name="cloud-sun" size={26} /></span>
          <span class="wx-text">
            <strong>Weather</strong>
            <span class="meta">Add your city to see it here.</span>
          </span>
          <button class="btn small" onclick={() => (searching = true)}><Icon name="plus" size={14} />Add city</button>
        </div>
      {/if}
    </div>
  </section>

  <section class="timer card" aria-label="Timer">
    <div class="dial" class:running={isRunning}>
      <svg viewBox="0 0 100 100" aria-hidden="true">
        <circle class="bed" cx="50" cy="50" r="45" pathLength="100" />
        <circle class="val" cx="50" cy="50" r="45" pathLength="100" style:stroke-dasharray="{fraction * 100} 100" />
      </svg>
      <span class="left">
        <span class="display num" role="timer" aria-live="off">{clock(left)}</span>
        <span class="meta">{isRunning ? "Running" : fresh ? `${Math.round(timer.total / 60_000)} min timer` : "Paused"}</span>
      </span>
    </div>
    <div class="presets" role="group" aria-label="Length">
      {#each PRESETS as m (m)}
        <button class="chip num" aria-pressed={timer.total === m * 60_000} onclick={() => setTotal(m * 60_000)}>{m} min</button>
      {/each}
    </div>
    <div class="actions">
      {#if isRunning}
        <button class="btn primary" onclick={pause}><Icon name="pause-fill" size={16} />Pause</button>
      {:else}
        <button class="btn primary" onclick={start}><Icon name="play-fill" size={16} />{fresh ? "Start" : "Resume"}</button>
      {/if}
      <button class="btn quiet" onclick={reset} disabled={fresh}><Icon name="arrow-counter-clockwise" size={16} />Reset</button>
    </div>
  </section>

  <section class="todos card" aria-labelledby="todos-title">
    <header>
      <h2 id="todos-title">To-dos</h2>
      <span class="meta num">{open ? `${open} left` : todos.length ? "All done" : ""}</span>
    </header>
    <form class="adder" onsubmit={add}>
      <div class="field">
        <Icon name="plus" size={16} />
        <input bind:value={draft} use:typing placeholder="Add a to-do" aria-label="New to-do" maxlength="200" />
        <button
          type="button"
          class="icon-btn when-btn"
          class:on={timing}
          aria-expanded={timing}
          aria-label="Remind me at a time"
          title="Remind me at a time"
          onclick={openTiming}
        >
          <Icon name="bell-ringing" size={15} />
        </button>
      </div>
      {#if timing}
        <div class="timing">
          <input type="datetime-local" bind:value={at} use:typing aria-label="When" />
          <select bind:value={repeat} aria-label="Repeat">
            {#each Object.entries(REPEAT_LABEL) as [v, label] (v)}<option value={v}>{label}</option>{/each}
          </select>
          <label class="alarm-toggle">
            <input type="checkbox" bind:checked={alarm} />
            <Icon name="alarm" size={14} />Alarm
          </label>
        </div>
      {/if}
      {#if todoError}<p class="error">{todoError}</p>{/if}
    </form>
    {#if todos.length}
      <ul>
        {#each todos as t (t.id)}
          <li class:done={t.done}>
            <button
              class="check"
              role="checkbox"
              aria-checked={t.done}
              aria-label={t.text}
              onclick={() => toggle(t)}
            >
              {#if t.done}<Icon name="check" size={13} />{/if}
            </button>
            <span class="text">
              {t.text}
              {#if t.at && !t.done}
                <span class="chip" class:late={t.fired} title={t.by_agent ? `Set by ${BRAND.name}` : undefined}>
                  <Icon name={KIND_ICON[t.kind]} size={11} />{when(t.at)}{#if t.repeat !== "never"}<Icon name="repeat" size={11} />{/if}
                </span>
              {/if}
            </span>
            <button class="icon-btn remove" aria-label="Remove {t.text}" onclick={() => remove(t)}>
              <Icon name="x" size={14} />
            </button>
          </li>
        {/each}
      </ul>
      {#if done}
        <button class="btn quiet small clear" onclick={clearDone}><Icon name="broom" size={14} />Clear done</button>
      {/if}
    {:else}
      <div class="empty">
        <span>Nothing on the list. Type above and press Enter, or ask {BRAND.name} to remind you.</span>
      </div>
    {/if}
  </section>
</div>

<style>
  .today-page {
    height: 100%;
    display: grid;
    grid-template-columns: 300px 264px minmax(0, 1fr);
    gap: 12px;
  }

  /* ---- date and weather ---- */

  .day {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-width: 0;
    container-type: inline-size;
  }
  .now {
    display: grid;
    gap: 2px;
    padding: 2px 4px 0;
  }
  .time {
    font-size: min(64px, 24cqi);
    line-height: 1;
    letter-spacing: -0.02em;
    white-space: nowrap;
  }
  .time small {
    margin-left: 0.14em;
    font-size: 0.32em;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--text-2);
  }
  .date {
    font-size: 14px;
    font-weight: 560;
    color: var(--text-2);
  }
  .week {
    list-style: none;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 4px;
  }
  .week li {
    height: 50px;
    border-radius: 14px;
    display: grid;
    place-items: center;
    align-content: center;
    gap: 3px;
  }
  .dow {
    font-size: 11px;
    font-weight: 560;
    color: var(--text-3);
  }
  .dom {
    font-weight: 600;
  }
  .week li.past .dom {
    color: var(--text-2);
  }
  .week li.current {
    background: var(--accent);
    color: var(--ink);
  }
  .week li.current .dow {
    color: color-mix(in oklab, var(--ink) 75%, transparent);
  }

  .weather {
    margin-top: auto;
    padding: 14px 14px 12px;
    display: grid;
    gap: 6px;
    min-height: 120px;
    align-content: center;
  }
  .wx,
  .wx-add {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .wx-icon {
    width: 46px;
    height: 46px;
    border-radius: 14px;
    display: grid;
    place-items: center;
    flex: none;
    background: var(--s2);
    color: var(--accent-text);
  }
  .temp {
    font-size: 34px;
    line-height: 1;
  }
  .wx-text {
    display: grid;
    min-width: 0;
    flex: 1;
  }
  .wx-text strong {
    font-weight: 600;
  }
  .place {
    justify-self: start;
    margin-left: -6px;
  }
  .wx-wait {
    display: grid;
    gap: 8px;
    justify-items: start;
  }

  /* ---- timer ---- */

  .timer {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 16px;
    padding: 18px;
  }
  .dial {
    position: relative;
    width: 172px;
    height: 172px;
  }
  .dial svg {
    width: 100%;
    height: 100%;
    rotate: -90deg;
  }
  .dial circle {
    fill: none;
    stroke-width: 5;
  }
  .bed {
    stroke: var(--s3);
  }
  .val {
    stroke: var(--accent);
    stroke-linecap: round;
    transition: stroke-dasharray 300ms linear;
  }
  .left {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    align-content: center;
    gap: 2px;
  }
  .left .display {
    font-size: 36px;
    line-height: 1.05;
  }
  .running .left .display {
    color: var(--accent-text);
  }
  .presets {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 6px;
  }
  .actions {
    display: flex;
    gap: 8px;
  }

  /* ---- to-dos ---- */

  .todos {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px;
  }
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding: 0 4px;
  }
  h2 {
    font-size: 14px;
    font-weight: 600;
  }
  .todos ul {
    list-style: none;
    padding: 0;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: grid;
    align-content: start;
    gap: 2px;
    margin-right: -6px;
    padding-right: 6px;
  }
  .todos li {
    min-height: 36px;
    padding: 6px 4px 6px 6px;
    border-radius: 12px;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .todos li:hover {
    background: var(--s1);
  }
  .check {
    width: 20px;
    height: 20px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    box-shadow: inset 0 0 0 1.5px var(--text-3);
    transition:
      background-color var(--t-fade) var(--out),
      box-shadow var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }
  .check:active {
    scale: 0.85;
  }
  .done .check {
    background: var(--accent);
    color: var(--ink);
    box-shadow: none;
  }
  .text {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
    transition: color var(--t-fade) var(--out);
  }
  .done .text {
    color: var(--text-3);
    text-decoration: line-through;
    text-decoration-color: var(--text-3);
  }
  .adder {
    display: grid;
    gap: 8px;
  }
  .when-btn {
    width: 28px;
    height: 28px;
    margin-right: -6px;
    color: var(--text-3);
  }
  .when-btn.on {
    color: var(--accent-text);
    background: var(--accent-20);
  }
  .timing {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    animation: unfold 200ms var(--out) both;
  }
  .timing input[type="datetime-local"],
  .timing select {
    height: 30px;
    padding: 0 10px;
    border: 0;
    border-radius: 10px;
    background: var(--s2);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
    color-scheme: dark;
  }
  .timing input[type="datetime-local"] {
    flex: 1 1 170px;
    min-width: 0;
  }
  .alarm-toggle {
    height: 30px;
    padding: 0 10px;
    border-radius: 10px;
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--s2);
    font-size: 12.5px;
    color: var(--text-2);
    cursor: pointer;
  }
  .alarm-toggle input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .alarm-toggle:has(input:checked) {
    background: var(--accent-20);
    color: var(--accent-text);
  }
  .alarm-toggle:has(input:focus-visible) {
    outline: 2px solid var(--accent);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-left: 6px;
    padding: 1px 7px;
    border-radius: 999px;
    vertical-align: 1px;
    background: var(--s2);
    color: var(--text-2);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .chip.late {
    color: var(--accent-text);
    background: var(--accent-20);
  }
  @keyframes unfold {
    from {
      opacity: 0;
      translate: 0 -4px;
    }
  }
  .remove {
    width: 26px;
    height: 26px;
    opacity: 0;
  }
  .todos li:hover .remove,
  .remove:focus-visible {
    opacity: 1;
  }
  .clear {
    align-self: flex-start;
  }
  .todos .empty {
    flex: 1;
    padding: 0 16px;
  }
  @container page (max-width: 820px) {
    .today-page {
      height: auto;
      min-height: 100%;
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .todos {
      grid-column: 1 / -1;
      min-height: 240px;
    }
  }
  @container page (max-width: 540px) {
    .today-page {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
