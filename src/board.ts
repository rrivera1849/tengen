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

  constructor(private svg: SVGSVGElement, private look: Look) {
    svg.addEventListener("pointermove", (e) => this.track(e));
    svg.addEventListener("pointerleave", () => this.setHover(null));
    svg.addEventListener("click", (e) => {
      const p = this.pointAt(e);
      if (this.marking && p && this.occupied(p)) return this.onMark?.(p[0], p[1]);
      if (this.interactive && p && !this.occupied(p)) this.onPlay(p[0], p[1]);
    });
  }

  setLook(look: Look) {
    this.look = look;
    this.render();
  }

  update(view: View) {
    this.view = view;
    this.render();
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
    this.svg.style.cursor = this.marking && this.pointAt(e) && this.occupied(this.pointAt(e)!) ? "pointer" : "";
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
    for (const [x, y, c] of v.stones) {
      this.svg.append(this.stone(x, y, c, dead.has(`${x},${y}`) ? "stone dead" : "stone"));
    }
    if (this.estimate) this.svg.append(this.territory(this.estimate));
    if (v.last && !this.line) this.svg.append(this.lastMark(v.last));
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
    return svg("defs", {}, [
      svg("radialGradient", { id: "slate", cx: "0.34", cy: "0.28", r: "0.78" }, [
        svg("stop", { offset: "0", "stop-color": "#8a8f8d" }),
        svg("stop", { offset: "0.22", "stop-color": "#3a3d3c" }),
        svg("stop", { offset: "0.55", "stop-color": "#151717" }),
        svg("stop", { offset: "1", "stop-color": "#050606" }),
      ]),
      svg("radialGradient", { id: "shell", cx: "0.36", cy: "0.28", r: "0.8" }, [
        svg("stop", { offset: "0", "stop-color": "#ffffff" }),
        svg("stop", { offset: "0.3", "stop-color": "#f6f3ea" }),
        svg("stop", { offset: "0.7", "stop-color": "#e2ddd0" }),
        svg("stop", { offset: "1", "stop-color": "#c4bdac" }),
      ]),
      // Light from the upper left: the lamp's warm pool, and a soft darkening at the rim.
      svg("radialGradient", { id: "lamp", cx: "0.42", cy: "0.38", r: "0.7" }, [
        svg("stop", { offset: "0", "stop-color": "#fff0d2", "stop-opacity": "0.2" }),
        svg("stop", { offset: "1", "stop-color": "#fff0d2", "stop-opacity": "0" }),
      ]),
      svg("radialGradient", { id: "rim", cx: "0.5", cy: "0.5", r: "0.75" }, [
        svg("stop", { offset: "0.7", "stop-color": "#281400", "stop-opacity": "0" }),
        svg("stop", { offset: "1", "stop-color": "#281400", "stop-opacity": "0.3" }),
      ]),
      svg("filter", { id: "drop", x: "-40%", y: "-40%", width: "190%", height: "190%" }, [
        svg("feDropShadow", { dx: "0.06", dy: "0.12", stdDeviation: "0.07", "flood-color": "#140800", "flood-opacity": "0.5" }),
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
    const g = svg("g", { class: "grid" });
    const line = WOODS[this.look.texture].line;
    const width = 0.034;
    for (let i = 0; i < n; i++) {
      const edge = i === 0 || i === n - 1;
      const sw = String(edge ? width * 1.6 : width);
      g.append(svg("line", { x1: "0", y1: String(i), x2: String(n - 1), y2: String(i), stroke: line, "stroke-width": sw, "stroke-linecap": "square", opacity: "0.74" }));
      g.append(svg("line", { x1: String(i), y1: "0", x2: String(i), y2: String(n - 1), stroke: line, "stroke-width": sw, "stroke-linecap": "square", opacity: "0.74" }));
    }
    for (const [x, y] of starPoints(n)) {
      g.append(svg("circle", { cx: String(x), cy: String(y), r: "0.09", fill: line, opacity: "0.9" }));
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
    const attrs: Record<string, string> = { cx: String(x), cy: String(y), r: "0.47" };
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
    return g;
  }

  /** The last move: a small brass dot. */
  private lastMark([x, y]: [number, number]) {
    return svg("circle", { class: "last", cx: String(x), cy: String(y), r: "0.17" });
  }
}

function svg(tag: string, attrs: Record<string, string>, children: Element[] = []) {
  const el = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  el.append(...children);
  return el;
}
