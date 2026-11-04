// Draws the board as SVG. Wood grain is procedural (feTurbulence), so every
// texture stays sharp at any window size and weighs nothing.

import type { Color, View } from "./api";

export type Texture = "kaya" | "shinkaya" | "bamboo" | "ink";
export type StoneStyle = "shell" | "flat";

export interface Look {
  texture: Texture;
  stones: StoneStyle;
  coords: boolean;
}

const NS = "http://www.w3.org/2000/svg";
const LETTERS = "ABCDEFGHJKLMNOPQRSTUVWXYZ"; // Go coordinates skip I

/**
 * Wood per texture. Frequencies are per board unit (one grid step), so a high
 * y-frequency with a low x-frequency draws long horizontal grain lines.
 * `slope`/`cut` sharpen the noise into distinct lines rather than a haze.
 */
const WOODS: Record<Texture, { base: string; dark: string; freq: string; octaves: number; slope: number; cut: number; amount: number }> = {
  kaya: { base: "#E2B46A", dark: "#A8742F", freq: "0.03 2.8", octaves: 4, slope: 2.4, cut: -0.75, amount: 0.24 },
  shinkaya: { base: "#ECCD90", dark: "#B98F49", freq: "0.015 3.6", octaves: 3, slope: 2.6, cut: -0.9, amount: 0.2 },
  bamboo: { base: "#D7B877", dark: "#8F6E33", freq: "1.6 0.018", octaves: 3, slope: 2.2, cut: -0.6, amount: 0.2 },
  ink: { base: "#F2F1EC", dark: "#F2F1EC", freq: "0 0", octaves: 1, slope: 0, cut: 0, amount: 0 },
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

  constructor(private svg: SVGSVGElement, private look: Look) {
    svg.addEventListener("pointermove", (e) => this.track(e));
    svg.addEventListener("pointerleave", () => this.setHover(null));
    svg.addEventListener("click", (e) => {
      const p = this.pointAt(e);
      if (p && !this.occupied(p)) this.onPlay(p[0], p[1]);
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

  private margin() {
    return this.look.coords ? 1.3 : 0.85;
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

  private track(e: PointerEvent) {
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
    for (const [x, y, c] of v.stones) this.svg.append(this.stone(x, y, c));
    if (v.last) this.svg.append(this.lastMark(v.last, v.stones));
    this.hover = null;
  }

  private defs() {
    const w = WOODS[this.look.texture];
    return svg("defs", {}, [
      svg("filter", { id: "grain", x: "0", y: "0", width: "100%", height: "100%" }, [
        svg("feTurbulence", { type: "fractalNoise", baseFrequency: w.freq, numOctaves: String(w.octaves), seed: "7" }),
        // Use the noise's red channel as a mask, then sharpen it into lines.
        svg("feColorMatrix", { type: "matrix", values: "0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  1 0 0 0 0" }),
        svg("feComponentTransfer", {}, [
          svg("feFuncA", { type: "linear", slope: String(w.slope), intercept: String(w.cut) }),
        ]),
        svg("feComposite", { in2: "SourceGraphic", operator: "in" }),
      ]),
      svg("radialGradient", { id: "slate", cx: "0.36", cy: "0.32", r: "0.75" }, [
        svg("stop", { offset: "0", "stop-color": "#5B605E" }),
        svg("stop", { offset: "0.45", "stop-color": "#232625" }),
        svg("stop", { offset: "1", "stop-color": "#0E100F" }),
      ]),
      svg("radialGradient", { id: "shell", cx: "0.38", cy: "0.32", r: "0.8" }, [
        svg("stop", { offset: "0", "stop-color": "#FFFFFF" }),
        svg("stop", { offset: "0.6", "stop-color": "#EEEDE8" }),
        svg("stop", { offset: "1", "stop-color": "#C9C8C1" }),
      ]),
      svg("filter", { id: "drop", x: "-30%", y: "-30%", width: "160%", height: "160%" }, [
        svg("feDropShadow", { dx: "0.05", dy: "0.07", stdDeviation: "0.05", "flood-color": "#2B1A05", "flood-opacity": "0.35" }),
      ]),
    ]);
  }

  private wood(m: number, span: number) {
    const w = WOODS[this.look.texture];
    const g = svg("g", { class: "wood" });
    g.append(svg("rect", { x: String(-m), y: String(-m), width: String(span), height: String(span), rx: "0.18", fill: w.base }));
    if (this.look.texture !== "ink") {
      g.append(svg("rect", {
        x: String(-m), y: String(-m), width: String(span), height: String(span), rx: "0.18",
        fill: w.dark, filter: "url(#grain)", opacity: String(w.amount),
      }));
    }
    return g;
  }

  private grid(n: number) {
    const g = svg("g", { class: "grid" });
    const ink = this.look.texture === "ink";
    const line = ink ? "#2A2E2C" : "#3B2A14";
    const width = ink ? 0.028 : 0.034;
    for (let i = 0; i < n; i++) {
      const edge = i === 0 || i === n - 1;
      const sw = String(edge ? width * 1.8 : width);
      g.append(svg("line", { x1: "0", y1: String(i), x2: String(n - 1), y2: String(i), stroke: line, "stroke-width": sw, "stroke-linecap": "square", opacity: "0.82" }));
      g.append(svg("line", { x1: String(i), y1: "0", x2: String(i), y2: String(n - 1), stroke: line, "stroke-width": sw, "stroke-linecap": "square", opacity: "0.82" }));
    }
    for (const [x, y] of starPoints(n)) {
      g.append(svg("circle", { cx: String(x), cy: String(y), r: "0.1", fill: line, opacity: "0.9" }));
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
        ? { fill: "#1C201F" }
        : { fill: "#F7F6F2", stroke: "#2A2E2C", "stroke-width": "0.035" });
    } else {
      Object.assign(attrs, { fill: color === "black" ? "url(#slate)" : "url(#shell)" });
      if (cls === "stone") attrs.filter = "url(#drop)";
      // On pale paper a shell stone needs a hairline edge to stay visible.
      if (color === "white" && this.look.texture === "ink") {
        Object.assign(attrs, { stroke: "#9a9c96", "stroke-width": "0.02" });
      }
    }
    g.append(svg("circle", attrs));
    return g;
  }

  private lastMark([x, y]: [number, number], stones: View["stones"]) {
    const c = stones.find(([a, b]) => a === x && b === y)?.[2];
    return svg("circle", {
      class: `last ${c === "black" ? "on-black" : "on-white"}`,
      cx: String(x), cy: String(y), r: "0.16", fill: "none", "stroke-width": "0.06",
    });
  }
}

function svg(tag: string, attrs: Record<string, string>, children: Element[] = []) {
  const el = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  el.append(...children);
  return el;
}
