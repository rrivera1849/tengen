import { describe, expect, it } from "vitest";
import type { PlayerClock } from "../src/api";
import { describe as describeClock, mmss, remaining, spend, tickSecond } from "../src/clock";

const byoyomi = (main: number, periods: number, left: number | null = null): PlayerClock => ({
  main, periods, periodTime: 30, periodLeft: left,
});

describe("clocks", () => {
  it("spends main time, then periods", () => {
    const c = byoyomi(10, 3);
    spend(c, 25);
    expect(c.main).toBe(0);
    expect(c.periods).toBe(3);
    expect(c.periodLeft).toBe(15);
    spend(c, 20);
    expect(c.periods).toBe(2);
    expect(c.periodLeft).toBe(25);
    spend(c, 1000);
    expect(c.periods).toBe(0);
    expect(c.periodLeft).toBe(0);
  });

  it("spends Canadian overtime as one block", () => {
    const c: PlayerClock = { main: 0, periods: 10, periodTime: 300, periodLeft: 120, canadian: true };
    spend(c, 30);
    expect(c.periodLeft).toBe(90);
    expect(describeClock(c)).toEqual(["1:30", "10 stones to play in this period"]);
  });

  it("describes what's left", () => {
    expect(describeClock(byoyomi(412, 5))).toEqual(["6:52", "then 5 × 30s"]);
    expect(describeClock(byoyomi(0, 3, 24))).toEqual(["0:24", "3 periods of 30s left"]);
    expect(describeClock(byoyomi(0, 1, 9))).toEqual(["0:09", "last period"]);
    expect(mmss(3725)).toBe("1:02:05");
    expect(remaining(byoyomi(0, 2, 7))).toBe(7);
  });

  it("ticks once a second in the last ten", () => {
    expect(tickSecond(10.4)).toBe(null);
    expect(tickSecond(10)).toBe(10);
    expect(tickSecond(3.2)).toBe(4);
    expect(tickSecond(0)).toBe(null);
  });
});
