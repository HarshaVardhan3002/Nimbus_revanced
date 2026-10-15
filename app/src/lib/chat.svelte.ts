// The conversation with the agent, as the Chat page shows it. The agent itself runs in
// Rust (agent/), which keeps the model's side of the conversation and reports each
// run as "agent" events; this turns them into turns. Routines and reminders that wake
// the agent show up here too, under their own name. Memory only: gone when the app quits.
import { listen } from "@tauri-apps/api/event";
import { api, reason, type AgentEvent, type Brain, type ChatStatus, type StepState } from "./api";
import { ui } from "./ui.svelte";

export const SESSION = "chat";

/** One piece of an answer, in the order it happened. */
export type Part =
  | { kind: "text"; text: string }
  | { kind: "step"; call: string; title: string; state: StepState; detail: string | null }
  | { kind: "ask"; ask: number; title: string; what: string; answer: boolean | null }
  | { kind: "note"; text: string };

export type Turn =
  | { id: number; role: "user"; text: string }
  | {
      id: number;
      role: "assistant";
      run: number;
      session: string;
      /** A routine or reminder's name, for runs that started on their own. */
      from: string | null;
      parts: Part[];
      state: "working" | "done" | "stopped" | "failed";
      error?: string;
    };
export type Answer = Extract<Turn, { role: "assistant" }>;

export const chat = $state({
  status: null as ChatStatus | null,
  checking: false,
  checkedAt: 0,
  turns: [] as Turn[],
  draft: "",
  /** The chat run going on now. */
  replying: null as number | null,
  /** Questions waiting for the person, across all runs. */
  asking: 0,
  /** The local brain Nimbus starts on the GPU. */
  brain: null as Brain | null,
});

let seq = Date.now();
/** Called when a run finishes on its own (a routine), so the notch can say so. */
let onFinished: (t: Answer) => void = () => {};

/** Find the server again (after a change in Settings, or when asked). */
export async function checkServer(force = false) {
  if (chat.checking || (!force && chat.status && Date.now() - chat.checkedAt < 30_000)) return;
  chat.checking = true;
  try {
    chat.status = await api.chatStatus();
  } catch (e) {
    chat.status = { url: null, server: "", models: [], model: null, custom: false, error: reason(e) };
  } finally {
    chat.checking = false;
    chat.checkedAt = Date.now();
  }
}

export function canSend() {
  return !!chat.status?.url && !!chat.status.model && chat.replying == null;
}

function answer(run: number): Answer | undefined {
  for (let i = chat.turns.length - 1; i >= 0; i--) {
    const t = chat.turns[i];
    if (t.role === "assistant" && t.run === run) return t;
  }
}

export async function send(text: string) {
  text = text.trim();
  if (!text || !canSend()) return false;
  chat.turns.push({ id: seq++, role: "user", text });
  chat.replying = -1;
  try {
    const run = await api.agentSend(SESSION, text);
    if (chat.replying === -1) chat.replying = run;
  } catch (e) {
    chat.replying = null;
    chat.turns.push({ id: seq++, role: "assistant", run: -1, session: SESSION, from: null, parts: [], state: "failed", error: reason(e) });
  }
  return true;
}

/** Send the last message again after a failed reply. */
export function retry() {
  const last = [...chat.turns].reverse().find((t) => t.role === "user");
  if (!last || !canSend()) return;
  chat.turns = chat.turns.filter((t) => t.id !== last.id && !(t.role === "assistant" && t.state === "failed" && t.session === SESSION));
  void send(last.text);
}

export function stop() {
  api.agentStop(SESSION).catch(() => {});
}

export function newChat() {
  chat.turns = chat.turns.filter((t) => t.role === "assistant" && t.state === "working" && t.session !== SESSION);
  chat.replying = null;
  api.agentReset(SESSION).catch(() => {});
}

export function decide(p: Extract<Part, { kind: "ask" }>, allowed: boolean) {
  if (p.answer != null) return;
  p.answer = allowed;
  // Who answered shows in the console, to trace an answer nobody meant to give.
  console.info("[nimbus] answer", p.ask, allowed, new Error().stack);
  // Refused (it came before the question could be read): the question stays on screen.
  api.agentAnswer(p.ask, allowed).catch((e) => {
    console.info("[nimbus] answer refused", p.ask, reason(e));
    p.answer = null;
  });
}

/** How long a question is on screen before its buttons take a click (as in agent/mod.rs). */
const READ_MS = 600;

/**
 * For a question's buttons: they take clicks only after a moment on screen, so a click
 * meant for what was there before cannot answer a question nobody has read.
 */
export function readFirst(node: HTMLElement) {
  const buttons = [...node.querySelectorAll("button")];
  for (const b of buttons) b.disabled = true;
  const t = setTimeout(() => {
    for (const b of buttons) b.disabled = false;
  }, READ_MS);
  return { destroy: () => clearTimeout(t) };
}

function lastText(t: Answer) {
  const last = t.parts.at(-1);
  if (last?.kind === "text") return last;
  const fresh = { kind: "text" as const, text: "" };
  t.parts.push(fresh);
  return t.parts[t.parts.length - 1] as typeof fresh;
}

function onEvent(e: AgentEvent) {
  let t = answer(e.run);
  if (!t) {
    if (e.kind === "answered") return;
    chat.turns.push({
      id: seq++,
      role: "assistant",
      run: e.run,
      session: e.session,
      from: e.kind === "start" ? e.title : null,
      parts: [],
      state: "working",
    });
    t = chat.turns[chat.turns.length - 1] as Answer;
    if (e.session === SESSION) chat.replying = e.run;
    ui.busy++;
  }
  switch (e.kind) {
    case "start":
      t.from = e.title;
      break;
    case "token":
      lastText(t).text += e.text;
      break;
    case "step": {
      const s = t.parts.find((p) => p.kind === "step" && p.call === e.call);
      if (s && s.kind === "step") Object.assign(s, { title: e.title, state: e.state, detail: e.detail });
      else t.parts.push({ kind: "step", call: e.call, title: e.title, state: e.state, detail: e.detail });
      break;
    }
    case "ask":
      t.parts.push({ kind: "ask", ask: e.ask, title: e.title, what: e.what, answer: null });
      chat.asking++;
      break;
    case "answered": {
      const a = t.parts.find((p) => p.kind === "ask" && p.ask === e.ask);
      if (a && a.kind === "ask") a.answer ??= e.allowed;
      chat.asking = Math.max(0, chat.asking - 1);
      break;
    }
    case "compacted":
      t.parts.push({ kind: "note", text: "Older messages were summed up to make room." });
      break;
    case "done":
    case "stopped":
    case "failed":
      t.state = e.kind === "done" ? "done" : e.kind === "stopped" ? "stopped" : "failed";
      if (e.kind === "failed") t.error = e.message;
      ui.busy = Math.max(0, ui.busy - 1);
      if (e.session === SESSION && chat.replying === e.run) chat.replying = null;
      // An unanswered question ends with its run.
      for (const p of t.parts) if (p.kind === "ask" && p.answer == null) p.answer = false;
      if (e.session !== SESSION) onFinished(t);
      break;
  }
}

/** Listen to the agent for the life of the app (runs go on with the Chat page closed). */
export function startChat(finished: (t: Answer) => void) {
  onFinished = finished;
  const off = listen<AgentEvent>("agent", ({ payload }) => onEvent(payload));
  api.brainGet().then((b) => {
    if (!chat.brain) chat.brain = b;
  }).catch(() => {});
  // Once the brain is loaded, the chat finds it straight away.
  const offBrain = listen<Brain>("brain", ({ payload }) => {
    chat.brain = payload;
    if (payload.state === "ready") void checkServer(true);
  });
  return () => {
    void off.then((f) => f());
    void offBrain.then((f) => f());
  };
}

/** The reply without its hidden reasoning (some models think inside <think> tags first). */
export function shown(text: string) {
  return text.replace(/<think>[\s\S]*?(<\/think>|$)/g, "").trimStart();
}

/** The model is still inside its <think> block. */
export function thinking(text: string) {
  const open = text.lastIndexOf("<think>");
  return open >= 0 && text.indexOf("</think>", open) < 0;
}

/** All the answer's words, without reasoning. */
export function words(t: Answer) {
  return shown(t.parts.map((p) => (p.kind === "text" ? p.text : "")).join(""));
}
