// The review panel: summary, score graph and one card per lesson.

import type { LessonView, Progress, Review } from "./api";
import type { Line } from "./board";

const NS = "http://www.w3.org/2000/svg";

export interface PanelHooks {
  /** Show the position before a lesson's move, with its better line. */
  open: (turn: number, line: Line) => void;
  /** Jump to a turn from the graph. */
  jump: (turn: number) => void;
  addKey: () => void;
  showMaxims: () => void;
}

let review: Review | null = null;
let openIndex: number | null = null;
let hooks: PanelHooks;

const panel = () => document.getElementById("review")!;

export function initPanel(h: PanelHooks) {
  hooks = h;
}

export function isOpen() {
  return document.body.classList.contains("reviewing");
}

export function close() {
  document.body.classList.remove("reviewing");
  openIndex = null;
}

/** Opens the panel in its "working" state. */
export function showWorking() {
  review = null;
  document.body.classList.add("reviewing");
  panel().replaceChildren(header(), el("p", "progress-text", "Getting KataGo ready"), progressBar(0));
}

const STAGES: Record<Progress["stage"], string> = {
  download: "Downloading KataGo",
  starting: "Starting KataGo (the first start can take a few minutes)",
  quick: "Looking at every move",
  deep: "Checking the biggest swings",
  lessons: "Writing the lessons",
};

export function showProgress(p: Progress) {
  if (review || !isOpen()) return;
  const text = panel().querySelector(".progress-text");
  if (!text) return;
  const count = p.total > 1 && p.stage !== "download" ? ` (${p.done} of ${p.total})` : "";
  const mb = p.stage === "download" && p.total ? ` (${Math.round(p.done / 1e6)} of ${Math.round(p.total / 1e6)} MB)` : "";
  text.textContent = STAGES[p.stage] + count + mb;
  const bar = panel().querySelector<HTMLElement>(".progress span");
  if (bar) bar.style.width = `${p.total ? Math.min(100, (100 * p.done) / p.total) : 0}%`;
}

export function showReview(r: Review, cursor: number) {
  review = r;
  openIndex = null;
  document.body.classList.add("reviewing");
  render(cursor);
}

/** Keeps the graph's marker in step with the board. */
export function setCursor(cursor: number) {
  if (!review || !isOpen()) return;
  const marker = panel().querySelector(".graph .cursor");
  if (marker) {
    const x = xAt(cursor);
    marker.setAttribute("x1", String(x));
    marker.setAttribute("x2", String(x));
  }
}

function render(cursor: number) {
  const r = review!;
  const parts: Node[] = [header()];
  if (r.summary) {
    parts.push(el("p", "summary", r.summary.summary));
    parts.push(el("p", "theme", r.summary.theme));
    parts.push(maximBlock(r.summary.maxim, "For your next game"));
  }
  parts.push(graph(r, cursor));
  if (r.note) {
    const note = el("div", "key-note");
    note.append(el("p", "", r.note));
    const b = button("Add API key", () => hooks.addKey());
    note.append(b);
    parts.push(note);
  }
  if (!r.lessons.length) {
    parts.push(el("p", "lede", "No big mistakes stood out in this game. Well played."));
  }
  const yours = r.lessons.filter((l) => l.side === "you");
  const theirs = r.lessons.filter((l) => l.side === "opponent");
  if (yours.length) parts.push(el("h3", "", "Your moments"), ...yours.map((l) => card(l)));
  if (theirs.length) parts.push(el("h3", "", "Your opponent's mistakes"), ...theirs.map((l) => card(l)));
  parts.push(button("Your maxims", () => hooks.showMaxims(), "quiet maxims-link"));
  panel().replaceChildren(...parts);
}

function header() {
  const h = el("div", "review-head");
  h.append(el("h2", "", "Review"));
  h.append(button("Close", () => close(), "quiet"));
  return h;
}

function card(l: LessonView) {
  const index = review!.lessons.indexOf(l);
  const c = el("article", `lesson${openIndex === index ? " open" : ""}`);
  const top = document.createElement("button");
  top.className = "lesson-top";
  top.setAttribute("aria-expanded", String(openIndex === index));
  const meta = `Move ${l.moveNumber}, ${l.kind.toLowerCase()}`;
  top.append(el("span", "lesson-meta", meta), el("span", "lesson-loss", `${l.pointsLost} points`));
  top.append(el("span", "lesson-headline", l.lesson?.headline ?? factsHeadline(l)));
  top.addEventListener("click", () => {
    openIndex = openIndex === index ? null : index;
    render(l.moveNumber - 1);
    if (openIndex !== index) return;
    if (l.side === "opponent" && l.answerLine.length) {
      // Show the position after their mistake, with how to punish it.
      const you = l.color === "black" ? "white" : "black";
      hooks.open(l.moveNumber, { played: l.yourReply, line: l.answerLine, color: you });
    } else {
      hooks.open(l.moveNumber - 1, { played: l.played, line: l.line, color: l.color });
    }
  });
  c.append(top);
  if (openIndex === index) c.append(body(l));
  return c;
}

function factsHeadline(l: LessonView) {
  return l.side === "you"
    ? `KataGo prefers ${l.bestCoord} to ${l.playedCoord}`
    : `${l.playedCoord} gave you a chance`;
}

function body(l: LessonView) {
  const b = el("div", "lesson-body");
  const chances = `Win chance ${l.winBefore}% before the move, ${l.winAfter}% after.`;
  if (l.lesson) {
    b.append(section("What happened", l.lesson.what_happened));
    b.append(section("Better", l.lesson.better_idea));
    b.append(section("Why", l.lesson.why));
    b.append(maximBlock(l.lesson.maxim));
  } else {
    const who = l.side === "you" ? "You played" : "Your opponent played";
    b.append(el("p", "", `${who} ${l.playedCoord}; KataGo's choice was ${l.bestCoord}, worth ${l.pointsLost} points more.`));
    if (l.problem) b.append(el("p", "hint", "The written lesson didn't pass Tengen's checks, so only KataGo's facts are shown."));
  }
  if (l.punish) {
    const best = l.punish.best_reply ?? "pass";
    const yours = l.punish.your_reply ?? "pass";
    b.append(el("p", "punish", l.punish.found
      ? `You found the answer at ${yours}.`
      : `Your answer was ${yours}; the punishing move was ${best}.`));
  }
  const shown = l.side === "opponent" && l.answerLine.length
    ? "The numbered stones show how to punish it; the dashed ring is what you played."
    : "The numbered stones show KataGo's line; the dashed ring is the move played.";
  b.append(el("p", "hint", `${chances} ${shown}`));
  return b;
}

function section(title: string, text: string) {
  const s = el("div", "lesson-part");
  s.append(el("p", "part-title", title), el("p", "", text));
  return s;
}

function maximBlock(text: string, label = "Maxim") {
  const m = el("figure", "maxim");
  m.append(el("figcaption", "", label), el("blockquote", "", text));
  return m;
}

// ---------- score graph ----------

const GW = 320;
const GH = 88;

function xAt(turn: number) {
  const n = Math.max(1, (review?.leads.length ?? 2) - 1);
  return (turn / n) * GW;
}

function graph(r: Review, cursor: number) {
  const wrap = el("div", "graph");
  const leads = r.leads;
  const max = Math.max(5, ...leads.map(Math.abs));
  const y = (v: number) => GH / 2 - (v / max) * (GH / 2 - 6);
  const s = svgEl("svg", { viewBox: `0 0 ${GW} ${GH}`, role: "img", "aria-label": "Score lead over the game" });
  s.append(svgEl("line", { x1: "0", x2: String(GW), y1: String(GH / 2), y2: String(GH / 2), class: "zero" }));
  const pts = leads.map((v, i) => `${xAt(i).toFixed(1)},${y(v).toFixed(1)}`).join(" ");
  s.append(svgEl("polyline", { points: pts, class: "lead" }));
  for (const l of r.lessons) {
    const t = l.moveNumber;
    if (t >= leads.length) continue;
    s.append(svgEl("circle", { cx: String(xAt(t)), cy: String(y(leads[t])), r: "3.2", class: `mark ${l.side}` }));
  }
  s.append(svgEl("line", { x1: String(xAt(cursor)), x2: String(xAt(cursor)), y1: "0", y2: String(GH), class: "cursor" }));
  s.addEventListener("click", (e) => {
    const box = s.getBoundingClientRect();
    const turn = Math.round(((e.clientX - box.left) / box.width) * (leads.length - 1));
    hooks.jump(Math.max(0, Math.min(leads.length - 1, turn)));
  });
  const legend = el("p", "graph-legend", "Above the line Black leads; below, White. Filled dots are your moments.");
  wrap.append(s, legend);
  return wrap;
}

// ---------- helpers ----------

function el(tag: string, cls: string, text?: string) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function button(text: string, onClick: () => void, cls = "") {
  const b = document.createElement("button");
  b.textContent = text;
  if (cls) b.className = cls;
  b.addEventListener("click", onClick);
  return b;
}

function svgEl(tag: string, attrs: Record<string, string>) {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  return e;
}

function progressBar(pct: number) {
  const p = el("div", "progress");
  const s = document.createElement("span");
  s.style.width = `${pct}%`;
  p.append(s);
  return p;
}
