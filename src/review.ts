// The review: a lit card beside the board, the lead graph under it (which
// doubles as a scrubber), and your mistakes circled on the board. You can
// try each position again before seeing KataGo's move.

import type { Color, LessonView, Progress, Review } from "./api";
import type { Line, Marks } from "./board";

const NS = "http://www.w3.org/2000/svg";
const TRIES = 3;

export interface PanelHooks {
  /** Show a position with a better line drawn on it. */
  open: (turn: number, line: Line) => Promise<void>;
  /** Move to a turn. */
  jump: (turn: number) => Promise<void>;
  /** Draw (or with null, clear) circles, targets and hints on the board. */
  marks: (m: Marks | null) => void;
  /** A wrong try, shown on the board for a moment. */
  miss: (x: number, y: number, color: Color) => void;
  addKey: () => void;
  showMaxims: () => void;
}

/** One position to try again: find `answer` where `played` was played. */
interface Question {
  lesson: number;
  turn: number;
  color: Color;
  answer: [number, number];
  played: [number, number] | null;
  line: Line;
}

type State =
  | { kind: "overview" }
  | { kind: "lesson"; index: number }
  | {
      kind: "quiz";
      questions: Question[];
      step: number;
      tries: number;
      hinted: boolean;
      /** Set once the move is found or shown. */
      outcome: "found" | "shown" | null;
      note: string;
    };

let review: Review | null = null;
let state: State = { kind: "overview" };
let cursor = 0;
let hooks: PanelHooks;

const panel = () => document.getElementById("review")!;
const graphBox = () => document.getElementById("review-graph")!;

export function initPanel(h: PanelHooks) {
  hooks = h;
  new ResizeObserver(() => drawGraph()).observe(graphBox());
}

export function isOpen() {
  return document.body.classList.contains("reviewing");
}

export function close() {
  document.body.classList.remove("reviewing");
  review = null;
  state = { kind: "overview" };
  graphBox().replaceChildren();
  hooks.marks(null);
}

/** Esc: leave a question or a lesson before closing the review. */
export function escape(): boolean {
  if (!isOpen()) return false;
  if (state.kind !== "overview") {
    setState({ kind: "overview" });
    return true;
  }
  close();
  return true;
}

/** Opens the panel in its "working" state. */
export function showWorking() {
  review = null;
  state = { kind: "overview" };
  document.body.classList.add("reviewing");
  graphBox().replaceChildren();
  hooks.marks(null);
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

export function showReview(r: Review) {
  review = r;
  state = { kind: "overview" };
  document.body.classList.add("reviewing");
  graphBox().replaceChildren();
  render();
  // The final position, with every mistake circled.
  hooks.jump(r.leads.length - 1);
}

/** Keeps the graph, the circles and any question in step with the board. */
export function setCursor(c: number) {
  cursor = c;
  if (!review || !isOpen()) return;
  if (state.kind === "quiz" && c !== state.questions[state.step].turn) {
    // Moving away from the position ends the question.
    state = { kind: "overview" };
    render();
    return;
  }
  moveGraphCursor();
  syncBoard();
}

/** A click on an empty point. True when the review takes it as a try. */
export function tryMove(x: number, y: number): boolean {
  if (state.kind !== "quiz") return false;
  if (state.outcome) return true;
  const q = state.questions[state.step];
  if (x === q.answer[0] && y === q.answer[1]) {
    state.outcome = "found";
    state.note = state.tries === 0 ? "Found it, first try." : "Found it.";
    reveal(q);
    return true;
  }
  state.tries += 1;
  hooks.miss(x, y, q.color);
  const left = TRIES - state.tries;
  if (left <= 0) {
    state.outcome = "shown";
    state.note = "Here's KataGo's move.";
    reveal(q);
    return true;
  }
  const same = q.played && x === q.played[0] && y === q.played[1];
  state.note = `${same ? "That's the move that was played." : "Not that one."} ${left === 1 ? "One try" : "Two tries"} left.`;
  render();
  return true;
}

/** Ring clicks on the board open that moment. */
export function openLessonAt(ringIndex: number) {
  const index = ringLessons()[ringIndex];
  if (index != null) openLesson(index);
}

function setState(s: State) {
  state = s;
  render();
}

function reveal(q: Question) {
  hooks.open(q.turn, q.line).then(() => {
    syncBoard();
    render();
  });
  render();
}

// ---------- building the panel ----------

function render() {
  const r = review;
  if (!r) return;
  const parts: Node[] = [header()];
  if (state.kind === "quiz") parts.push(quizCard(state));
  else if (state.kind === "lesson") parts.push(lessonCard(r.lessons[state.index], state.index));
  else {
    if (r.summary) parts.push(el("p", "summary", r.summary.summary), el("p", "theme", r.summary.theme));
    const qs = questions();
    if (qs.length) parts.push(introCard(qs.length));
  }
  if (r.summary && state.kind !== "lesson") parts.push(maximBlock(r.summary.maxim, "For your next game"));
  if (r.note && state.kind === "overview") {
    const note = el("div", "key-note");
    note.append(el("p", "", r.note), button("Add API key", () => hooks.addKey()));
    parts.push(note);
  }
  parts.push(moments(r));
  parts.push(button("Your maxims", () => hooks.showMaxims(), "quiet maxims-link"));
  const scroll = panel().scrollTop;
  panel().replaceChildren(...parts);
  panel().scrollTop = state.kind === "overview" ? scroll : 0;
  moveGraphCursor();
  syncBoard();
}

function header() {
  const h = el("div", "review-head");
  h.append(el("h2", "", "Review"));
  h.append(button("Close", () => close(), "quiet"));
  return h;
}

/** Your mistakes, as questions: the position before each, in game order. */
function questions(): Question[] {
  const r = review!;
  return r.lessons
    .map((l, i) => question(l, i))
    .filter((q): q is Question => !!q && r.lessons[q.lesson].side === "you")
    .sort((a, b) => a.turn - b.turn);
}

function question(l: LessonView, index: number): Question | null {
  if (l.side === "you") {
    if (!l.best) return null;
    return {
      lesson: index, turn: l.moveNumber - 1, color: l.color, answer: l.best, played: l.played,
      line: { played: l.played, line: l.line, color: l.color },
    };
  }
  // An opponent's mistake: find the move that punishes it.
  const answer = l.answerLine[0];
  if (!answer) return null;
  const you = other(l.color);
  return {
    lesson: index, turn: l.moveNumber, color: you, answer, played: l.yourReply,
    line: { played: l.yourReply, line: l.answerLine, color: you },
  };
}

function startQuiz(qs: Question[]) {
  if (!qs.length) return;
  state = { kind: "quiz", questions: qs, step: 0, tries: 0, hinted: false, outcome: null, note: "" };
  goToQuestion();
}

function goToQuestion() {
  if (state.kind !== "quiz") return;
  const q = state.questions[state.step];
  render();
  hooks.jump(q.turn);
}

function introCard(n: number) {
  const c = el("section", "try");
  c.setAttribute("aria-label", "Try again");
  c.append(el("p", "try-meta", "Try again"));
  c.append(el("p", "try-title", n === 1 ? "Find the move you missed" : `Find the moves you missed`));
  c.append(el("p", "try-text", `${n === 1 ? "One position" : `${n} positions`} from this game, before your mistake${n === 1 ? "" : "s"}. ${TRIES} tries each, and a hint if you want one.`));
  const row = el("div", "try-buttons");
  row.append(button("Start", () => startQuiz(questions()), "solid"));
  c.append(row);
  return c;
}

function quizCard(s: Extract<State, { kind: "quiz" }>) {
  const q = s.questions[s.step];
  const l = review!.lessons[q.lesson];
  const mover = q.color === "black" ? "Black" : "White";
  const c = el("section", "try");
  c.setAttribute("aria-label", "Try again");
  const meta = el("p", "try-meta");
  meta.append(
    el("span", "", s.questions.length > 1 ? `Try again, ${s.step + 1} of ${s.questions.length}` : "Try again"),
    el("span", "num", `${l.pointsLost} points`),
  );
  c.append(meta);
  const move = q.turn + 1;
  const ask = l.side === "you"
    ? `Move ${move}. ${mover} to play: there's a better move than the circled one.`
    : `Move ${move}. ${mover} to play: punish ${other(q.color) === "black" ? "Black" : "White"}'s last move.`;
  c.append(el("p", "try-title", ask));
  if (!s.outcome) {
    c.append(el("p", "try-text", s.note || `Click where you'd play. You have ${words(TRIES)} tries before Tengen shows you.`));
    const row = el("div", "try-buttons");
    row.append(button(s.hinted ? "Hint shown" : "Give me a hint", () => {
      if (state.kind !== "quiz") return;
      state.hinted = true;
      render();
    }, "solid"));
    row.lastElementChild!.toggleAttribute("disabled", s.hinted);
    row.append(button("Show me", () => {
      if (state.kind !== "quiz") return;
      state.outcome = "shown";
      state.note = "Here's KataGo's move.";
      reveal(q);
    }, "outline"));
    c.append(row);
    return c;
  }
  c.classList.add(s.outcome);
  c.append(el("p", "try-result", s.note));
  if (l.lesson) {
    c.append(el("p", "try-headline", l.lesson.headline));
    c.append(el("p", "try-text", l.lesson.better_idea));
    c.append(el("p", "try-text", l.lesson.why));
  } else {
    c.append(el("p", "try-text", l.side === "you"
      ? `KataGo's choice was ${l.bestCoord}, worth ${l.pointsLost} points more than ${l.playedCoord}.`
      : `KataGo's answer was ${l.punish?.best_reply ?? "the stone marked 1"}.`));
  }
  c.append(el("p", "hint", "The numbered stones show KataGo's line; the dashed ring is the move played."));
  const row = el("div", "try-buttons");
  const last = s.step === s.questions.length - 1;
  row.append(button(last ? "Done" : "Next position", () => {
    if (state.kind !== "quiz") return;
    if (last) {
      setState({ kind: "overview" });
      hooks.jump(review!.leads.length - 1);
      return;
    }
    Object.assign(state, { step: state.step + 1, tries: 0, hinted: false, outcome: null, note: "" });
    goToQuestion();
  }, "solid"));
  c.append(row);
  return c;
}

function openLesson(index: number) {
  const l = review!.lessons[index];
  setState({ kind: "lesson", index });
  // Make sure the stone is on the board, so its circle shows.
  if (cursor < l.moveNumber) hooks.jump(l.moveNumber);
}

function lessonCard(l: LessonView, index: number) {
  const c = el("section", `moment-card ${l.side}`);
  c.setAttribute("aria-label", `Move ${l.moveNumber}`);
  const meta = el("p", "try-meta");
  meta.append(el("span", "", `Move ${l.moveNumber}, ${l.kind.toLowerCase()}`), el("span", "num", `${l.pointsLost} points`));
  c.append(meta, el("p", "moment-headline", l.lesson?.headline ?? factsHeadline(l)));
  if (l.lesson) {
    c.append(el("p", "", l.lesson.what_happened), el("p", "", l.lesson.better_idea), el("p", "", l.lesson.why));
    c.append(maximBlock(l.lesson.maxim, "Maxim"));
  } else {
    const who = l.side === "you" ? "You played" : "Your opponent played";
    c.append(el("p", "", `${who} ${l.playedCoord}; KataGo's choice was ${l.bestCoord}, worth ${l.pointsLost} points more.`));
    if (l.problem) c.append(el("p", "hint", "The written lesson didn't pass Tengen's checks, so only KataGo's facts are shown."));
  }
  if (l.punish) {
    const best = l.punish.best_reply ?? "pass";
    const yours = l.punish.your_reply ?? "pass";
    c.append(el("p", "punish", l.punish.found ? `You found the answer at ${yours}.` : `Your answer was ${yours}; the punishing move was ${best}.`));
  }
  c.append(el("p", "hint", `Win chance ${l.winBefore}% before the move, ${l.winAfter}% after.`));
  const row = el("div", "try-buttons");
  const q = question(l, index);
  if (q) row.append(button(l.side === "you" ? "Try this position" : "Try punishing it", () => startQuiz([q]), "solid"));
  row.append(button(l.side === "you" ? "Show KataGo's line" : "Show the punishment", () => {
    const line = q?.line ?? { played: l.played, line: l.line, color: l.color };
    hooks.open(q?.turn ?? l.moveNumber - 1, line);
  }, "outline"));
  c.append(row);
  return c;
}

function factsHeadline(l: LessonView) {
  return l.side === "you" ? `KataGo prefers ${l.bestCoord} to ${l.playedCoord}` : `${l.playedCoord} gave you a chance`;
}

/** Every lesson, in game order: orange dots for yours, blue rings for theirs. */
function moments(r: Review) {
  const s = el("section", "moments");
  s.setAttribute("aria-label", "Moments");
  if (!r.lessons.length) {
    s.append(el("p", "lede", "No big mistakes stood out in this game. Well played."));
    return s;
  }
  const order = r.lessons.map((_, i) => i).sort((a, b) => r.lessons[a].moveNumber - r.lessons[b].moveNumber);
  for (const i of order) {
    const l = r.lessons[i];
    const b = document.createElement("button");
    b.className = `moment ${l.side}`;
    const current = state.kind === "lesson" && state.index === i;
    if (current) b.setAttribute("aria-current", "true");
    const text = el("span", "moment-text");
    text.append(el("span", "moment-meta", `Move ${l.moveNumber}, ${l.kind.toLowerCase()}`), el("span", "moment-title", l.lesson?.headline ?? factsHeadline(l)));
    b.append(el("span", "moment-dot"), text, el("span", "moment-points", `${l.pointsLost}`));
    b.setAttribute("aria-label", `Move ${l.moveNumber}, ${l.side === "you" ? "your mistake" : "your opponent's mistake"}, ${l.pointsLost} points: ${l.lesson?.headline ?? factsHeadline(l)}`);
    b.addEventListener("click", () => (current ? setState({ kind: "overview" }) : openLesson(i)));
    s.append(b);
  }
  return s;
}

function maximBlock(text: string, label: string) {
  const m = el("figure", "maxim");
  m.append(el("figcaption", "", label), el("blockquote", "", text));
  return m;
}

// ---------- the board's marks ----------

/** Lesson indexes in the order their rings are drawn. */
function ringLessons(): number[] {
  if (!review || state.kind === "quiz") return [];
  return review.lessons
    .map((l, i) => [l, i] as const)
    .filter(([l]) => l.played && l.moveNumber <= cursor)
    .map(([, i]) => i);
}

function syncBoard() {
  if (!review || !isOpen()) return;
  if (state.kind === "quiz") {
    const q = state.questions[state.step];
    hooks.marks(state.outcome ? null : { target: q.played, hint: state.hinted ? q.answer : null });
    return;
  }
  const selected = state.kind === "lesson" ? state.index : -1;
  hooks.marks({
    rings: ringLessons().map((i) => {
      const l = review!.lessons[i];
      return { at: l.played!, color: l.color, mine: l.side === "you", points: l.pointsLost, selected: i === selected };
    }),
  });
}

// ---------- the lead graph, under the board ----------

const GH = 96;
let dragging = false;
/** The parts of the graph that follow the board, and the turn they show. */
let graphParts: { svg: SVGSVGElement; line: Element; legend: HTMLElement; x: (t: number) => number; last: number } | null = null;
let graphTurn = 0;

/** Moves the graph's cursor without rebuilding it (it may be mid-drag). */
function moveGraphCursor() {
  const g = graphParts;
  if (!g || !g.svg.isConnected) return drawGraph();
  const c = Math.min(cursor, g.last);
  graphTurn = c;
  g.line.setAttribute("x1", String(g.x(c)));
  g.line.setAttribute("x2", String(g.x(c)));
  g.svg.setAttribute("aria-valuenow", String(c));
  g.legend.textContent = c === 0 ? "Start" : `Move ${c} of ${g.last}`;
}

function drawGraph() {
  const box = graphBox();
  const r = review;
  graphParts = null;
  if (!r || !isOpen() || r.leads.length < 2) return box.replaceChildren();
  const gw = Math.max(200, Math.round(box.clientWidth));
  const leads = r.leads;
  const last = leads.length - 1;
  const max = Math.max(5, ...leads.map(Math.abs));
  const mid = GH / 2;
  const x = (t: number) => (t / last) * gw;
  const y = (v: number) => mid - (v / max) * (mid - 8);
  const pts = leads.map((v, i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(" ");
  const area = `0,${mid} ${pts} ${gw},${mid}`;
  const s = svgEl("svg", {
    viewBox: `0 0 ${gw} ${GH}`, width: String(gw), height: String(GH), role: "slider",
    "aria-label": "Who led over the game: Black above the middle line, White below. Drag along it to move through the game.",
    "aria-valuemin": "0", "aria-valuemax": String(last), "aria-valuenow": String(cursor), tabindex: "0",
  });
  const id = `g${Math.random().toString(36).slice(2, 7)}`;
  const defs = svgEl("defs", {});
  const clip = (name: string, top: number) => {
    const c = svgEl("clipPath", { id: `${id}-${name}` });
    c.append(svgEl("rect", { x: "0", y: String(top), width: String(gw), height: String(mid) }));
    return c;
  };
  defs.append(clip("above", 0), clip("below", mid));
  s.append(defs);
  s.append(svgEl("rect", { x: "0", y: "0", width: String(gw), height: String(GH), rx: "8", class: "bg" }));
  s.append(svgEl("polygon", { points: area, class: "area black", "clip-path": `url(#${id}-above)` }));
  s.append(svgEl("polygon", { points: area, class: "area white", "clip-path": `url(#${id}-below)` }));
  s.append(svgEl("line", { x1: "0", x2: String(gw), y1: String(mid), y2: String(mid), class: "zero" }));
  s.append(svgEl("polyline", { points: pts, class: "lead" }));
  const c = Math.min(cursor, last);
  graphTurn = c;
  const cursorLine = svgEl("line", { x1: String(x(c)), x2: String(x(c)), y1: "-6", y2: String(GH + 6), class: "cursor" });
  for (const l of r.lessons) {
    const t = l.moveNumber;
    if (t > last) continue;
    s.append(svgEl("circle", { cx: x(t).toFixed(1), cy: y(leads[t]).toFixed(1), r: "5", class: `mark ${l.side}` }));
  }
  s.insertBefore(cursorLine, s.querySelector(".mark"));
  const turnAt = (e: PointerEvent) => {
    const b = s.getBoundingClientRect();
    return Math.max(0, Math.min(last, Math.round(((e.clientX - b.left) / b.width) * last)));
  };
  const go = (t: number) => {
    if (t === graphTurn) return;
    graphTurn = t;
    hooks.jump(t);
  };
  s.addEventListener("pointerdown", (e) => {
    dragging = true;
    s.setPointerCapture(e.pointerId);
    go(turnAt(e));
  });
  s.addEventListener("pointermove", (e) => dragging && go(turnAt(e)));
  s.addEventListener("pointerup", () => (dragging = false));
  s.addEventListener("keydown", (e) => {
    const step = e.key === "ArrowLeft" ? -1 : e.key === "ArrowRight" ? 1 : 0;
    if (!step) return;
    e.preventDefault();
    e.stopPropagation();
    go(Math.max(0, Math.min(last, graphTurn + step)));
  });
  const legend = el("p", "graph-legend");
  const where = el("span", "", c === 0 ? "Start" : `Move ${c} of ${last}`);
  legend.append(where, el("span", "", "Drag along the graph to move through the game"));
  const hadFocus = box.contains(document.activeElement);
  box.replaceChildren(s, legend);
  graphParts = { svg: s as SVGSVGElement, line: cursorLine, legend: where, x, last };
  if (hadFocus) s.focus();
}

// ---------- helpers ----------

function other(c: Color): Color {
  return c === "black" ? "white" : "black";
}

function words(n: number) {
  return ["no", "one", "two", "three", "four"][n] ?? String(n);
}

function el(tag: string, cls = "", text?: string) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function button(text: string, onClick: () => void, cls = "") {
  const b = document.createElement("button");
  b.type = "button";
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
