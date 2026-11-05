// What a try in the review's "Try again" means, once KataGo has judged it.
// Kept apart from the panel so it can be tested on its own.

import type { Judgement } from "./api";

export const TRIES = 3;

export type TryResult =
  | { kind: "found"; note: string }
  | { kind: "miss"; note: string }
  | { kind: "shown"; note: string };

/**
 * `judgement` is KataGo's verdict on the try, or null when KataGo couldn't
 * judge it. `exact` is true for KataGo's own move, `played` for the move
 * played in the game. `triesBefore` counts misses before this try.
 */
export function afterTry(
  judgement: Judgement | null,
  opts: { exact: boolean; played: boolean; triesBefore: number },
): TryResult {
  if (opts.exact) {
    return { kind: "found", note: opts.triesBefore === 0 ? "Found it, first try." : "Found it." };
  }
  if (!opts.played && judgement?.verdict === "best") {
    return { kind: "found", note: "As good as KataGo's choice, marked 1." };
  }
  if (!opts.played && judgement?.verdict === "good") {
    return { kind: "found", note: `Good too: it gives up only ${points(judgement.loss)} next to KataGo's choice, marked 1.` };
  }
  const left = TRIES - (opts.triesBefore + 1);
  if (left <= 0) return { kind: "shown", note: "Here's KataGo's move." };
  const why = opts.played
    ? "That's the move that was played."
    : judgement?.verdict === "closer"
      ? `Closer: better than the game's move, but it still gives up about ${points(judgement.loss)}.`
      : judgement
        ? "Not that one."
        : "Not KataGo's choice.";
  return { kind: "miss", note: `${why} ${left === 1 ? "One try" : "Two tries"} left.` };
}

function points(n: number) {
  const r = Math.round(n * 10) / 10;
  return `${r} ${r === 1 ? "point" : "points"}`;
}
