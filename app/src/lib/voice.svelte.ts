// "Hey Nimbus": Rust listens to the microphone (voice.rs) and passes on what is asked
// out loud as "voice-request" events. This sends each one to the chat, like typed words,
// and reads the answer aloud with the system's own voices, telling Rust while it speaks
// so the microphone does not hear it. The chat's microphone button (push to talk) ends
// the same way: its words are sent, and the answer is read aloud.
import { listen } from "@tauri-apps/api/event";
import { api, reason, type AgentEvent, type Talk, type Voice } from "./api";
import { canSend, chat, checkServer, send, SESSION, words, type Answer } from "./chat.svelte";
import { openPanel, ui } from "./ui.svelte";

export const voice = $state({
  now: null as Voice | null,
  speakReplies: true,
  /** The chat run that a spoken request started, whose answer is read aloud. */
  run: null as number | null,
  speaking: false,
  /** The microphone button: idle, listening or writing down. */
  talk: "idle" as "idle" | "listening" | "writing",
  /** Why the last press did not work, shown under the chat. */
  talkError: "",
});

/** The microphone button: start listening, or, while listening, take what was said. */
export async function toggleTalk() {
  voice.talkError = "";
  if (voice.talk === "listening") {
    await api.talkStop(true).catch(() => {});
    return;
  }
  if (voice.talk !== "idle") return;
  stopSpeaking();
  try {
    voice.talk = "listening";
    await api.talkStart();
  } catch (e) {
    voice.talk = "idle";
    voice.talkError = reason(e);
  }
}

/** Drop what is being said (Escape). */
export function cancelTalk() {
  if (voice.talk === "listening") void api.talkStop(false).catch(() => {});
}

function onTalk(t: Talk) {
  if (t.state === "listening" || t.state === "writing") {
    voice.talk = t.state;
    return;
  }
  voice.talk = "idle";
  if (t.state === "failed") voice.talkError = t.message;
  else if (t.text) void onRequest(t.text);
}

let foldTimer: ReturnType<typeof setTimeout> | undefined;

/** Words for the ear: no code, links or markdown marks. */
export function forSpeech(text: string) {
  return text
    .replace(/```[\s\S]*?(```|$)/g, " ")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/https?:\/\/\S+/g, "a link")
    .replace(/^\s{0,3}(#{1,6}|[-*+]|\d+[.)])\s+/gm, "")
    .replace(/[*_~|>#]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** A German answer gets a German voice; anything else, the system's default. */
function pickVoice(text: string): SpeechSynthesisVoice | undefined {
  const german = (text.match(/\b(der|die|das|und|nicht|ist|ich|Sie|eine?|mit|für)\b/g) ?? []).length >= 3;
  const lang = german ? "de" : (navigator.language || "en").slice(0, 2);
  const all = speechSynthesis.getVoices().filter((v) => v.lang.toLowerCase().startsWith(lang));
  // The natural-sounding voices first, where the system has them.
  return all.find((v) => /natural|neural|online/i.test(v.name)) ?? all.find((v) => v.default) ?? all[0];
}

export function stopSpeaking() {
  if (!("speechSynthesis" in window)) return;
  speechSynthesis.cancel();
}

export function speak(text: string) {
  const said = forSpeech(text);
  if (!said || !("speechSynthesis" in window)) return;
  speechSynthesis.cancel();
  const u = new SpeechSynthesisUtterance(said);
  const v = pickVoice(said);
  if (v) {
    u.voice = v;
    u.lang = v.lang;
  }
  const done = () => {
    voice.speaking = false;
    api.voiceSpeaking(false).catch(() => {});
    // The panel opened for the request folds a little after the answer.
    clearTimeout(foldTimer);
    foldTimer = setTimeout(() => (ui.sticky = false), 6000);
  };
  u.onend = done;
  u.onerror = done;
  voice.speaking = true;
  api.voiceSpeaking(true).catch(() => {});
  speechSynthesis.speak(u);
}

async function onRequest(text: string) {
  stopSpeaking();
  clearTimeout(foldTimer);
  await openPanel({ tab: "chat", sticky: true });
  // Which model the chat uses is only looked up when a page shows it: look now, or a
  // request heard before the Chat page was opened finds no model.
  if (!chat.status?.model && chat.replying == null) await checkServer(true);
  if (!canSend()) {
    const why = chat.replying != null ? "I'm still working on the last request." : "No model is set up for the chat yet. You can choose one in Settings.";
    if (voice.speakReplies) speak(why);
    return;
  }
  if (await send(text)) voice.run = chat.replying;
}

function answerOf(run: number): Answer | undefined {
  for (let i = chat.turns.length - 1; i >= 0; i--) {
    const t = chat.turns[i];
    if (t.role === "assistant" && t.run === run) return t;
  }
}

function onAgent(e: AgentEvent) {
  if (e.session !== SESSION) return;
  // The run id arrives after the request was sent; the first event of the run fixes it.
  if (voice.run === -1) voice.run = e.run;
  if (e.run !== voice.run) return;
  if (e.kind === "ask") {
    // Approvals are never given by voice: the person answers on screen.
    if (voice.speakReplies) speak(`I need your OK on screen first: ${e.title}.`);
    return;
  }
  if (e.kind !== "done" && e.kind !== "failed" && e.kind !== "stopped") return;
  voice.run = null;
  if (!voice.speakReplies || e.kind === "stopped") return;
  // Let the chat page apply this last event first.
  queueMicrotask(() => {
    const t = answerOf(e.run);
    const said = e.kind === "failed" ? "Sorry, that did not work. The details are in the chat." : t ? words(t) : "";
    if (said) speak(said);
  });
}

export function startVoice() {
  api.voiceGet().then((v) => {
    if (!voice.now) voice.now = v;
  }).catch(() => {});
  api.settingsGet().then((s) => (voice.speakReplies = s.speak_replies ?? false)).catch(() => {});
  // The list of voices fills in a moment after the page loads.
  if ("speechSynthesis" in window) speechSynthesis.getVoices();
  const offs = [
    listen<Voice>("voice", ({ payload }) => (voice.now = payload)),
    listen<{ text: string }>("voice-request", ({ payload }) => void onRequest(payload.text)),
    listen<Talk>("talk", ({ payload }) => onTalk(payload)),
    listen<AgentEvent>("agent", ({ payload }) => onAgent(payload)),
  ];
  return () => {
    stopSpeaking();
    for (const off of offs) void off.then((f) => f());
  };
}
