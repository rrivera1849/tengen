import { describe, expect, it } from "vitest";
import { afterTry } from "../src/quiz";

const try0 = { exact: false, played: false, triesBefore: 0 };

describe("Try again", () => {
  it("counts KataGo's own move as found", () => {
    expect(afterTry(null, { ...try0, exact: true })).toEqual({ kind: "found", note: "Found it, first try." });
    expect(afterTry(null, { ...try0, exact: true, triesBefore: 2 }).note).toBe("Found it.");
  });

  it("counts a move within a point as found", () => {
    const r = afterTry({ verdict: "good", loss: 0.64 }, try0);
    expect(r.kind).toBe("found");
    expect(r.note).toContain("0.6 points");
    expect(afterTry({ verdict: "best", loss: 0.1 }, try0).kind).toBe("found");
    expect(afterTry({ verdict: "good", loss: 1 }, try0).note).toContain("1 point next");
  });

  it("encourages a closer move but counts it as a try", () => {
    const r = afterTry({ verdict: "closer", loss: 1.8 }, try0);
    expect(r).toEqual({ kind: "miss", note: "Closer: better than the game's move, but it still gives up about 1.8 points. Two tries left." });
  });

  it("names the move that was played, whatever KataGo says", () => {
    expect(afterTry({ verdict: "good", loss: 0.2 }, { ...try0, played: true }).note).toMatch(/^That's the move that was played/);
  });

  it("shows the answer after the third miss", () => {
    expect(afterTry({ verdict: "worse", loss: 5 }, { ...try0, triesBefore: 1 }).note).toBe("Not that one. One try left.");
    expect(afterTry({ verdict: "worse", loss: 5 }, { ...try0, triesBefore: 2 })).toEqual({ kind: "shown", note: "Here's KataGo's move." });
  });

  it("falls back gracefully when KataGo couldn't judge", () => {
    expect(afterTry(null, try0).note).toBe("Not KataGo's choice. Two tries left.");
  });
});
