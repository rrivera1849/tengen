// The bridge to the Rust core. Every call mirrors a command in src-tauri/src/lib.rs,
// and every event mirrors an `emit` in src-tauri/src/live.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Color = "black" | "white";
export type Speed = "blitz" | "rapid" | "live";
/** OGS signs in through the browser; KGS, IGS (Pandanet) and Fox with a name and password. */
export type ServerName = "OGS" | "KGS" | "IGS" | "Fox";

export interface LiveView {
  gameId: number;
  myColor: Color | null;
  phase: "play" | "stone removal" | "finished" | string;
  removed: [number, number][];
  /** True while scoring on a server where you click dead stones yourself. */
  markDead: boolean;
}

/** A game against KataGo playing like a person of `rank`. */
export interface BotView {
  you: Color;
  rank: string;
  thinking: boolean;
  over: boolean;
}

export interface View {
  size: number;
  stones: [number, number, Color][];
  toMove: Color;
  cursor: number;
  total: number;
  last: [number, number] | null;
  lastWasPass: boolean;
  capturesBlack: number;
  capturesWhite: number;
  black: string;
  white: string;
  blackRank: string | null;
  whiteRank: string | null;
  result: string | null;
  server: string | null;
  komi: number;
  live: LiveView | null;
  bot: BotView | null;
}

export interface ServerStatus {
  server: ServerName;
  /** False for OGS until Tengen is registered there (see the README). */
  configured: boolean;
  signedIn: boolean;
  connected: boolean;
  username: string | null;
  rank: string | null;
  searching: boolean;
  inGame: boolean;
}

export interface PlayerClock {
  main: number;
  periods: number | null;
  periodTime: number | null;
  periodLeft: number | null;
  /** Canadian overtime: `periods` is the stones left to play in this period. */
  canadian?: boolean;
}

export interface Clock {
  gameId: number;
  toMove: Color | null;
  black: PlayerClock;
  white: PlayerClock;
  paused: boolean;
}

export interface Estimate {
  lead: number;
  blackWinChance: number;
  territory: [number, number, Color][];
  dead: [number, number][];
  headline: string;
}

export interface AiStatus {
  katago: boolean;
  apiKey: boolean;
  /** KataGo's human-style network, for playing against KataGo. */
  humanModel: boolean;
}

/** How a move tried in "Try again" compares with KataGo's best. */
export interface Judgement {
  verdict: "best" | "good" | "closer" | "worse";
  /** Points it loses against KataGo's best. */
  loss: number;
}

/** A game in your library. */
export interface LibraryEntry {
  file: string;
  date: string | null;
  black: string;
  white: string;
  blackRank: string | null;
  whiteRank: string | null;
  result: string | null;
  server: string | null;
  size: number;
  moves: number;
  reviewed: boolean;
}

export interface Update {
  version: string;
  url: string;
}

/** The strengths KataGo can play at, weakest first. */
export const RANKS = ["20k", "15k", "10k", "7k", "5k", "3k", "1k", "3d", "6d"] as const;

export interface Progress {
  stage: "download" | "starting" | "quick" | "deep" | "lessons";
  done: number;
  total: number;
}

export interface Lesson {
  headline: string;
  what_happened: string;
  better_idea: string;
  why: string;
  maxim: string;
  points: string[];
}

export interface LessonView {
  side: "you" | "opponent";
  moveNumber: number;
  color: Color;
  kind: string;
  pointsLost: number;
  played: [number, number] | null;
  best: [number, number] | null;
  line: [number, number][];
  answerLine: [number, number][];
  yourReply: [number, number] | null;
  winBefore: number;
  winAfter: number;
  bestCoord: string;
  playedCoord: string;
  punish: { your_reply: string | null; best_reply: string | null; found: boolean } | null;
  lesson: Lesson | null;
  problem: string | null;
}

export interface Review {
  fingerprint: string;
  you: Color;
  leads: number[];
  summary: { summary: string; theme: string; maxim: string } | null;
  lessons: LessonView[];
  note: string | null;
}

/** True inside the Mac app; false when the UI is opened in a plain browser. */
export const inApp = "__TAURI_INTERNALS__" in window;

export const api = {
  current: () => call<View>("current"),
  newGame: (size: number, komi: number) => call<View>("new_game", { size, komi }),
  play: (x: number, y: number) => call<View>("play", { x, y }),
  pass: () => call<View>("pass_turn"),
  undo: () => call<View>("undo"),
  goTo: (n: number) => call<View>("go_to", { n }),
  openSgf: () => call<View | null>("open_sgf"),
  saveSgf: () => call<string | null>("save_sgf"),
  downloadOgs: (game: string) => call<View>("download_ogs", { game }),

  serverStatus: () => call<ServerStatus[]>("server_status"),
  /** OGS: opens the browser to sign in. */
  ogsSignIn: () => call<ServerStatus[]>("ogs_sign_in"),
  /** KGS and Pandanet: the login is checked, then kept in Keychain. */
  signIn: (server: ServerName, username: string, password: string) =>
    call<ServerStatus[]>("server_sign_in", { server, username, password }),
  signOut: (server: ServerName) => call<ServerStatus[]>("server_sign_out", { server }),
  findMatch: (server: ServerName, size: number, speed: Speed) => call<ServerStatus[]>("find_match", { server, size, speed }),
  cancelMatch: () => call<ServerStatus[]>("cancel_match"),
  resign: () => call<void>("resign"),
  acceptScore: () => call<void>("accept_score"),
  leaveGame: () => call<View>("leave_game"),

  aiStatus: () => call<AiStatus>("ai_status"),
  installKatago: () => call<AiStatus>("ai_install"),
  estimate: () => call<Estimate>("estimate_score"),
  reviewSide: () => call<Color | null>("review_side"),
  startReview: (you: Color) => call<Review>("start_review", { you }),
  setClaudeKey: (key: string) => call<AiStatus>("set_claude_key", { key }),
  clearClaudeKey: () => call<AiStatus>("clear_claude_key"),
  maxims: () => call<[string, string[]][]>("list_maxims"),
  nextMaxim: () => call<string | null>("next_maxim"),
  judgeMove: (turn: number, x: number, y: number, playedLoss: number) =>
    call<Judgement>("judge_move", { turn, x, y, playedLoss }),

  installHumanModel: () => call<AiStatus>("install_human_model"),
  /** `you` null lets Tengen pick a color. */
  newBotGame: (size: number, komi: number, you: Color | null, rank: string) =>
    call<View>("new_bot_game", { size, komi, you, rank }),

  library: () => call<LibraryEntry[]>("library"),
  openLibraryGame: (file: string) => call<View>("open_library_game", { file }),

  diagnostics: () => call<string>("diagnostics"),
  checkUpdate: () => call<Update | null>("check_update"),
  openRelease: (url: string) => call<void>("open_release", { url }),
};

/** Updates pushed from Rust: the board, the account, clocks and short notices. */
export function onEvents(handlers: {
  view: (v: View) => void;
  servers: (s: ServerStatus[]) => void;
  clock: (c: Clock) => void;
  notice: (text: string) => void;
  progress: (p: Progress) => void;
}) {
  if (!inApp) return;
  listen<Progress>("ai-progress", (e) => handlers.progress(e.payload));
  listen<View>("view", (e) => handlers.view(e.payload));
  listen<ServerStatus[]>("servers", (e) => handlers.servers(e.payload));
  listen<Clock>("clock", (e) => handlers.clock(e.payload));
  listen<string>("notice", (e) => handlers.notice(e.payload));
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!inApp) return preview(command, args) as T;
  return invoke<T>(command, args);
}

// A fixed position so the interface can be designed in a browser with `npm run dev`.
// Add `?live` to the address to preview a live game with clocks.
const PREVIEW_MOVES: [number, number][] = [[15, 3], [3, 15], [16, 15], [3, 3], [14, 16], [2, 5], [9, 9], [15, 9], [16, 9], [15, 8], [5, 2], [2, 9]];

/** The preview's game: moves (null is a pass), a cursor and an optional KataGo opponent. */
const pv = {
  size: 19,
  moves: PREVIEW_MOVES.slice() as ([number, number] | null)[],
  cursor: PREVIEW_MOVES.length,
  bot: null as BotView | null,
  result: null as string | null,
  black: "Sora",
  white: "Mina",
  blackRank: "3d" as string | null,
  whiteRank: "2k" as string | null,
  server: "OGS" as string | null,
};

function previewView(live: boolean): View {
  const shown = pv.moves.slice(0, pv.cursor);
  const stones = new Map<string, [number, number, Color]>();
  shown.forEach((m, i) => {
    if (m) stones.set(`${m[0]},${m[1]}`, [m[0], m[1], i % 2 ? "white" : "black"]);
  });
  const last = shown[shown.length - 1];
  return {
    size: pv.size,
    stones: [...stones.values()],
    toMove: pv.cursor % 2 ? "white" : "black",
    cursor: pv.cursor,
    total: pv.moves.length,
    last: last ?? null,
    lastWasPass: shown.length > 0 && last === null,
    capturesBlack: 0,
    capturesWhite: 0,
    black: pv.black,
    white: pv.white,
    blackRank: pv.blackRank,
    whiteRank: pv.whiteRank,
    result: pv.result,
    server: pv.server,
    komi: 6.5,
    live: live ? { gameId: 1, myColor: "black", phase: "play", removed: [], markDead: false } : null,
    bot: pv.bot,
  };
}

/** The preview's KataGo answers on the nearest free point to the board's center. */
function previewBotMove() {
  const taken = new Set(pv.moves.filter(Boolean).map((m) => `${m![0]},${m![1]}`));
  const c = (pv.size - 1) / 2;
  for (let r = 0; r < pv.size; r++) {
    for (let dy = -r; dy <= r; dy++) {
      for (let dx = -r; dx <= r; dx++) {
        const x = Math.round(c + dx + 3);
        const y = Math.round(c + dy - 3);
        if (x < 0 || y < 0 || x >= pv.size || y >= pv.size || taken.has(`${x},${y}`)) continue;
        pv.moves.push([x, y]);
        pv.cursor = pv.moves.length;
        return;
      }
    }
  }
  pv.moves.push(null);
  pv.cursor = pv.moves.length;
}

function previewMove(m: [number, number] | null) {
  pv.moves = pv.moves.slice(0, pv.cursor);
  pv.moves.push(m);
  pv.cursor = pv.moves.length;
  if (pv.bot && !pv.bot.over) {
    if (m === null) {
      // The preview's KataGo answers a pass with a pass, and counts.
      pv.moves.push(null);
      pv.cursor = pv.moves.length;
      pv.bot = { ...pv.bot, over: true };
      pv.result = "B+3.5";
      return;
    }
    previewBotMove();
  }
}

function previewLibrary(): LibraryEntry[] {
  return [
    { file: "2026-11-04 21.30 KataGo You vs KataGo (5k).sgf", date: "2026-11-04", black: "You", white: "KataGo", blackRank: null, whiteRank: "5k", result: "B+3.5", server: "KataGo", size: 9, moves: 64, reviewed: false },
    { file: "2026-11-03 20.12 OGS Sora (3d) vs Kenji (2k).sgf", date: "2026-11-03", black: "Sora", white: "Kenji", blackRank: "3d", whiteRank: "2k", result: "W+R", server: "OGS", size: 19, moves: 187, reviewed: true },
    { file: "2026-11-01 18.40 Pandanet Mika (1k) vs Sora (2d).sgf", date: "2026-11-01", black: "Mika", white: "Sora", blackRank: "1k", whiteRank: "2d", result: "B+12.5", server: "Pandanet", size: 19, moves: 241, reviewed: false },
  ];
}

function preview(command: string, args?: Record<string, unknown>): unknown {
  const params = new URLSearchParams(location.search);
  const live = params.has("live");
  if (command === "server_status") {
    const signedIn = live || params.has("signedin");
    const status = (server: ServerName, on: boolean): ServerStatus => ({
      server, configured: true, signedIn: on, connected: on && !params.has("offline"), username: on ? "rafael" : null, rank: on ? "12k" : null,
      searching: params.has("searching") && server === "OGS", inGame: live && server === "OGS",
    });
    return [status("OGS", signedIn), status("KGS", params.has("kgs")), status("IGS", false), status("Fox", params.has("fox"))];
  }
  if (command === "ai_status") return { katago: true, apiKey: true, humanModel: !params.has("nohuman") };
  if (command === "review_side") return "black";
  if (command === "estimate_score") return previewEstimate();
  if (command === "start_review") return previewReview();
  if (command === "list_maxims") return [["Direction of play", ["When your opponent builds, choose the biggest area first"]]];
  if (command === "next_maxim") return "Before answering, ask whether the move needs an answer";
  if (command === "library") return previewLibrary();
  if (command === "diagnostics") return "Tengen diagnostics\nTengen: preview\n";
  if (command === "check_update") return params.has("update") ? { version: "1.1.0", url: "https://github.com/rrivera1849/tengen/releases/tag/v1.1.0" } : null;
  if (command === "install_human_model") return { katago: true, apiKey: true, humanModel: true };
  if (command === "judge_move") {
    // Points near KataGo's choice in the preview review (K16) count as good.
    const near = Math.abs(Number(args?.x) - 9) + Math.abs(Number(args?.y) - 3);
    return near <= 1 ? { verdict: "good", loss: 0.6 } : near <= 3 ? { verdict: "closer", loss: 1.4 } : { verdict: "worse", loss: 4.2 };
  }
  if (command === "go_to") pv.cursor = Math.max(0, Math.min(pv.moves.length, Number(args?.n ?? pv.moves.length)));
  if (command === "play") previewMove([Number(args?.x), Number(args?.y)]);
  if (command === "pass_turn") previewMove(null);
  if (command === "new_bot_game") {
    Object.assign(pv, {
      size: Number(args?.size ?? 19), moves: [], cursor: 0, result: null, server: "KataGo",
      bot: { you: (args?.you as Color | null) ?? "black", rank: String(args?.rank), thinking: false, over: false },
    });
    const you = pv.bot!.you;
    pv.black = you === "black" ? "You" : "KataGo";
    pv.white = you === "white" ? "You" : "KataGo";
    pv.blackRank = you === "black" ? null : String(args?.rank);
    pv.whiteRank = you === "white" ? null : String(args?.rank);
    if (you === "white") previewBotMove();
  }
  if (command === "resign" && pv.bot) {
    pv.bot = { ...pv.bot, over: true };
    pv.result = pv.bot.you === "black" ? "W+R" : "B+R";
    return undefined;
  }
  if (command === "leave_game") pv.bot = null;
  if (command === "open_library_game") {
    Object.assign(pv, { moves: PREVIEW_MOVES.slice(), cursor: 0, bot: null, result: "W+R", server: "OGS" });
  }
  if (!["current", "go_to", "play", "pass_turn", "new_bot_game", "leave_game", "open_library_game"].includes(command)) {
    throw new Error("Open the Tengen app to play; this browser view is a preview.");
  }
  return previewView(live);
}

/** A clock for the browser preview. */
export function previewClock(): Clock | null {
  if (inApp || !new URLSearchParams(location.search).has("live")) return null;
  return {
    gameId: 1,
    toMove: "black",
    paused: false,
    black: { main: 0, periods: 3, periodTime: 30, periodLeft: 24 },
    white: { main: 412, periods: 5, periodTime: 30, periodLeft: null },
  };
}

function previewEstimate(): Estimate {
  const territory: [number, number, Color][] = [];
  const stones = new Set(previewView(false).stones.map(([x, y]) => `${x},${y}`));
  for (let y = 0; y < 19; y++) {
    for (let x = 0; x < 19; x++) {
      if (stones.has(`${x},${y}`)) continue;
      if ((x < 6 && y > 12) || (x < 5 && y < 6)) territory.push([x, y, "white"]);
      if ((x > 13 && y < 6) || (x > 13 && y > 12)) territory.push([x, y, "black"]);
    }
  }
  return { lead: 4.3, blackWinChance: 0.68, territory, dead: [], headline: "Black leads by 4.5" };
}

function previewReview(): Review {
  const leads = [0.5, 0.4, 1, 0.6, 1.2, 1.4, -1.6, -1.8, -1.2, -1.5, -0.4, -2.6, -2.1];
  return {
    fingerprint: "preview",
    you: "black",
    leads,
    summary: {
      summary: "You built a strong position on the right, then let White settle comfortably on the left side.",
      theme: "Twice you answered locally when a bigger area was open.",
      maxim: "Before answering, ask whether the move needs an answer",
    },
    note: null,
    lessons: [
      {
        side: "you", moveNumber: 7, color: "black", kind: "Tempo", pointsLost: 3.1,
        played: [9, 9], best: [9, 3], line: [[9, 3], [9, 15], [13, 13]], answerLine: [], yourReply: null,
        winBefore: 55, winAfter: 47, bestCoord: "K16", playedCoord: "K10", punish: null,
        lesson: {
          headline: "The center stone could wait; the top side couldn't",
          what_happened: "Tengen at the center looks big, but nothing there was under attack.",
          better_idea: "K16 claims the open top side while White's two stones on the left are still thin.",
          why: "A move in the center only gains value once the sides are settled. Points on a wide open side are worth more now.",
          maxim: "Before answering, ask whether the move needs an answer",
          points: ["D16", "K16"],
        },
        problem: null,
      },
      {
        side: "opponent", moveNumber: 6, color: "white", kind: "Local fight", pointsLost: 4.4,
        played: [2, 5], best: [5, 2], line: [[3, 6], [2, 7]], answerLine: [[3, 6], [2, 6], [4, 5]], yourReply: [9, 9],
        winBefore: 52, winAfter: 40, bestCoord: "F17", playedCoord: "C14",
        punish: { your_reply: "K10", best_reply: "D13", found: false },
        lesson: {
          headline: "White's extension left a cut you could have used",
          what_happened: "White extended down the left side at C14 and left a weakness behind it.",
          better_idea: "D13 cuts White apart; you played K10 elsewhere and the chance passed.",
          why: "When your opponent's stones are thin, the cut is worth more than a big point, because it creates two weak groups at once.",
          maxim: "Check for a cut before taking a big point",
          points: ["C14", "D13", "K10"],
        },
        problem: null,
      },
      {
        side: "you", moveNumber: 11, color: "black", kind: "Shape", pointsLost: 1.6,
        played: [5, 2], best: [4, 3], line: [[4, 3], [2, 2], [5, 4]], answerLine: [], yourReply: null,
        winBefore: 49, winAfter: 44, bestCoord: "E16", playedCoord: "F17", punish: null,
        lesson: null,
        problem: null,
      },
    ],
  };
}
