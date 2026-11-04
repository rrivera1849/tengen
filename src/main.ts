import { api, type View } from "./api";
import { BoardView, type Look, type StoneStyle, type Texture } from "./board";
import { click } from "./sound";

interface Settings extends Look {
  sound: boolean;
}

const DEFAULTS: Settings = { texture: "kaya", stones: "shell", coords: false, sound: true };

function loadSettings(): Settings {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem("tengen.settings") ?? "{}") };
  } catch {
    return { ...DEFAULTS };
  }
}

function saveSettings(s: Settings) {
  try {
    localStorage.setItem("tengen.settings", JSON.stringify(s));
  } catch {
    // Settings simply won't persist.
  }
}

const $ = <T extends HTMLElement>(sel: string) => document.querySelector(sel) as T;

let settings = loadSettings();
let view: View | null = null;
const board = new BoardView($("#board") as unknown as SVGSVGElement, settings);

// ---------- rendering ----------

function show(v: View) {
  const placed = view && v.total > view.total && v.cursor === v.total && !v.lastWasPass;
  view = v;
  board.update(v);
  if (placed && settings.sound) click();

  $("#black-name").textContent = v.black;
  $("#white-name").textContent = v.white;
  $("#black-meta").textContent = meta(v.blackRank, v.capturesBlack);
  $("#white-meta").textContent = meta(v.whiteRank, v.capturesWhite, `komi ${v.komi}`);
  $("#result").textContent = v.cursor === v.total && v.result ? v.result : "";
  document.querySelectorAll(".player").forEach((el) => {
    el.classList.toggle("to-move", el.classList.contains(v.toMove));
  });

  const scrub = $<HTMLInputElement>("#scrub");
  scrub.max = String(v.total);
  scrub.value = String(v.cursor);
  scrub.disabled = v.total === 0;
  $("#move-count").textContent =
    v.cursor === 0 ? "Start" : v.cursor === v.total ? `Move ${v.cursor}` : `Move ${v.cursor} of ${v.total}`;
  if (v.lastWasPass) notify(`${v.toMove === "black" ? "White" : "Black"} passed`);
}

function meta(rank: string | null, captures: number, extra?: string) {
  const parts = [rank, captures ? `${captures} captured` : null, extra].filter(Boolean);
  return parts.join(", ");
}

let noticeTimer = 0;
function notify(text: string) {
  const el = $("#notice");
  el.textContent = text;
  el.classList.add("on");
  clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => el.classList.remove("on"), 2600);
}

async function run(task: () => Promise<View | null | undefined>) {
  try {
    const v = await task();
    if (v) show(v);
  } catch (e) {
    notify(explain(e));
  }
}

function explain(e: unknown) {
  const text = String(e instanceof Error ? e.message : e);
  if (text.includes("ko")) return "Ko: play elsewhere before retaking";
  if (text.includes("suicide")) return "That stone would have no liberties";
  if (text.includes("occupied")) return "There's already a stone there";
  return text;
}

// ---------- actions ----------

board.onPlay = (x, y) => run(() => api.play(x, y));

const actions: Record<string, () => void> = {
  new: () => $<HTMLDialogElement>("#new-dialog").showModal(),
  open: () => run(api.openSgf),
  save: async () => {
    try {
      const path = await api.saveSgf();
      if (path) notify(`Saved to ${path.split("/").pop()}`);
    } catch (e) {
      notify(explain(e));
    }
  },
  ogs: () => {
    const d = $<HTMLDialogElement>("#ogs-dialog");
    $<HTMLInputElement>("#ogs-input").value = "";
    d.showModal();
  },
  pass: () => run(api.pass),
  look: () => {
    syncLookDialog();
    $<HTMLDialogElement>("#look-dialog").showModal();
  },
};

document.querySelectorAll<HTMLButtonElement>("[data-action]").forEach((b) => {
  b.addEventListener("click", () => actions[b.dataset.action!]?.());
});

$<HTMLDialogElement>("#new-dialog").addEventListener("close", (e) => {
  const size = Number((e.target as HTMLDialogElement).returnValue);
  if (size) run(() => api.newGame(size, size === 19 ? 6.5 : 5.5));
});

$<HTMLDialogElement>("#ogs-dialog").addEventListener("close", async (e) => {
  const choice = (e.target as HTMLDialogElement).returnValue;
  const game = $<HTMLInputElement>("#ogs-input").value.trim();
  if (!game || (choice !== "review" && choice !== "download")) return;
  notify("Fetching the game from OGS");
  await run(() => api.downloadOgs(game));
  if (choice === "download" && view?.server) await actions.save();
});

$<HTMLInputElement>("#scrub").addEventListener("input", (e) => {
  run(() => api.goTo(Number((e.target as HTMLInputElement).value)));
});

// ---------- board settings ----------

function syncLookDialog() {
  const pressed: Record<string, (v: string) => boolean> = {
    texture: (v) => v === settings.texture,
    stones: (v) => v === settings.stones,
    toggles: (v) => (v === "coords" ? settings.coords : settings.sound),
  };
  document.querySelectorAll<HTMLElement>("#look-dialog [data-setting]").forEach((group) => {
    group.querySelectorAll<HTMLButtonElement>("button").forEach((b) => {
      b.setAttribute("aria-pressed", String(pressed[group.dataset.setting!](b.value)));
    });
  });
}

document.querySelectorAll<HTMLElement>("#look-dialog [data-setting]").forEach((group) => {
  group.addEventListener("click", (e) => {
    const b = (e.target as HTMLElement).closest("button");
    if (!b) return;
    e.preventDefault();
    const kind = group.dataset.setting;
    if (kind === "texture") settings.texture = b.value as Texture;
    if (kind === "stones") settings.stones = b.value as StoneStyle;
    if (kind === "toggles" && b.value === "coords") settings.coords = !settings.coords;
    if (kind === "toggles" && b.value === "sound") settings.sound = !settings.sound;
    saveSettings(settings);
    board.setLook(settings);
    syncLookDialog();
  });
});

// ---------- keyboard ----------

window.addEventListener("keydown", (e) => {
  if (document.querySelector("dialog[open]")) return;
  const cmd = e.metaKey || e.ctrlKey;
  const v = view;
  const keys: Record<string, (() => void) | undefined> = {
    ArrowLeft: v ? () => run(() => api.goTo(Math.max(0, v.cursor - 1))) : undefined,
    ArrowRight: v ? () => run(() => api.goTo(v.cursor + 1)) : undefined,
    Home: () => run(() => api.goTo(0)),
    End: v ? () => run(() => api.goTo(v.total)) : undefined,
  };
  const cmdKeys: Record<string, () => void> = {
    z: () => run(api.undo),
    s: actions.save,
    o: actions.open,
    n: actions.new,
    d: actions.ogs,
  };
  const handler = cmd ? cmdKeys[e.key.toLowerCase()] : keys[e.key];
  if (handler) {
    e.preventDefault();
    handler();
  }
});

run(api.current);
