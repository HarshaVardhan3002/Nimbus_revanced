<script lang="ts">
  // Routines: jobs written once, in plain words, that the assistant does on its own:
  // at a time, when the app starts, or when started here. Each run shows up in the
  // chat under the routine's name, and its outcome stays on the card.
  import { BRAND } from "../brand";
  import { api, reason, type Routine, type Trigger } from "./api";
  import Icon from "./Icon.svelte";
  import { book, DAYS, trigger } from "./todos.svelte";
  import { typing } from "./ui.svelte";
  import { chat } from "./chat.svelte";

  let { ready }: { ready: boolean } = $props();

  type Draft = { id: string; name: string; steps: string; when: Trigger["when"]; time: string; day: number; trusted: boolean; enabled: boolean };

  let editing = $state<Draft | null>(null);
  let error = $state("");
  let saving = $state(false);
  let confirmRemove = $state<string | null>(null);

  const ago = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  function since(ms: number) {
    const min = Math.round((ms - Date.now()) / 60_000);
    if (min > -60) return ago.format(min, "minute");
    if (min > -24 * 60) return ago.format(Math.round(min / 60), "hour");
    return ago.format(Math.round(min / 1440), "day");
  }

  /** A routine running right now. */
  function running(r: Routine) {
    return chat.turns.some((t) => t.role === "assistant" && t.state === "working" && t.session === `routine:${r.id}`);
  }

  function edit(r?: Routine) {
    error = "";
    const t = r?.trigger ?? { when: "manual" };
    editing = {
      id: r?.id ?? "",
      name: r?.name ?? "",
      steps: r?.steps ?? "",
      when: t.when,
      time: "time" in t ? t.time : "08:00",
      day: t.when === "weekly" ? t.day : 0,
      trusted: r?.trusted ?? false,
      enabled: r?.enabled ?? true,
    };
  }

  function toTrigger(d: Draft): Trigger {
    switch (d.when) {
      case "daily":
      case "weekdays":
        return { when: d.when, time: d.time };
      case "weekly":
        return { when: "weekly", day: d.day, time: d.time };
      default:
        return { when: d.when };
    }
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    if (!editing) return;
    const d = editing;
    const old = book.routines.find((r) => r.id === d.id);
    saving = true;
    error = "";
    try {
      await api.routineSave({
        id: d.id,
        name: d.name.trim(),
        steps: d.steps.trim(),
        trigger: toTrigger(d),
        trusted: d.trusted,
        enabled: d.enabled,
        last_run: old?.last_run ?? null,
        last_result: old?.last_result ?? null,
      });
      editing = null;
    } catch (err) {
      error = reason(err);
    } finally {
      saving = false;
    }
  }

  async function run(r: Routine) {
    error = "";
    try {
      await api.routineRun(r.id);
    } catch (err) {
      error = reason(err);
    }
  }

  function toggle(r: Routine) {
    api.routineSave({ ...$state.snapshot(r), enabled: !r.enabled }).catch((err) => (error = reason(err)));
  }

  function remove(r: Routine) {
    if (confirmRemove !== r.id) {
      confirmRemove = r.id;
      setTimeout(() => confirmRemove === r.id && (confirmRemove = null), 3000);
      return;
    }
    confirmRemove = null;
    if (editing?.id === r.id) editing = null;
    api.routineRemove(r.id).catch((err) => (error = reason(err)));
  }

  const EXAMPLES = [
    { name: "Morning start", steps: "Open Outlook and Spotify.\nSearch the web for today's weather in my city and tell me in one line." },
    { name: "Stand-up note", steps: "Read the clipboard, turn it into three short bullet points, and copy them back to the clipboard." },
  ];
</script>

<div class="routines">
  {#if editing}
    <form class="editor" onsubmit={save}>
      <label class="row">
        <span class="label">Name</span>
        <input bind:value={editing.name} use:typing placeholder="Morning start" maxlength="60" required />
      </label>
      <label class="steps">
        <span class="label">What to do, and how</span>
        <textarea
          bind:value={editing.steps}
          use:typing
          rows="5"
          maxlength="4000"
          placeholder={"Open Spotify and play my Focus playlist.\nCopy today's date to the clipboard.\nRemind me at 17:30 to send the report."}
          required
        ></textarea>
      </label>
      <div class="row when">
        <span class="label">Runs</span>
        <select bind:value={editing.when} aria-label="When it runs">
          <option value="manual">When I start it</option>
          <option value="daily">Every day</option>
          <option value="weekdays">On weekdays</option>
          <option value="weekly">Once a week</option>
          <option value="startup">When {BRAND.name} starts</option>
        </select>
        {#if editing.when === "weekly"}
          <select bind:value={editing.day} aria-label="Day">
            {#each DAYS as d, i (d)}<option value={i}>{d}</option>{/each}
          </select>
        {/if}
        {#if editing.when === "daily" || editing.when === "weekdays" || editing.when === "weekly"}
          <input type="time" bind:value={editing.time} use:typing aria-label="Time" required />
        {/if}
      </div>
      <label class="trust">
        <input type="checkbox" bind:checked={editing.trusted} />
        <span class="switch" aria-hidden="true"></span>
        <span>
          <strong>Don't ask before each step</strong>
          <span class="meta">Steps that usually need your OK go ahead without asking. Dangerous ones stay blocked either way.</span>
        </span>
      </label>
      {#if error}<p class="error">{error}</p>{/if}
      <div class="acts">
        <button type="button" class="btn small quiet" onclick={() => (editing = null)}>Cancel</button>
        <button type="submit" class="btn small primary" disabled={saving || !editing.name.trim() || !editing.steps.trim()}>
          <Icon name="check" size={14} />{editing.id ? "Save" : "Add routine"}
        </button>
      </div>
    </form>
  {:else if !book.routines.length}
    <div class="empty">
      <span class="badge"><Icon name="lightning" size={26} /></span>
      <strong>Hand over the routine stuff</strong>
      <span>Write a job once, in your own words. {BRAND.name} does it when you say, or every day at a set time.</span>
      <div class="examples">
        {#each EXAMPLES as ex (ex.name)}
          <button class="example" onclick={() => { edit(); editing!.name = ex.name; editing!.steps = ex.steps; }}>
            <strong>{ex.name}</strong>
            <span class="meta">{ex.steps.split("\n")[0]}</span>
          </button>
        {/each}
      </div>
      <button class="btn small primary" onclick={() => edit()}><Icon name="plus" size={14} />New routine</button>
    </div>
  {:else}
    <div class="head">
      <span class="meta">{book.routines.length} saved</span>
      <button class="btn small" onclick={() => edit()}><Icon name="plus" size={14} />New routine</button>
    </div>
    {#if error}<p class="error">{error}</p>{/if}
    <ul class="list">
      {#each book.routines as r (r.id)}
        {@const busy = running(r)}
        <li class="card-ish" class:paused={!r.enabled}>
          <div class="top">
            <button class="name" onclick={() => edit(r)} title="Edit">
              <strong class="ellipsis">{r.name}</strong>
              <span class="meta ellipsis">
                {trigger(r.trigger)}{#if !r.enabled} · paused{/if}{#if r.trusted} · runs without asking{/if}
              </span>
            </button>
            <span class="row-acts">
              {#if r.trigger.when !== "manual"}
                <button class="icon-btn" onclick={() => toggle(r)} aria-label={r.enabled ? "Pause" : "Resume"} title={r.enabled ? "Pause" : "Resume"}>
                  <Icon name={r.enabled ? "pause-fill" : "play"} size={14} />
                </button>
              {/if}
              <button class="icon-btn" class:danger={confirmRemove === r.id} onclick={() => remove(r)} aria-label="Delete {r.name}" title={confirmRemove === r.id ? "Click again to delete" : "Delete"}>
                <Icon name={confirmRemove === r.id ? "check" : "trash"} size={14} />
              </button>
              <button class="btn small run" onclick={() => run(r)} disabled={busy || !ready} title={ready ? "" : "Connect a model first"}>
                <Icon name={busy ? "circle-notch" : "play-fill"} size={13} />{busy ? "Running" : "Run now"}
              </button>
            </span>
          </div>
          <p class="result meta">
            {#if r.last_run}
              <span class="faint">{since(r.last_run)}:</span> {r.last_result ?? "Running…"}
            {:else}
              <span class="faint">Not run yet.</span> {r.steps.split("\n")[0]}
            {/if}
          </p>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .routines {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
    overflow-y: auto;
    margin-right: -8px;
    padding-right: 8px;
  }
  .label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
  }
  input:not([type="checkbox"]),
  select,
  textarea {
    height: 34px;
    padding: 0 12px;
    border: 0;
    outline: 0;
    border-radius: 12px;
    background: var(--s2);
    color: var(--text);
    font: inherit;
    color-scheme: dark;
    transition: box-shadow var(--t-fade) var(--out);
  }
  input:focus-visible,
  select:focus-visible,
  textarea:focus-visible {
    box-shadow: inset 0 0 0 1.5px var(--accent-35);
  }
  textarea {
    height: auto;
    padding: 10px 12px;
    resize: vertical;
    min-height: 96px;
    line-height: 1.5;
  }

  /* ---- editor ---- */

  .editor {
    display: grid;
    gap: 12px;
    padding: 2px 4px 4px;
    animation: arrive 220ms var(--out) both;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .row .label {
    width: 72px;
    flex: none;
  }
  .row input {
    flex: 1;
    min-width: 160px;
  }
  .row input[type="time"] {
    flex: 0 0 132px;
    min-width: 0;
  }
  .steps {
    display: grid;
    gap: 6px;
  }
  .trust {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 10px 12px;
    border-radius: 14px;
    background: var(--s1);
    cursor: pointer;
  }
  .trust input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .trust > span:last-child {
    display: grid;
    gap: 2px;
  }
  .trust strong {
    font-weight: 600;
  }
  .switch {
    width: 34px;
    height: 20px;
    flex: none;
    margin-top: 1px;
    border-radius: 999px;
    background: var(--s4);
    position: relative;
    transition: background var(--t-fade) var(--out);
  }
  .switch::after {
    content: "";
    position: absolute;
    top: 3px;
    left: 3px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--text);
    transition: translate var(--t-pop) var(--spring-pop);
  }
  .trust:has(input:checked) .switch {
    background: var(--accent);
  }
  .trust:has(input:checked) .switch::after {
    translate: 14px 0;
    background: var(--ink);
  }
  .trust:has(input:focus-visible) .switch {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .acts {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  /* ---- list ---- */

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 4px;
  }
  .list {
    list-style: none;
    padding: 0;
    display: grid;
    gap: 8px;
  }
  .card-ish {
    padding: 10px 10px 10px 14px;
    border-radius: 16px;
    background: var(--s1);
    display: grid;
    gap: 6px;
    transition: opacity var(--t-fade) var(--out);
  }
  .card-ish.paused {
    opacity: 0.6;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .name {
    flex: 1;
    min-width: 0;
    display: grid;
    text-align: left;
  }
  .name strong {
    font-weight: 600;
  }
  .row-acts {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: none;
  }
  .row-acts .icon-btn {
    width: 28px;
    height: 28px;
  }
  .icon-btn.danger {
    color: var(--bad);
  }
  .run {
    margin-left: 4px;
  }
  .result {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    overflow-wrap: anywhere;
  }

  /* ---- empty ---- */

  .empty {
    flex: 1;
    padding: 0 28px;
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
  .empty strong {
    font-size: 15px;
  }
  .examples {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 8px;
    width: min(100%, 460px);
    margin: 8px 0 4px;
  }
  .example {
    padding: 10px 12px;
    border-radius: 14px;
    background: var(--s1);
    display: grid;
    gap: 2px;
    text-align: left;
    transition: background var(--t-fade) var(--out);
  }
  .example:hover {
    background: var(--s2);
  }
  .example .meta {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  @keyframes arrive {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .editor {
      animation: none;
    }
  }
</style>
