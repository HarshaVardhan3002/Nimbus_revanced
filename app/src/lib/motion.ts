// The motion grammar in JavaScript, for Svelte transitions. Matches the CSS: whatever
// travels rides a spring, whatever appears fades up 8 px, whatever leaves just fades.

const reduced = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

/** A damped spring as an easing over 0..1, settled (within 0.2 %) at t = 1. */
function springEasing(zeta: number) {
  const w = 6.2 / zeta;
  const wd = w * Math.sqrt(1 - zeta * zeta);
  return (t: number) =>
    t >= 1 ? 1 : 1 - Math.exp(-zeta * w * t) * (Math.cos(wd * t) + ((zeta * w) / wd) * Math.sin(wd * t));
}

export const spring = springEasing(0.76);
export const springPop = springEasing(0.66);
export const out = (t: number) => 1 - Math.pow(1 - t, 4);

/** Appear: fade in while rising 8 px. */
export function rise(_node: Element, { delay = 0, duration = 260, y = 8 } = {}) {
  const move = reduced() ? 0 : y;
  return {
    delay,
    duration,
    easing: out,
    css: (t: number) => `opacity: ${t}; translate: 0 ${(1 - t) * move}px`,
  };
}

/** Leave: a quick fade, so the next thing is not kept waiting. */
export function leave(_node: Element, { delay = 0, duration = 120 } = {}) {
  return { delay, duration, css: (t: number) => `opacity: ${t}` };
}

/** Tabs spreading out from the core: they start nearer the middle and slide out. */
export function spread(_node: Element, { delay = 0, dx = 0 } = {}) {
  const move = reduced() ? 0 : dx;
  return {
    delay,
    duration: reduced() ? 160 : 520,
    easing: spring,
    css: (t: number, u: number) => `opacity: ${Math.min(1, t * 1.6)}; translate: ${u * move}px 0`,
  };
}
