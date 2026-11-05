// Shows each player's time, counting down locally between OGS clock updates.

import type { Clock, Color, PlayerClock } from "./api";

let clock: Clock | null = null;
let receivedAt = 0;
let timer = 0;
/** The most byo-yomi periods seen for each side this game: how many dots to draw. */
let periodsAtStart: Record<Color, number> = { black: 0, white: 0 };

export function setClock(c: Clock | null) {
  if (!c || c.gameId !== clock?.gameId) periodsAtStart = { black: 0, white: 0 };
  if (c) {
    for (const color of ["black", "white"] as Color[]) {
      const p = c[color];
      if (!p.canadian && p.periodTime != null) periodsAtStart[color] = Math.max(periodsAtStart[color], p.periods ?? 0);
    }
  }
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
    const pips = document.getElementById(`${color}-pips`)!;
    if (!clock) {
      el.textContent = "";
      note.textContent = "";
      pips.replaceChildren();
      el.className = "clock";
      continue;
    }
    const running = clock.toMove === color && !clock.paused;
    const c = { ...clock[color] };
    if (running) spend(c, (performance.now() - receivedAt) / 1000);
    const [time, detail] = describe(c);
    const low = running && remaining(c) < 10;
    el.textContent = time;
    note.textContent = detail;
    el.className = `clock${running ? " running" : ""}${low ? " low" : ""}`;
    drawPips(pips, c.canadian || c.periodTime == null ? 0 : periodsAtStart[color], c.periods ?? 0, c.main <= 0);
    pips.parentElement!.classList.toggle("low", low);
  }
}

/** Same rule as the Rust side: main time first, then byo-yomi periods. */
function spend(c: PlayerClock, secs: number) {
  const fromMain = Math.min(secs, c.main);
  c.main -= fromMain;
  secs -= fromMain;
  if (secs <= 0 || c.periodTime == null) return;
  if (c.canadian) {
    // One period for the block of stones; there is no next one.
    c.periodLeft = Math.max(0, (c.periodLeft ?? c.periodTime) - secs);
    return;
  }
  if (c.periods == null) return;
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

/** One dot per byo-yomi period: filled while it's still to come. */
function drawPips(el: HTMLElement, total: number, left: number, inOvertime: boolean) {
  const shown = Math.min(total, 10);
  if (el.childElementCount !== shown) el.replaceChildren(...Array.from({ length: shown }, () => document.createElement("span")));
  [...el.children].forEach((pip, i) => pip.classList.toggle("left", i < left));
  el.classList.toggle("overtime", inOvertime);
}

/** The big time, and a note about byo-yomi underneath it. */
function describe(c: PlayerClock): [string, string] {
  if (c.canadian) {
    if (c.main > 0) return [mmss(c.main), c.periodTime ? `then ${mmss(c.periodTime)} for each block of stones` : ""];
    const stones = c.periods;
    const note = stones == null ? "overtime" : `${stones} ${stones === 1 ? "stone" : "stones"} to play in this period`;
    return [mmss(c.periodLeft ?? c.periodTime ?? 0), note];
  }
  const periods = c.periods ?? 0;
  const period = c.periodTime != null ? `${Math.round(c.periodTime)}s` : "";
  if (c.main > 0) {
    return [mmss(c.main), periods && period ? `then ${periods} × ${period}` : ""];
  }
  if (c.periodTime != null) {
    const left = c.periodLeft ?? c.periodTime;
    return [mmss(left), periods <= 1 ? "last period" : `${periods} periods of ${period} left`];
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
