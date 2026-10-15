// Speech to text: whether Lemonade and its Whisper model are ready. Rust starts both
// when Nimbus starts (speech.rs) and reports each step here.
import { listen } from "@tauri-apps/api/event";
import { api, type Speech } from "./api";

export const speech = $state({ now: null as Speech | null });

export function startSpeech() {
  api.speechStatus().then((s) => {
    if (!speech.now) speech.now = s;
  }).catch(() => {});
  const off = listen<Speech>("speech", ({ payload }) => {
    speech.now = payload;
  });
  return () => void off.then((f) => f());
}

/** The status in a few plain words. */
export function speechLine(s: Speech | null): string {
  switch (s?.state) {
    case undefined:
    case "starting":
      return "Starting Lemonade…";
    case "downloading":
      return `Downloading ${s.model} (first start only)…`;
    case "loading":
      return `Loading ${s.model}…`;
    case "ready":
      return `${s.model} is listening, through Lemonade.`;
    case "missing":
      return "Lemonade is not installed on this computer.";
    case "failed":
      return `${s.message}.`;
  }
}
