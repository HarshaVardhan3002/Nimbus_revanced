<script lang="ts">
  // Controls: what is playing, four quick switches, and the two levels.
  import { onMount } from "svelte";
  import { api, appLabel, clock, reason, type MediaAction, type PrivacyStatus, type Radio, type RadioKind } from "../lib/api";
  import Icon from "../lib/Icon.svelte";
  import type { IconName } from "../lib/icons";
  import { live, refreshLevels, refreshMedia, refreshToggles } from "../lib/live.svelte";
  import PillSlider from "../lib/PillSlider.svelte";

  let privacy = $state<PrivacyStatus | null>(null);
  let pending = $state<Record<string, boolean>>({});
  let error = $state("");
  let errorTimer: ReturnType<typeof setTimeout> | undefined;
  let now = $state(performance.now());

  onMount(() => {
    refreshLevels();
    refreshToggles();
    api
      .privacyStatus()
      .then((p) => (privacy = p))
      .catch(() => {});
    // Levels and switches can change from elsewhere (keyboard keys, the system tray).
    const levels = setInterval(refreshLevels, 2000);
    const toggles = setInterval(refreshToggles, 5000);
    const progress = setInterval(() => (now = performance.now()), 250);
    return () => {
      clearInterval(levels);
      clearInterval(toggles);
      clearInterval(progress);
      clearTimeout(errorTimer);
    };
  });

  function fail(what: string, e: unknown) {
    error = `${what}: ${reason(e)}. Try again, or use the system settings.`;
    clearTimeout(errorTimer);
    errorTimer = setTimeout(() => (error = ""), 7000);
  }

  async function busyWith(key: string, work: () => Promise<void>) {
    if (pending[key]) return;
    pending[key] = true;
    try {
      await work();
    } finally {
      pending[key] = false;
    }
  }

  // ---- media ----

  let np = $derived(live.np);
  let position = $derived.by(() => {
    if (!np || np.position_ms == null || !np.duration_ms) return null;
    const run = np.playing ? Math.max(0, now - live.npAt) : 0;
    return Math.min(np.duration_ms, np.position_ms + run);
  });
  let byline = $derived(np ? [np.artist || np.album, appLabel(np.app)].filter(Boolean).join(" · ") : "");

  async function control(action: MediaAction) {
    if (action === "play_pause" && live.np) {
      // Show the new state at once; the next read confirms it.
      live.np = { ...live.np, playing: !live.np.playing, position_ms: position ?? live.np.position_ms };
      live.npAt = performance.now();
    }
    try {
      await api.mediaControl(action);
    } catch (e) {
      fail("The player did not respond", e);
    }
    setTimeout(refreshMedia, 300);
  }

  // ---- switches ----

  function radioDetail(r: Radio | null | undefined, kind: RadioKind) {
    if (!r) return "Not available";
    if (!r.on) return "Off";
    return r.detail ?? (kind === "wifi" ? "Not connected" : "On");
  }

  const radio = (kind: RadioKind) =>
    busyWith(kind, async () => {
      const r = live.radios?.[kind];
      if (!r) return;
      try {
        live.radios = await api.radioSet(kind, !r.on);
      } catch (e) {
        fail(kind === "wifi" ? "Wi-Fi did not switch" : "Bluetooth did not switch", e);
      }
    });

  const theme = () =>
    busyWith("theme", async () => {
      if (live.dark == null) return;
      try {
        live.dark = await api.themeSet(!live.dark);
      } catch (e) {
        fail("Dark mode did not switch", e);
      }
    });

  const hide = () =>
    busyWith("privacy", async () => {
      if (!privacy) return;
      try {
        privacy = await api.privacySet(!privacy.enabled);
      } catch (e) {
        fail("Hiding from capture did not switch", e);
      }
    });

  let wifi = $derived(live.radios?.wifi);
  let bt = $derived(live.radios?.bluetooth);

  // ---- levels ----

  let volIcon = $derived<IconName>(
    !live.vol
      ? "speaker-high"
      : live.vol.muted
        ? "speaker-x"
        : live.vol.percent === 0
          ? "speaker-none"
          : live.vol.percent < 50
            ? "speaker-low"
            : "speaker-high",
  );

  async function setVolume(v: number) {
    try {
      live.vol = await api.volumeSet(v);
    } catch (e) {
      fail("Volume did not change", e);
    }
  }
  async function toggleMute() {
    if (!live.vol) return;
    try {
      live.vol = await api.volumeMute(!live.vol.muted);
    } catch (e) {
      fail("Mute did not switch", e);
    }
  }
  async function setBrightness(v: number) {
    try {
      live.bright = await api.brightnessSet(v);
    } catch (e) {
      fail("Brightness did not change", e);
    }
  }
</script>

{#snippet tile(name: string, icon: IconName, detail: string, on: boolean, available: boolean, key: string, act: () => void)}
  <button
    class="tile"
    class:on
    role="switch"
    aria-checked={on}
    aria-label={name}
    aria-describedby="tile-{key}"
    aria-busy={!!pending[key]}
    disabled={!available}
    onclick={act}
  >
    <span class="tile-icon" class:working={pending[key]}><Icon name={icon} size={18} /></span>
    <span class="tile-name">{name}</span>
    <span class="tile-detail ellipsis" id="tile-{key}">{detail}</span>
  </button>
{/snippet}

<div class="controls">
  <section class="media card" aria-label="Now playing">
    {#if np}
      <div class="art-wrap">
        {#if np.artwork}
          <img class="glow" src={np.artwork} alt="" aria-hidden="true" />
          <img class="art" src={np.artwork} alt="" />
        {:else}
          <div class="art blank"><Icon name="music-notes" size={44} /></div>
        {/if}
      </div>
      <div class="titles">
        <strong class="title ellipsis">{np.title || "Unknown title"}</strong>
        <span class="meta ellipsis">{byline}</span>
      </div>
      {#if position != null && np.duration_ms}
        <div
          class="progress"
          role="progressbar"
          aria-label="Position in the track"
          aria-valuemin={0}
          aria-valuemax={Math.round(np.duration_ms / 1000)}
          aria-valuenow={Math.round(position / 1000)}
          aria-valuetext="{clock(position)} of {clock(np.duration_ms)}"
        >
          <span class="track"><span class="done" style:--p={position / np.duration_ms}></span></span>
          <span class="times num">
            <span>{clock(position)}</span>
            <span>−{clock(np.duration_ms - position)}</span>
          </span>
        </div>
      {/if}
      <div class="transport">
        <button class="icon-btn skip" aria-label="Previous track" onclick={() => control("previous")}>
          <Icon name="skip-back-fill" size={20} />
        </button>
        <button class="play" aria-label={np.playing ? "Pause" : "Play"} onclick={() => control("play_pause")}>
          <Icon name={np.playing ? "pause-fill" : "play-fill"} size={24} />
        </button>
        <button class="icon-btn skip" aria-label="Next track" onclick={() => control("next")}>
          <Icon name="skip-forward-fill" size={20} />
        </button>
      </div>
    {:else}
      <div class="empty">
        <span class="art blank small"><Icon name="music-notes" size={30} /></span>
        <strong>Nothing playing</strong>
        <span>Play music or a video in any app and it shows up here.</span>
      </div>
    {/if}
  </section>

  <div class="side">
    <div class="tiles">
      {@render tile(
        "Wi-Fi",
        wifi?.on ? "wifi-high" : "wifi-slash",
        radioDetail(wifi, "wifi"),
        !!wifi?.on,
        !!wifi,
        "wifi",
        () => radio("wifi"),
      )}
      {@render tile(
        "Bluetooth",
        bt?.on ? "bluetooth" : "bluetooth-slash",
        radioDetail(bt, "bluetooth"),
        !!bt?.on,
        !!bt,
        "bluetooth",
        () => radio("bluetooth"),
      )}
      {@render tile(
        "Dark mode",
        live.dark === false ? "sun" : "moon",
        live.dark == null ? "Not available" : live.dark ? "On" : "Off",
        !!live.dark,
        live.dark != null,
        "theme",
        theme,
      )}
      {@render tile(
        "Hide from capture",
        privacy?.enabled ? "eye-slash" : "eye",
        !privacy ? "Not available" : privacy.enabled ? "Screenshots skip the island" : "Shows in screenshots",
        !!privacy?.enabled,
        !!privacy,
        "privacy",
        hide,
      )}
    </div>

    <div class="levels">
      <PillSlider
        label="Volume"
        value={live.vol?.percent ?? null}
        icon={volIcon}
        muted={!!live.vol?.muted}
        iconLabel={live.vol?.muted ? "Unmute" : "Mute"}
        onicon={toggleMute}
        onset={setVolume}
        disabled={!live.vol}
      />
      <PillSlider
        label="Brightness"
        value={live.bright}
        icon="sun"
        onset={setBrightness}
        disabled={live.bright == null}
        note={live.bright == null && live.brightError ? "This screen does not let apps change its brightness" : ""}
      />
    </div>

    <p class="foot meta" aria-live="polite">
      {#if error}
        <span class="error">{error}</span>
      {:else if live.vol?.device}
        <Icon name="headphones" size={14} /><span class="ellipsis">{live.vol.device}</span>
      {/if}
    </p>
  </div>
</div>

<style>
  .controls {
    height: 100%;
    display: grid;
    grid-template-columns: minmax(0, 340px) minmax(0, 1fr);
    gap: 12px;
  }

  /* ---- media ---- */

  .media {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 14px;
    padding: 20px 22px;
    overflow: hidden;
  }
  .art-wrap {
    position: relative;
    width: 168px;
    height: 168px;
    flex: none;
  }
  .art {
    position: relative;
    width: 100%;
    height: 100%;
    border-radius: 16px;
    object-fit: cover;
    box-shadow: 0 10px 24px -10px rgb(0 0 0 / 0.7);
  }
  /* The cover's own colours, blurred, as its shadow. */
  .glow {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    border-radius: 16px;
    object-fit: cover;
    filter: blur(22px) saturate(1.3);
    opacity: 0.55;
    translate: 0 12px;
    scale: 0.92;
  }
  .art.blank {
    display: grid;
    place-items: center;
    background: var(--s2);
    color: var(--text-3);
  }
  .art.blank.small {
    width: 72px;
    height: 72px;
    margin-bottom: 4px;
    border-radius: 14px;
  }
  .titles {
    width: 100%;
    display: grid;
    gap: 2px;
    text-align: center;
  }
  .title {
    font-size: 15px;
    font-weight: 600;
  }
  .progress {
    width: 100%;
    display: grid;
    gap: 5px;
  }
  .track {
    height: 4px;
    border-radius: 2px;
    background: var(--s3);
    overflow: hidden;
  }
  .done {
    display: block;
    height: 100%;
    width: 100%;
    border-radius: 2px;
    background: var(--accent);
    transform-origin: left;
    scale: var(--p) 1;
    transition: scale 250ms linear;
  }
  .times {
    display: flex;
    justify-content: space-between;
    font-size: 11px;
    color: var(--text-3);
  }
  .transport {
    display: flex;
    align-items: center;
    gap: 18px;
  }
  .skip {
    width: 40px;
    height: 40px;
    border-radius: 20px;
    color: var(--text);
  }
  .play {
    width: 52px;
    height: 52px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--ink);
    box-shadow: 0 8px 20px -8px color-mix(in oklab, var(--accent) 70%, transparent);
    transition:
      scale var(--t-pop) var(--spring-pop),
      background-color var(--t-fade) var(--out);
  }
  .play:hover {
    background: color-mix(in oklab, var(--accent) 86%, white);
  }
  .play:active {
    scale: 0.92;
  }
  .empty {
    padding: 0 12px;
  }

  /* ---- switches and levels ---- */

  .side {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .tiles {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
  .tile {
    height: 104px;
    padding: 12px 14px;
    border-radius: 18px;
    display: grid;
    grid-template-rows: 1fr auto auto;
    justify-items: start;
    text-align: left;
    background: var(--s1);
    box-shadow: inset 0 0 0 1px var(--line);
    transition:
      background-color var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }
  .tile:hover:not(:disabled) {
    background: var(--s2);
  }
  .tile:active:not(:disabled) {
    scale: 0.97;
  }
  .tile:disabled {
    opacity: 0.5;
  }
  .tile.on {
    background: color-mix(in oklab, var(--accent) 15%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--accent) 22%, transparent);
  }
  .tile.on:hover:not(:disabled) {
    background: color-mix(in oklab, var(--accent) 21%, transparent);
  }
  .tile-icon {
    width: 34px;
    height: 34px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--s3);
    color: var(--text-2);
    transition:
      background-color var(--t-fade) var(--out),
      color var(--t-fade) var(--out);
  }
  .on .tile-icon {
    background: var(--accent);
    color: var(--ink);
  }
  .tile-icon.working {
    animation: working 0.9s ease-in-out infinite;
  }
  .tile-name {
    font-weight: 600;
  }
  .tile-detail {
    max-width: 100%;
    font-size: 12px;
    color: var(--text-2);
  }
  .on .tile-detail {
    color: var(--accent-text);
  }

  .levels {
    display: grid;
    gap: 10px;
  }
  .foot {
    min-height: 18px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 4px;
  }

  @keyframes working {
    50% {
      opacity: 0.55;
    }
  }
  @container page (max-width: 620px) {
    .controls {
      height: auto;
      min-height: 100%;
      grid-template-columns: minmax(0, 1fr);
    }
    .media {
      min-height: 300px;
    }
    .art-wrap {
      width: 128px;
      height: 128px;
    }
  }
</style>
