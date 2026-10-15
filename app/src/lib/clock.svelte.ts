// One clock for the whole island, ticking on the second.
export const time = $state({ now: new Date() });

export function startClock(): () => void {
  let interval: ReturnType<typeof setInterval> | undefined;
  const align = setTimeout(() => {
    time.now = new Date();
    interval = setInterval(() => (time.now = new Date()), 1000);
  }, 1000 - (Date.now() % 1000));
  return () => {
    clearTimeout(align);
    clearInterval(interval);
  };
}
