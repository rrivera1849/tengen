// The bridge to the Rust core. Every call mirrors a command in src-tauri/src/lib.rs,
// and every event mirrors an `emit` in src-tauri/src/live.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Color = "black" | "white";
export type Speed = "blitz" | "rapid" | "live";

export interface LiveView {
  gameId: number;
  myColor: Color | null;
  phase: "play" | "stone removal" | "finished" | string;
  removed: [number, number][];
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

export interface OgsStatus {
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
}

export interface Clock {
  gameId: number;
  toMove: Color | null;
  black: PlayerClock;
  white: PlayerClock;
  paused: boolean;
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

  ogsStatus: () => call<OgsStatus>("ogs_status"),
  signIn: () => call<OgsStatus>("ogs_sign_in"),
  signOut: () => call<OgsStatus>("ogs_sign_out"),
  findMatch: (size: number, speed: Speed) => call<OgsStatus>("find_match", { size, speed }),
  cancelMatch: () => call<OgsStatus>("cancel_match"),
  resign: () => call<void>("resign"),
  acceptScore: () => call<void>("accept_score"),
  leaveGame: () => call<View>("leave_game"),
};

/** Updates pushed from Rust: the board, the account, clocks and short notices. */
export function onEvents(handlers: {
  view: (v: View) => void;
  ogs: (s: OgsStatus) => void;
  clock: (c: Clock) => void;
  notice: (text: string) => void;
}) {
  if (!inApp) return;
  listen<View>("view", (e) => handlers.view(e.payload));
  listen<OgsStatus>("ogs", (e) => handlers.ogs(e.payload));
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
  if (command === "ogs_status") {
    const signedIn = live || params.has("signedin");
    return { configured: true, signedIn, connected: signedIn, username: "rafael", rank: "3d", searching: false, inGame: live };
  }
  if (command !== "current") throw new Error("Open the Tengen app to play; this browser view is a preview.");
  const moves: [number, number][] = [[15, 3], [3, 15], [16, 15], [3, 3], [14, 16], [2, 5], [9, 9], [15, 9]];
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
    live: live ? { gameId: 1, myColor: "black", phase: "play", removed: [] } : null,
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
