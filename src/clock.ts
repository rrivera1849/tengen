// Shows each player's time, counting down locally between OGS clock updates.

import type { Clock, Color, PlayerClock } from "./api";

let clock: Clock | null = null;
let receivedAt = 0;
let timer = 0;

export function setClock(c: Clock | null) {
  clock = c;
  receivedAt = performance.now();
  render();
  clearInterval(timer);
  if (c) timer = window.setInterval(render, 250);
}

function render() {
  for (const color of ["black", "white"] as Color[]) {
    const el = document.getElementById(`${color}-clock`)!;
    const note = document.getElementById(`${color}-clock-note`)!;
    if (!clock) {
      el.textContent = "";
      note.textContent = "";
      el.className = "clock";
      continue;
    }
    const running = clock.toMove === color && !clock.paused;
    const c = { ...clock[color] };
    if (running) spend(c, (performance.now() - receivedAt) / 1000);
    const [time, detail] = describe(c);
    el.textContent = time;
    note.textContent = detail;
    el.className = `clock${running ? " running" : ""}${running && remaining(c) < 10 ? " low" : ""}`;
  }
}

/** Same rule as the Rust side: main time first, then byo-yomi periods. */
function spend(c: PlayerClock, secs: number) {
  const fromMain = Math.min(secs, c.main);
  c.main -= fromMain;
  secs -= fromMain;
  if (secs <= 0 || c.periods == null || c.periodTime == null) return;
  let left = c.periodLeft ?? c.periodTime;
  let periods = c.periods;
  while (secs > 0 && periods > 0) {
    if (secs < left) {
      left -= secs;
      secs = 0;
    } else {
      secs -= left;
      periods -= 1;
      left = c.periodTime;
    }
  }
  c.periods = periods;
  c.periodLeft = periods === 0 ? 0 : left;
}

function remaining(c: PlayerClock) {
  return c.main > 0 ? c.main : (c.periodLeft ?? c.periodTime ?? 0);
}

/** The big time, and a note about byo-yomi underneath it. */
function describe(c: PlayerClock): [string, string] {
  const periods = c.periods ?? 0;
  const period = c.periodTime != null ? `${Math.round(c.periodTime)}s` : "";
  if (c.main > 0) {
    return [mmss(c.main), periods && period ? `then ${periods} periods of ${period}` : ""];
  }
  if (c.periodTime != null) {
    const left = c.periodLeft ?? c.periodTime;
    return [mmss(left), periods <= 1 ? "last period" : `${periods} periods of ${period}`];
  }
  return [mmss(0), ""];
}

function mmss(secs: number) {
  const s = Math.max(0, Math.ceil(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${r}` : `${m}:${r}`;
}
