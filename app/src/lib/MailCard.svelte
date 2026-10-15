<script lang="ts">
  // Mail accounts for the assistant's briefings. Read-only by design: it can list and
  // read mail, never send; a reply opens as a draft that the person sends.
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { BRAND } from "../brand";
  import { api, reason, type MailAccount, type MailProvider, type SignInKind } from "./api";
  import Icon from "./Icon.svelte";
  import { typing } from "./ui.svelte";

  let accounts = $state<MailAccount[]>([]);
  let adding = $state(false);
  let email = $state("");
  let password = $state("");
  let user = $state("");
  let host = $state("");
  let port = $state("");
  let advanced = $state(false);
  let provider = $state<MailProvider | null>(null);
  let busy = $state(false);
  let error = $state("");
  let done = $state("");
  let confirmRemove = $state<string | null>(null);

  // Signing in through the browser.
  const NAME: Record<SignInKind, string> = { google: "Google", microsoft: "Microsoft" };
  let ready = $state<Record<SignInKind, boolean> | null>(null);
  let waiting = $state<SignInKind | null>(null);
  let setup = $state<SignInKind | null>(null);
  let clientId = $state("");
  let clientSecret = $state("");
  let browserError = $state("");

  onMount(() => {
    api
      .mailAccounts()
      .then((a) => (accounts = a))
      .catch(() => {});
    api
      .mailBrowserReady()
      .then((r) => (ready = r))
      .catch(() => {});
    const off = listen<MailAccount[]>("mail-accounts", ({ payload }) => (accounts = payload));
    return () => void off.then((f) => f());
  });

  // What to sign in with depends on the provider: look it up as the address is typed.
  let lookup: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const e = email.trim();
    clearTimeout(lookup);
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(e)) {
      provider = null;
      return;
    }
    lookup = setTimeout(() => {
      api
        .mailProvider(e)
        .then((p) => {
          provider = p;
          if (!advanced) {
            host = p.host;
            port = String(p.port);
          }
        })
        .catch(() => {});
    }, 250);
  });

  const unreadLine = (n: number) => (n === 0 ? "No unread mail." : n === 1 ? "1 unread mail." : `${n} unread mails.`);

  function reset() {
    adding = false;
    email = password = user = host = port = "";
    advanced = false;
    provider = null;
    setup = null;
    error = browserError = "";
  }

  async function withBrowser(kind: SignInKind) {
    if (waiting) return;
    error = browserError = "";
    done = "";
    if (!ready?.[kind]) {
      // Not set up yet: show the one-time steps instead.
      setup = kind;
      clientId = clientSecret = "";
      return;
    }
    setup = null;
    waiting = kind;
    try {
      const r = await api.mailBrowserSignIn(kind, email.trim() || null);
      reset();
      done = `Signed in as ${r.email}. ${unreadLine(r.unread)}`;
    } catch (e) {
      const why = reason(e);
      if (!why.includes("cancelled")) browserError = why;
    } finally {
      waiting = null;
    }
  }

  async function saveSetup(ev: SubmitEvent) {
    ev.preventDefault();
    if (!setup) return;
    browserError = "";
    try {
      await api.mailBrowserSetup(setup, clientId.trim(), clientSecret.trim() || null);
      ready = await api.mailBrowserReady();
      void withBrowser(setup);
    } catch (e) {
      browserError = reason(e);
    }
  }

  function cancelAdding() {
    if (waiting) void api.mailBrowserCancel();
    reset();
  }

  async function signIn(ev: SubmitEvent) {
    ev.preventDefault();
    if (busy) return;
    busy = true;
    error = "";
    done = "";
    try {
      const custom = advanced ? host.trim() || null : null;
      const login = provider?.login || advanced ? user.trim() || null : null;
      const n = await api.mailAdd(email.trim(), password, login, custom, advanced && port ? Number(port) : null);
      reset();
      done = `Signed in. ${unreadLine(n)}`;
    } catch (e) {
      error = reason(e);
    } finally {
      busy = false;
    }
  }

  function remove(a: MailAccount) {
    if (confirmRemove !== a.id) {
      confirmRemove = a.id;
      setTimeout(() => confirmRemove === a.id && (confirmRemove = null), 3000);
      return;
    }
    confirmRemove = null;
    api.mailRemove(a.id).catch((e) => (error = reason(e)));
  }

  function openLink(url: string) {
    api.openLink(url).catch(() => {});
  }
</script>

<div class="mail">
  <p class="meta promise">
    <Icon name="lock-key" size={13} />
    {BRAND.name} reads mail for briefings and never sends any. Replies open as drafts for you to send.
  </p>

  {#each accounts as a (a.id)}
    <div class="item account">
      <span class="avatar" aria-hidden="true">{a.email[0].toUpperCase()}</span>
      <div class="text">
        <span class="name ellipsis">{a.email}</span>
        <span class="meta ellipsis">{a.auth ? `Signed in with ${NAME[a.auth]}` : a.host}</span>
      </div>
      <button class="btn small quiet" class:danger={confirmRemove === a.id} onclick={() => remove(a)}>
        {confirmRemove === a.id ? "Sign out now" : "Sign out"}
      </button>
    </div>
  {/each}

  {#if done}<p class="meta ok" role="status"><Icon name="check" size={13} />{done}</p>{/if}

  {#if adding}
    <div class="ways">
      {#each ["google", "microsoft"] as const as k (k)}
        <button
          type="button"
          class="btn way"
          class:lead={provider?.browser === k}
          class:on={waiting === k || setup === k}
          disabled={!!waiting && waiting !== k}
          onclick={() => withBrowser(k)}
        >
          <span class:spin={waiting === k}><Icon name={waiting === k ? "circle-notch" : k === "google" ? "google-logo" : "microsoft-outlook-logo"} size={16} /></span>
          Continue with {NAME[k]}
        </button>
      {/each}
    </div>

    {#if waiting}
      <div class="hint waiting" role="status">
        <span>Finish signing in with {NAME[waiting]} in your browser. {BRAND.name} never sees your password.</span>
        <button type="button" class="btn small quiet" onclick={() => api.mailBrowserCancel()}>Cancel</button>
      </div>
    {:else if setup}
      <form class="setup" onsubmit={saveSetup}>
        <p class="name">Set up Continue with {NAME[setup]} once</p>
        {#if setup === "google"}
          <ol class="meta">
            <li>
              In Google Cloud, make a project and open Google Auth Platform.
              <button type="button" class="link" onclick={() => openLink("https://console.cloud.google.com/auth/overview")}>Open it</button>
            </li>
            <li>Under Audience, choose External. Add your address as a test user, or publish the app so the sign-in lasts longer than 7 days.</li>
            <li>Under Data access, add the scope <code>https://mail.google.com/</code>.</li>
            <li>Under Clients, create a client of the type Desktop app. Paste its ID and secret here.</li>
          </ol>
        {:else}
          <ol class="meta">
            <li>
              In Microsoft Entra, open App registrations and choose New registration.
              <button type="button" class="link" onclick={() => openLink("https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade")}>Open it</button>
            </li>
            <li>Who can use it: accounts in any organisation, and personal Microsoft accounts.</li>
            <li>Redirect address: the platform Public client (mobile and desktop), with <code>http://localhost</code>.</li>
            <li>Paste the Application (client) ID here.</li>
          </ol>
        {/if}
        <label class="field">
          <Icon name="key" size={15} />
          <input bind:value={clientId} use:typing placeholder={setup === "google" ? "Client ID" : "Application (client) ID"} aria-label="Client ID" spellcheck="false" autocomplete="off" required />
        </label>
        {#if setup === "google"}
          <label class="field">
            <Icon name="lock-key" size={15} />
            <input bind:value={clientSecret} use:typing type="password" placeholder="Client secret" aria-label="Client secret" autocomplete="off" required />
          </label>
        {/if}
        {#if browserError}<p class="error" role="alert">{browserError}</p>{/if}
        <p class="meta faint">This is your own sign-in app. Nothing goes to {BRAND.name}'s makers.</p>
        <div class="row end">
          <span class="acts">
            <button type="button" class="btn small quiet" onclick={() => ((setup = null), (browserError = ""))}>Back</button>
            <button type="submit" class="btn small primary" disabled={!clientId.trim()}><Icon name="check" size={14} />Save and continue</button>
          </span>
        </div>
      </form>
    {:else}
      {#if browserError}<p class="error" role="alert">{browserError}</p>{/if}
      <p class="meta faint">Gmail and Google Workspace, or Outlook, Hotmail and Microsoft 365 mail from work or university. You sign in on their own page in your browser.</p>
    {/if}

    <div class="or" aria-hidden="true"><span>or with a password</span></div>

    <form class="add" onsubmit={signIn}>
      <label class="field">
        <Icon name="envelope-simple" size={15} />
        <input bind:value={email} use:typing type="email" placeholder="you@example.com" aria-label="Mail address" autocomplete="email" spellcheck="false" required />
      </label>
      {#if provider}
        <div class="hint" class:blocked={provider.blocked}>
          <span><strong>{provider.name}.</strong> {provider.hint}</span>
          {#if provider.link}
            <button type="button" class="btn small quiet" onclick={() => openLink(provider!.link!)}>
              <Icon name="arrow-square-out" size={13} />Make one
            </button>
          {/if}
        </div>
      {/if}
      {#if provider?.login || advanced}
        <label class="field">
          <Icon name="user" size={15} />
          <input bind:value={user} use:typing placeholder={provider?.login ?? "Username, if not the address"} aria-label="Username" autocomplete="username" spellcheck="false" required={!!provider?.login} />
        </label>
      {/if}
      <label class="field">
        <Icon name="lock-key" size={15} />
        <input bind:value={password} use:typing type="password" placeholder="Password or app password" aria-label="Password" autocomplete="off" required />
      </label>
      {#if advanced}
        <div class="server">
          <label class="field grow">
            <Icon name="globe-simple" size={15} />
            <input bind:value={host} use:typing placeholder="imap.example.com" aria-label="IMAP server" spellcheck="false" />
          </label>
          <label class="field port">
            <input bind:value={port} use:typing inputmode="numeric" placeholder="993" aria-label="Port" />
          </label>
        </div>
      {/if}
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <div class="row">
        <button type="button" class="link meta" onclick={() => (advanced = !advanced)}>{advanced ? "Hide server" : "Other server"}</button>
        <span class="acts">
          <button type="button" class="btn small quiet" onclick={cancelAdding}>Cancel</button>
          <button type="submit" class="btn small primary" disabled={busy || !email.trim() || !password || (provider?.blocked && !advanced)}>
            <Icon name={busy ? "circle-notch" : "check"} size={14} />{busy ? "Checking…" : "Sign in"}
          </button>
        </span>
      </div>
      <p class="meta faint">The password goes to the system's credential store, not to {BRAND.name}'s files.</p>
    </form>
  {:else}
    <div class="item">
      <div class="text">
        <span class="name">{accounts.length ? "Another account" : "Add a mail account"}</span>
        <span class="meta">Gmail, Outlook, iCloud, GMX, WEB.DE, Yahoo, OVGU, Posteo and most others.</span>
      </div>
      <button class="btn small" onclick={() => ((adding = true), (done = ""))}><Icon name="plus" size={14} />Add</button>
    </div>
  {/if}
</div>

<style>
  .mail {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 8px;
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
  .promise,
  .ok {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    padding: 0 4px;
  }
  .promise :global(svg),
  .ok :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .ok {
    color: var(--done);
  }
  .account .avatar {
    width: 30px;
    height: 30px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent-20);
    color: var(--accent-text);
    font-weight: 600;
    font-size: 13px;
  }
  .add {
    display: grid;
    gap: 8px;
    padding: 4px 0;
  }
  .hint {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 12px;
    background: var(--s1);
    font-size: 12.5px;
    color: var(--text-2);
  }
  .hint span {
    flex: 1;
    min-width: 0;
  }
  .hint strong {
    color: var(--text);
    font-weight: 600;
  }
  .hint.blocked {
    box-shadow: inset 0 0 0 1px var(--bad);
  }
  .server {
    display: flex;
    gap: 8px;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .port {
    width: 84px;
    flex: none;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .row.end {
    justify-content: flex-end;
  }
  .acts {
    display: flex;
    gap: 6px;
  }
  .link {
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .btn.danger {
    color: var(--bad);
  }

  /* Signing in through the browser: two equal buttons, the provider of the typed
     address marked. */
  .ways {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
    gap: 8px;
  }
  .way {
    min-width: 0;
    justify-content: center;
    gap: 8px;
    min-height: 40px;
  }
  .way.lead,
  .way.on {
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .spin {
    display: grid;
    color: var(--accent-text);
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      rotate: 1turn;
    }
  }
  .waiting {
    box-shadow: inset 0 0 0 1px var(--accent-35);
  }
  .setup {
    display: grid;
    gap: 8px;
    padding: 12px;
    border-radius: 14px;
    background: var(--s1);
  }
  .setup ol {
    margin: 0;
    padding-left: 18px;
    display: grid;
    gap: 4px;
  }
  .setup code {
    font-size: 11.5px;
    color: var(--text);
    word-break: break-all;
  }
  .or {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 12px;
    color: var(--text-3);
  }
  .or::before,
  .or::after {
    content: "";
    flex: 1;
    height: 1px;
    background: var(--line);
  }
  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: none;
    }
  }
</style>
