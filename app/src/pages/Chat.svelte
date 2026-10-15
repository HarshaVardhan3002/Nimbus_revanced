<script lang="ts">
  // Chat: type to the assistant. It is the way in that always works, whether or not
  // voice does. The agent answers here, and each thing it does shows as a step; when
  // a step needs the person's OK it asks right in the conversation. Routines (saved
  // jobs the agent does on its own) live in the second view.
  import { onMount, tick } from "svelte";
  import { BRAND } from "../brand";
  import { api, reason } from "../lib/api";
  import { canSend, chat, checkServer, decide, newChat, retry, send, shown, stop, thinking, words, type Answer } from "../lib/chat.svelte";
  import Routines from "../lib/Routines.svelte";
  import { fade } from "svelte/transition";
  import Icon from "../lib/Icon.svelte";
  import Kbd from "../lib/Kbd.svelte";
  import { selectTab, typing, ui } from "../lib/ui.svelte";
  import { cancelTalk, toggleTalk, voice } from "../lib/voice.svelte";

  let input = $state<HTMLTextAreaElement | null>(null);
  let scroller = $state<HTMLElement | null>(null);
  let view = $state<"chat" | "routines">("chat");
  let copied = $state<number | null>(null);
  let copyError = $state("");
  /** Follow new words to the bottom, unless the user scrolled up to read. */
  let pinned = true;

  let st = $derived(chat.status);
  let ready = $derived(!!st?.url && !!st.model);
  let replying = $derived(chat.replying != null);
  let local = $derived(!!st?.url && /\/\/(localhost|127\.0\.0\.1)[:/]/.test(st.url));

  onMount(() => {
    checkServer();
    // The panel folding (or another tab) drops what the microphone button was hearing.
    return cancelTalk;
  });

  // Opened to type (the core, or Ctrl+Alt+Enter): the cursor goes in the box.
  $effect(() => {
    if (ui.wantInput && ready && input) {
      ui.wantInput = false;
      input.focus();
    }
  });

  // Keep the newest words in view while they stream.
  $effect(() => {
    const last = chat.turns.at(-1);
    if (last?.role === "assistant") {
      void last.parts.length;
      void (last.parts.at(-1) as { text?: string } | undefined)?.text;
    }
    void chat.turns.length;
    if (pinned && scroller) tick().then(() => scroller && (scroller.scrollTop = scroller.scrollHeight));
  });

  function onScroll() {
    if (!scroller) return;
    pinned = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24;
  }

  function submit(e?: Event) {
    e?.preventDefault();
    if (!chat.draft.trim() || !canSend()) return;
    const text = chat.draft;
    chat.draft = "";
    pinned = true;
    tick().then(grow);
    void send(text);
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape" && voice.talk === "listening") {
      e.preventDefault();
      cancelTalk();
    } else if (e.key === "Escape" && replying) {
      // The first Escape stops the reply; the next one folds the panel.
      e.preventDefault();
      stop();
    }
  }

  /** The box grows with what is typed, up to five lines. */
  function grow() {
    if (!input) return;
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, 132)}px`;
  }

  async function copy(id: number, text: string) {
    copyError = "";
    try {
      await api.clipboardWrite(text);
      copied = id;
      setTimeout(() => copied === id && (copied = null), 1600);
    } catch (e) {
      copyError = `Not copied: ${reason(e)}. Select the text and press Ctrl+C instead.`;
    }
  }

  function openSettings() {
    selectTab("settings");
  }

  const STEP_ICON = { running: "circle-notch", done: "check", failed: "x", blocked: "prohibit", declined: "hand" } as const;

  /** Waiting for the first words, with nothing else to show yet. */
  function quiet(t: Answer) {
    return t.state === "working" && !t.parts.some((p) => p.kind !== "text" || shown(p.text));
  }

  /** Text and fenced code blocks, in order. */
  function parts(text: string) {
    const out: { code: boolean; text: string }[] = [];
    const re = /```[^\n]*\n?([\s\S]*?)(```|$)/g;
    let at = 0;
    for (let m; (m = re.exec(text)); ) {
      if (m.index > at) out.push({ code: false, text: text.slice(at, m.index) });
      out.push({ code: true, text: m[1].replace(/\n$/, "") });
      at = re.lastIndex;
      if (!m[0]) break;
    }
    if (at < text.length) out.push({ code: false, text: text.slice(at) });
    return out;
  }
</script>

<div class="chat">
  <header class="bar">
    {#if st?.url && st.model}
      <span class="where meta ellipsis" title={st.url}>
        <span class="led" aria-hidden="true"></span>{st.model}<span class="faint"> · {st.server}{local ? ", on this computer" : ""}</span>
      </span>
    {:else}
      <span class="where meta">{chat.checking ? "Looking for a model…" : st?.url ? `${st.server} · no chat model yet` : "No model server"}</span>
    {/if}
    <span class="acts">
      {#if view === "chat" && chat.turns.length}
        <button class="btn small quiet" onclick={newChat}><Icon name="note-pencil" size={14} />New chat</button>
      {/if}
      <span class="switch" role="tablist" aria-label="Chat or routines">
        <button role="tab" aria-selected={view === "chat"} class:on={view === "chat"} onclick={() => (view = "chat")}>Chat</button>
        <button role="tab" aria-selected={view === "routines"} class:on={view === "routines"} onclick={() => (view = "routines")}>Routines</button>
      </span>
    </span>
  </header>

  {#if view === "routines"}
    <Routines {ready} />
  {:else}

  <div class="scroll" bind:this={scroller} onscroll={onScroll} aria-live="polite" aria-busy={replying}>
    {#if !st}
      <div class="empty" role="status">
        <span class="spin"><Icon name="circle-notch" size={22} /></span>
        <span>Looking for a model on this computer…</span>
      </div>
    {:else if !st.url || st.error || !st.model}
      <div class="empty">
        <span class="badge"><Icon name="plugs" size={26} /></span>
        {#if !st.url}
          <strong>No model server found</strong>
          <span>Start Lemonade, Ollama or LM Studio on this computer, or add a server address in Settings.</span>
        {:else if st.error}
          <strong>{st.server} did not answer</strong>
          <span>{st.error[0].toUpperCase() + st.error.slice(1)}. Check the address in Settings, then try again.</span>
        {:else}
          <strong>{st.server} has no chat model</strong>
          <span>It is running, but only with models that cannot chat (like speech to text). Load a chat model in {st.server}, then try again.</span>
        {/if}
        <div class="row">
          <button class="btn small" onclick={() => checkServer(true)} disabled={chat.checking}>
            <Icon name={chat.checking ? "circle-notch" : "arrow-clockwise"} size={14} />{chat.checking ? "Checking…" : "Try again"}
          </button>
          <button class="btn small quiet" onclick={openSettings}><Icon name="gear-six" size={14} />Settings</button>
        </div>
      </div>
    {:else if !chat.turns.length}
      <div class="empty hello">
        <strong class="display">What can I do for you?</strong>
        <span>Ask a question or hand over a job: look something up, set a reminder, open an app. Nothing here is kept after {BRAND.name} closes.</span>
        <span class="keys meta"><Kbd k="Ctrl" /><Kbd k="Alt" /><Kbd k="enter" /> opens this box from any app</span>
      </div>
    {:else}
      <ol class="turns">
        {#each chat.turns as t (t.id)}
          <li class="turn {t.role}" class:failed={t.role === "assistant" && t.state === "failed"}>
            {#if t.role === "user"}
              <p class="bubble">{t.text}</p>
            {:else}
              {@const text = words(t)}
              {#if t.from}<span class="from meta"><Icon name="lightning" size={12} />{t.from}</span>{/if}
              {#each t.parts as p, i (i)}
                {#if p.kind === "text"}
                  {@const said = shown(p.text)}
                  {#if said}
                    <div class="answer">
                      {#each parts(said) as q, k (k)}
                        {#if q.code}<pre><code>{q.text}</code></pre>{:else}<p>{q.text}</p>{/if}
                      {/each}{#if t.state === "working" && i === t.parts.length - 1}<span class="caret" aria-hidden="true"></span>{/if}
                    </div>
                  {/if}
                {:else if p.kind === "step"}
                  {@const next = t.parts[i + 1]}
                  {#if !(next?.kind === "ask" && next.answer == null && next.title === p.title)}
                  <div class="step {p.state}" title={p.detail ?? ""}>
                    <span class="step-icon" class:spin={p.state === "running"}><Icon name={STEP_ICON[p.state]} size={12} /></span>
                    <span class="ellipsis">{p.title}</span>
                    {#if p.detail && p.state !== "running"}<span class="meta faint ellipsis">{p.detail}</span>{/if}
                  </div>
                  {/if}
                {:else if p.kind === "ask"}
                  {#if p.answer == null}
                  <div class="ask" role="group" aria-label="Permission" out:fade={{ duration: 120 }}>
                    <span class="ask-icon"><Icon name="shield-check" size={18} /></span>
                    <span class="ask-text">
                      <strong>{p.title}</strong>
                      <span class="meta">{p.what}</span>
                    </span>
                    <span class="ask-acts">
                      <button class="btn small quiet" onclick={() => decide(p, false)}>Don't allow</button>
                      <button class="btn small primary" onclick={() => decide(p, true)}>Allow</button>
                    </span>
                  </div>
                  {/if}
                {:else}
                  <p class="note meta faint">{p.text}</p>
                {/if}
              {/each}
              {#if quiet(t)}
                <p class="waiting meta">
                  <span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>
                  {thinking(t.parts.map((p) => (p.kind === "text" ? p.text : "")).join("")) ? "Thinking it through…" : "Working on it…"}
                </p>
              {/if}
              {#if t.state === "failed"}
                <p class="error">{t.error}</p>
              {/if}
              {#if t.state !== "working"}
                <div class="tools">
                  {#if t.state === "stopped"}<span class="meta faint">Stopped</span>{/if}
                  {#if text}
                    <button class="btn small quiet" onclick={() => copy(t.id, text)}>
                      <Icon name={copied === t.id ? "check" : "copy"} size={14} />{copied === t.id ? "Copied" : "Copy"}
                    </button>
                  {/if}
                  {#if t.state === "failed" && t.session === "chat" && t.id === chat.turns.at(-1)?.id}
                    <button class="btn small" onclick={retry} disabled={!canSend()}><Icon name="arrow-clockwise" size={14} />Try again</button>
                  {/if}
                </div>
              {/if}
            {/if}
          </li>
        {/each}
      </ol>
    {/if}
  </div>

  {#if copyError}<p class="error foot" role="alert">{copyError}</p>{/if}
  {#if voice.talkError}<p class="error foot" role="alert">{voice.talkError}</p>{/if}

  <form class="composer" class:off={!ready} onsubmit={submit}>
    <textarea
      id="chat-input"
      bind:this={input}
      bind:value={chat.draft}
      use:typing
      oninput={grow}
      onkeydown={key}
      rows="1"
      placeholder={ready ? `Message ${BRAND.name}` : "Connect a model to start"}
      aria-label="Message {BRAND.name}"
      disabled={!ready}
      maxlength="8000"
    ></textarea>
    {#if !replying}
      <button
        class="mic"
        class:on={voice.talk === "listening"}
        type="button"
        onclick={toggleTalk}
        disabled={!ready || voice.talk === "writing"}
        aria-label={voice.talk === "listening" ? "Done talking" : voice.talk === "writing" ? "Writing down what you said" : "Talk to " + BRAND.name}
        title={voice.talk === "listening" ? "Listening. Press again when you are done, or Escape to drop it." : "Talk instead of typing"}
      >
        {#if voice.talk === "writing"}
          <span class="spin"><Icon name="circle-notch" size={16} /></span>
        {:else}
          <Icon name={voice.talk === "listening" ? "microphone-fill" : "microphone"} size={16} />
        {/if}
      </button>
    {/if}
    {#if replying}
      <button class="send stop" type="button" onclick={stop} aria-label="Stop"><Icon name="stop-fill" size={14} /></button>
    {:else}
      <button class="send" type="submit" aria-label="Send" disabled={!ready || !chat.draft.trim()}>
        <Icon name="paper-plane-right" size={16} />
      </button>
    {/if}
  </form>
  {/if}
</div>

<style>
  .chat {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .bar {
    min-height: 28px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 0 4px;
  }
  .where {
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  .switch {
    display: flex;
    padding: 2px;
    border-radius: 999px;
    background: var(--s2);
  }
  .switch button {
    height: 24px;
    padding: 0 12px;
    border-radius: 999px;
    color: var(--text-2);
    font-size: 12px;
    font-weight: 500;
    transition:
      background var(--t-fade) var(--out),
      color var(--t-fade) var(--out);
  }
  .switch button.on {
    background: var(--s4);
    color: var(--text);
  }
  .led {
    width: 7px;
    height: 7px;
    flex: none;
    border-radius: 50%;
    background: var(--done);
    box-shadow: 0 0 8px var(--done);
  }

  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin-right: -8px;
    padding-right: 8px;
    display: flex;
    flex-direction: column;
  }
  .scroll > .empty {
    flex: 1;
    padding: 0 40px;
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
  .row {
    display: flex;
    gap: 8px;
    margin-top: 6px;
  }
  .hello strong {
    font-size: 26px;
    letter-spacing: -0.01em;
  }
  .hello .keys {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 8px;
  }
  .spin {
    display: grid;
    color: var(--accent-text);
    animation: spin 0.9s linear infinite;
  }

  /* ---- the conversation ---- */

  .turns {
    list-style: none;
    padding: 4px 0 8px;
    margin-top: auto;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 14px;
  }
  .turn {
    min-width: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 6px;
    animation: arrive 260ms var(--out) both;
  }
  .turn.user {
    justify-items: end;
  }
  .bubble {
    max-width: min(80%, 520px);
    padding: 9px 14px;
    border-radius: 18px 18px 6px 18px;
    background: var(--accent-20);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .answer {
    max-width: 92%;
    padding: 0 4px;
    line-height: 1.55;
    overflow-wrap: anywhere;
  }
  .answer p {
    display: inline;
    white-space: pre-wrap;
  }
  pre {
    margin: 8px 0;
    padding: 10px 12px;
    border-radius: 12px;
    background: var(--bezel);
    box-shadow: inset 0 0 0 1px var(--line);
    overflow-x: auto;
    font: 12px/1.5 ui-monospace, "Cascadia Code", Consolas, monospace;
  }
  .caret {
    display: inline-block;
    width: 8px;
    height: 15px;
    margin-left: 2px;
    vertical-align: -2px;
    border-radius: 2px;
    background: var(--accent);
    animation: blink 1s steps(2, start) infinite;
  }
  .waiting {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px;
  }
  .dots {
    display: flex;
    gap: 4px;
  }
  .dots i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent-text);
    animation: bob 1.1s var(--out) infinite;
  }
  .dots i:nth-child(2) {
    animation-delay: 120ms;
  }
  .dots i:nth-child(3) {
    animation-delay: 240ms;
  }
  .from {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 4px;
    color: var(--accent-text);
  }
  .step {
    max-width: 92%;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 4px;
    min-height: 22px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .step .meta {
    flex: 1 1 0;
    min-width: 0;
  }
  .step-icon {
    width: 18px;
    height: 18px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--s2);
    color: var(--text-2);
  }
  .step.done .step-icon {
    color: var(--done);
  }
  .step.failed .step-icon,
  .step.blocked .step-icon {
    color: var(--bad);
  }
  .step-icon.spin {
    color: var(--accent-text);
    animation: spin 0.9s linear infinite;
  }
  .ask {
    max-width: min(100%, 560px);
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 10px 10px 12px;
    border-radius: 16px;
    background: var(--s2);
    box-shadow: inset 0 0 0 1.5px var(--accent-35);
    flex-wrap: wrap;
  }
  .ask-icon {
    width: 34px;
    height: 34px;
    flex: none;
    border-radius: 11px;
    display: grid;
    place-items: center;
    background: var(--accent-20);
    color: var(--accent-text);
  }
  .ask-text {
    flex: 1 1 220px;
    min-width: 0;
    display: grid;
    gap: 2px;
  }
  .ask-text strong {
    font-weight: 600;
  }
  .ask-text .meta {
    overflow-wrap: anywhere;
  }
  .ask-acts {
    display: flex;
    gap: 6px;
    flex: none;
    margin-left: auto;
  }
  .note {
    padding: 0 4px;
    font-size: 12px;
  }
  .turn .error {
    padding: 0 4px;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 28px;
    opacity: 0;
    transition: opacity var(--t-fade) var(--out);
  }
  .tools .btn {
    height: 26px;
    padding: 0 10px;
  }
  .turn:hover .tools,
  .tools:focus-within,
  .failed .tools,
  .turn:last-child .tools {
    opacity: 1;
  }

  /* ---- the box ---- */

  .composer {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 6px 6px 6px 16px;
    border-radius: 22px;
    background: var(--s2);
    transition: box-shadow var(--t-fade) var(--out);
  }
  .composer:focus-within {
    box-shadow: inset 0 0 0 1.5px var(--accent-35);
  }
  .composer.off {
    opacity: 0.55;
  }
  textarea {
    flex: 1;
    min-width: 0;
    height: 32px;
    max-height: 132px;
    padding: 6px 0;
    resize: none;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--text);
    font: inherit;
    line-height: 20px;
  }
  textarea::placeholder {
    color: var(--text-3);
  }
  .send {
    width: 32px;
    height: 32px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--ink);
    transition:
      opacity var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }
  .send:disabled {
    opacity: 0.35;
  }
  .send:active:not(:disabled) {
    scale: 0.9;
  }
  .mic {
    width: 32px;
    height: 32px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: var(--text-2);
    transition:
      background var(--t-fade) var(--out),
      color var(--t-fade) var(--out),
      scale var(--t-pop) var(--spring-pop);
  }
  .mic:hover:not(:disabled) {
    background: var(--s3, var(--s2));
    color: var(--text);
  }
  .mic:disabled {
    opacity: 0.35;
  }
  .mic:active:not(:disabled) {
    scale: 0.9;
  }
  /* Listening: lit, with a soft ring that breathes while the microphone is open. */
  .mic.on {
    background: var(--accent);
    color: var(--ink);
    animation: listen 1.6s ease-in-out infinite;
  }
  @keyframes listen {
    50% {
      box-shadow: 0 0 0 5px var(--accent-35);
    }
  }
  .send.stop {
    background: var(--s4);
    color: var(--text);
  }
  .foot {
    padding: 0 4px;
  }

  @keyframes arrive {
    from {
      opacity: 0;
      translate: 0 8px;
    }
  }
  @keyframes blink {
    to {
      opacity: 0;
    }
  }
  @keyframes bob {
    0%,
    60%,
    100% {
      translate: 0 0;
      opacity: 0.5;
    }
    30% {
      translate: 0 -4px;
      opacity: 1;
    }
  }
  @keyframes spin {
    to {
      rotate: 1turn;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .turn,
    .caret,
    .dots i,
    .spin,
    .step-icon.spin,
    .mic.on {
      animation: none;
    }
  }
</style>
