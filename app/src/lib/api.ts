// Typed wrappers over the Rust commands (src-tauri/src/commands.rs). Every setter
// returns what the system reports afterwards, never what was asked for.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";

/** URL the web view can load for a cached file on disk (thumbnails, icons, clips). */
export const fileUrl = (path: string) => convertFileSrc(path);

export type Palette = {
  accent: string;
  ink: string;
  chest: string;
  done: string;
  ember: string;
  panel: string;
  hue: number;
  neutral: boolean;
};

export type Volume = { percent: number; muted: boolean; device: string | null };

export type NowPlaying = {
  app: string;
  title: string;
  artist: string;
  album: string;
  playing: boolean;
  position_ms: number | null;
  duration_ms: number | null;
  artwork: string | null;
};
export type MediaAction = "play_pause" | "play" | "pause" | "next" | "previous";

export type Radio = { on: boolean; detail: string | null; signal: number | null };
export type Radios = { wifi: Radio | null; bluetooth: Radio | null };
export type RadioKind = "wifi" | "bluetooth";

export type PowerAction = "lock" | "sleep" | "restart" | "shut_down";

export type Battery = { percent: number; charging: boolean; plugged_in: boolean };
export type Proc = { name: string; count: number; cpu_percent: number; mem: number };
export type Snapshot = {
  cpu_percent: number;
  cpu_per_core: number[];
  cpu_name: string;
  cpu_mhz: number | null;
  processes: number;
  mem_used: number;
  mem_total: number;
  swap_used: number;
  swap_total: number;
  disk_used: number;
  disk_total: number;
  net_rx_per_s: number;
  net_tx_per_s: number;
  gpu_percent: number | null;
  gpu_mem_used: number | null;
  gpu_name: string | null;
  battery: Battery | null;
  uptime_s: number;
  top_cpu: Proc[];
  top_mem: Proc[];
};

export type Settings = {
  hide_from_capture: boolean;
  wallpaper_dir: string | null;
  clipboard_history: boolean;
  accent: string | null;
  chat_url: string | null;
  chat_model: string | null;
  chat_key: string | null;
  speech_model: string | null;
  speech_language?: string | null;
  context_window: number;
  search_searxng: string | null;
  search_brave_key: string | null;
  file_search?: boolean;
  voice?: boolean;
  speak_replies?: boolean;
  local_brain?: boolean;
  brain_model?: string | null;
  /** Minutes unused before the local model is unloaded; 0 keeps it loaded. */
  brain_idle_minutes?: number;
  control?: boolean;
};
export type ChatStatus = {
  url: string | null;
  server: string;
  models: string[];
  model: string | null;
  custom: boolean;
  error: string | null;
};
/** Speech to text through Lemonade, which Nimbus starts and readies on its own. */
export type Speech =
  | { state: "starting" }
  | { state: "downloading"; model: string }
  | { state: "loading"; model: string }
  | { state: "ready"; model: string; url: string }
  | { state: "missing" }
  | { state: "failed"; message: string };

/** What an agent run does, as it happens. `session` is "chat" for the Chat page,
 * "routine:<id>" or "task:<id>" for runs that started on their own. */
export type AgentEvent = { session: string; run: number } & (
  | { kind: "start"; title: string | null }
  | { kind: "token"; text: string }
  | { kind: "step"; call: string; title: string; state: StepState; detail: string | null }
  | { kind: "ask"; ask: number; title: string; what: string }
  | { kind: "answered"; ask: number; allowed: boolean }
  | { kind: "compacted"; before: number; after: number }
  | { kind: "done"; text: string }
  | { kind: "failed"; message: string }
  | { kind: "stopped" }
);
export type StepState = "running" | "done" | "failed" | "blocked" | "declined";

/** A line heard from the computer's sound; times in ms since 1970. */
export type HeardLine = { from: number; to: number; text: string };
export type Hearing = { on: boolean; lines: HeardLine[]; trouble: string | null };
/** "Hey Nimbus": the microphone listened to, and whether a request is awaited now. */
export type Voice = { on: boolean; awake: boolean; trouble: string | null };
/** The chat's microphone button (push to talk). */
export type Talk =
  | { state: "listening" }
  | { state: "writing" }
  | { state: "done"; text: string }
  | { state: "failed"; message: string };
/** The model Nimbus starts on this computer's GPU for the chat. */
export type Brain =
  | { state: "off" }
  | { state: "missing"; message: string }
  | { state: "installing"; what: string; done: number; total: number }
  | { state: "starting"; model: string }
  | { state: "ready"; model: string }
  /** Unloaded to free the GPU; the next message loads it again. */
  | { state: "asleep"; model: string }
  | { state: "failed"; message: string };

/** What runs the local brain on this computer, and the models found for it. */
export type BrainPlan = {
  chip: string | null;
  backend: "cuda" | "vulkan" | "rocm" | "metal" | "cpu";
  backend_label: string;
  /** Bytes a model and its context may use. */
  usable: number;
  server: string | null;
  release: string | null;
  recommended: { name: string; size: number; present: boolean };
  models: { path: string; name: string; size: number; fits: boolean; automatic: boolean }[];
  chosen: string | null;
  folder: string | null;
};

/** A signed-in mail account (the password is in the system's credential store). */
/** Signing in through the browser, with Google or Microsoft. */
/** A file or folder found by name; size and date are read when it is found. */
export type FileHit = { path: string; name: string; folder: string; dir: boolean; size: number | null; modified: number | null };
export type FilesStatus = { on: boolean; files: number; scanning: boolean; walked: number | null; took: number | null; trouble: string | null };
export type SignInKind = "google" | "microsoft";
export type MailAccount = { id: string; email: string; host: string; port: number; user: string; auth: SignInKind | null };
export type MailProvider = { name: string; host: string; port: number; hint: string; link: string | null; blocked: boolean; login: string | null; browser: SignInKind | null };

export type Repeat = "never" | "daily" | "weekdays" | "weekly";
export type TodoKind = "todo" | "alarm" | "task";
export type Todo = {
  id: string;
  text: string;
  done: boolean;
  /** Due, in ms since 1970. */
  at: number | null;
  repeat: Repeat;
  kind: TodoKind;
  task: string | null;
  fired: boolean;
  by_agent: boolean;
};
/** A to-do whose time has come. */
export type Due = { id: string; text: string; kind: TodoKind };
export type Trigger =
  | { when: "manual" }
  | { when: "startup" }
  | { when: "daily"; time: string }
  | { when: "weekdays"; time: string }
  | { when: "weekly"; day: number; time: string };
export type Routine = {
  id: string;
  name: string;
  steps: string;
  trigger: Trigger;
  trusted: boolean;
  enabled: boolean;
  last_run: number | null;
  last_result: string | null;
};
export type PrivacyStatus = { enabled: boolean; verified: boolean; mode: string };
export type Rect = { x: number; y: number; w: number; h: number };

export type Thumb = { path: string; thumb: string; palette: Palette | null };
export type WallpaperFolder = { dir: string; thumbs: Thumb[] };
export type EngineItem = {
  id: string;
  title: string;
  kind: string;
  preview: string | null;
  source: "workshop" | "mine" | "default";
  thumb: string | null;
  palette: Palette | null;
};
export type EngineStatus = { installed: boolean; running: boolean; current: string | null };

export type AppEntry = { id: string; name: string; icon: string | null };

export type ClipKind = "text" | "link" | "image";
export type ClipItem = {
  id: string;
  kind: ClipKind;
  /** The first 2000 characters; copy and paste always use the full text. */
  text?: string;
  chars: number;
  size?: [number, number];
  app: string | null;
  at: number;
  pinned: boolean;
  image: string | null;
  thumb: string | null;
};
export type ClipView = { enabled: boolean; guards_secrets: boolean; skipped: number; items: ClipItem[] };

export const api = {
  volumeGet: () => invoke<Volume>("volume_get"),
  volumeSet: (percent: number) => invoke<Volume>("volume_set", { percent }),
  volumeMute: (muted: boolean) => invoke<Volume>("volume_mute", { muted }),
  brightnessGet: () => invoke<number>("brightness_get"),
  brightnessSet: (percent: number) => invoke<number>("brightness_set", { percent }),
  mediaNow: (artwork: boolean) => invoke<NowPlaying | null>("media_now", { artwork }),
  mediaControl: (action: MediaAction) => invoke<NowPlaying | null>("media_control", { action }),
  audioPeak: () => invoke<number>("audio_peak"),
  radiosGet: () => invoke<Radios>("radios_get"),
  radioSet: (kind: RadioKind, on: boolean) => invoke<Radios>("radio_set", { kind, on }),
  themeGet: () => invoke<boolean>("theme_get"),
  themeSet: (dark: boolean) => invoke<boolean>("theme_set", { dark }),
  powerRun: (action: PowerAction) => invoke<void>("power_run", { action }),
  wallpaperGet: () => invoke<string>("wallpaper_get"),
  wallpaperSet: (path: string) => invoke<string>("wallpaper_set", { path }),
  wallpaperThumbs: () => invoke<WallpaperFolder>("wallpaper_thumbs"),
  wallpaperPickFolder: () => invoke<string | null>("wallpaper_pick_folder"),
  engineStatus: () => invoke<EngineStatus>("engine_status"),
  engineList: () => invoke<EngineItem[]>("engine_list"),
  engineOpen: (id: string) => invoke<string>("engine_open", { id }),
  paletteCurrent: () => invoke<Palette>("palette_current"),
  accentSet: (accent: string | null) => invoke<Palette>("accent_set", { accent }),
  statsSample: () => invoke<Snapshot>("stats_sample"),
  appsList: () => invoke<AppEntry[]>("apps_list"),
  appLaunch: (id: string) => invoke<void>("app_launch", { id }),
  filesSearch: (query: string, limit?: number) => invoke<FileHit[]>("files_search", { query, limit }),
  filesStatus: () => invoke<FilesStatus>("files_status"),
  filesSet: (on: boolean) => invoke<void>("files_set", { on }),
  fileOpen: (path: string) => invoke<void>("file_open", { path }),
  fileReveal: (path: string) => invoke<void>("file_reveal", { path }),
  clipboardList: () => invoke<ClipView>("clipboard_list"),
  clipboardCopy: (id: string) => invoke<void>("clipboard_copy", { id }),
  clipboardPaste: (id: string) => invoke<void>("clipboard_paste", { id }),
  clipboardPin: (id: string, pinned: boolean) => invoke<void>("clipboard_pin", { id, pinned }),
  clipboardDelete: (id: string) => invoke<void>("clipboard_delete", { id }),
  clipboardClear: () => invoke<void>("clipboard_clear"),
  clipboardEnable: (on: boolean) => invoke<void>("clipboard_enable", { on }),
  storeGet: <T>(key: string) => invoke<T | null>("store_get", { key }),
  storeSet: (key: string, value: unknown) => invoke<void>("store_set", { key, value }),
  autostartGet: () => invoke<boolean>("autostart_get"),
  autostartSet: (on: boolean) => invoke<boolean>("autostart_set", { on }),
  setHitRect: (rect: Rect) => invoke<void>("set_hit_rect", { rect }),
  setKeyboardFocus: (on: boolean) => invoke<void>("set_keyboard_focus", { on }),
  settingsGet: () => invoke<Settings>("settings_get"),
  privacySet: (on: boolean) => invoke<PrivacyStatus>("privacy_set", { on }),
  privacyStatus: () => invoke<PrivacyStatus>("privacy_status"),
  chatStatus: () => invoke<ChatStatus>("chat_status"),
  chatConfigure: (url: string | null, model: string | null, key: string | null) =>
    invoke<void>("chat_configure", { url, model, key }),
  agentSend: (session: string, text: string) => invoke<number>("agent_send", { session, text }),
  agentStop: (session: string) => invoke<void>("agent_stop", { session }),
  agentReset: (session: string) => invoke<void>("agent_reset", { session }),
  agentAnswer: (ask: number, allowed: boolean) => invoke<void>("agent_answer", { ask, allowed }),
  noticeAnswer: (id: string, button: string) => invoke<void>("notice_answer", { id, button }),
  todosList: () => invoke<Todo[]>("todos_list"),
  todoAdd: (text: string, at: number | null = null, repeat: Repeat = "never", kind: TodoKind = "todo") =>
    invoke<Todo>("todo_add", { text, at, repeat, kind }),
  todoEdit: (todo: Todo) => invoke<Todo>("todo_edit", { todo }),
  todoRemove: (id: string) => invoke<void>("todo_remove", { id }),
  todoSnooze: (id: string, minutes: number) => invoke<Todo>("todo_snooze", { id, minutes }),
  todosClearDone: () => invoke<void>("todos_clear_done"),
  mailAccounts: () => invoke<MailAccount[]>("mail_accounts"),
  mailProvider: (email: string) => invoke<MailProvider>("mail_provider", { email }),
  /** Signs in (checked with the server first); resolves to the unread count. */
  mailAdd: (email: string, password: string, user: string | null, host: string | null, port: number | null) =>
    invoke<number>("mail_add", { email, password, user, host, port }),
  mailRemove: (id: string) => invoke<void>("mail_remove", { id }),
  mailBrowserReady: () => invoke<Record<SignInKind, boolean>>("mail_browser_ready"),
  mailBrowserSetup: (kind: SignInKind, clientId: string, clientSecret: string | null) =>
    invoke<void>("mail_browser_setup", { kind, clientId, clientSecret }),
  mailBrowserSignIn: (kind: SignInKind, hint: string | null) => invoke<{ email: string; unread: number }>("mail_browser_sign_in", { kind, hint }),
  mailBrowserCancel: () => invoke<void>("mail_browser_cancel"),
  openLink: (url: string) => invoke<void>("open_link", { url }),
  /** Shows the agent's on-screen overlay for a few seconds, with nothing being done. */
  overlayPreview: () => invoke<void>("overlay_preview"),
  hearingGet: () => invoke<Hearing>("hearing_get"),
  hearingSet: (on: boolean) => invoke<void>("hearing_set", { on }),
  voiceGet: () => invoke<Voice>("voice_get"),
  brainGet: () => invoke<Brain>("brain_get"),
  brainPlan: () => invoke<BrainPlan>("brain_plan"),
  brainUse: (model: string | null) => invoke<void>("brain_use", { model }),
  brainIdleSet: (minutes: number) => invoke<void>("brain_idle_set", { minutes }),
  speechLanguageSet: (language: string | null) => invoke<void>("speech_language_set", { language }),
  brainSetup: () => invoke<void>("brain_setup"),
  brainStopSetup: () => invoke<void>("brain_stop_setup"),
  brainOpenFolder: () => invoke<void>("brain_open_folder"),
  voiceSet: (on: boolean) => invoke<void>("voice_set", { on }),
  voiceSpeaking: (on: boolean) => invoke<void>("voice_speaking", { on }),
  talkStart: () => invoke<void>("talk_start"),
  talkStop: (keep: boolean) => invoke<void>("talk_stop", { keep }),
  speakRepliesSet: (on: boolean) => invoke<void>("speak_replies_set", { on }),
  screenGet: () => invoke<{ on: boolean; pictures: number }>("screen_get"),
  screenSet: (on: boolean) => invoke<void>("screen_set", { on }),
  controlSet: (on: boolean) => invoke<void>("control_set", { on }),
  routinesList: () => invoke<Routine[]>("routines_list"),
  routineSave: (routine: Routine) => invoke<Routine>("routine_save", { routine }),
  routineRemove: (id: string) => invoke<void>("routine_remove", { id }),
  routineRun: (id: string) => invoke<void>("routine_run", { id }),
  clipboardWrite: (text: string) => invoke<void>("clipboard_write", { text }),
  speechStatus: () => invoke<Speech>("speech_status"),
  speechRetry: (model: string | null = null) => invoke<void>("speech_retry", { model }),
};

/** The error text a failed command carries, for showing to the user. */
export const reason = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

export function bytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1000 && i < units.length - 1) {
    n /= 1000;
    i++;
  }
  return `${n >= 100 || i === 0 ? n.toFixed(0) : n.toFixed(1)} ${units[i]}`;
}

/** Bytes per second as a speed, in bits like an internet plan: "38.2 Mb/s". */
export function rate(bytesPerSecond: number): string {
  const bits = bytesPerSecond * 8;
  if (bits < 1000) return `${bits.toFixed(0)} b/s`;
  if (bits < 1e6) return `${(bits / 1e3).toFixed(bits < 1e4 ? 1 : 0)} kb/s`;
  if (bits < 1e9) return `${(bits / 1e6).toFixed(bits < 1e7 ? 1 : 0)} Mb/s`;
  return `${(bits / 1e9).toFixed(1)} Gb/s`;
}

export function clock(ms: number | null): string {
  if (ms == null || !Number.isFinite(ms)) return "–:––";
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

export function duration(s: number): string {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) return `${d} d ${h} h`;
  if (h) return `${h} h ${m} min`;
  return `${m} min`;
}

const time = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const dayMonth = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short" });

/** "Just now", "4 min ago", then the clock time, "Yesterday", the weekday, the date. */
export function ago(at: number, now = Date.now()): string {
  const s = (now - at) / 1000;
  if (s < 45) return "Just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  const d = new Date(at);
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const days = Math.floor((today.getTime() - new Date(d).setHours(0, 0, 0, 0)) / 86400000);
  if (days <= 0) return time.format(d);
  if (days === 1) return `Yesterday ${time.format(d)}`;
  if (days < 7) return `${weekday.format(d)} ${time.format(d)}`;
  return dayMonth.format(d);
}

/** "Spotify.exe", "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify" or "spotify" → "Spotify". */
export function appLabel(app: string): string {
  let name = app.split("!").pop() ?? app;
  name = name.replace(/\.exe$/i, "").split(/[\\/]/).pop() ?? name;
  const known: Record<string, string> = {
    msedge: "Microsoft Edge",
    chrome: "Google Chrome",
    brave: "Brave",
    firefox: "Firefox",
    vlc: "VLC",
    zune: "Media Player",
    microsoft: "Media Player",
  };
  const key = name.toLowerCase();
  return known[key] ?? name.charAt(0).toUpperCase() + name.slice(1);
}
