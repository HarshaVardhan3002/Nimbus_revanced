// To-dos and routines, kept in Rust (routines.rs) and mirrored here. Rust tells us
// whenever they change, whoever changed them: the person, the agent, or the clock.
import { listen } from "@tauri-apps/api/event";
import { api, type Due, type Repeat, type Routine, type Todo, type Trigger } from "./api";

export const book = $state({
  todos: [] as Todo[],
  routines: [] as Routine[],
  loaded: false,
});

/** Load both lists and follow changes; `due` hears each reminder as its time comes. */
export function startTodos(due: (d: Due) => void) {
  Promise.all([api.todosList(), api.routinesList()])
    .then(([t, r]) => {
      book.todos = t;
      book.routines = r;
      book.loaded = true;
    })
    .catch(() => (book.loaded = true));
  const offs = [
    listen<Todo[]>("todos", ({ payload }) => (book.todos = payload)),
    listen<Routine[]>("routines", ({ payload }) => (book.routines = payload)),
    listen<Due>("reminder", ({ payload }) => due(payload)),
  ];
  return () => offs.forEach((p) => p.then((f) => f()));
}

const hm = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dayHm = new Intl.DateTimeFormat(undefined, { weekday: "short", hour: "numeric", minute: "2-digit" });
const dateHm = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", hour: "numeric", minute: "2-digit" });

/** "18:30", "Tomorrow 7:15", "Fri 9:00", "24 Dec 18:00". */
export function when(ms: number, now = Date.now()) {
  const d = new Date(ms);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(d) - day(new Date(now))) / 86_400_000);
  if (days === 0) return hm.format(d);
  if (days === 1) return `Tomorrow ${hm.format(d)}`;
  if (days > 1 && days < 7) return dayHm.format(d);
  return dateHm.format(d);
}

export const REPEAT_LABEL: Record<Repeat, string> = {
  never: "Once",
  daily: "Every day",
  weekdays: "Weekdays",
  weekly: "Every week",
};

export const DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

export function trigger(t: Trigger) {
  switch (t.when) {
    case "manual":
      return "When you start it";
    case "startup":
      return "When the app starts";
    case "daily":
      return `Every day at ${t.time}`;
    case "weekdays":
      return `Weekdays at ${t.time}`;
    case "weekly":
      return `${DAYS[t.day % 7]}s at ${t.time}`;
  }
}

/** A local date-time for <input type="datetime-local">, and back. */
export function toLocalInput(ms: number) {
  const d = new Date(ms - new Date(ms).getTimezoneOffset() * 60_000);
  return d.toISOString().slice(0, 16);
}
export function fromLocalInput(v: string) {
  const ms = new Date(v).getTime();
  return Number.isFinite(ms) ? ms : null;
}
