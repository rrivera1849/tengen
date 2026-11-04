// The bridge to the Rust core. Every call mirrors a command in src-tauri/src/lib.rs,
// and every event mirrors an `emit` in src-tauri/src/live.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Color = "black" | "white";
export type Speed = "blitz" | "rapid" | "live";
/** OGS signs in through the browser; KGS and IGS (Pandanet) with a name and password. */
export type ServerName = "OGS" | "KGS" | "IGS";

export interface LiveView {
  gameId: number;
  myColor: Color | null;
  phase: "play" | "stone removal" | "finished" | string;
  removed: [number, number][];
  /** True while scoring on a server where you click dead stones yourself. */
  markDead: boolean;
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
}

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
  if (!inApp) return preview(command) as T;
  return invoke<T>(command, args);
}

// A fixed position so the interface can be designed in a browser with `npm run dev`.
// Add `?live` to the address to preview a live game with clocks.
function preview(command: string): unknown {
  const params = new URLSearchParams(location.search);
  const live = params.has("live");
  if (command === "server_status") {
    const signedIn = live || params.has("signedin");
    const status = (server: ServerName, on: boolean): ServerStatus => ({
      server, configured: true, signedIn: on, connected: on, username: on ? "rafael" : null, rank: on ? "12k" : null,
      searching: false, inGame: live && server === "OGS",
    });
    return [status("OGS", signedIn), status("KGS", params.has("kgs")), status("IGS", false)];
  }
  if (command === "ai_status") return { katago: true, apiKey: true };
  if (command === "review_side") return "black";
  if (command === "estimate_score") return previewEstimate();
  if (command === "start_review") return previewReview();
  if (command === "list_maxims") return [["Direction of play", ["When your opponent builds, choose the biggest area first"]]];
  if (command !== "current" && command !== "go_to") throw new Error("Open the Tengen app to play; this browser view is a preview.");
  const moves: [number, number][] = [[15, 3], [3, 15], [16, 15], [3, 3], [14, 16], [2, 5], [9, 9], [15, 9], [16, 9], [15, 8], [5, 2], [2, 9]];
  return {
    size: 19,
    stones: moves.map(([x, y], i) => [x, y, i % 2 ? "white" : "black"]),
    toMove: "black",
    cursor: moves.length,
    total: moves.length,
    last: moves[moves.length - 1],
    lastWasPass: false,
    capturesBlack: 0,
    capturesWhite: 0,
    black: "Rafael",
    white: "Opponent",
    blackRank: "3d",
    whiteRank: "2k",
    result: null,
    server: "OGS",
    komi: 6.5,
    live: live ? { gameId: 1, myColor: "black", phase: "play", removed: [], markDead: false } : null,
  } satisfies View;
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
  const stones = new Set((preview("current") as View).stones.map(([x, y]) => `${x},${y}`));
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
  const leads = [0, 0.5, 1, -2, -1.5, 3, 2.5, 4, 4.3, 3.8, 5.1, 4.6, 5.0];
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
        side: "you", moveNumber: 4, color: "black", kind: "Tempo", pointsLost: 3.1,
        played: [3, 3], best: [9, 3], line: [[9, 3], [9, 15], [13, 13]], answerLine: [], yourReply: null,
        winBefore: 55, winAfter: 47, bestCoord: "K16", playedCoord: "D16", punish: null,
        lesson: {
          headline: "The top side was bigger than the corner answer",
          what_happened: "White approached your corner and you answered next to it at D16, a solid but small move.",
          better_idea: "K16 claims the top side while it is still open and leaves the corner for later.",
          why: "The corner was already safe, so answering cost you the move with the most value. Points left open on a wide side are worth more than securing a corner that wasn't in danger.",
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
    ],
  };
}
