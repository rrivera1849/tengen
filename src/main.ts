import { api, onEvents, previewClock, type OgsStatus, type Speed, type View } from "./api";
import { BoardView, type Look, type StoneStyle, type Texture } from "./board";
import { setClock } from "./clock";
import { click } from "./sound";

interface Settings extends Look {
  sound: boolean;
  matchSize: number;
  matchSpeed: Speed;
}

const DEFAULTS: Settings = {
  texture: "kaya", stones: "shell", coords: false, sound: true, matchSize: 19, matchSpeed: "rapid",
};

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
const dialog = (id: string) => $<HTMLDialogElement>(`#${id}`);

let settings = loadSettings();
let view: View | null = null;
let ogs: OgsStatus | null = null;
const board = new BoardView($("#board") as unknown as SVGSVGElement, settings);

// ---------- rendering ----------

type Mode = "review" | "live" | "scoring" | "finished";

function mode(v: View): Mode {
  if (!v.live) return "review";
  if (v.live.phase === "stone removal") return "scoring";
  if (v.live.phase === "finished") return "finished";
  return "live";
}

function show(v: View) {
  const placed = view && v.total > view.total && v.cursor === v.total && !v.lastWasPass;
  view = v;
  const m = mode(v);
  board.interactive = m === "review" || (m === "live" && v.toMove === v.live?.myColor);
  board.update(v);
  if (placed && settings.sound) click();

  $("#black-name").textContent = v.black;
  $("#white-name").textContent = v.white;
  $("#black-meta").textContent = meta(v.blackRank, v.capturesBlack, mine(v, "black"));
  $("#white-meta").textContent = meta(v.whiteRank, v.capturesWhite, mine(v, "white"), `komi ${v.komi}`);
  $("#result").textContent = v.cursor === v.total && v.result ? v.result : "";
  document.querySelectorAll(".player").forEach((el) => {
    el.classList.toggle("to-move", m !== "finished" && el.classList.contains(v.toMove));
  });

  $(".bar").dataset.mode = m;
  if (m === "review" || m === "finished") setClock(null);

  const scrub = $<HTMLInputElement>("#scrub");
  scrub.max = String(v.total);
  scrub.value = String(v.cursor);
  scrub.disabled = v.total === 0;
  $("#move-count").textContent =
    v.cursor === 0 ? "Start" : v.cursor === v.total ? `Move ${v.cursor}` : `Move ${v.cursor} of ${v.total}`;
  if (v.lastWasPass && v.cursor === v.total && m !== "scoring") {
    notify(`${v.toMove === "black" ? "White" : "Black"} passed`);
  }
}

function mine(v: View, color: "black" | "white") {
  return v.live?.myColor === color ? "you" : null;
}

function meta(rank: string | null, captures: number, ...extra: (string | null)[]) {
  return [rank, captures ? `${captures} captured` : null, ...extra].filter(Boolean).join(", ");
}

function showOgs(s: OgsStatus) {
  ogs = s;
  $("#searching").hidden = !s.searching;
}

let noticeTimer = 0;
function notify(text: string) {
  const el = $("#notice");
  el.textContent = text;
  el.classList.add("on");
  clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => el.classList.remove("on"), 3200);
}

async function run<T>(task: () => Promise<T>, then?: (value: T) => void) {
  try {
    const value = await task();
    then?.(value);
  } catch (e) {
    notify(explain(e));
  }
}

const runView = (task: () => Promise<View | null | undefined>) => run(task, (v) => v && show(v));

function explain(e: unknown) {
  const text = String(e instanceof Error ? e.message : e);
  if (text.includes("recapture the ko")) return "Ko: play elsewhere before retaking";
  if (text.includes("suicide")) return "That stone would have no liberties";
  if (text.includes("occupied")) return "There's already a stone there";
  return text;
}

// ---------- actions ----------

board.onPlay = (x, y) => runView(() => api.play(x, y));

const actions: Record<string, () => void> = {
  new: () => dialog("new-dialog").showModal(),
  open: () => runView(api.openSgf),
  save: () => run(api.saveSgf, (path) => path && notify(`Saved ${path.split("/").pop()}`)),
  ogs: () => {
    $<HTMLInputElement>("#ogs-input").value = "";
    dialog("ogs-dialog").showModal();
  },
  pass: () => runView(api.pass),
  online: () => openOnline(),
  resign: () => dialog("resign-dialog").showModal(),
  accept: () => run(api.acceptScore, () => notify("Score accepted. Waiting for your opponent")),
  leave: () => runView(api.leaveGame),
  "cancel-match": () => run(api.cancelMatch, showOgs),
  look: () => {
    syncLookDialog();
    dialog("look-dialog").showModal();
  },
};

document.querySelectorAll<HTMLButtonElement>("[data-action]").forEach((b) => {
  b.addEventListener("click", () => actions[b.dataset.action!]?.());
});

dialog("new-dialog").addEventListener("close", (e) => {
  const size = Number((e.target as HTMLDialogElement).returnValue);
  if (size) runView(() => api.newGame(size, size === 19 ? 6.5 : 5.5));
});

dialog("ogs-dialog").addEventListener("close", async (e) => {
  const choice = (e.target as HTMLDialogElement).returnValue;
  const game = $<HTMLInputElement>("#ogs-input").value.trim();
  if (!game || (choice !== "review" && choice !== "download")) return;
  notify("Fetching the game from OGS");
  let loaded = false;
  await run(() => api.downloadOgs(game), (v) => {
    show(v);
    loaded = true;
  });
  if (loaded && choice === "download") actions.save();
});

dialog("resign-dialog").addEventListener("close", (e) => {
  if ((e.target as HTMLDialogElement).returnValue === "resign") run(api.resign);
});

$<HTMLInputElement>("#scrub").addEventListener("input", (e) => {
  runView(() => api.goTo(Number((e.target as HTMLInputElement).value)));
});

// ---------- playing online ----------

const SPEED_HINTS: Record<Speed, string> = {
  blitz: "The fastest games, a few seconds a move.",
  rapid: "A brisk game with time to think.",
  live: "The most thinking time of the three.",
};

async function openOnline() {
  await run(api.ogsStatus, showOgs);
  const s = ogs;
  const signedIn = !!s?.signedIn;
  document.querySelectorAll<HTMLElement>("#online-dialog [data-when]").forEach((el) => {
    el.hidden = el.dataset.when !== (signedIn ? "signed-in" : "signed-out");
  });
  $("#not-configured").hidden = !!s?.configured;
  $<HTMLButtonElement>("#sign-in").disabled = !s?.configured;
  $("#account-name").textContent = s?.username
    ? `Signed in as ${s.username}${s.rank ? ` (${s.rank})` : ""}${s.connected ? "" : ", reconnecting"}`
    : "Signed in";
  syncPicks();
  dialog("online-dialog").showModal();
}

function syncPicks() {
  document.querySelectorAll<HTMLElement>("#online-dialog [data-pick]").forEach((group) => {
    const current = group.dataset.pick === "size" ? String(settings.matchSize) : settings.matchSpeed;
    group.querySelectorAll<HTMLButtonElement>("button").forEach((b) => {
      b.setAttribute("aria-pressed", String(b.value === current));
    });
  });
  $("#speed-hint").textContent = SPEED_HINTS[settings.matchSpeed];
}

document.querySelectorAll<HTMLElement>("#online-dialog [data-pick]").forEach((group) => {
  group.addEventListener("click", (e) => {
    const b = (e.target as HTMLElement).closest("button");
    if (!b) return;
    e.preventDefault();
    if (group.dataset.pick === "size") settings.matchSize = Number(b.value);
    else settings.matchSpeed = b.value as Speed;
    saveSettings(settings);
    syncPicks();
  });
});

dialog("online-dialog").addEventListener("close", (e) => {
  const choice = (e.target as HTMLDialogElement).returnValue;
  if (choice === "sign-in") {
    notify("Finish signing in in your browser");
    run(api.signIn, (s) => {
      showOgs(s);
      notify(`Signed in as ${s.username ?? "you"}`);
      openOnline();
    });
  }
  if (choice === "sign-out") run(api.signOut, (s) => {
    showOgs(s);
    notify("Signed out of OGS");
  });
  if (choice === "find") run(() => api.findMatch(settings.matchSize, settings.matchSpeed), showOgs);
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
    ArrowLeft: v ? () => runView(() => api.goTo(Math.max(0, v.cursor - 1))) : undefined,
    ArrowRight: v ? () => runView(() => api.goTo(v.cursor + 1)) : undefined,
    Home: () => runView(() => api.goTo(0)),
    End: v ? () => runView(() => api.goTo(v.total)) : undefined,
  };
  const cmdKeys: Record<string, () => void> = {
    z: () => runView(api.undo),
    s: actions.save,
    o: actions.open,
    n: actions.new,
    d: actions.ogs,
    l: actions.online,
  };
  const handler = cmd ? cmdKeys[e.key.toLowerCase()] : keys[e.key];
  if (handler) {
    e.preventDefault();
    handler();
  }
});

// ---------- start ----------

onEvents({
  view: show,
  ogs: showOgs,
  clock: (c) => (view?.live?.gameId === c.gameId ? setClock(c) : undefined),
  notice: notify,
});

runView(api.current).then(() => {
  const sample = previewClock();
  if (sample) setClock(sample);
});
run(api.ogsStatus, showOgs);
