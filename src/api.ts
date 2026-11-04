// The bridge to the Rust core. Every call mirrors a command in src-tauri/src/lib.rs.

import { invoke } from "@tauri-apps/api/core";

export type Color = "black" | "white";

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
};

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!inApp) return preview(command) as T;
  return invoke<T>(command, args);
}

// A fixed position so the interface can be designed in a browser with `npm run dev`.
function preview(command: string): View | null {
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
    whiteRank: "3d",
    result: null,
    server: "OGS",
    komi: 6.5,
  };
}
