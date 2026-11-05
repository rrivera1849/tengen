// Draws the board as SVG: a photographed-style wood texture under lamplight,
// the grid, glossy or flat stones, and the estimate and lesson overlays.

import type { Color, View } from "./api";
import bambooUrl from "./assets/bamboo.jpg";
import kayaUrl from "./assets/kaya.jpg";
import paperUrl from "./assets/paper.jpg";
import shinkayaUrl from "./assets/shinkaya.jpg";

export type Texture = "kaya" | "shinkaya" | "bamboo" | "ink";
export type StoneStyle = "shell" | "flat";

/** A score estimate's marks: owned points and dead stones. */
export interface Overlay {
  territory: [number, number, Color][];
  dead: [number, number][];
}

/** A better line to draw: the mover's color first. */
export interface Line {
  played: [number, number] | null;
  line: [number, number][];
  color: Color;
}

/** A mistake to circle on the board: orange for yours, blue for your opponent's. */
export interface Ring {
  at: [number, number];
  color: Color;
  mine: boolean;
  points: number;
  selected: boolean;
}

/** Review marks: circled mistakes, the try-again target and a hint's glow. */
export interface Marks {
  rings?: Ring[];
  target?: [number, number] | null;
  hint?: [number, number] | null;
  /** A move you tried that was right; it stays until the marks change. */
  found?: [number, number, Color] | null;
}

export interface Look {
  texture: Texture;
  stones: StoneStyle;
  coords: boolean;
}

const NS = "http://www.w3.org/2000/svg";
const LETTERS = "ABCDEFGHJKLMNOPQRSTUVWXYZ"; // Go coordinates skip I

/** Each board's texture and grid ink; the ink-on-paper board has no lamplight. */
const WOODS: Record<Texture, { url: string; line: string; lit: boolean }> = {
  kaya: { url: kayaUrl, line: "#3a2410", lit: true },
  shinkaya: { url: shinkayaUrl, line: "#3d2a12", lit: true },
  bamboo: { url: bambooUrl, line: "#3a2a12", lit: true },
  ink: { url: paperUrl, line: "#24221f", lit: false },
};

export function starPoints(size: number): [number, number][] {
  const edge = size >= 13 ? 3 : 2;
  const far = size - 1 - edge;
  const mid = (size - 1) / 2;
  const pts: [number, number][] = [[edge, edge], [far, edge], [edge, far], [far, far]];
  if (size % 2 === 1 && size >= 9) pts.push([mid, mid]);
  if (size >= 19) pts.push([mid, edge], [mid, far], [edge, mid], [far, mid]);
  return pts;
}

export class BoardView {
  private hover: [number, number] | null = null;
  private view: View | null = null;
  onPlay: (x: number, y: number) => void = () => {};
  /** While scoring on servers where you mark dead stones yourself: a click on a stone. */
  onMark: ((x: number, y: number) => void) | null = null;
  /** A click on a circled mistake, by its index in the rings. */
  onRing: ((index: number) => void) | null = null;
  private marks: Marks = {};
  /** Called with the board's side in CSS pixels whenever it's refitted. */
  onFit: (side: number) => void = () => {};
  /** Device pixels per grid step, once the board has been fitted. */
  private step = 0;

  constructor(private svg: SVGSVGElement, private look: Look) {
    svg.addEventListener("pointermove", (e) => this.track(e));
    svg.addEventListener("pointerleave", () => this.setHover(null));
    svg.addEventListener("click", (e) => {
      const p = this.pointAt(e);
      const ring = p ? this.ringAt(p) : -1;
      if (ring >= 0) return this.onRing?.(ring);
      if (this.marking && p && this.occupied(p)) return this.onMark?.(p[0], p[1]);
      if (this.interactive && p && !this.occupied(p)) this.onPlay(p[0], p[1]);
    });
  }

  setLook(look: Look) {
    this.look = look;
    if (this.fitted) this.fit(this.fitted);
    this.render();
  }

  update(view: View) {
    const resized = this.view?.size !== view.size;
    this.view = view;
    if (resized && this.fitted) this.fit(this.fitted);
    this.render();
  }

  private fitted = 0;

  /**
   * Sizes the board to fit `width` CSS pixels, rounded down so each grid
   * step is a whole number of device pixels: every line then lands on the
   * pixel grid and stays sharp. Returns the width used.
   */
  fit(width: number) {
    this.fitted = width;
    const dpr = window.devicePixelRatio || 1;
    const span = (this.view?.size ?? 19) - 1 + 2 * this.margin();
    this.step = Math.max(4, Math.floor((width * dpr) / span));
    const side = (this.step * span) / dpr;
    this.svg.style.width = this.svg.style.height = `${side}px`;
    this.render();
    this.onFit(side);
    return side;
  }

  /** Review marks; null clears them. */
  setMarks(m: Marks | null) {
    this.marks = m ?? {};
    this.render();
  }

  /** A wrong try: a stone that appears with a cross, then fades. */
  flashMiss(x: number, y: number, color: Color) {
    const g = this.stone(x, y, color, "miss");
    const d = 0.17;
    g.append(svg("path", { d: `M${x - d} ${y - d}L${x + d} ${y + d}M${x + d} ${y - d}L${x - d} ${y + d}`, class: "miss-cross" }));
    this.svg.append(g);
    window.setTimeout(() => g.remove(), 1100);
  }

  private ringAt([x, y]: [number, number]) {
    return (this.marks.rings ?? []).findIndex((r) => r.at[0] === x && r.at[1] === y && this.stoneAt(r.at) === r.color);
  }

  private stoneAt([x, y]: [number, number]): Color | null {
    return this.view?.stones.find(([a, b]) => a === x && b === y)?.[2] ?? null;
  }

  /** One grid step of wood around the lines; more room for coordinates. */
  private margin() {
    return this.look.coords ? 1.35 : 1;
  }

  private pointAt(e: MouseEvent): [number, number] | null {
    if (!this.view) return null;
    const box = this.svg.getBoundingClientRect();
    const span = this.view.size - 1 + 2 * this.margin();
    const x = Math.round(((e.clientX - box.left) / box.width) * span - this.margin());
    const y = Math.round(((e.clientY - box.top) / box.height) * span - this.margin());
    const n = this.view.size;
    return x >= 0 && y >= 0 && x < n && y < n ? [x, y] : null;
  }

  private occupied([x, y]: [number, number]) {
    return this.view?.stones.some(([a, b]) => a === x && b === y) ?? false;
  }

  /** False while it's the opponent's turn in a live game, or during scoring. */
  interactive = true;
  /** True while scoring, when clicking a stone marks its group dead or alive. */
  marking = false;

  private estimate: Overlay | null = null;
  private line: Line | null = null;

  /** Shows (or with null, hides) who owns what. */
  setEstimate(e: Overlay | null) {
    this.estimate = e;
    this.render();
  }

  /** Shows (or with null, hides) a better line as numbered ghost stones. */
  setLine(l: Line | null) {
    this.line = l;
    this.render();
  }

  private track(e: PointerEvent) {
    const at = this.pointAt(e);
    const clickable = at && ((this.marking && this.occupied(at)) || this.ringAt(at) >= 0);
    this.svg.style.cursor = clickable ? "pointer" : "";
    if (!this.interactive) return this.setHover(null);
    const p = this.pointAt(e);
    this.setHover(p && !this.occupied(p) ? p : null);
  }

  private setHover(p: [number, number] | null) {
    if (p?.[0] === this.hover?.[0] && p?.[1] === this.hover?.[1]) return;
    this.hover = p;
    this.svg.querySelector(".ghost")?.remove();
    if (p && this.view) this.svg.append(this.stone(p[0], p[1], this.view.toMove, "ghost"));
  }

  private render() {
    const v = this.view;
    if (!v) return;
    const m = this.margin();
    const n = v.size;
    const span = n - 1 + 2 * m;
    this.svg.setAttribute("viewBox", `${-m} ${-m} ${span} ${span}`);
    this.svg.dataset.texture = this.look.texture;
    this.svg.replaceChildren(this.defs(), this.wood(m, span), this.grid(n));
    if (this.look.coords) this.svg.append(this.coordinates(n, m));
    const deadList = [...(v.live?.removed ?? []), ...(this.estimate?.dead ?? [])];
    const dead = new Set(deadList.map(([x, y]) => `${x},${y}`));
    if (this.marks.hint) this.svg.append(this.hintGlow(this.marks.hint));
    for (const [x, y, c] of v.stones) {
      this.svg.append(this.stone(x, y, c, dead.has(`${x},${y}`) ? "stone dead" : "stone"));
    }
    if (this.estimate) this.svg.append(this.territory(this.estimate));
    if (v.last && !this.line && !this.marks.target) this.svg.append(this.lastMark(v.last));
    if (this.marks.rings) this.svg.append(this.rings(this.marks.rings));
    if (this.marks.target) this.svg.append(this.target(this.marks.target));
    if (this.marks.found) {
      const [x, y, c] = this.marks.found;
      this.svg.append(this.stone(x, y, c, "stone found"));
    }
    if (this.line) this.svg.append(this.ghostLine(this.line));
    this.hover = null;
  }

  private territory(e: Overlay) {
    const g = svg("g", { class: "territory" });
    for (const [x, y, c] of e.territory) {
      const s = 0.35;
      g.append(svg("rect", {
        x: String(x - s / 2), y: String(y - s / 2), width: String(s), height: String(s), rx: "0.06",
        class: c,
      }));
    }
    return g;
  }

  /** Circled mistakes, bigger for costlier ones; only on stones still on the board. */
  private rings(list: Ring[]) {
    const g = svg("g", { class: "rings" });
    for (const r of list) {
      if (this.stoneAt(r.at) !== r.color) continue;
      const radius = 0.68 + Math.min(r.points, 6) * 0.06;
      g.append(svg("circle", {
        cx: String(r.at[0]), cy: String(r.at[1]), r: radius.toFixed(3),
        class: `ring ${r.mine ? "mine" : "theirs"}${r.selected ? " selected" : ""}`,
      }));
    }
    return g;
  }

  /** Where you played, in a position you're trying again. */
  private target([x, y]: [number, number]) {
    return svg("circle", { class: "target", cx: String(x), cy: String(y), r: "0.52" });
  }

  /** A soft pool of light over the area where the better move is. */
  private hintGlow([x, y]: [number, number]) {
    // Off-center, so the brightest point doesn't give the answer away.
    const h = hash(x, y);
    const cx = x + ((h % 7) - 3) * 0.3;
    const cy = y + (((h >> 3) % 7) - 3) * 0.3;
    return svg("g", { class: "hint-glow" }, [
      svg("circle", { cx: cx.toFixed(2), cy: cy.toFixed(2), r: "2.8", fill: "url(#hint)" }),
      svg("circle", { cx: cx.toFixed(2), cy: cy.toFixed(2), r: "2.8", class: "hint-edge" }),
    ]);
  }

  /** The move played (a ring) and the better line, numbered from 1. */
  private ghostLine(l: Line) {
    const g = svg("g", { class: "better-line" });
    let color = l.color;
    const used = new Set<string>();
    l.line.forEach(([x, y], i) => {
      const key = `${x},${y}`;
      if (!used.has(key)) {
        used.add(key);
        const stone = this.stone(x, y, color, "line-stone");
        const label = svg("text", {
          x: String(x), y: String(y), "text-anchor": "middle", "dominant-baseline": "central",
          class: color === "black" ? "on-black" : "on-white",
        });
        label.textContent = String(i + 1);
        stone.append(label);
        g.append(stone);
      }
      color = color === "black" ? "white" : "black";
    });
    // Last, so the move actually played stays visible over the AI's stones.
    if (l.played) {
      g.append(svg("circle", { class: "played", cx: String(l.played[0]), cy: String(l.played[1]), r: "0.3" }));
    }
    return g;
  }

  private defs() {
    const shadow = (dx: number, dy: number, blur: number, alpha: number, name: string) => [
      svg("feGaussianBlur", { in: "SourceAlpha", stdDeviation: String(blur) }),
      svg("feOffset", { dx: String(dx), dy: String(dy), result: `${name}-o` }),
      svg("feFlood", { "flood-color": "#190c00", "flood-opacity": String(alpha) }),
      svg("feComposite", { in2: `${name}-o`, operator: "in", result: name }),
    ];
    return svg("defs", {}, [
      svg("radialGradient", { id: "slate", cx: "0.36", cy: "0.3", r: "0.72" }, [
        svg("stop", { offset: "0", "stop-color": "#5d6261" }),
        svg("stop", { offset: "0.26", "stop-color": "#2c2f2f" }),
        svg("stop", { offset: "0.58", "stop-color": "#161818" }),
        svg("stop", { offset: "1", "stop-color": "#0a0b0b" }),
      ]),
      svg("radialGradient", { id: "shell", cx: "0.4", cy: "0.34", r: "0.72" }, [
        svg("stop", { offset: "0", "stop-color": "#fbf8f1" }),
        svg("stop", { offset: "0.55", "stop-color": "#efe9dc" }),
        svg("stop", { offset: "0.88", "stop-color": "#d9d1bf" }),
        svg("stop", { offset: "1", "stop-color": "#c6bca7" }),
      ]),
      // The shell's highlight sits over its grain.
      svg("radialGradient", { id: "sheen", cx: "0.36", cy: "0.28", r: "0.5" }, [
        svg("stop", { offset: "0", "stop-color": "#ffffff", "stop-opacity": "0.95" }),
        svg("stop", { offset: "0.84", "stop-color": "#ffffff", "stop-opacity": "0" }),
      ]),
      // Clamshell stones have faint parallel growth lines.
      svg("pattern", { id: "grain", width: "0.11", height: "1", patternUnits: "userSpaceOnUse" }, [
        svg("rect", { x: "0.07", y: "0", width: "0.018", height: "1", fill: "#aa9673", "fill-opacity": "0.16" }),
      ]),
      // Light from the upper left: the lamp's warm pool, and a soft darkening at the rim.
      svg("radialGradient", { id: "lamp", cx: "0.42", cy: "0.38", r: "0.7" }, [
        svg("stop", { offset: "0", "stop-color": "#fff0d2", "stop-opacity": "0.16" }),
        svg("stop", { offset: "0.6", "stop-color": "#fff0d2", "stop-opacity": "0" }),
      ]),
      svg("radialGradient", { id: "rim", cx: "0.5", cy: "0.5", r: "0.75" }, [
        svg("stop", { offset: "0.7", "stop-color": "#281400", "stop-opacity": "0" }),
        svg("stop", { offset: "1", "stop-color": "#281400", "stop-opacity": "0.26" }),
      ]),
      svg("radialGradient", { id: "hint" }, [
        svg("stop", { offset: "0", "stop-color": "#fff3d6", "stop-opacity": "0.75" }),
        svg("stop", { offset: "0.7", "stop-color": "#ffe2a8", "stop-opacity": "0.35" }),
        svg("stop", { offset: "1", "stop-color": "#ffe2a8", "stop-opacity": "0" }),
      ]),
      // Two shadows: a tight one where the stone meets the wood, a soft one under it.
      svg("filter", { id: "drop", x: "-40%", y: "-40%", width: "190%", height: "190%" }, [
        ...shadow(0.03, 0.06, 0.02, 0.35, "near"),
        ...shadow(0.06, 0.15, 0.1, 0.36, "far"),
        svg("feMerge", {}, [
          svg("feMergeNode", { in: "far" }),
          svg("feMergeNode", { in: "near" }),
          svg("feMergeNode", { in: "SourceGraphic" }),
        ]),
      ]),
      svg("filter", { id: "ring-glow", x: "-50%", y: "-50%", width: "200%", height: "200%" }, [
        svg("feGaussianBlur", { in: "SourceGraphic", stdDeviation: "0.12", result: "blur" }),
        svg("feMerge", {}, [svg("feMergeNode", { in: "blur" }), svg("feMergeNode", { in: "SourceGraphic" })]),
      ]),
    ]);
  }

  private wood(m: number, span: number) {
    const w = WOODS[this.look.texture];
    const box = { x: String(-m), y: String(-m), width: String(span), height: String(span) };
    const g = svg("g", { class: "wood" });
    g.append(svg("image", { ...box, href: w.url, preserveAspectRatio: "xMidYMid slice" }));
    if (w.lit) {
      g.append(svg("rect", { ...box, fill: "url(#lamp)" }));
      g.append(svg("rect", { ...box, fill: "url(#rim)" }));
    }
    return g;
  }

  private grid(n: number) {
    const line = WOODS[this.look.texture].line;
    // Fitted, a line is one CSS pixel (the border two), on whole device
    // pixels; an odd width needs a half-pixel shift to land there.
    const dpr = window.devicePixelRatio || 1;
    const px = this.step ? Math.max(1, Math.round(dpr)) : 0;
    const width = px ? px / this.step : 0.034;
    const edgeWidth = px ? (2 * px) / this.step : width * 1.6;
    const shift = px && px % 2 ? 0.5 / this.step : 0;
    const g = svg("g", { class: "grid", transform: `translate(${shift} ${shift})` });
    for (let i = 0; i < n; i++) {
      const edge = i === 0 || i === n - 1;
      const sw = String(edge ? edgeWidth : width);
      g.append(svg("line", { x1: "0", y1: String(i), x2: String(n - 1), y2: String(i), stroke: line, "stroke-width": sw, "stroke-linecap": "square", opacity: "0.78" }));
      g.append(svg("line", { x1: String(i), y1: "0", x2: String(i), y2: String(n - 1), stroke: line, "stroke-width": sw, "stroke-linecap": "square", opacity: "0.78" }));
    }
    for (const [x, y] of starPoints(n)) {
      g.append(svg("circle", { cx: String(x), cy: String(y), r: "0.085", fill: line, opacity: "0.92" }));
    }
    return g;
  }

  private coordinates(n: number, m: number) {
    const g = svg("g", { class: "coords" });
    const off = m * 0.62;
    for (let i = 0; i < n; i++) {
      const letter = LETTERS[i];
      const num = String(n - i);
      for (const [x, y, t] of [[i, -off, letter], [i, n - 1 + off, letter], [-off, i, num], [n - 1 + off, i, num]] as const) {
        const text = svg("text", { x: String(x), y: String(y), "text-anchor": "middle", "dominant-baseline": "central" });
        text.textContent = t;
        g.append(text);
      }
    }
    return g;
  }

  private stone(x: number, y: number, color: Color, cls = "stone") {
    const g = svg("g", { class: `${cls} ${color}` });
    const flat = this.look.stones === "flat";
    // Real stones never sit exactly on the point.
    const placed = cls.startsWith("stone") && !flat;
    const h = hash(x, y);
    const [cx, cy] = placed ? nudge(x, y) : [x, y];
    const attrs: Record<string, string> = { cx: cx.toFixed(3), cy: cy.toFixed(3), r: "0.478" };
    if (flat) {
      Object.assign(attrs, color === "black"
        ? { fill: "#202226" }
        : { fill: "#fbfbf8", stroke: "#2b2d31", "stroke-width": "0.04" });
    } else {
      Object.assign(attrs, { fill: color === "black" ? "url(#slate)" : "url(#shell)" });
      // On pale paper a shell stone needs a hairline edge to stay visible.
      if (color === "white" && this.look.texture === "ink") {
        Object.assign(attrs, { stroke: "#9a9c96", "stroke-width": "0.02" });
      }
    }
    if (cls.startsWith("stone") || cls === "line-stone") attrs.filter = "url(#drop)";
    g.append(svg("circle", attrs));
    if (!flat && color === "white") {
      const r = attrs.r;
      const turn = h % 180;
      g.append(svg("circle", { cx: attrs.cx, cy: attrs.cy, r, fill: "url(#grain)", transform: `rotate(${turn} ${attrs.cx} ${attrs.cy})`, "pointer-events": "none" }));
      g.append(svg("circle", { cx: attrs.cx, cy: attrs.cy, r, fill: "url(#sheen)", "pointer-events": "none" }));
    }
    return g;
  }

  /** The last move: a small brass dot. */
  private lastMark([x, y]: [number, number]) {
    const [cx, cy] = this.look.stones === "flat" ? [x, y] : nudge(x, y);
    return svg("circle", { class: "last", cx: cx.toFixed(3), cy: cy.toFixed(3), r: "0.13" });
  }
}

/** A stable little number per point, for placement and grain. */
function hash(x: number, y: number) {
  let h = (x * 73856093) ^ (y * 19349663);
  h = Math.imul(h ^ (h >>> 13), 0x5bd1e995);
  return (h ^ (h >>> 15)) >>> 0;
}

/** Where a stone actually rests: up to 3% of a grid step off the point. */
function nudge(x: number, y: number): [number, number] {
  const h = hash(x, y);
  return [x + ((h & 15) / 15 - 0.5) * 0.06, y + (((h >> 4) & 15) / 15 - 0.5) * 0.06];
}

function svg(tag: string, attrs: Record<string, string>, children: Element[] = []) {
  const el = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  el.append(...children);
  return el;
}
