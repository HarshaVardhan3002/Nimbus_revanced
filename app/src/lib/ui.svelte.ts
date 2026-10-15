// The island's own state: open or folded, which page, who has the keyboard, and
// what the core is looking at.
import { tick } from "svelte";
import { api } from "./api";
import type { IconName } from "./icons";

export type Tab = "controls" | "today" | "stats" | "clipboard" | "chat" | "apps" | "wallpaper" | "power" | "settings";

/**
 * Pages in tab order with the panel size each wants: four each side of the core, and
 * the core itself in the middle, which is chat. On small screens a panel shrinks to fit
 * (see `fitted`) and the page reflows.
 */
export const TABS: { id: Tab; label: string; icon: IconName; w: number; h: number }[] = [
  { id: "controls", label: "Controls", icon: "sliders-horizontal", w: 720, h: 452 },
  { id: "today", label: "Today", icon: "calendar-blank", w: 900, h: 452 },
  { id: "stats", label: "Stats", icon: "gauge", w: 900, h: 372 },
  { id: "clipboard", label: "Clipboard", icon: "clipboard-text", w: 900, h: 470 },
  { id: "chat", label: "Chat", icon: "chat-circle-dots", w: 720, h: 520 },
  { id: "apps", label: "Apps", icon: "squares-four", w: 720, h: 600 },
  { id: "wallpaper", label: "Wallpaper", icon: "image", w: 1060, h: 352 },
  { id: "power", label: "Power", icon: "power", w: 720, h: 200 },
  { id: "settings", label: "Settings", icon: "gear-six", w: 720, h: 440 },
];

/** The eight pages with a label in the tab bar (Ctrl+1 to Ctrl+8). */
export const NAMED = TABS.filter((t) => t.id !== "chat");

/** Room left around an open panel: its shoulders at the sides, a gap at the bottom. */
const SIDE = 14;
const BOTTOM = 10;

/** A page's panel size on this screen. */
export function fitted(t: { w: number; h: number }) {
  return { w: Math.min(t.w, Math.max(320, ui.vw - SIDE * 2)), h: Math.min(t.h, Math.max(240, ui.vh - BOTTOM)) };
}

export type Peek =
  | { kind: "track"; title: string; artist: string; art: string | null }
  | { kind: "timer"; label: string }
  /** A to-do whose time has come; an alarm rings until it is dismissed. */
  | { kind: "reminder"; id: string; text: string; alarm: boolean; task: boolean }
  /** A routine (or a planned task) finished on its own. */
  | { kind: "ran"; name: string; result: string; failed: boolean }
  /** The assistant is waiting for an OK, with the panel folded. */
  | { kind: "ask"; title: string }
  /** A plugin says something, maybe with buttons to answer. */
  | { kind: "notice"; id: string; from: string; text: string; buttons: string[] };

export const ui = $state({
  open: false,
  tab: "controls" as Tab,
  /** The pointer is over the island (Rust reports it; it also makes the rest click-through). */
  hover: false,
  /** Opened from the keyboard: the island keeps the keyboard until it folds. */
  keyboard: false,
  /** Opened from the tray: stays open until the pointer has visited and left. */
  sticky: false,
  /** Text fields holding the keyboard right now. */
  typing: 0,
  /** Reasons the panel must not fold yet: a press-and-hold, a system dialog. */
  stay: 0,
  peek: null as Peek | null,
  /** The pointer, in window CSS px, while it is near the island. */
  pointer: null as { x: number; y: number } | null,
  /** Something the core should look at instead of the pointer (a hovered tab). */
  gaze: null as { x: number; y: number } | null,
  /** Work in progress (applying a wallpaper, launching an app): the core thinks. */
  busy: 0,
  /** Bumped on each fresh copy (the core gulps) and each private copy left out (it looks away). */
  gulps: 0,
  shies: 0,
  /** Bumped when something worked out (a wallpaper applied, a timer finished): the core smiles. */
  cheers: 0,
  /** The window's size in CSS px; it follows the screen (Rust fits the window to it). */
  /** Put the cursor in the chat box once the chat page is up (opened to type). */
  wantInput: false,
  vw: window.innerWidth,
  vh: window.innerHeight,
});

window.addEventListener("resize", () => {
  ui.vw = window.innerWidth;
  ui.vh = window.innerHeight;
});

export async function openPanel(how: { keyboard?: boolean; sticky?: boolean; tab?: Tab } = {}) {
  if (how.tab) ui.tab = how.tab;
  ui.peek = null;
  ui.sticky = !!how.sticky;
  if (how.keyboard && !ui.keyboard) {
    ui.keyboard = true;
    await api.setKeyboardFocus(true).catch(() => {});
  }
  ui.open = true;
  if (how.keyboard) {
    await tick();
    // The chat page may already have put the cursor in its box.
    if (document.activeElement?.id !== "chat-input") focusActiveTab();
  }
}

export function closePanel() {
  if (!ui.open) return;
  ui.open = false;
  ui.sticky = false;
  ui.stay = 0;
  ui.gaze = null;
  if (ui.keyboard || ui.typing) {
    ui.keyboard = false;
    ui.typing = 0;
    // Hands the keyboard back to the app that had it.
    api.setKeyboardFocus(false).catch(() => {});
  }
}

export function focusActiveTab() {
  document.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus();
}

export function selectTab(tab: Tab) {
  ui.tab = tab;
  ui.gaze = null;
}

export function stepTab(by: number) {
  const i = TABS.findIndex((t) => t.id === ui.tab);
  selectTab(TABS[(i + by + TABS.length) % TABS.length].id);
}

/** Run some work with the core in its thinking mood. */
export async function busy<T>(work: Promise<T>): Promise<T> {
  ui.busy++;
  try {
    return await work;
  } finally {
    ui.busy--;
  }
}

/**
 * For text fields. Clicks on the island never take the keyboard from the app the user
 * is in, so a field asks for it when pressed and gives it back when left.
 */
export function typing(node: HTMLInputElement | HTMLTextAreaElement) {
  let held = false;
  const grab = () => {
    if (held) return;
    held = true;
    ui.typing++;
    if (!ui.keyboard && ui.typing === 1) {
      api
        .setKeyboardFocus(true)
        .then(() => node.isConnected && node.focus())
        .catch(() => {});
    }
  };
  const release = () => {
    if (!held) return;
    held = false;
    // A focused field that is removed blurs in the middle of Svelte's update, where state
    // may not change: give the keyboard back just after.
    queueMicrotask(() => {
      ui.typing = Math.max(0, ui.typing - 1);
      if (!ui.keyboard && ui.typing === 0 && ui.open) api.setKeyboardFocus(false).catch(() => {});
    });
  };
  node.addEventListener("pointerdown", grab);
  node.addEventListener("focus", grab);
  node.addEventListener("blur", release);
  return {
    destroy() {
      release();
      node.removeEventListener("pointerdown", grab);
      node.removeEventListener("focus", grab);
      node.removeEventListener("blur", release);
    },
  };
}
