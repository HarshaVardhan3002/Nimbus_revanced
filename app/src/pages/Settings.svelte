<script lang="ts">
  // Settings: privacy, startup, chat, mail, the island's colour, the weather city, and what this is.
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { BRAND } from "../brand";
  import { api, reason, type BrainPlan, type FilesStatus, type Hearing, type PrivacyStatus, type Settings } from "../lib/api";
  import { chat, checkServer } from "../lib/chat.svelte";
  import { speech, speechLine } from "../lib/speech.svelte";
  import { stopSpeaking, voice } from "../lib/voice.svelte";
  import Core from "../lib/Core.svelte";
  import Icon from "../lib/Icon.svelte";
  import type { IconName } from "../lib/icons";
  import Kbd from "../lib/Kbd.svelte";
  import MailCard from "../lib/MailCard.svelte";
  import { colors } from "../lib/palette.svelte";
  import PlaceSearch from "../lib/PlaceSearch.svelte";
  import Switch from "../lib/Switch.svelte";
  import { typing, ui } from "../lib/ui.svelte";
  import { setPlace, weather } from "../lib/weather.svelte";

  const SWATCHES = [
    { hex: "#8AB4FF", name: "Blue" },
    { hex: "#7FD6F2", name: "Sky" },
    { hex: "#7FE0C8", name: "Mint" },
    { hex: "#9BE37A", name: "Green" },
    { hex: "#F2D16B", name: "Yellow" },
    { hex: "#FFB27A", name: "Orange" },
    { hex: "#FF9EC4", name: "Pink" },
    { hex: "#B49CFF", name: "Violet" },
  ];
  const MAC = navigator.userAgent.includes("Mac");
  const WINDOWS = navigator.userAgent.includes("Windows");
  const SHORTCUT = MAC ? ["Control", "Option", "Space"] : ["Ctrl", "Alt", "Space"];
  const CHAT_SHORTCUT = MAC ? ["Control", "Option", "enter"] : ["Ctrl", "Alt", "enter"];

  let settings = $state<Settings | null>(null);
  let privacy = $state<PrivacyStatus | null>(null);
  let autostart = $state<boolean | null>(null);
  let hex = $state("");
  let searching = $state(false);
  let confirmClear = $state(false);
  let cleared = $state(false);
  let errors = $state({ privacy: "", startup: "", chat: "", search: "", colour: "", weather: "" });
  let chatUrl = $state("");
  let chatKey = $state("");
  let chatSaved = $state(false);
  let searxng = $state("");
  let braveKey = $state("");
  let searchSaved = $state(false);
  let clearTimer: ReturnType<typeof setTimeout> | undefined;
  let hearing = $state<Hearing | null>(null);
  let seeing = $state<{ on: boolean; pictures: number } | null>(null);
  let fileIndex = $state<FilesStatus | null>(null);
  let now = $state(Date.now());
  let plan = $state<BrainPlan | null>(null);

  onMount(() => {
    api
      .settingsGet()
      .then((s) => {
        settings = s;
        hex = s.accent ?? "";
        chatUrl = s.chat_url ?? "";
        chatKey = s.chat_key ?? "";
        searxng = s.search_searxng ?? "";
        braveKey = s.search_brave_key ?? "";
      })
      .catch((e) => (errors.privacy = `Could not read the settings: ${reason(e)}.`));
    api
      .privacyStatus()
      .then((p) => (privacy = p))
      .catch(() => {});
    api
      .autostartGet()
      .then((on) => (autostart = on))
      .catch(() => {});
    checkServer();
    api
      .hearingGet()
      .then((h) => (hearing = h))
      .catch(() => {});
    const offHearing = listen<Hearing>("hearing", ({ payload }) => (hearing = payload));
    // The "12 s ago" beside each heard line keeps counting.
    const readScreen = () =>
      api
        .screenGet()
        .then((s) => (seeing = s))
        .catch(() => {});
    readScreen();
    api
      .filesStatus()
      .then((s) => (fileIndex = s))
      .catch(() => {});
    const offFiles = listen<FilesStatus>("files", (e) => (fileIndex = e.payload));
    const readPlan = () =>
      api
        .brainPlan()
        .then((p) => (plan = p))
        .catch(() => {});
    readPlan();
    // A model put in the folder, a download finished, another model loaded.
    const offModels = listen("brain-models", readPlan);
    const offBrain = listen<{ state: string }>("brain", ({ payload }) => {
      if (payload.state !== "installing") readPlan();
    });
    const tick = setInterval(() => {
      now = Date.now();
      if (seeing?.on) readScreen();
    }, 1000);
    return () => {
      clearTimeout(clearTimer);
      clearInterval(tick);
      void offHearing.then((f) => f());
      void offFiles.then((f) => f());
      void offModels.then((f) => f());
      void offBrain.then((f) => f());
    };
  });

  // ---- privacy ----

  let privacyLine = $derived(
    !privacy
      ? "Checking…"
      : privacy.verified
        ? "Screenshots, recordings and screen sharing leave the island out."
        : privacy.enabled
          ? "Asked the system to hide it, but it has not confirmed. Shown in captures."
          : "The island shows up in screenshots and screen sharing.",
  );

  async function setPrivacy(on: boolean) {
    errors.privacy = "";
    try {
      privacy = await api.privacySet(on);
      if (settings) settings.hide_from_capture = on;
    } catch (e) {
      errors.privacy = `Hiding from capture did not switch: ${reason(e)}. Try again.`;
    }
  }

  async function setClipboard(on: boolean) {
    errors.privacy = "";
    try {
      await api.clipboardEnable(on);
      if (settings) settings.clipboard_history = on;
    } catch (e) {
      errors.privacy = `Clipboard history did not switch: ${reason(e)}. Try again.`;
    }
  }

  // ---- listening to the computer's sound ----

  let hearingLine = $derived(
    !hearing?.on
      ? "Off. When on, the last 20 seconds of what the computer plays are written down, so you can ask what was said. Only the words are kept, never the sound, and nothing is saved."
      : hearing.trouble
        ? `On, but ${hearing.trouble}.`
        : "On. Keeps the words of the last 20 seconds, then lets them go. The microphone is never used.",
  );

  let seeingLine = $derived(
    !seeing?.on
      ? `Off. When on, the last 10 seconds of the main screen are kept in memory, one picture a second, so you can ask about what is on it. ${BRAND.name}'s own windows are left out, nothing is saved, and pictures go only to the model you chat with.`
      : `On. ${seeing.pictures} of 10 pictures in memory; each one goes after 10 seconds.`,
  );

  let controlLine = $derived(
    settings?.control
      ? `On. ${BRAND.name} can press buttons, fill in fields and pick from lists in your apps, by their names. Buttons that send, pay or delete ask you first, and Esc stops it at once.`
      : `Off. ${BRAND.name} still opens apps, pages and files for you, but does not click or type inside them. When on, it can press buttons, fill in fields and pick from lists. It never types passwords or opens password managers.`,
  );

  async function setControl(on: boolean) {
    errors.chat = "";
    try {
      await api.controlSet(on);
      if (settings) settings.control = on;
    } catch (e) {
      errors.chat = `Clicking and typing did not switch: ${reason(e)}. Try again.`;
    }
  }

  async function setSeeing(on: boolean) {
    errors.chat = "";
    try {
      await api.screenSet(on);
      seeing = await api.screenGet();
    } catch (e) {
      errors.chat = `Seeing the screen did not switch: ${reason(e)}. Try again.`;
    }
  }

  let fileLine = $derived.by(() => {
    const f = fileIndex;
    if (!f?.on) return "Off. When on, the Apps search also finds files and folders by name, anywhere on this computer.";
    if (!f.files && f.scanning) return "Reading the drives for the first time. This takes about a minute.";
    const n = f.files >= 1e6 ? `${(f.files / 1e6).toFixed(1)} million` : f.files.toLocaleString();
    return `${n} names, kept current as files come and go. Only names are read, never what is in the files, and the list stays on this computer.`;
  });

  async function setFiles(on: boolean) {
    errors.privacy = "";
    try {
      await api.filesSet(on);
      fileIndex = await api.filesStatus();
    } catch (e) {
      errors.privacy = `File search did not switch: ${reason(e)}. Try again.`;
    }
  }

  // ---- "Hey Nimbus" ----

  let voiceLine = $derived(
    !voice.now?.on
      ? `Off. When on, the microphone is listened to. Say "Hey ${BRAND.name}" and then what you want. Words without the name are let go, the sound is never saved, and it stays on this computer.`
      : voice.now.trouble
        ? `On, but ${voice.now.trouble}.`
        : `On. Say "Hey ${BRAND.name}, …" or "Hey ${BRAND.name}" and wait a moment. Approvals still need a click.`,
  );

  async function setVoice(on: boolean) {
    errors.chat = "";
    try {
      await api.voiceSet(on);
      voice.now = await api.voiceGet();
    } catch (e) {
      errors.chat = `Voice did not switch: ${reason(e)}. Try again.`;
    }
  }

  async function setSpeakReplies(on: boolean) {
    errors.chat = "";
    try {
      await api.speakRepliesSet(on);
      voice.speakReplies = on;
      if (!on) stopSpeaking();
    } catch (e) {
      errors.chat = `Reading answers aloud did not switch: ${reason(e)}. Try again.`;
    }
  }

  async function setHearing(on: boolean) {
    errors.chat = "";
    try {
      await api.hearingSet(on);
    } catch (e) {
      errors.chat = `Listening did not switch: ${reason(e)}. Try again.`;
    }
  }

  // Two steps: the first click asks, the second clears. Pinned copies stay.
  async function clearClipboard() {
    clearTimeout(clearTimer);
    if (!confirmClear) {
      confirmClear = true;
      clearTimer = setTimeout(() => (confirmClear = false), 4000);
      return;
    }
    confirmClear = false;
    errors.privacy = "";
    try {
      await api.clipboardClear();
      cleared = true;
      clearTimer = setTimeout(() => (cleared = false), 2400);
    } catch (e) {
      errors.privacy = `The history was not cleared: ${reason(e)}. Try again.`;
    }
  }

  // ---- startup ----

  async function setAutostart(on: boolean) {
    errors.startup = "";
    try {
      autostart = await api.autostartSet(on);
    } catch (e) {
      errors.startup = `Starting with the computer did not switch: ${reason(e)}.`;
    }
  }

  // ---- chat ----

  let cs = $derived(chat.status);
  let chatLine = $derived.by(() => {
    const b = chat.brain;
    if (b?.state === "starting") return `Loading the local brain (${b.model})…`;
    if (b?.state === "asleep") return `The local brain (${b.model}) is unloaded to free the GPU; the next message loads it.`;
    if (b?.state === "installing") return "Setting up the local brain…";
    if (!cs || chat.checking) return "Looking for a model server…";
    if ((b?.state === "failed" || b?.state === "missing") && !cs.models.length) return `The local brain did not start: ${b.message}.`;
    if (!cs.url) return "No model server found on this computer.";
    if (cs.error) return `${cs.server} did not answer: ${cs.error}.`;
    const where = /\/\/(localhost|127\.0\.0\.1)[:/]/.test(cs.url) ? " on this computer" : "";
    if (!cs.models.length) return `${cs.server}${where} is running, but has no chat model.`;
    return `${cs.custom ? "Using" : "Found"} ${cs.server}${where}.`;
  });

  const gb = (bytes: number) => `${(bytes / 1e9).toFixed(1)} GB`;
  let brainBusy = $derived(chat.brain?.state === "installing" ? chat.brain : null);
  let brainLine = $derived.by(() => {
    if (brainBusy) {
      const { what, done, total } = brainBusy;
      return total ? `Downloading ${what}: ${gb(done)} of ${gb(total)} (${Math.floor((done * 100) / total)}%).` : `Downloading ${what}…`;
    }
    if (!plan) return "Looking at this computer…";
    const where = plan.chip ? `${plan.chip} with ${plan.backend_label}` : "the processor";
    const room = `${gb(plan.usable)} for a model.`;
    if (!plan.models.length) return `Runs on ${where}, ${room} Best for it: ${plan.recommended.name} (${gb(plan.recommended.size)}).`;
    return `Runs on ${where}, ${room}`;
  });
  let brainPct = $derived(brainBusy?.total ? (brainBusy.done * 100) / brainBusy.total : 0);
  let canSetUp = $derived(chat.brain?.state === "missing" && !!plan && (!plan.server || !plan.models.length));

  async function useModel(path: string | null) {
    if (!plan || plan.chosen === path) return;
    plan.chosen = path;
    try {
      await api.brainUse(path);
    } catch (e) {
      errors.chat = `The model was not switched: ${reason(e)}.`;
    }
  }

  const IDLE = [
    { minutes: 10, label: "10 minutes" },
    { minutes: 30, label: "30 minutes" },
    { minutes: 60, label: "1 hour" },
    { minutes: 0, label: "Never" },
  ];

  // What is spoken to Nimbus, for speech to text (left to guess, short English requests
  // come out as Urdu or Arabic).
  const LANGUAGES = [
    { code: "en", label: "English" },
    { code: "de", label: "Deutsch" },
    { code: null, label: "Let it guess" },
  ];

  async function setLanguage(code: string | null) {
    if (!settings) return;
    const before = settings.speech_language ?? null;
    if (before === code) return;
    settings.speech_language = code;
    try {
      await api.speechLanguageSet(code);
    } catch (e) {
      settings.speech_language = before;
      errors.chat = `The setting was not saved: ${reason(e)}.`;
    }
  }

  async function setIdle(minutes: number) {
    if (!settings || settings.brain_idle_minutes === minutes) return;
    const before = settings.brain_idle_minutes;
    settings.brain_idle_minutes = minutes;
    try {
      await api.brainIdleSet(minutes);
    } catch (e) {
      settings.brain_idle_minutes = before;
      errors.chat = `The setting was not saved: ${reason(e)}.`;
    }
  }

  async function saveChat(e: SubmitEvent) {
    e.preventDefault();
    errors.chat = "";
    const url = chatUrl.trim();
    if (url && !/^https?:\/\/\S+$/i.test(url)) {
      errors.chat = "Type the address with http:// or https://, like http://localhost:11434/v1.";
      return;
    }
    try {
      await api.chatConfigure(url || null, settings?.chat_model ?? null, chatKey.trim() || null);
      if (settings) {
        settings.chat_url = url || null;
        settings.chat_key = chatKey.trim() || null;
      }
      await checkServer(true);
      chatSaved = true;
      setTimeout(() => (chatSaved = false), 2000);
    } catch (err) {
      errors.chat = `The chat server was not saved: ${reason(err)}.`;
    }
  }

  async function saveSearch(e: SubmitEvent) {
    e.preventDefault();
    errors.search = "";
    const url = searxng.trim();
    if (url && !/^https?:\/\/\S+$/i.test(url)) {
      errors.search = "Type the address with http:// or https://, like http://127.0.0.1:8888.";
      return;
    }
    try {
      await api.searchSet(url || null, braveKey.trim() || null);
      if (settings) {
        settings.search_searxng = url.replace(/\/+$/, "") || null;
        settings.search_brave_key = braveKey.trim() || null;
      }
      searchSaved = true;
      setTimeout(() => (searchSaved = false), 2000);
    } catch (err) {
      errors.search = `Web search was not saved: ${reason(err)}.`;
    }
  }

  async function pickModel(model: string) {
    errors.chat = "";
    try {
      await api.chatConfigure(settings?.chat_url ?? null, model, settings?.chat_key ?? null);
      if (settings) settings.chat_model = model;
      if (chat.status) chat.status.model = model;
    } catch (err) {
      errors.chat = `The model did not change: ${reason(err)}.`;
    }
  }

  // ---- colour ----

  let follow = $derived(settings ? settings.accent == null : true);
  let chosen = $derived(settings?.accent?.toUpperCase() ?? null);

  async function setAccent(accent: string | null) {
    errors.colour = "";
    try {
      colors.current = await api.accentSet(accent);
      if (settings) settings.accent = accent;
      hex = accent ?? "";
    } catch (e) {
      errors.colour = `${reason(e)}.`;
    }
  }

  function setFollow(on: boolean) {
    // Turning it off keeps today's colour, so nothing jumps.
    setAccent(on ? null : (colors.current?.accent ?? SWATCHES[0].hex).toUpperCase());
  }

  function submitHex(e: SubmitEvent) {
    e.preventDefault();
    const v = hex.trim().replace(/^#?/, "#").toUpperCase();
    if (!/^#[0-9A-F]{6}$/.test(v)) {
      errors.colour = "Type a colour as six hex digits, like #4F8FFF.";
      return;
    }
    setAccent(v);
  }

  function swatchKey(e: KeyboardEvent, i: number) {
    const by = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    if (!by) return;
    e.preventDefault();
    const next = (i + by + SWATCHES.length) % SWATCHES.length;
    setAccent(SWATCHES[next].hex);
    document.getElementById(`swatch-${next}`)?.focus();
  }
  let rovingIndex = $derived(Math.max(0, SWATCHES.findIndex((s) => s.hex === chosen)));

  // ---- weather ----

  async function removePlace() {
    errors.weather = "";
    try {
      await setPlace(null);
    } catch (e) {
      errors.weather = `The city was not removed: ${reason(e)}.`;
    }
  }
</script>

{#snippet head(icon: IconName, title: string, id: string)}
  <h2 {id}><span class="head-icon"><Icon name={icon} size={15} /></span>{title}</h2>
{/snippet}

<div class="settings">
  <div class="col">
    <section class="card group" aria-labelledby="set-privacy">
      {@render head("shield-check", "Privacy", "set-privacy")}
      <div class="item">
        <div class="text">
          <span class="name">Hide from screen capture</span>
          <span class="meta" class:warn={privacy?.enabled && !privacy.verified}>{privacyLine}</span>
        </div>
        <Switch on={!!privacy?.enabled} label="Hide from screen capture" disabled={!privacy} onchange={setPrivacy} />
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Clipboard history</span>
          <span class="meta">
            Keeps the last 200 copies on this computer{WINDOWS ? ". Passwords from password managers are left out" : ""}.
          </span>
        </div>
        <Switch on={!!settings?.clipboard_history} label="Clipboard history" disabled={!settings} onchange={setClipboard} />
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Clear the history</span>
          <span class="meta">{cleared ? "Cleared. Pinned copies stayed." : "Pinned copies stay."}</span>
        </div>
        <button class="btn small" class:danger={confirmClear} onclick={clearClipboard}>
          {#if cleared}<Icon name="check" size={14} />Done{:else}<Icon name="broom" size={14} />{confirmClear ? "Clear now" : "Clear"}{/if}
        </button>
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Find files from the Apps search</span>
          <span class="meta">{fileLine}</span>
        </div>
        <Switch on={!!fileIndex?.on} label="Find files from the Apps search" disabled={!fileIndex} onchange={setFiles} />
      </div>
      {#if errors.privacy}<p class="error" role="alert">{errors.privacy}</p>{/if}
    </section>

    <section class="card group" aria-labelledby="set-startup">
      {@render head("rocket-launch", "Startup", "set-startup")}
      <div class="item">
        <div class="text">
          <span class="name">Start with the computer</span>
          <span class="meta">Opens quietly when you sign in.</span>
        </div>
        <Switch on={!!autostart} label="Start with the computer" disabled={autostart == null} onchange={setAutostart} />
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Open from the keyboard</span>
          <span class="meta">Works from any app.</span>
        </div>
        <span class="keys" aria-label={SHORTCUT.join(" + ")}>
          {#each SHORTCUT as k, i (k)}{#if i}<span class="plus" aria-hidden="true">+</span>{/if}<Kbd {k} />{/each}
        </span>
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Chat from the keyboard</span>
          <span class="meta">Opens the chat box, ready to type.</span>
        </div>
        <span class="keys" aria-label={CHAT_SHORTCUT.join(" + ").replace("enter", "Enter")}>
          {#each CHAT_SHORTCUT as k, i (k)}{#if i}<span class="plus" aria-hidden="true">+</span>{/if}<Kbd {k} />{/each}
        </span>
      </div>
      {#if errors.startup}<p class="error" role="alert">{errors.startup}</p>{/if}
    </section>

    <section class="card group" aria-labelledby="set-chat">
      {@render head("chat-circle-dots", "Chat and voice", "set-chat")}
      <div class="item">
        <div class="text">
          <span class="name">Speech to text</span>
          <span class="meta" class:warn={speech.now?.state === "failed" || speech.now?.state === "missing"}>{speechLine(speech.now)}</span>
        </div>
        {#if speech.now?.state === "failed" || speech.now?.state === "missing"}
          <button class="btn small" onclick={() => api.speechRetry().catch(() => {})}>
            <Icon name="arrow-clockwise" size={14} />Try again
          </button>
        {/if}
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Let {BRAND.name} see the screen</span>
          <span class="meta">{seeingLine}</span>
        </div>
        <Switch on={!!seeing?.on} label="Let {BRAND.name} see the screen" disabled={!seeing} onchange={setSeeing} />
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Let {BRAND.name} click and type in apps</span>
          <span class="meta">{controlLine}</span>
        </div>
        <Switch on={!!settings?.control} label="Let {BRAND.name} click and type in apps" disabled={!settings} onchange={setControl} />
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Listen to the computer's sound</span>
          <span class="meta" class:warn={!!hearing?.on && !!hearing.trouble}>{hearingLine}</span>
        </div>
        <Switch on={!!hearing?.on} label="Listen to the computer's sound" disabled={!hearing} onchange={setHearing} />
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Answer to "Hey {BRAND.name}"</span>
          <span class="meta" class:warn={!!voice.now?.on && !!voice.now.trouble}>{voiceLine}</span>
        </div>
        <Switch on={!!voice.now?.on} label="Answer to Hey {BRAND.name}" disabled={!voice.now} onchange={setVoice} />
      </div>
      <div class="text">
        <span class="meta">The language you speak to {BRAND.name} in</span>
      </div>
      <div class="models" role="radiogroup" aria-label="The language you speak in">
        {#each LANGUAGES as o (o.label)}
          <button class="chip" role="radio" aria-checked={(settings?.speech_language ?? null) === o.code} onclick={() => setLanguage(o.code)}>{o.label}</button>
        {/each}
      </div>
      {#if voice.now?.on}
        <div class="item">
          <div class="text">
            <span class="name">Read answers aloud</span>
            <span class="meta">Answers to spoken requests are read with the computer's own voices.</span>
          </div>
          <Switch on={voice.speakReplies} label="Read answers aloud" onchange={setSpeakReplies} />
        </div>
      {/if}
      {#if hearing?.on && hearing.lines.length}
        <ol class="heard" aria-label="Heard in the last 20 seconds">
          {#each hearing.lines.slice(-3) as l (l.to)}
            <li><span class="ago num">{Math.max(0, Math.round((now - l.to) / 1000))} s</span><span class="said">{l.text}</span></li>
          {/each}
        </ol>
      {/if}
      <div class="item">
        <div class="text">
          <span class="name">While it works on screen</span>
          <span class="meta">A light runs along the screen's edges while {BRAND.name} opens apps and pages. Esc stops it.</span>
        </div>
        <button class="btn small" onclick={() => api.overlayPreview().catch(() => {})}>
          <Icon name="play" size={14} />Preview
        </button>
      </div>
      <div class="item">
        <div class="text">
          <span class="name">Model server</span>
          <span class="meta" class:warn={!!cs && !chat.checking && (!cs.url || !!cs.error || !cs.models.length)}>{chatLine}</span>
        </div>
        <button class="btn small" onclick={() => checkServer(true)} disabled={chat.checking}>
          <Icon name="arrow-clockwise" size={14} />Check again
        </button>
      </div>
      {#if settings?.local_brain !== false}
        <div class="item">
          <div class="text">
            <span class="name">Local brain</span>
            <span class="meta">{brainLine}</span>
          </div>
          {#if brainBusy}
            <button class="btn small" onclick={() => api.brainStopSetup().catch(() => {})}>
              <Icon name="stop-fill" size={14} />Stop
            </button>
          {:else if canSetUp}
            <button class="btn small primary" onclick={() => api.brainSetup().catch(() => {})}>
              <Icon name="arrow-down" size={14} />Download
            </button>
          {/if}
        </div>
        {#if brainBusy}
          <div class="bar" role="progressbar" aria-label="Download" aria-valuenow={Math.round(brainPct)} aria-valuemin={0} aria-valuemax={100}>
            <span style:width="{brainPct}%"></span>
          </div>
        {/if}
        {#if plan?.models.length}
          <div class="models" role="radiogroup" aria-label="Local model">
            <button class="chip" role="radio" aria-checked={!plan.chosen} onclick={() => useModel(null)}>Best that fits</button>
            {#each plan.models as m (m.path)}
              <button class="chip" role="radio" aria-checked={plan.chosen === m.path} title={m.path} onclick={() => useModel(m.path)}>
                {m.name}<span class="size" class:warn={!m.fits}>{gb(m.size)}{m.fits ? "" : ", too big for the chip"}</span>
              </button>
            {/each}
          </div>
        {/if}
        <div class="text">
          <span class="meta">Unload the model from the GPU when unused for</span>
        </div>
        <div class="models" role="radiogroup" aria-label="Unload the model when unused for">
          {#each IDLE as o (o.minutes)}
            <button class="chip" role="radio" aria-checked={(settings?.brain_idle_minutes ?? 30) === o.minutes} onclick={() => setIdle(o.minutes)}>{o.label}</button>
          {/each}
        </div>
        <div class="save-row">
          <span class="meta faint">Put a GGUF model from Hugging Face in the models folder and it shows up here.</span>
          <button class="btn small" onclick={() => api.brainOpenFolder().catch(() => {})}>
            <Icon name="folder-open" size={14} />Models folder
          </button>
        </div>
      {/if}
      {#if cs && cs.models.length > 1}
        <div class="models" role="radiogroup" aria-label="Model">
          {#each cs.models as m (m)}
            <button class="chip" role="radio" aria-checked={cs.model === m} onclick={() => pickModel(m)}>{m}</button>
          {/each}
        </div>
      {/if}
      <form class="server" onsubmit={saveChat}>
        <label class="field">
          <Icon name="globe-simple" size={15} />
          <input
            bind:value={chatUrl}
            use:typing
            placeholder="Found on its own, or https://…/v1"
            aria-label="Server address"
            spellcheck="false"
            autocomplete="off"
          />
        </label>
        <label class="field">
          <Icon name="lock-key" size={15} />
          <input
            bind:value={chatKey}
            use:typing
            type="password"
            placeholder="Key, for cloud servers only"
            aria-label="Key"
            autocomplete="off"
          />
        </label>
        <div class="save-row">
          <span class="meta faint">The key stays in the settings file on this computer.</span>
          <button class="btn small primary" type="submit">
            {#if chatSaved}<Icon name="check" size={14} />Saved{:else}Save{/if}
          </button>
        </div>
      </form>
      {#if errors.chat}<p class="error" role="alert">{errors.chat}</p>{/if}
    </section>

    <section class="card group" aria-labelledby="set-search">
      {@render head("magnifying-glass", "Web search", "set-search")}
      <div class="text">
        <span class="meta">
          {settings?.search_searxng
            ? "Searches go to your SearXNG."
            : settings?.search_brave_key
              ? "Searches go to Brave Search."
              : "Searches go to DuckDuckGo, which sometimes asks for a human check. A SearXNG address or a Brave Search key avoids that."}
        </span>
      </div>
      <form class="server" onsubmit={saveSearch}>
        <label class="field">
          <Icon name="globe-simple" size={15} />
          <input
            bind:value={searxng}
            use:typing
            placeholder="SearXNG address, like http://127.0.0.1:8888"
            aria-label="SearXNG address"
            spellcheck="false"
            autocomplete="off"
          />
        </label>
        <label class="field">
          <Icon name="lock-key" size={15} />
          <input
            bind:value={braveKey}
            use:typing
            type="password"
            placeholder="Brave Search key"
            aria-label="Brave Search key"
            autocomplete="off"
          />
        </label>
        <div class="save-row">
          <span class="meta faint">SearXNG is used first when both are set. It needs JSON results turned on.</span>
          <button class="btn small primary" type="submit">
            {#if searchSaved}<Icon name="check" size={14} />Saved{:else}Save{/if}
          </button>
        </div>
      </form>
      {#if errors.search}<p class="error" role="alert">{errors.search}</p>{/if}
    </section>
  </div>

  <div class="col">
    <section class="card group" aria-labelledby="set-mail">
      {@render head("envelope-simple", "Mail", "set-mail")}
      <MailCard />
    </section>

    <section class="card group" aria-labelledby="set-colour">
      {@render head("palette", "Colour", "set-colour")}
      <div class="item">
        <div class="text">
          <span class="name">Follow the wallpaper</span>
          <span class="meta">{follow ? "The island takes its colours from the wallpaper." : "Your own colour, below."}</span>
        </div>
        <Switch on={follow} label="Follow the wallpaper" disabled={!settings} onchange={setFollow} />
      </div>
      <div class="pick">
        <div class="swatches" role="radiogroup" aria-label="Accent colour">
          {#each SWATCHES as s, i (s.hex)}
            <button
              id="swatch-{i}"
              class="swatch"
              role="radio"
              aria-checked={chosen === s.hex}
              aria-label={s.name}
              tabindex={i === rovingIndex ? 0 : -1}
              style:--c={s.hex}
              onclick={() => setAccent(s.hex)}
              onkeydown={(e) => swatchKey(e, i)}
            >
              {#if chosen === s.hex}<Icon name="check" size={13} />{/if}
            </button>
          {/each}
        </div>
        <form class="field hex" onsubmit={submitHex}>
          <span class="dot" style:--c={/^#?[0-9a-f]{6}$/i.test(hex.trim()) ? hex.trim().replace(/^#?/, "#") : "transparent"}></span>
          <input
            bind:value={hex}
            use:typing
            placeholder="#4F8FFF"
            aria-label="Accent colour as hex"
            maxlength="7"
            spellcheck="false"
            autocomplete="off"
            class="num"
          />
          <button class="icon-btn set" type="submit" aria-label="Use this colour"><Icon name="arrow-elbow-down-left" size={14} /></button>
        </form>
      </div>
      {#if errors.colour}<p class="error" role="alert">{errors.colour}</p>{/if}
    </section>

    <section class="card group" aria-labelledby="set-weather">
      {@render head("map-pin", "Weather", "set-weather")}
      {#if searching}
        <PlaceSearch ondone={() => (searching = false)} />
      {:else if weather.place}
        <div class="item">
          <div class="text">
            <span class="name ellipsis">{weather.place.name}</span>
            <span class="meta ellipsis">{weather.place.region || "Shown on the Today page"}</span>
          </div>
          <div class="pair">
            <button class="btn small" onclick={() => (searching = true)}>Change</button>
            <button class="btn small quiet" onclick={removePlace}>Remove</button>
          </div>
        </div>
      {:else}
        <div class="item">
          <div class="text">
            <span class="name">No city yet</span>
            <span class="meta">Add one to see the weather on the Today page.</span>
          </div>
          <button class="btn small" onclick={() => (searching = true)}><Icon name="plus" size={14} />Add city</button>
        </div>
      {/if}
      {#if errors.weather}<p class="error" role="alert">{errors.weather}</p>{/if}
    </section>

    <section class="card about" aria-label="About">
      <span class="about-core"><Core mood="awake" look={ui.pointer} size={26} /></span>
      <div class="text">
        <span class="name">{BRAND.name} <span class="meta num">{BRAND.version}</span></span>
        <span class="meta faint">Icons by Phosphor. Type is Mona Sans and Hubot Sans.</span>
      </div>
    </section>
  </div>
</div>

<style>
  .settings {
    min-height: 100%;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    align-content: start;
    gap: 12px;
  }
  .col {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .group {
    padding: 12px 14px 10px;
    display: grid;
    gap: 2px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
    font-size: 13px;
    font-weight: 600;
  }
  .head-icon {
    width: 24px;
    height: 24px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    background: var(--accent-12);
    color: var(--accent-text);
  }
  .item {
    min-height: 44px;
    padding: 6px 0;
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .item + .item {
    box-shadow: inset 0 1px 0 var(--line);
  }
  .text {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 1px;
  }
  .name {
    font-weight: 560;
  }
  .warn {
    color: var(--ember);
  }
  .keys {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .plus {
    font-size: 11px;
    color: var(--text-3);
  }
  .pair {
    display: flex;
    gap: 4px;
  }

  /* ---- chat ---- */

  .heard {
    list-style: none;
    padding: 2px 0 10px;
    display: grid;
    gap: 4px;
  }
  .heard li {
    display: flex;
    gap: 10px;
    padding: 6px 10px;
    border-radius: 10px;
    background: var(--s1);
    font-size: 12.5px;
    color: var(--text-2);
  }
  .heard .ago {
    flex: none;
    width: 30px;
    color: var(--text-3);
    text-align: right;
  }
  .heard .said {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .models {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 2px 0 8px;
  }
  .models .size {
    font-weight: 450;
    color: var(--text-3);
  }
  .models .size.warn {
    color: var(--ember);
  }
  .bar {
    height: 4px;
    margin: 0 0 8px;
    border-radius: 2px;
    background: var(--s2);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.25s linear;
  }
  .server {
    display: grid;
    gap: 8px;
    padding: 4px 0 6px;
  }
  .save-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  /* ---- colour ---- */

  .pick {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px 12px;
    padding: 4px 0 6px;
  }
  .swatches {
    display: flex;
    gap: 6px;
  }
  .swatch {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--c);
    color: #0b0f17;
    box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.2);
    transition: scale var(--t-pop) var(--spring-pop);
  }
  .swatch:hover {
    scale: 1.12;
  }
  .swatch[aria-checked="true"] {
    box-shadow:
      0 0 0 2px var(--panel),
      0 0 0 3.5px var(--c);
  }
  .hex {
    flex: 1 1 130px;
    min-width: 0;
    height: 32px;
    padding: 0 4px 0 10px;
    gap: 8px;
  }
  .dot {
    width: 14px;
    height: 14px;
    flex: none;
    border-radius: 50%;
    background: var(--c);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .set {
    width: 26px;
    height: 26px;
  }

  /* ---- about ---- */

  .about {
    padding: 12px 14px;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .about-core {
    width: 40px;
    height: 40px;
    flex: none;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: var(--bezel);
  }
  @container page (max-width: 600px) {
    .settings {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
