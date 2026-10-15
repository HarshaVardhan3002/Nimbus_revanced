// The colours the island wears. Rust sends the wallpaper's palette (or the user's
// own accent) as "palette" events. The wallpaper picker may preview another
// wallpaper's palette while it is centred, without applying anything.
import { listen } from "@tauri-apps/api/event";
import { api, type Palette } from "./api";

export const colors = $state({
  current: null as Palette | null,
  preview: null as Palette | null,
});

const ROLES = ["accent", "ink", "chest", "done", "ember", "panel"] as const;

function paint(p: Palette) {
  const style = document.documentElement.style;
  for (const role of ROLES) style.setProperty(`--${role}`, p[role]);
}

export function startPalette(): () => void {
  api
    .paletteCurrent()
    .then((p) => {
      colors.current = p;
    })
    .catch(() => {});
  const unlisten = listen<Palette>("palette", (e) => {
    colors.current = e.payload;
  });
  const stop = $effect.root(() => {
    $effect(() => {
      const p = colors.preview ?? colors.current;
      if (p) paint(p);
    });
  });
  return () => {
    unlisten.then((f) => f());
    stop();
  };
}
