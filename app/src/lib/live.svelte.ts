// Live system state shared by the island and its pages. Only what is on screen is
// polled: the notch needs the media session (and the speaker level while music
// plays); everything else is read by the page that shows it, while it is open.
import { listen } from "@tauri-apps/api/event";
import { api, type NowPlaying, type Radios, type Snapshot, type Volume } from "./api";
import { ui } from "./ui.svelte";

const HISTORY = 48;

export const live = $state({
  np: null as NowPlaying | null,
  /** performance.now() when `np` was read, to run the progress bar between reads. */
  npAt: 0,
  /** Speaker loudness, smoothed three ways (fast, medium, slow), 0..1, while music plays. */
  bands: [0, 0, 0] as number[],
  vol: null as Volume | null,
  bright: null as number | null,
  brightError: "",
  radios: null as Radios | null,
  dark: null as boolean | null,
  stats: null as Snapshot | null,
  /** The last readings, oldest first, for the Stats page. */
  history: { cpu: [] as number[], gpu: [] as number[], mem: [] as number[] },
  /** Bumped whenever the clipboard history changes. */
  clipVersion: 0,
});

let artworkFor = "";

export async function refreshMedia() {
  try {
    let np = await api.mediaNow(false);
    if (np) {
      const key = `${np.app}|${np.title}|${np.artist}`;
      if (key !== artworkFor) {
        // A new track: fetch the cover once, not on every read.
        const withArt = await api.mediaNow(true);
        if (withArt) np = withArt;
        artworkFor = key;
      } else {
        np.artwork = live.np?.artwork ?? null;
      }
    } else {
      artworkFor = "";
    }
    live.np = np;
    live.npAt = performance.now();
  } catch {
    live.np = null;
  }
}

export async function refreshLevels() {
  api
    .volumeGet()
    .then((v) => {
      live.vol = v;
    })
    .catch(() => {});
  api
    .brightnessGet()
    .then((b) => {
      live.bright = b;
      live.brightError = "";
    })
    .catch((e) => {
      live.bright = null;
      live.brightError = String(e);
    });
}

export async function refreshToggles() {
  api
    .radiosGet()
    .then((r) => {
      live.radios = r;
    })
    .catch(() => {});
  api
    .themeGet()
    .then((d) => {
      live.dark = d;
    })
    .catch(() => {});
}

const push = (list: number[], v: number) => [...list.slice(-(HISTORY - 1)), v];

export async function refreshStats() {
  try {
    const s = await api.statsSample();
    live.stats = s;
    live.history = {
      cpu: push(live.history.cpu, s.cpu_percent),
      gpu: push(live.history.gpu, s.gpu_percent ?? 0),
      mem: push(live.history.mem, (s.mem_used / Math.max(1, s.mem_total)) * 100),
    };
  } catch {
    // The next tick tries again.
  }
}

/** Peak amplitude (0..1) to a loudness that looks right on a short bar. */
const loudness = (peak: number) => Math.min(1, Math.max(0, (20 * Math.log10(Math.max(peak, 1e-5)) + 48) / 44));

/** Start the island's background reads; returns a function that stops them. */
export function startLive(): () => void {
  refreshMedia();
  const media = setInterval(() => !document.hidden && refreshMedia(), 1500);

  // The speaker level, only while music plays: fast attack, three release speeds.
  let meter: ReturnType<typeof setInterval> | undefined;
  const stopMeter = $effect.root(() => {
    $effect(() => {
      if (!live.np?.playing) {
        live.bands = [0, 0, 0];
        return;
      }
      meter = setInterval(async () => {
        const v = loudness(await api.audioPeak().catch(() => 0));
        const [a, b, c] = live.bands;
        live.bands = [v > a ? v : a * 0.55 + v * 0.45, v > b ? b * 0.4 + v * 0.6 : b * 0.78 + v * 0.22, v > c ? c * 0.7 + v * 0.3 : c * 0.9 + v * 0.1];
      }, 66);
      return () => clearInterval(meter);
    });
  });

  const unlisten = listen<{ fresh: boolean; skipped: boolean }>("clipboard", (e) => {
    live.clipVersion++;
    if (e.payload.fresh) ui.gulps++;
    if (e.payload.skipped) ui.shies++;
  });

  return () => {
    clearInterval(media);
    stopMeter();
    unlisten.then((f) => f());
  };
}
