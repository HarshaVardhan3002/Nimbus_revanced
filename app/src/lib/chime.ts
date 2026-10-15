// Nimbus's own timer chime, synthesised (no sound files): three soft bell tones
// rising a fifth and an octave, played twice.

let ctx: AudioContext | null = null;

/** Create the audio context during a click, so the system lets it play later. */
export function unlockSound() {
  try {
    ctx ??= new AudioContext();
    if (ctx.state === "suspended") void ctx.resume();
  } catch {
    ctx = null;
  }
}

function bell(ac: AudioContext, freq: number, at: number) {
  const body = ac.createOscillator();
  const bodyGain = ac.createGain();
  body.type = "sine";
  body.frequency.value = freq;
  bodyGain.gain.setValueAtTime(0.0001, at);
  bodyGain.gain.exponentialRampToValueAtTime(0.14, at + 0.012);
  bodyGain.gain.exponentialRampToValueAtTime(0.0001, at + 1.3);
  body.connect(bodyGain).connect(ac.destination);
  body.start(at);
  body.stop(at + 1.35);

  // A quiet inharmonic overtone gives the strike its bell edge.
  const edge = ac.createOscillator();
  const edgeGain = ac.createGain();
  edge.type = "sine";
  edge.frequency.value = freq * 2.76;
  edgeGain.gain.setValueAtTime(0.0001, at);
  edgeGain.gain.exponentialRampToValueAtTime(0.025, at + 0.006);
  edgeGain.gain.exponentialRampToValueAtTime(0.0001, at + 0.3);
  edge.connect(edgeGain).connect(ac.destination);
  edge.start(at);
  edge.stop(at + 0.35);
}

export function chime() {
  unlockSound();
  if (!ctx) return;
  const start = ctx.currentTime + 0.03;
  const notes = [659.25, 987.77, 1318.51]; // E5, B5, E6
  for (let round = 0; round < 2; round++) {
    notes.forEach((f, i) => bell(ctx!, f, start + round * 1.15 + i * 0.13));
  }
}
