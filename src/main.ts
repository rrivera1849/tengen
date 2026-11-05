import "@fontsource-variable/fraunces/opsz.css";
import "@fontsource-variable/fraunces/opsz-italic.css";
import { api, onEvents, previewClock, type Color, type ServerName, type ServerStatus, type Speed, type View } from "./api";
import { BoardView, type Look, type StoneStyle, type Texture } from "./board";
import { setClock } from "./clock";
import * as panel from "./review";
import { click } from "./sound";

interface Settings extends Look {
  sound: boolean;
  matchSize: number;
  matchSpeed: Speed;
  matchServer: ServerName;
}

const DEFAULTS: Settings = {
  texture: "kaya", stones: "shell", coords: false, sound: true, matchSize: 19, matchSpeed: "rapid", matchServer: "OGS",
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
let servers: ServerStatus[] = [];
const board = new BoardView($("#board") as unknown as SVGSVGElement, settings);

// ---------- rendering ----------

type Mode = "review" | "live" | "scoring" | "finished";

function mode(v: View): Mode {
  if (!v.live) return "review";
  if (v.live.phase === "stone removal") return "scoring";
  if (v.live.phase === "finished") return "finished";
  return "live";
}

/** The score estimate stays on (and follows the game) until E or Esc. */
let estimateOn = false;
/** The turn a lesson's better line belongs to; moving away hides the line. */
let lineTurn: number | null = null;

function show(v: View) {
  const placed = view && v.total > view.total && v.cursor === v.total && !v.lastWasPass;
  const moved = !view || view.cursor !== v.cursor || view.total !== v.total;
  view = v;
  if (moved && lineTurn !== null && v.cursor !== lineTurn) {
    lineTurn = null;
    board.setLine(null);
  }
  panel.setCursor(v.cursor);
  const m = mode(v);
  board.interactive = m === "review" || (m === "live" && v.toMove === v.live?.myColor);
  board.marking = m === "scoring" && !!v.live?.markDead;
  board.update(v);
  if (placed && settings.sound) click();

  $("#black-name").textContent = v.black;
  $("#white-name").textContent = v.white;
  $("#mu-black").textContent = [v.black, v.blackRank].filter(Boolean).join(" ");
  $("#mu-white").textContent = [v.white, v.whiteRank].filter(Boolean).join(" ");
  $("#game-info").textContent = `${v.server ? `${v.server} game` : "Local game"}, ${v.size} × ${v.size}`;
  const mineToMove = !!v.live && v.live.myColor === v.toMove;
  document.querySelectorAll(".player .turn-text").forEach((el) => {
    el.textContent = mineToMove ? "Your move" : "To play";
  });
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
  if (estimateOn && moved) refreshEstimate();
}

// ---------- score estimate ----------

let estimateRun = 0;

async function refreshEstimate() {
  const run = ++estimateRun;
  try {
    const e = await api.estimate();
    if (run !== estimateRun || !estimateOn) return;
    board.setEstimate(e);
    $("#result").textContent = e.headline;
  } catch (err) {
    if (run === estimateRun) {
      setEstimate(false);
      notify(explain(err));
    }
  }
}

async function setEstimate(on: boolean) {
  if (on && !(await ensureKatago())) return;
  estimateOn = on;
  $("#estimate-button").setAttribute("aria-pressed", String(on));
  if (on) {
    $("#result").textContent = "Estimating";
    refreshEstimate();
  } else {
    estimateRun++;
    board.setEstimate(null);
    if (view) $("#result").textContent = view.cursor === view.total && view.result ? view.result : "";
  }
}

// ---------- KataGo, keys and reviews ----------

/** Opens a dialog and resolves with the button the player chose. */
function ask(id: string): Promise<string> {
  const d = dialog(id);
  return new Promise((resolve) => {
    d.addEventListener("close", () => resolve(d.returnValue), { once: true });
    d.returnValue = "";
    d.showModal();
  });
}

async function ensureKatago(): Promise<boolean> {
  const status = await api.aiStatus().catch(() => null);
  if (status?.katago) return true;
  $("#katago-progress").textContent = "";
  if ((await ask("katago-dialog")) !== "download") return false;
  // Reopen the dialog as a progress view until the download finishes.
  const d = dialog("katago-dialog");
  const row = d.querySelector<HTMLElement>(".row")!;
  row.hidden = true;
  $("#katago-progress").textContent = "Starting the download";
  d.showModal();
  try {
    await api.installKatago();
    notify("KataGo is ready");
    return true;
  } catch (e) {
    notify(explain(e));
    return false;
  } finally {
    d.close();
    row.hidden = false;
  }
}

async function startReview() {
  if (!view) return;
  if (view.live && view.live.phase !== "finished") return notify("Reviews open once the game is over");
  if (view.total < 10) return notify("This game is too short to review");
  if (!(await ensureKatago())) return;
  let you: Color | null = await api.reviewSide().catch(() => null);
  if (!you) {
    $("#side-black").textContent = `Black, ${view.black}`;
    $("#side-white").textContent = `White, ${view.white}`;
    const choice = await ask("side-dialog");
    if (choice !== "black" && choice !== "white") return;
    you = choice;
  }
  setEstimate(false);
  panel.showWorking();
  try {
    const r = await api.startReview(you);
    panel.showReview(r, view?.cursor ?? 0);
  } catch (e) {
    panel.close();
    notify(explain(e));
  }
}

async function editKey() {
  const status = await api.aiStatus().catch(() => null);
  $<HTMLInputElement>("#key-input").value = "";
  $("#key-remove").hidden = !status?.apiKey;
  const choice = await ask("key-dialog");
  if (choice === "save") {
    await run(() => api.setClaudeKey($<HTMLInputElement>("#key-input").value), () => {
      notify("API key saved. Run the review again for written lessons");
    });
  }
  if (choice === "remove") await run(api.clearClaudeKey, () => notify("API key removed"));
}

async function showMaxims() {
  const groups = await api.maxims().catch((e) => {
    notify(explain(e));
    return [] as [string, string[]][];
  });
  const list = $("#maxims-list");
  list.replaceChildren();
  if (!groups.length) {
    const p = document.createElement("p");
    p.className = "lede";
    p.textContent = "Maxims from your reviews will collect here.";
    list.append(p);
  }
  for (const [kind, maxims] of groups) {
    const h = document.createElement("p");
    h.className = "group-label";
    h.textContent = kind;
    const ul = document.createElement("ul");
    ul.className = "maxim-list";
    for (const m of maxims) {
      const li = document.createElement("li");
      li.textContent = m;
      ul.append(li);
    }
    list.append(h, ul);
  }
  await ask("maxims-dialog");
}

panel.initPanel({
  open: (turn, line) => {
    runView(() => api.goTo(turn)).then(() => {
      lineTurn = turn;
      board.setLine(line);
    });
  },
  jump: (turn) => runView(() => api.goTo(turn)),
  addKey: () => editKey(),
  showMaxims: () => showMaxims(),
});

function mine(v: View, color: "black" | "white") {
  return v.live?.myColor === color ? "you" : null;
}

function meta(rank: string | null, captures: number, ...extra: (string | null)[]) {
  return [rank, captures ? `${captures} captured` : null, ...extra].filter(Boolean).join(", ");
}

/** What each server is called in the interface. */
const SERVER_LABEL: Record<ServerName, string> = { OGS: "OGS", KGS: "KGS", IGS: "Pandanet", Fox: "Fox" };

function showServers(list: ServerStatus[]) {
  servers = list;
  const looking = list.find((s) => s.searching);
  $("#searching").hidden = !looking;
  if (looking) $("#searching-text").textContent = `Looking for an opponent on ${SERVER_LABEL[looking.server]}`;
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
board.onMark = (x, y) => runView(() => api.play(x, y));

const actions: Record<string, () => void> = {
  new: () => dialog("new-dialog").showModal(),
  open: () => runView(api.openSgf),
  save: () => run(api.saveSgf, (path) => path && notify(`Saved ${path.split("/").pop()}`)),
  ogs: () => {
    $<HTMLInputElement>("#ogs-input").value = "";
    dialog("ogs-dialog").showModal();
  },
  pass: () => runView(api.pass),
  prev: () => view && runView(() => api.goTo(Math.max(0, view!.cursor - 1))),
  next: () => view && runView(() => api.goTo(view!.cursor + 1)),
  estimate: () => setEstimate(!estimateOn),
  review: () => startReview(),
  online: () => openOnline(),
  resign: () => dialog("resign-dialog").showModal(),
  accept: () => run(api.acceptScore, () => notify("Score accepted. Waiting for your opponent")),
  leave: () => runView(api.leaveGame),
  "cancel-match": () => run(api.cancelMatch, showServers),
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

/** Board sizes each server's automatch offers. */
const AUTOMATCH_SIZES: Record<ServerName, number[]> = { OGS: [9, 13, 19], KGS: [19], IGS: [19], Fox: [9, 13, 19] };

const SIGN_UP: Record<ServerName, string> = {
  OGS: "",
  KGS: "Kept in your Mac's Keychain. No KGS account yet? At gokgs.com, enter as a guest, then choose Register.",
  IGS: "Kept in your Mac's Keychain. No Pandanet account yet? Register at pandanet-igs.com.",
  Fox: "Your Fox account name and password, kept in your Mac's Keychain. No password? Register at account.foxwq.com/reg/foreign.html. Fox doesn't allow unofficial apps, so it could restrict accounts that use Tengen.",
};

async function openOnline() {
  await run(api.serverStatus, showServers);
  syncOnline();
  dialog("online-dialog").showModal();
}

function syncOnline() {
  const server = settings.matchServer;
  const s = servers.find((x) => x.server === server);
  const signedIn = !!s?.signedIn;
  const browser = server === "OGS";
  document.querySelectorAll<HTMLElement>("#online-dialog [data-when]").forEach((el) => {
    el.hidden = el.dataset.when !== (signedIn ? "signed-in" : "signed-out");
  });
  document.querySelectorAll<HTMLElement>("#online-dialog [data-auth]").forEach((el) => {
    el.hidden = el.dataset.auth !== (browser ? "browser" : "password");
  });
  $("#not-configured").hidden = !browser || !!s?.configured;
  const signIn = $<HTMLButtonElement>("#sign-in");
  signIn.disabled = browser && !s?.configured;
  signIn.textContent = browser ? "Sign in with OGS" : `Sign in to ${SERVER_LABEL[server]}`;
  $("#login-hint").textContent = SIGN_UP[server];
  $("#account-name").textContent = s?.username
    ? `Signed in as ${s.username}${s.rank ? ` (${s.rank})` : ""}${s.connected ? "" : ", reconnecting"}`
    : "Signed in";
  syncPicks();
}

function syncPicks() {
  const sizes = AUTOMATCH_SIZES[settings.matchServer];
  if (!sizes.includes(settings.matchSize)) settings.matchSize = 19;
  document.querySelectorAll<HTMLElement>("#online-dialog [data-pick]").forEach((group) => {
    const pick = group.dataset.pick;
    const current = pick === "size" ? String(settings.matchSize) : pick === "speed" ? settings.matchSpeed : settings.matchServer;
    group.querySelectorAll<HTMLButtonElement>("button").forEach((b) => {
      b.setAttribute("aria-pressed", String(b.value === current));
      if (pick === "size") b.disabled = !sizes.includes(Number(b.value));
    });
  });
  const only19 = sizes.length === 1 ? ` ${SERVER_LABEL[settings.matchServer]} automatch plays 19\u00a0×\u00a019 only.` : "";
  $("#speed-hint").textContent = SPEED_HINTS[settings.matchSpeed] + only19;
}

document.querySelectorAll<HTMLElement>("#online-dialog [data-pick]").forEach((group) => {
  group.addEventListener("click", (e) => {
    const b = (e.target as HTMLElement).closest("button");
    if (!b || b.disabled) return;
    e.preventDefault();
    if (group.dataset.pick === "size") settings.matchSize = Number(b.value);
    else if (group.dataset.pick === "speed") settings.matchSpeed = b.value as Speed;
    else settings.matchServer = b.value as ServerName;
    saveSettings(settings);
    syncOnline();
  });
});

// Return in the password field signs in (the form's first button is Cancel).
$("#login-password").addEventListener("keydown", (e) => {
  if (e.key !== "Enter") return;
  e.preventDefault();
  dialog("online-dialog").close("sign-in");
});

dialog("online-dialog").addEventListener("close", (e) => {
  const choice = (e.target as HTMLDialogElement).returnValue;
  const server = settings.matchServer;
  const name = SERVER_LABEL[server];
  const password = $<HTMLInputElement>("#login-password");
  if (choice === "sign-in" && server === "OGS") {
    notify("Finish signing in in your browser");
    run(api.ogsSignIn, (list) => {
      showServers(list);
      notify(`Signed in to OGS`);
      openOnline();
    });
  } else if (choice === "sign-in") {
    const username = $<HTMLInputElement>("#login-name").value.trim();
    const secret = password.value;
    notify(`Signing in to ${name}`);
    run(() => api.signIn(server, username, secret), (list) => {
      showServers(list);
      notify(`Signed in to ${name}`);
      openOnline();
    });
  }
  password.value = "";
  if (choice === "sign-out") run(() => api.signOut(server), (list) => {
    showServers(list);
    notify(`Signed out of ${name}`);
  });
  if (choice === "find") run(() => api.findMatch(server, settings.matchSize, settings.matchSpeed), showServers);
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
  if (!cmd && (e.key === "e" || e.key === "E")) return void actions.estimate();
  if (!cmd && (e.key === "r" || e.key === "R")) return void actions.review();
  if (!cmd && (e.key === "p" || e.key === "P")) return void actions.pass();
  if (e.key === "Escape") {
    if (lineTurn !== null) {
      lineTurn = null;
      board.setLine(null);
    } else if (estimateOn) setEstimate(false);
    else if (panel.isOpen()) panel.close();
    return;
  }
  const handler = cmd ? cmdKeys[e.key.toLowerCase()] : keys[e.key];
  if (handler) {
    e.preventDefault();
    handler();
  }
});

// ---------- start ----------

onEvents({
  view: show,
  servers: showServers,
  clock: (c) => (view?.live?.gameId === c.gameId ? setClock(c) : undefined),
  notice: notify,
  progress: (p) => {
    if (p.stage === "download") {
      const mb = (n: number) => Math.round(n / 1e6);
      $("#katago-progress").textContent = p.total ? `${mb(p.done)} of ${mb(p.total)} MB` : `${mb(p.done)} MB`;
    }
    panel.showProgress(p);
  },
});

runView(api.current).then(() => {
  const sample = previewClock();
  if (sample) setClock(sample);
});
run(api.serverStatus, showServers);
