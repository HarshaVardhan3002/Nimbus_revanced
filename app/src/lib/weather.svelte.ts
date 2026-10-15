// Weather for the Today page, from Open-Meteo (free, no account). Nothing is
// fetched until the user names a place; then it refreshes every 30 minutes.
import { api } from "./api";
import type { IconName } from "./icons";

export type Place = { name: string; region: string; lat: number; lon: number };
type Now = { temp: number; code: number; day: boolean; high: number; low: number; at: number };

export const weather = $state({
  place: null as Place | null,
  now: null as Now | null,
  error: "",
});

type GeoResult = { name: string; admin1?: string; country?: string; latitude: number; longitude: number };

export async function findPlaces(query: string): Promise<Place[]> {
  const url = `https://geocoding-api.open-meteo.com/v1/search?count=5&format=json&language=en&name=${encodeURIComponent(query)}`;
  const r = await fetch(url);
  if (!r.ok) throw new Error(`The place search is not answering (${r.status}). Try again in a minute.`);
  const j: { results?: GeoResult[] } = await r.json();
  return (j.results ?? []).map((p) => ({
    name: p.name,
    region: [p.admin1, p.country].filter(Boolean).join(", "),
    lat: p.latitude,
    lon: p.longitude,
  }));
}

export async function refreshWeather() {
  const p = weather.place;
  if (!p) return;
  try {
    const url =
      `https://api.open-meteo.com/v1/forecast?latitude=${p.lat}&longitude=${p.lon}` +
      `&current=temperature_2m,weather_code,is_day&daily=temperature_2m_max,temperature_2m_min&timezone=auto&forecast_days=1`;
    const r = await fetch(url);
    if (!r.ok) throw new Error(String(r.status));
    const j = await r.json();
    weather.now = {
      temp: j.current.temperature_2m,
      code: j.current.weather_code,
      day: j.current.is_day === 1,
      high: j.daily.temperature_2m_max[0],
      low: j.daily.temperature_2m_min[0],
      at: Date.now(),
    };
    weather.error = "";
  } catch {
    weather.error = "The weather service is not answering. It tries again on its own.";
  }
}

export async function setPlace(p: Place | null) {
  weather.place = p;
  weather.now = null;
  weather.error = "";
  await api.storeSet("weather-place", p);
  if (p) await refreshWeather();
}

export async function startWeather(): Promise<() => void> {
  weather.place = await api.storeGet<Place>("weather-place").catch(() => null);
  if (weather.place) refreshWeather();
  const t = setInterval(() => weather.place && refreshWeather(), 30 * 60_000);
  return () => clearInterval(t);
}

/** WMO weather codes in words, with an icon. */
export function describe(code: number, day: boolean): { label: string; icon: IconName } {
  if (code === 0) return { label: "Clear", icon: day ? "sun" : "moon" };
  if (code <= 2) return { label: "Partly cloudy", icon: day ? "cloud-sun" : "cloud-moon" };
  if (code === 3) return { label: "Overcast", icon: "cloud" };
  if (code <= 48) return { label: "Fog", icon: "cloud-fog" };
  if (code <= 57) return { label: "Drizzle", icon: "cloud-rain" };
  if (code <= 67) return { label: "Rain", icon: "cloud-rain" };
  if (code <= 77) return { label: "Snow", icon: "cloud-snow" };
  if (code <= 82) return { label: "Showers", icon: "cloud-rain" };
  if (code <= 86) return { label: "Snow showers", icon: "cloud-snow" };
  return { label: "Thunderstorm", icon: "cloud-lightning" };
}
