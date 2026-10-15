// The timer on the Today page. It keeps running while the panel is folded (the
// notch shows the countdown) and survives a restart (saved in the store).
import { api } from "./api";
import { chime, unlockSound } from "./chime";

type Saved = { total: number; endAt: number | null; left: number };

export const timer = $state({
  total: 5 * 60_000,
  /** When a running timer ends (ms since 1970), or null while stopped. */
  endAt: null as number | null,
  /** Time left while paused or stopped. */
  left: 5 * 60_000,
  now: Date.now(),
  /** Bumped each time a timer finishes. */
  finished: 0,
});

export const running = () => timer.endAt != null;
export const remaining = () => (timer.endAt != null ? Math.max(0, timer.endAt - timer.now) : timer.left);

let ticker: ReturnType<typeof setInterval> | undefined;

function save() {
  const s: Saved = { total: timer.total, endAt: timer.endAt, left: timer.left };
  api.storeSet("timer", s).catch(() => {});
}

function run() {
  clearInterval(ticker);
  if (timer.endAt == null) return;
  ticker = setInterval(() => {
    timer.now = Date.now();
    if (timer.endAt != null && timer.now >= timer.endAt) {
      clearInterval(ticker);
      timer.endAt = null;
      timer.left = timer.total;
      timer.finished++;
      chime();
      save();
    }
  }, 250);
}

export function start() {
  unlockSound();
  if (timer.left <= 0) timer.left = timer.total;
  timer.now = Date.now();
  timer.endAt = timer.now + timer.left;
  run();
  save();
}

export function pause() {
  timer.left = remaining();
  timer.endAt = null;
  clearInterval(ticker);
  save();
}

export function reset() {
  timer.endAt = null;
  timer.left = timer.total;
  clearInterval(ticker);
  save();
}

export function setTotal(ms: number) {
  timer.total = ms;
  timer.left = ms;
  timer.endAt = null;
  clearInterval(ticker);
  save();
}

export async function loadTimer() {
  const s = await api.storeGet<Saved>("timer").catch(() => null);
  if (!s) return;
  timer.total = s.total;
  timer.left = s.left;
  timer.endAt = s.endAt;
  timer.now = Date.now();
  if (timer.endAt == null) return;
  if (timer.endAt <= timer.now) {
    // It ran out while Nimbus was closed; no chime for a timer nobody is waiting on.
    timer.endAt = null;
    timer.left = timer.total;
    save();
  } else {
    run();
  }
}
