// A soft stone click, synthesised so no audio files ship with the app:
// a very short burst of filtered noise, like slate meeting wood.

let ctx: AudioContext | null = null;

export function click() {
  try {
    ctx ??= new AudioContext();
    const t = ctx.currentTime;
    const len = Math.floor(ctx.sampleRate * 0.05);
    const buffer = ctx.createBuffer(1, len, ctx.sampleRate);
    const data = buffer.getChannelData(0);
    for (let i = 0; i < len; i++) data[i] = (Math.random() * 2 - 1) * Math.pow(1 - i / len, 6);

    const src = ctx.createBufferSource();
    src.buffer = buffer;
    const body = ctx.createBiquadFilter();
    body.type = "bandpass";
    body.frequency.value = 2100;
    body.Q.value = 1.4;
    const gain = ctx.createGain();
    gain.gain.setValueAtTime(0.5, t);
    gain.gain.exponentialRampToValueAtTime(0.001, t + 0.06);
    src.connect(body).connect(gain).connect(ctx.destination);
    src.start(t);
  } catch {
    // No audio device: stay silent.
  }
}
