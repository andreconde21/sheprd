// sheprd's sidebar: every agent on every machine in one list, grouped by project
// (the same repo on two machines is one project), with a status filter, an
// active-only switch and marks. The layout lives in the hub's `sheprd-view`
// herdr-core document (see herdr-app/CONTRACT.md in the sheprd repo), read and
// written through the `cards` CLI; cards come from each machine's `sheprd-cards`.
//
// Narrowed to a rail (the sidebar toggle, or dragging it under 160 px) it shows
// one tag per project with its most urgent status, a needs-you count and a dot
// per agent, like sheprd's collapsed sidebar.
//
// Opened as a popup with props { menu } (a row's right-click menu),
// { newProject } (name a new project) or { newIn } (a new tab or space for a
// project, from its header's or an agent's right-click).

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { agents, spaces, machines, panes, run, theme, shared, setShared, openOverlay, closeOverlay, props, exec, layout, window, playSound, knownAgents, notify } from "herdr";

const RANK = { blocked: 4, done: 3, working: 2, idle: 1 };
// Each filter's label, and the shorter one for a narrow sidebar.
const FILTERS = [
  ["needs-you", "Needs you", "Needs"],
  ["working", "Working", "Work"],
  ["marked", "Marked", "★"],
  ["all", "All", "All"],
];
const CARDS_EVERY_MS = 5000;

const EMPTY_VIEW = { projects: [], hidden: [], filter: "all", active_only: false, other_collapsed: false, density: "detailed", machine_tags: {}, marks: { unread: [], kept: [], dismissed: [], marked: [] } };
const view = () => ({ ...EMPTY_VIEW, .../** @type {object} */ (shared("sheprd.view") ?? {}) });
const marks = () => ({ unread: [], kept: [], dismissed: [], marked: [], ...(view().marks ?? {}) });

// Marked: the user's own flag on an agent, for the Marked filter. Separate
// from `kept`, which only keeps an agent in the active view.
function toggleMarked(address) {
  saveMarks((x) => ({ ...x, marked: x.marked.includes(address) ? x.marked.filter((k) => k !== address) : [...x.marked, address] }));
}

// Changes show at once; the hub's document follows, one write at a time so a
// quick second click can't land before the first. A failed write leaves the
// screen ahead of the document until the next start, which is the lesser harm.
let writes = Promise.resolve();
function saveView(patch) {
  setShared("sheprd.view", { ...view(), ...patch });
  writes = writes.then(() => exec("cards", ["view-set", JSON.stringify(patch)])).then(() => {}, () => {});
}

function saveMarks(change) {
  saveView({ marks: change(marks()) });
}

// Moves project [name] next to project [target] (before it, or after it with
// [after]). The saved list becomes the order on screen and pins go: once you
// order projects yourself, the list is the order (pins came from sheprd).
function moveProject(name, target, after) {
  const all = view().projects;
  const list = [...all.filter((x) => x.pinned), ...all.filter((x) => !x.pinned)].map(({ pinned: _pinned, ...x }) => x);
  const me = list.find((x) => x.name === name);
  const rest = list.filter((x) => x.name !== name);
  const at = rest.findIndex((x) => x.name === target);
  if (!me || at < 0) return;
  rest.splice(after ? at + 1 : at, 0, me);
  saveView({ projects: rest });
}

// A space's name key: its machine (`local` for this one) and its name. Two
// spaces can share it, so the sidebar writes `spaceRef` (name key + `#` + the
// space's id) and still reads plain name keys from older views.
const spaceKey = (s) => `${s?.machine ?? "local"}/${s?.name ?? ""}`;
const spaceRef = (s) => `${spaceKey(s)}#${s?.id ?? ""}`;
const isRef = (key, s) => Boolean(s) && key === spaceRef(s);
const isNameKey = (key, s) => Boolean(s) && key.toLowerCase() === spaceKey(s).toLowerCase();
const names = (key, s) => isRef(key, s) || isNameKey(key, s);
const isHiddenSpace = (s, hidden) => hidden.some((key) => names(key, s));

// [keys] without those naming [space]. A name key it shared with other spaces
// becomes their refs, so they stay where they were.
function untie(keys, space) {
  const out = keys.filter((key) => !names(key, space));
  for (const key of keys) {
    if (!isNameKey(key, space)) continue;
    for (const twin of spaces()) {
      if (twin.id !== space.id && isNameKey(key, twin) && !out.includes(spaceRef(twin))) out.push(spaceRef(twin));
    }
  }
  return out;
}

function projectOf(space, projects) {
  if (!space) return null;
  const byMember = projects.find((p) => (p.members ?? []).some((m) => isRef(m, space)))
    ?? projects.find((p) => (p.members ?? []).some((m) => isNameKey(m, space)));
  if (byMember) return byMember;
  const hay = `${space.name} ${space.folder ?? ""}`.toLowerCase();
  return projects.find((p) => (p.match ?? []).some((m) => m && hay.includes(m.toLowerCase()))) ?? null;
}

// The agent menu's height: room for every project (28 px a line, plus mark,
// read, hide, the divider, the label, Other and New project), capped so a long
// list scrolls instead of leaving the window.
const MENU_FIXED = 5 * 28 + 9 + 22 + 10; // new, copy, mark, read, hide, divider, label, padding
const menuHeight = (projects) => Math.min(MENU_FIXED + 28 * (projects + 3), 560);

// Where a new tab or space can go for a project (null: Other): a tab in each of
// its spaces, then a new space on each machine, starting in the folder the
// project already uses there.
function placesFor(name) {
  const v = view();
  const mine = spaces().filter((s) => !isHiddenSpace(s, v.hidden) && (projectOf(s, v.projects)?.name ?? null) === name);
  const out = mine.map((s) => ({ space: s.id, machine: s.machine ?? null, label: s.name, folder: s.folder }));
  // Every machine the app reaches, and any a space already lives on.
  const names = new Set(machines().filter((x) => x.layout !== "unavailable").map((x) => x.name ?? null));
  for (const s of spaces()) names.add(s.machine ?? null);
  for (const m of [null, ...[...names].filter(Boolean).sort()]) {
    const there = mine.find((s) => (s.machine ?? null) === m);
    out.push({ space: null, machine: m, label: `New space on ${m ?? shared("sheprd.hostname") ?? "this machine"}`, folder: there?.folder ?? "" });
  }
  return out;
}

// The agents a machine can start (installed ones).
async function startable(machine) {
  try {
    return (await knownAgents({ on: machine })).filter((a) => a.agent && a.installed);
  } catch {
    return [];
  }
}

// How many lines the What chips take in the dialog (Terminal first; about
// 7 px a character plus 20 px padding, 6 px apart, 352 px a line).
function chipLines(list) {
  let lines = 1;
  let x = 0;
  for (const label of ["Terminal", ...list.map((a) => a.name)]) {
    const w = 20 + 7 * label.length;
    if (x && x + 6 + w > 352) {
      lines++;
      x = w;
    } else x += (x ? 6 : 0) + w;
  }
  return lines;
}

// A dialog can't fit its content, so it opens frameless at the most it can
// need (places, 8 before they scroll; the widest machine's chip lines, from
// the agents the sidebar keeps in "sheprd.startable"; the fixed rows) and
// draws its own panel, as tall as what it shows, centred in that space.
function openNew(project, space = null) {
  const places = placesFor(project);
  const known = /** @type {Record<string, any[]>} */ (shared("sheprd.startable") ?? {});
  const agentsOn = Object.fromEntries([...new Set(places.map((x) => x.machine ?? ""))].filter((m) => known[m]).map((m) => [m, known[m]]));
  const lines = Math.max(1, ...Object.values(agentsOn).map(chipLines));
  const height = 236 + 28 * Math.min(places.length, 8) + 26 * lines + 6 * (lines - 1);
  openOverlay({ mod: "sheprd-sidebar", kind: "dialog", frame: "none", width: 380, height, props: { newIn: { project, space, agents: agentsOn } } });
}

// Cards of every machine, by agent address (`t7` here, `dev/t7` elsewhere).
function cardsByAddress() {
  const out = new Map();
  for (const [machine, doc] of Object.entries(/** @type {Record<string, any>} */ (shared("sheprd.cards") ?? {}))) {
    for (const card of Object.values(doc?.cards ?? {})) {
      const address = machine === "local" ? card.terminal : `${machine}/${card.terminal}`;
      if (!out.has(address)) out.set(address, []);
      out.get(address).push(card);
    }
  }
  return out;
}

const live = (q) => q.state === "open" || q.state === "staged" || q.state === "sending";
const openQuestions = (cards) => (cards ?? []).filter((c) => !c.dismissed).flatMap((c) => c.questions.filter(live));

// `cards` lives in ~/.local/bin, which the herdr-core of a machine reached over SSH often does not
// have on its PATH. Try the PATH first, then that file. Resolves to the result, or null when it
// could not be run at all.
async function runCards(args, on) {
  const missing = (r) => r.code === 127 || /not found|no such file/i.test(r.stderr ?? "");
  let first = null;
  try {
    first = await exec("cards", args, { on });
    if (first.code === 0 || !missing(first)) return first;
  } catch {
    first = null;
  }
  try {
    return await exec("sh", ["-c", 'exec "$HOME/.local/bin/cards" "$@"', "cards", ...args], { on });
  } catch {
    return first;
  }
}

// Why a machine's cards could not be read: a line to show instead of an empty list.
const cardsFailure = (r) => (r ? (r.stderr || r.stdout || `exit ${r.code}`).trim().split("\n")[0].slice(0, 120) : "could not run it");

// `cards json` from the given machines, merged over the last good result.
async function fetchCards(list) {
  const docs = { .../** @type {object} */ (shared("sheprd.cards") ?? {}) };
  const errs = { .../** @type {object} */ (shared("sheprd.cards.err") ?? {}) };
  const present = new Set(machines().map((m) => m.name ?? "local"));
  for (const key of Object.keys(docs)) if (!present.has(key)) delete docs[key];
  const got = await Promise.all(list.filter((m) => m.layout !== "unavailable").map(async (m) => {
    const name = m.name ?? "local";
    try {
      const r = await runCards(["json"], m.name);
      if (r && r.code === 0) {
        delete errs[name];
        return [name, JSON.parse(r.stdout)];
      }
      errs[name] = cardsFailure(r); // not installed there, or the machine went away
    } catch {
      errs[name] = "unreadable answer";
    }
    return null;
  }));
  for (const entry of got) if (entry) docs[entry[0]] = entry[1];
  setShared("sheprd.cards.err", errs);
  return docs;
}

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The rail goes by the sidebar's width, as in Herdr's own sidebar mods.
const narrow = () => (find(layout(), "sidebar")?.size?.px ?? Infinity) < 160;

// To the rail, or back to the width it had (the same toggle as workspace-chrome's).
function toggleSidebar() {
  const width = find(layout(), "sidebar")?.size?.px;
  if (!narrow()) setShared("sidebar.width", width);
  setShared("sidebar.collapsed", !narrow());
  run("layout.resize", { id: "sidebar", px: narrow() ? (shared("sidebar.width") ?? 300) : window()?.platform === "macos" ? 76 : 44 });
}

// A project's rail tag, as sheprd makes it: `short`, else the first letters of
// the first two words ("Acme ops" -> "AO"), else two capitals ("StoreFront" ->
// "SF"), else the first two letters ("Infrastructure" -> "In").
function projectTag(name, short) {
  if (short?.trim()) return [...short.trim()].slice(0, 2).join("");
  const words = name.split(/\s+/).filter(Boolean);
  if (words.length >= 2) return words.slice(0, 2).map((w) => [...w][0].toUpperCase()).join("");
  const capitals = [...name].filter((c) => c !== c.toLowerCase() && c === c.toUpperCase()).slice(0, 2);
  if (capitals.length === 2) return capitals.join("");
  const [first = "", second = ""] = [...name];
  return first.toUpperCase() + second;
}

// A machine's short tag: its `machine_tags` entry, else the name's first part,
// cut to 3 letters when longer than 5 ("development-central" -> "dev").
function machineTag(name) {
  if (!name) return "";
  const tag = view().machine_tags?.[name];
  if (tag) return tag;
  const first = name.split(/[-_.\s]/)[0] || name;
  return first.length > 5 ? first.slice(0, 3) : first;
}

// An agent's work: its card summary, else the terminal's title, else a named
// tab, else [fallback].
const workOf = (r, fallback) => r.summary || r.a.title || (r.a.tab_name && !/^\d+$/.test(r.a.tab_name) ? r.a.tab_name : "") || fallback;

// An agent's hc address as other machines write it: `dev/t7` stays, a local
// `t7` gets this machine's name (once known).
const pingAddress = (address) => (address.includes("/") || !shared("sheprd.hostname") ? address : `${shared("sheprd.hostname")}/${address}`);

function copyAddress(address) {
  const text = pingAddress(address);
  exec("wl-copy", [text]).catch(() => exec("pbcopy", [])).then(
    () => notify({ title: `Copied ${text}`, level: "success", timeout: 2000 }),
    () => notify({ title: `Address: ${text}`, body: "No clipboard tool here (wl-copy)", level: "info" }));
}

const tokens = (n) => (n >= 1000 ? `${Math.round(n / 1000)}k` : String(n));

function duration(ms) {
  const m = Math.floor(ms / 60000);
  if (m < 1) return "<1m";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h}h` : `${Math.floor(h / 24)}d`;
}

export default class SheprdSidebar extends View {
  init(_props, cx) {
    const p = props() ?? {};
    if (p.newProject) return this.initPrompt(cx);
    if (p.newIn) return this.initNew(cx);
    if (p.menu) return;
    this.folded = new Set();
    this.showHidden = localStorage.getItem("showHidden") === "1";
    // Each agent's last state and when it changed (ms; 0 = unknown), kept
    // across restarts. v2: "changed" stamped every first sighting as now.
    this.changed = JSON.parse(localStorage.getItem("changed.v2") ?? "{}");
    for (const c of Object.values(this.changed)) c.first ??= Date.now();
    localStorage.removeItem("changed");
    cx.spawn(async (cx) => {
      try {
        const r = await exec("cards", ["view"]);
        if (r.code === 0) setShared("sheprd.view", JSON.parse(r.stdout));
        else this.problem = "cards is not installed here (see herdr-app/README.md)";
      } catch {
        this.problem = "cards is not installed here (see herdr-app/README.md)";
      }
      cx.notify();
    });
    this.loadCards(cx);
    // The companion's view of each agent (context, permission mode, last
    // tool), by address: asked of a machine on hover, at most every 10 s.
    this.facts = new Map();
    this.factsAt = new Map();
    if (!shared("sheprd.hostname")) {
      exec("hostname", []).then((r) => r.code === 0 && setShared("sheprd.hostname", r.stdout.trim()), () => {});
    }
    cx.timer.every(CARDS_EVERY_MS, (cx) => this.loadCards(cx));
    // What each machine can start, for the New tab or space dialog.
    const loadStartable = async () => {
      const list = machines().filter((x) => x.layout !== "unavailable");
      setShared("sheprd.startable", Object.fromEntries(await Promise.all(list.map(async (m) => [m.name ?? "", await startable(m.name ?? null)]))));
    };
    loadStartable();
    cx.timer.every(60000, () => { loadStartable(); });
    cx.timer.every(30000, (cx) => cx.notify());
  }

  loadFacts(machine, cx) {
    const key = machine ?? "local";
    if (Date.now() - (this.factsAt.get(key) ?? 0) < 10000) return;
    this.factsAt.set(key, Date.now());
    cx.spawn(async (cx) => {
      try {
        const r = await exec("sh", ["-lc", "conductore-hostd status --json"], { on: machine });
        if (r.code !== 0) return;
        for (const x of JSON.parse(r.stdout).agents ?? []) {
          if (x.hc?.terminal) this.facts.set(machine ? `${machine}/${x.hc.terminal}` : x.hc.terminal, x);
        }
      } catch {
        // no companion there: the details go without it
      }
      cx.notify();
    });
  }

  // A row's hover tooltip: how to ping it, where it runs and what it's doing.
  details(r, now) {
    const a = r.a;
    const machine = a.address.includes("/") ? a.address.split("/")[0] : null;
    const pane = panes().find((x) => x.pane === a.pane);
    const f = this.facts.get(a.address);
    const since = this.changed[a.address]?.at;
    const lines = [workOf(r, a.space_name || a.address)];
    lines.push(`Ping: ${pingAddress(a.address)}${machine ? "" : `  (here: ${a.address})`}`);
    lines.push(`${a.space_name || "?"} · tab ${a.tab_index}${machine ? ` · ${machine}` : ""}`);
    if (pane?.cwd) lines.push(`${pane.cwd}${pane.branch ? ` (${pane.branch})` : ""}`);
    lines.push(`${a.app || a.program?.name || "agent"} · ${a.status}${since ? ` for ${duration(now - since)}` : ""}`);
    if (f?.usage?.contextUsedPct != null) lines.push(`Context ${f.usage.contextUsedPct}% of ${f.usage.windowLabel ?? "?"} (${tokens(f.usage.contextTokens ?? 0)} tokens)`);
    if (f?.permissionMode) lines.push(`Permissions: ${f.permissionMode}`);
    if (f?.lastToolName && a.status === "working") lines.push(`Last tool: ${f.lastToolName}`);
    if (f?.startedAt) lines.push(`Session started ${duration(now - f.startedAt)} ago`);
    if (r.questions) lines.push(`${r.questions} open question${r.questions > 1 ? "s" : ""}`);
    if (!f) lines.push(this.factsAt.has(machine ?? "local") ? "No companion facts for it" : "Asking the companion…");
    lines.push("Shift-click: copy the address");
    return lines.join("\n");
  }

  // Every reachable machine's cards, into shared state for the panel too. One
  // load at a time, machines asked in parallel; a machine that fails this time
  // keeps its last cards.
  loadCards(cx) {
    if (this.loading) return;
    this.loading = true;
    cx.spawn(async (cx) => {
      try {
        setShared("sheprd.cards", await fetchCards(machines()));
        setShared("sheprd.cards.at", Date.now());
      } finally {
        this.loading = false;
      }
      cx.notify();
    });
  }

  // Stamps agents whose state changed (for the age column and the sort); done
  // and idle count as one. A first sighting isn't a change: a working agent
  // counts from now, anything else from its tab's last change if the app saw
  // one, else unknown (0). Returns the agents never seen before, once the
  // sidebar has seen any (so a fresh install doesn't count everyone as new).
  track(list, now) {
    let dirty = false;
    const fresh = [];
    const known = Object.keys(this.changed).length > 0;
    let tabById = null;
    for (const a of list) {
      const s = a.status === "done" ? "idle" : a.status;
      const prev = this.changed[a.address];
      if (prev?.s === s) continue;
      let at = now;
      if (!prev && s !== "working") {
        tabById ??= new Map(spaces().flatMap((sp) => sp.tabs ?? []).map((x) => [x.id, x]));
        at = tabById.get(a.tab)?.changed_at ?? 0;
      }
      if (!prev && known) fresh.push(a.address);
      this.changed[a.address] = { s, at, first: prev?.first ?? now };
      dirty = true;
    }
    if (!dirty) return fresh;
    // Forgotten 30 days after first seen once gone, so an agent on a machine
    // that drops off for a while isn't "new" when it comes back.
    const listed = new Set(list.map((a) => a.address));
    for (const [k, c] of Object.entries(this.changed)) if (!listed.has(k) && now - (c.first ?? c.at) > 30 * 86400000) delete this.changed[k];
    localStorage.setItem("changed.v2", JSON.stringify(this.changed));
    return fresh;
  }

  // An agent that turns blocked or done: the attention sound and, when the app
  // isn't in front, a desktop notification. Per project: a muted project
  // (right-click its header) stays quiet. The first look at each agent only
  // records its state.
  attend(list, spaceById) {
    this.seen ??= new Map();
    const v = view();
    for (const a of list) {
      const before = this.seen.get(a.address);
      this.seen.set(a.address, a.status);
      if (before === undefined || before === a.status) continue;
      if (a.status !== "blocked" && a.status !== "done") continue;
      const active = window()?.active;
      if (active && a.focused) continue;
      if (isHiddenSpace(spaceById.get(a.space), v.hidden)) continue; // hidden: quiet too
      const project = projectOf(spaceById.get(a.space), v.projects);
      if (project ? project.muted : v.other_muted) continue;
      playSound(a.status === "done" ? "done" : "blocked");
      if (!active) {
        const machine = a.address.includes("/") ? ` · ${machineTag(a.address.split("/")[0])}` : "";
        exec("notify-send", ["-a", "sheprd", `${a.space_name || a.address}${machine}: ${a.status === "done" ? "finished" : "needs you"}`, a.title || ""]).catch(() => {});
      }
    }
  }

  // ---- popups -------------------------------------------------------------

  initPrompt(_cx) {
    this.text = "";
    this.input = InputState.new({ value: "" });
    this.input.on("change", (_e, cx) => { this.text = this.input.value(); cx.notify(); });
    this.input.on("submit", () => this.createProject());
    this.input.focus();
  }

  createProject() {
    const p = props();
    const name = this.text.trim();
    closeOverlay(p.overlay);
    if (!name) return;
    const space = spaces().find((s) => s.id === p.newProject.space);
    if (!space) return;
    const projects = view().projects.map((x) => ({ ...x, members: untie(x.members ?? [], space) }));
    projects.push({ name, members: [spaceRef(space)], match: [] });
    saveView({ projects });
  }

  // ---- new tab or space ---------------------------------------------------

  initNew(cx) {
    const p = props().newIn;
    this.places = placesFor(p.project);
    this.pick = p.space ? Math.max(0, this.places.findIndex((x) => x.space === p.space)) : 0;
    this.what = localStorage.getItem("new.what") ?? "shell";
    // machine (null: this one) -> agents installed there, as asked before opening
    this.agentsOn = new Map(Object.entries(p.agents ?? {}).map(([m, list]) => [m || null, list]));
    this.folder = InputState.new({ value: this.places[this.pick]?.folder ?? "", placeholder: "Folder (empty: home)" });
    this.task = InputState.new({ value: "", placeholder: "First message (agents only, optional)" });
    this.task.on("submit", () => this.createNew());
    this.loadAgents(cx);
  }

  loadAgents(cx) {
    const machine = this.places[this.pick]?.machine ?? null;
    if (this.agentsOn.has(machine)) return;
    this.agentsOn.set(machine, null);
    cx.spawn(async (cx) => {
      try {
        this.agentsOn.set(machine, await startable(machine));
      } finally {
        cx.notify();
      }
    });
  }

  createNew() {
    const p = props();
    const place = this.places[this.pick];
    closeOverlay(p.overlay);
    if (!place) return;
    const agent = (this.agentsOn.get(place.machine) ?? []).find((a) => a.key === this.what);
    const task = agent ? this.task.value().trim() : "";
    const start = { ...(agent ? { argv: agent.run } : {}), ...(task ? { task } : {}) };
    localStorage.setItem("new.what", agent ? agent.key : "shell");
    const folder = this.folder.value().trim();
    if (place.space) return run("tab.new", { space: place.space, ...(folder ? { folder } : {}), ...start });
    // The new space joins the project once it shows up (render picks it up).
    if (p.newIn.project !== null) setShared("sheprd.joining", { project: p.newIn.project, machine: place.machine, before: spaces().map((s) => s.id), at: Date.now() });
    // Always name the machine: without one, space.new from a click goes to the
    // focused space's machine, not the one picked ("": this computer).
    run("space.new", { ...(folder ? { folder } : {}), machine: place.machine ?? "", ...start });
  }

  newView(t) {
    const p = props();
    const place = this.places[this.pick];
    const agents = place ? this.agentsOn.get(place.machine) : [];
    const pill = (id, label, on, action) => h_flex().id(id).h(26).px(10).rounded(6).items_center().text_size(12)
      .bg(on ? t.selected : t.raised).text_color(on ? t.foreground : t.muted)
      .cursor_pointer().hover((el) => el.text_color(t.foreground))
      .on_click((_e, cx) => { action(); cx.notify(); }).child(label);
    const line = (x, i) => h_flex().id(`ni-${i}`).h(28).px(8).gap(8).flex_none().rounded(6).items_center()
      .when(i === this.pick, (el) => el.bg(t.selected))
      .cursor_pointer().hover((el) => el.bg(t.hover))
      .on_click((_e, cx) => {
        this.pick = i;
        this.folder.set_value(x.folder);
        this.loadAgents(cx);
        cx.notify();
      })
      .child(div().flex_none().text_color(t.dim).child(x.space ? "Tab in" : "+"))
      .child(div().flex_1().min_w(0).truncate().text_color(x.space ? t.foreground : t.muted).child(x.label))
      .when(Boolean(x.machine && x.space), (el) => el.child(div().flex_none().text_size(10.5).text_color(t.dim).child(machineTag(x.machine))));
    const name = p.newIn.project ?? "Other";
    const picked = (agents ?? []).some((a) => a.key === this.what);
    const panel = v_flex().w_full().p(14).gap(8).rounded(10).border_1().border_color(t.border).shadow_lg()
      .bg(t.popover).text_color(t.foreground).text_size(12.8)
      .child(div().text_size(14).font_semibold().child(`New in ${name}`))
      .child(div().text_size(11).text_color(t.dim).child("Where"))
      .child(v_flex().id("ni-places").h(28 * Math.min(this.places.length, 8)).flex_none().overflow_y_scrollbar().children(this.places.map(line)))
      .child(Input.new(this.folder).h(30).px(8).rounded(7).border_1().border_color(t.border).bg(t.background))
      .child(div().text_size(11).text_color(t.dim).child("What"))
      .child(h_flex().gap(6).flex_wrap()
        .child(pill("nw-shell", "Terminal", !picked, () => { this.what = "shell"; }))
        .children((agents ?? []).map((a) => pill(`nw-${a.key}`, a.name, a.key === this.what, () => { this.what = a.key; })))
        .when(agents === null, (el) => el.child(div().text_size(11).text_color(t.dim).child("finding agents…"))))
      .when(picked, (el) => el.child(Input.new(this.task).h(30).px(8).rounded(7).border_1().border_color(t.border).bg(t.background)))
      .child(h_flex().gap(8).justify_end()
        .child(div().id("ni-cancel").px(12).h(30).flex().items_center().rounded(7).bg(t.raised).cursor_pointer().on_click(() => closeOverlay(p.overlay)).child("Cancel"))
        .child(div().id("ni-go").px(12).h(30).flex().items_center().rounded(7).bg(t.solid).text_color(t.solid_foreground).cursor_pointer().on_click(() => this.createNew()).child("Create")));
    return v_flex().size_full().justify_center().child(panel);
  }

  promptView(t) {
    const p = props();
    return v_flex().w_full().p(16).gap(10).bg(t.popover).text_color(t.foreground)
      .child(div().text_size(14).font_semibold().child("New project"))
      .child(div().text_size(11.5).text_color(t.dim).child(`Starts with ${p.newProject.key}. Add more spaces from their right-click menu.`))
      .child(Input.new(this.input).h(34).px(10).rounded(8).border_1().border_color(t.accent).bg(t.background).text_size(12.8))
      .child(h_flex().gap(8).justify_end()
        .child(div().id("np-cancel").px(12).h(30).flex().items_center().rounded(7).bg(t.raised).cursor_pointer().on_click(() => closeOverlay(p.overlay)).child("Cancel"))
        .child(div().id("np-save").px(12).h(30).flex().items_center().rounded(7).bg(t.solid).text_color(t.solid_foreground).cursor_pointer().on_click(() => this.createProject()).child("Create")));
  }

  menuView(t) {
    const p = props();
    const { address, seq, key, unhideOnly } = p.menu;
    const space = spaces().find((s) => s.id === p.menu.space);
    const isHidden = space ? isHiddenSpace(space, view().hidden) : view().hidden.includes(key);
    // Unhide: this space only (its twins stay hidden); a key with no live space goes.
    const unhide = () => saveView({ hidden: space ? untie(view().hidden, space) : view().hidden.filter((h) => h !== key) });
    const m = marks();
    const marked = m.marked.includes(address);
    const projects = view().projects;
    const item = (id, label, action, dim = false) => h_flex().id(id).h(28).px(8).rounded(6).items_center()
      .text_color(dim ? t.muted : t.foreground).hover((el) => el.bg(t.hover)).cursor_pointer()
      .on_click(() => { closeOverlay(p.overlay); action(); }).child(label);
    const moveTo = (name) => {
      if (!space) return;
      const list = projects.map((x) => {
        const members = untie(x.members ?? [], space);
        return x.name === name ? { ...x, members: [...members, spaceRef(space)] } : { ...x, members };
      });
      saveView({ projects: list });
    };
    if (p.menu.project !== undefined) {
      const name = p.menu.project;
      const other = name === null;
      const muted = other ? Boolean(view().other_muted) : Boolean(projects.find((x) => x.name === name)?.muted);
      const setMuted = (on) => other
        ? saveView({ other_muted: on })
        : saveView({ projects: view().projects.map((x) => (x.name === name ? { ...x, muted: on } : x)) });
      // Up and down go past the next project on screen (empty ones are not
      // shown); Other stays last.
      const peers = other ? [] : (p.menu.shown ?? []);
      const at = peers.indexOf(name);
      return v_flex().w_full().p(5).bg(t.popover).text_size(12.8)
        .child(item("m-new", "New tab or space…", () => openNew(name)))
        .child(item("m-mute", muted ? "Unmute: sound and notify again" : "Mute: no sound or notification", () => setMuted(!muted)))
        .when(at > 1, (el) => el.child(item("m-top", "Move to top", () => moveProject(name, peers[0], false))))
        .when(at > 0, (el) => el.child(item("m-up", "Move up", () => moveProject(name, peers[at - 1], false))))
        .when(at >= 0 && at < peers.length - 1, (el) => el.child(item("m-down", "Move down", () => moveProject(name, peers[at + 1], true))))
        .when(at >= 0 && at < peers.length - 2, (el) => el.child(item("m-bottom", "Move to bottom", () => moveProject(name, peers[peers.length - 1], true))));
    }
    if (unhideOnly) {
      return v_flex().w_full().p(5).bg(t.popover).text_size(12.8)
        .child(item("m-unhide", "Unhide this space", unhide));
    }
    // Mark, read and hide first, so they never scroll away; the projects
    // scroll when the window is too short for all of them.
    return v_flex().size_full().p(5).bg(t.popover).text_size(12.8)
      .child(item("m-new", "New tab or space here…", () => openNew(space ? projectOf(space, projects)?.name ?? null : null, space?.id ?? null)))
      .child(item("m-copy", `Copy address ${pingAddress(address)}`, () => copyAddress(address)))
      .child(item("m-mark", marked ? "Unmark" : "Mark", () => toggleMarked(address)))
      // Read: out of Needs you until its next change. Unread: in it until opened.
      .child(p.menu.needs
        ? item("m-read", "Mark read", () => saveMarks((x) => ({ ...x, unread: x.unread.filter((u) => u !== address), dismissed: [...x.dismissed.filter((d) => !d.startsWith(`${address}@`)), `${address}@${seq}`].slice(-200) })))
        : item("m-read", "Mark unread", () => saveMarks((x) => ({ ...x, unread: [...x.unread.filter((u) => u !== address), address], dismissed: x.dismissed.filter((d) => !d.startsWith(`${address}@`)) }))))
      .child(isHidden
        ? item("m-unhide", "Unhide this space", unhide)
        : item("m-hide", "Hide this space", () => space && saveView({ hidden: [...view().hidden, spaceRef(space)] }), true))
      .child(div().pt(4).pb(4).child(div().h(1).bg(t.border)))
      .child(div().px(8).pt(2).pb(4).text_size(11).text_color(t.dim).truncate().child(`Move ${key} to`))
      // An explicit height: a flex_1 list inside the popup collapsed to nothing.
      .child(v_flex().id("m-projects").h(menuHeight(projects.length) - MENU_FIXED).flex_none().overflow_y_scrollbar()
        .children(projects.map((x, i) => item(`m-p${i}`, x.name, () => moveTo(x.name))))
        .child(item("m-other", "Other", () => moveTo(null), true))
        .child(item("m-new", "New project…", () => openOverlay({ mod: "sheprd-sidebar", kind: "dialog", width: 360, height: 190, props: { newProject: { key, space: p.menu.space } } }))));
  }

  // ---- dragging a project ---------------------------------------------------

  // A project's header and rows: where a dragged header lands, with a line
  // where it will go (above when dragged up, below when dragged down).
  dropTarget(t, g, children) {
    const d = this.drag?.moved ? this.drag : null;
    const name = g.other ? null : g.project.name;
    const over = Boolean(d && name && d.over === name && name !== d.name);
    const down = over && this.shownOrder.indexOf(name) > this.shownOrder.indexOf(d.name);
    return v_flex().id(`dt-${g.project.name}`).w_full().flex_none()
      .when(over && !down, (el) => el.border_t_2().border_color(t.solid))
      .when(over && down, (el) => el.border_b_2().border_color(t.solid))
      .on_mouse_move((e, cx) => {
        const x = this.drag;
        if (!x) return;
        if (!x.moved && Math.abs(e.position.y - x.y) < 5) return;
        const was = x.moved && x.over;
        x.moved = true;
        x.over = name;
        if (was !== name) cx.notify();
      })
      .children(children);
  }

  dropProject(cx) {
    const d = this.drag;
    this.drag = null;
    if (!d?.moved) return;
    this.droppedAt = Date.now();
    const order = this.shownOrder ?? [];
    if (d.over && d.over !== d.name) moveProject(d.name, d.over, order.indexOf(d.over) > order.indexOf(d.name));
    cx.notify();
  }

  // ---- the list -----------------------------------------------------------

  status(t, s, questions = 0) {
    if (questions > 0) return div().flex_none().px(5).h(17).rounded(5).bg(t.blocked + "26").text_size(10.5).font_semibold().text_color(t.blocked).flex().items_center().child(`?${questions}`);
    if (s === "working") return svg("mods/workspace-chrome/spinner.svg").size(12).mx(1).flex_none().text_color(t.working).spin(900);
    if (s === "blocked") return div().size(14).flex_none().rounded(4).bg(t.blocked + "26").text_size(10.5).font_bold().text_color(t.blocked).flex().items_center().justify_center().child("!");
    if (s === "idle") return div().size(7).mx(3).flex_none().rounded(4).border(1.5).border_color(t.idle ?? t.dim);
    return div().size(7).mx(3).flex_none().rounded(4).bg(s === "done" ? t.done : t.dim + "00");
  }

  // ---- the rail ----------------------------------------------------------

  // One column: each project's tag in the colour of its most urgent agent
  // (blocked > done > working > idle) with a needs-you count, then a dot per
  // agent (tooltip: space and summary, click: focus). Other comes last.
  rail(t, list, m) {
    const colour = (s) => ({ blocked: t.blocked, done: t.done, working: t.working })[s] ?? t.idle ?? t.dim;
    const dot = (r) => {
      const a = r.a;
      const machine = a.address.includes("/") ? `${machineTag(a.address.split("/")[0])} · ` : "";
      const name = a.space_name || a.title || a.program?.name || a.address;
      return div().id(`rd-${a.address}`).size(14).flex_none().flex().items_center().justify_center().rounded(4)
        .when(a.focused, (el) => el.bg(t.selected))
        .cursor_pointer().hover((el) => el.bg(t.hover))
        .tooltip(`${machine}${name}${r.summary ? ` — ${r.summary}` : ""}`)
        .on_click(() => {
          run("pane.focus", { pane: a.pane });
          if (m.unread.includes(a.address)) saveMarks((x) => ({ ...x, unread: x.unread.filter((u) => u !== a.address) }));
        })
        .child(div().size(7).rounded(4).bg(colour(r.dot)));
    };
    const block = (g) => {
      const top = g.rows.reduce((best, r) => ((RANK[r.dot] ?? 0) > (RANK[best] ?? 0) ? r.dot : best), "idle");
      const waiting = g.rows.filter((r) => r.needs).length;
      const tag = g.other ? "··" : projectTag(g.project.name, g.project.short);
      const dots = [];
      for (let i = 0; i < g.rows.length; i += 2) dots.push(h_flex().gap(2).justify_center().children(g.rows.slice(i, i + 2).map(dot)));
      return v_flex().id(`rp-${g.project.name}`).flex_none().items_center().gap(3).pt(6)
        .child(div().relative().w(30).h(22).flex().items_center().justify_center().rounded(6)
          .bg(colour(top) + "26").text_color(colour(top)).text_size(11).font_semibold()
          .tooltip(`${g.project.name}: ${g.rows.length} agent${g.rows.length === 1 ? "" : "s"}${waiting ? `, ${waiting} need you` : ""}`)
          .child(tag)
          .when(waiting > 0, (el) => el.child(div().absolute().top(-5).right(-6).min_w(14).h(14).px(3).rounded(7)
            .bg(t.blocked).text_color(t.solid_foreground ?? t.background).text_size(9).font_bold()
            .flex().items_center().justify_center().child(String(waiting)))))
        .children(dots);
    };
    // Other at the bottom; the list already puts it last.
    return v_flex().size_full().min_h(0).bg(t.chrome).border_t(1).border_color(t.border)
      .child(v_flex().id("sp-rail").flex_1().min_h(0).pb(6).overflow_y_scrollbar().children(list.map(block)))
      .child(h_flex().h(28).flex_none().items_center().justify_center()
        .child(div().id("rail-expand").text_size(13).text_color(t.dim).cursor_pointer()
          .hover((el) => el.text_color(t.foreground)).tooltip("Expand the sidebar").on_click(toggleSidebar).child("»")));
  }

  // Hidden spaces, folded at the bottom: one entry each with what its agents
  // work on (so two spaces with one name tell apart), its agents count and
  // Unhide. A click focuses the first agent.
  hiddenSection(t, v, everyRow) {
    const open = this.showHidden;
    const head = h_flex().id("pj-hidden").px(8).pt(8).h(30).flex_none().gap(6).items_center()
      .text_size(11).font_medium().text_color(t.dim).cursor_pointer().hover((s) => s.text_color(t.foreground))
      .on_click((_e, cx) => {
        this.showHidden = !open;
        localStorage.setItem("showHidden", this.showHidden ? "1" : "0");
        cx.notify();
      })
      .child(svg(`mods/workspace-chrome/chevron-${open ? "down" : "right"}.svg`).size(11).flex_none())
      .child(div().child("Hidden"))
      .child(div().child(String(v.hidden.length)));
    if (!open) return [head];
    const line = (key) => {
      const agentsThere = everyRow.filter((r) => names(key, r.space));
      const [machine, ...rest] = key.split("/");
      const name = (rest.join("/") || key).replace(/#[^#]*$/, "");
      const first = agentsThere[0]?.a;
      const works = [...new Set(agentsThere.map((r) => workOf(r, "")).filter(Boolean))];
      return h_flex().id(`hd-${key}`).px(8).py(4).flex_none().rounded(8).items_center().gap(8)
        .cursor_pointer().hover((s) => s.bg(t.hover))
        .on_click(() => first && run("pane.focus", { pane: first.pane }))
        .on_mouse_down("right", (_e, cx) => {
          cx.stop_propagation();
          openOverlay({ mod: "sheprd-sidebar", kind: "popup", width: 200, height: 60, props: { menu: { key, space: agentsThere[0]?.space?.id, unhideOnly: true } } });
        })
        .tooltip(works.length ? `${name} — ${works.join(" · ")}` : name)
        .child(v_flex().flex_1().min_w(0).gap(1)
          .child(div().truncate().text_color(t.muted).child(works[0] ?? name))
          .when(works.length > 0, (el) => el.child(div().truncate().text_size(11).text_color(t.dim)
            .child([works.length > 1 ? `+${works.length - 1} more` : "", name].filter(Boolean).join(" · ")))))
        .when(machine !== "local", (el) => el.child(div().flex_none().text_size(10).text_color(t.dim).child(machineTag(machine))))
        .child(div().flex_none().text_size(11).text_color(t.dim).child(agentsThere.length ? String(agentsThere.length) : "–"))
        .child(div().id(`uh-${key}`).flex_none().px(6).h(20).rounded(5).flex().items_center().text_size(11)
          .bg(t.raised).text_color(t.foreground).hover((s) => s.bg(t.hover))
          .on_click((_e, cx) => {
            cx.stop_propagation();
            saveView({ hidden: view().hidden.filter((h) => h !== key) }); // the whole entry, as listed
          })
          .child("Unhide"));
    };
    return [head, ...v.hidden.map(line)];
  }

  // A space made from "New tab or space" joins its project when it shows up
  // (space.new doesn't return its id); given up after 30 s.
  join(now) {
    const joining = shared("sheprd.joining");
    if (!joining) return;
    const added = spaces().find((s) => !joining.before.includes(s.id) && (s.machine ?? null) === joining.machine);
    if (!added && now - joining.at < 30000) return;
    setShared("sheprd.joining", null);
    if (added) saveView({ projects: view().projects.map((x) => ({ ...x, members: x.name === joining.project ? [...untie(x.members ?? [], added), spaceRef(added)] : untie(x.members ?? [], added) })) });
  }

  render(cx) {
    const t = theme();
    const p = props() ?? {};
    if (p.newProject) return this.promptView(t);
    if (p.newIn) return this.newView(t);
    if (p.menu) return this.menuView(t);

    const now = Date.now();
    this.join(now);
    const v = view();
    const all = agents();
    const fresh = this.track(all, now);
    // A new agent started while showing Marked is marked, so it stays in view.
    if (v.filter === "marked" && fresh.length) {
      saveMarks((x) => ({ ...x, marked: [...x.marked, ...fresh.filter((a) => !x.marked.includes(a))] }));
    }
    const m = marks();
    const spaceById = new Map(spaces().map((s) => [s.id, s]));
    this.attend(all, spaceById);
    const cards = cardsByAddress();
    const dismissed = new Set(m.dismissed);

    const everyRow = all
      .map((a) => {
        const space = spaceById.get(a.space);
        const questions = openQuestions(cards.get(a.address)).length;
        const isDismissed = dismissed.has(`${a.address}@${a.seq}`);
        const needs = !isDismissed && (a.status === "blocked" || a.status === "done" || questions > 0 || m.unread.includes(a.address));
        const summary = (cards.get(a.address) ?? []).find((c) => !c.dismissed && c.summary)?.summary;
        // The circle follows read state: a finished agent marked read shows
        // idle, an idle one marked unread shows finished.
        const dot = a.status === "done" && isDismissed ? "idle" : a.status === "idle" && m.unread.includes(a.address) ? "done" : a.status;
        return { a, space, dot, key: spaceKey(space), questions, needs, summary, marked: m.marked.includes(a.address), rank: needs ? Math.max(RANK[a.status] ?? 0, 3) : RANK[a.status] ?? 0 };
      });
    const rows = everyRow.filter((r) => !isHiddenSpace(r.space, v.hidden));

    const counts = {
      "needs-you": rows.filter((r) => r.needs).length,
      working: rows.filter((r) => r.a.status === "working").length,
      marked: rows.filter((r) => r.marked).length,
      all: rows.length,
    };
    // Needs you: the agent opened last stays in its place, greyed, until the
    // next one is opened, so working down the queue never jumps.
    const held = v.filter === "needs-you" ? this.held : null;
    for (const r of rows) r.held = held?.address === r.a.address && !r.needs;
    const shown = rows.filter((r) => {
      if (v.filter === "needs-you" && !r.needs && !r.held) return false;
      if (v.filter === "working" && r.a.status !== "working") return false;
      if (v.filter === "marked" && !r.marked) return false;
      return true;
    });

    // Projects in their saved order, pinned first; Other last.
    const order = [...v.projects.filter((x) => x.pinned), ...v.projects.filter((x) => !x.pinned)];
    // Inside a project: Needs you by urgency, then latest change (a held row
    // keeps the place it had when opened). Every other view, and the rail, in
    // the spaces' own order, so reading or a state change never moves a row.
    const spaceAt = new Map(spaces().map((s, i) => [s.id, i]));
    const tabAt = new Map(spaces().flatMap((s) => (s.tabs ?? []).map((x, i) => [x.id, i])));
    const fixed = (x, y) => (spaceAt.get(x.a.space) ?? 1e9) - (spaceAt.get(y.a.space) ?? 1e9)
      || (tabAt.get(x.a.tab) ?? 1e9) - (tabAt.get(y.a.tab) ?? 1e9)
      || x.a.address.localeCompare(y.a.address, undefined, { numeric: true });
    const placed = (r) => (r.held ? held : { rank: r.rank, at: this.changed[r.a.address]?.at ?? 0, seq: r.a.seq });
    const urgent = (x, y) => {
      const a = placed(x), b = placed(y);
      return b.rank - a.rank || b.at - a.at || b.seq - a.seq;
    };
    const grouped = (source, sort) => {
      const groups = new Map(order.map((x) => [x.name, { project: x, rows: [] }]));
      const other = { project: { name: "Other", collapsed: v.other_collapsed }, rows: [], other: true };
      for (const r of source) (groups.get(projectOf(r.space, v.projects)?.name)?.rows ?? other.rows).push(r);
      const out = [...groups.values(), other].filter((g) => g.rows.length > 0);
      for (const g of out) g.rows.sort(sort);
      return out;
    };
    // The rail has no filter chips, so it shows every agent that isn't hidden.
    if (narrow()) return this.rail(t, grouped(rows, fixed), m);
    const list = grouped(shown, v.filter === "needs-you" ? urgent : fixed);
    this.shownOrder = list.filter((g) => !g.other).map((g) => g.project.name);

    const fold = (g) => {
      if (this.drag?.moved || Date.now() - (this.droppedAt ?? 0) < 400) return;
      if (g.other) return saveView({ other_collapsed: !v.other_collapsed });
      saveView({ projects: v.projects.map((x) => (x.name === g.project.name ? { ...x, collapsed: !x.collapsed } : x)) });
    };

    const header = (g) => {
      const open = !g.project.collapsed;
      const top = g.rows.reduce((best, r) => (r.rank > best.rank ? r : best), { rank: 0 });
      const waiting = g.rows.filter((r) => r.needs).length;
      return h_flex().id(`pj-${g.project.name}`).px(8).pt(8).h(30).flex_none().gap(6).items_center()
        .text_size(11).font_medium().cursor_pointer().hover((s) => s.text_color(t.foreground))
        .on_click(() => fold(g))
        // Press and drag a header onto another project to move it there.
        .on_mouse_down("left", (e) => {
          if (!g.other) this.drag = { name: g.project.name, y: e.position.y, moved: false, over: null };
        })
        .when(this.drag?.moved && this.drag.name === g.project.name, (el) => el.opacity(0.5))
        .on_mouse_down("right", (_e, cx) => {
          cx.stop_propagation();
          // The menu's height follows its items: 28 px each, plus padding.
          const shown = list.filter((x) => !x.other).map((x) => x.project.name);
          const at = g.other ? -1 : shown.indexOf(g.project.name);
          const moves = at < 0 ? 0 : [at > 1, at > 0, at < shown.length - 1, at < shown.length - 2].filter(Boolean).length;
          openOverlay({ mod: "sheprd-sidebar", kind: "popup", width: 260, height: 78 + 28 * moves, props: { menu: { project: g.other ? null : g.project.name, shown } } });
        })
        .child(svg(`mods/workspace-chrome/chevron-${open ? "down" : "right"}.svg`).size(11).flex_none().text_color(t.dim))
        .child(div().truncate().text_color(waiting ? t.blocked : t.muted).child(g.project.name))
        .child(div().text_color(t.dim).child(String(g.rows.length)))
        .when(g.other ? Boolean(v.other_muted) : Boolean(g.project.muted), (el) => el.child(div().text_color(t.dim).child("muted")))
        .when(Boolean(g.project.note), (el) => el.child(div().flex_1().min_w(0).truncate().text_color(t.dim).child(`· ${g.project.note}`)))
        .child(div().flex_1())
        .when(!open && top.rank >= 3, (el) => el.child(this.status(t, top.dot ?? "done", top.questions ?? 0)));
    };

    const detailed = v.density !== "compact";
    const row = (r) => {
      const a = r.a;
      const machine = a.address.includes("/") ? machineTag(a.address.split("/")[0]) : "";
      const when = this.changed[a.address]?.at;
      const quiet = (a.status === "idle" && !a.focused && !r.needs) || r.held;
      const space = a.space_name || a.tab_name || a.program?.name || a.address;
      const work = workOf(r, space);
      const base = h_flex().id(`ag-${a.address}`).px(8).flex_none().rounded(8).items_center().gap(8)
        .when(a.focused, (el) => el.bg(t.selected))
        .cursor_pointer().hover((s) => s.bg(t.hover))
        // Hover: the details (Shift-hover never worked: a held Shift sends no
        // event to a row the pointer rests on). Only the hovered row builds them.
        .tooltip(this.hovered === a.address
          ? this.details(r, now)
          : `${space}${machine ? ` · ${machine}` : ""}${work !== space ? ` — ${work}` : ""}\n${pingAddress(a.address)}`)
        .on_hover((on, cx) => {
          if (on) this.hovered = a.address;
          else if (this.hovered === a.address) this.hovered = null;
          if (on) this.loadFacts(a.address.includes("/") ? a.address.split("/")[0] : null, cx);
          cx.notify();
        })
        .on_click((e) => {
          if (e.modifiers.shift) return copyAddress(a.address);
          if (v.filter === "needs-you" && !r.held) this.held = { address: a.address, ...placed(r) };
          run("pane.focus", { pane: a.pane });
          if (m.unread.includes(a.address)) saveMarks((x) => ({ ...x, unread: x.unread.filter((u) => u !== a.address) }));
        })
        .on_mouse_down("right", (_e, cx) => {
          cx.stop_propagation();
          openOverlay({ mod: "sheprd-sidebar", kind: "popup", width: 320, height: menuHeight(view().projects.length), props: { menu: { address: a.address, seq: a.seq, key: r.key, space: r.space?.id, needs: r.needs } } });
        });
      // Hovered or marked: the mark toggle (★) in place of the age.
      const age = r.marked || this.hovered === a.address
        ? div().id(`mk-${a.address}`).flex_none().px(2).rounded(4).text_size(12)
          .text_color(r.marked ? t.accent ?? t.foreground : t.dim).hover((el) => el.bg(t.hover).text_color(t.foreground))
          .tooltip(r.marked ? "Unmark" : "Mark (shows under Marked)")
          .on_click((_e, cx) => {
            cx.stop_propagation();
            toggleMarked(a.address);
          })
          .child(r.marked ? "★" : "☆")
        : div().flex_none().text_size(11).text_color(t.dim).child(when ? duration(now - when) : "");
      if (!detailed) {
        return base.h(28)
          .child(this.status(t, r.dot, r.questions))
          .child(div().flex_1().min_w(0).truncate().text_color(quiet ? t.muted : t.foreground).when(r.needs, (el) => el.font_semibold()).child(space))
          .when(Boolean(machine), (el) => el.child(div().flex_none().text_size(10).text_color(t.dim).child(machine)))
          .child(age);
      }
      return base.py(4).items_start()
        .child(div().pt(3).flex_none().child(this.status(t, r.dot, r.questions)))
        .child(v_flex().flex_1().min_w(0).gap(1)
          .child(h_flex().gap(6).items_center()
            .child(div().flex_1().min_w(0).truncate().text_color(quiet ? t.muted : t.foreground).when(r.needs, (el) => el.font_semibold()).child(work))
            .child(age))
          .child(div().truncate().text_size(11).text_color(t.dim).child(machine ? `${space} · ${machine}` : space)));
    };

    // Narrower than the default width: short labels, and chips shrink before
    // anything is cut.
    const short = (find(layout(), "sidebar")?.size?.px ?? Infinity) < 280;
    const chip = ([id, long, brief]) => {
      const on = v.filter === id;
      const label = short ? brief : long;
      return h_flex().id(`f-${id}`).h(22).px(short ? 6 : 8).gap(4).rounded(6).items_center().text_size(11).min_w(0)
        .bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.dim)
        .cursor_pointer().hover((s) => s.text_color(t.foreground))
        .on_click(() => {
          this.held = null;
          saveView({ filter: id });
        })
        .child(div().min_w(0).truncate().child(label))
        .child(div().flex_none().text_color(id === "needs-you" && counts[id] ? t.blocked : t.dim).child(String(counts[id])));
    };

    return v_flex().size_full().min_h(0).bg(t.chrome).border_t(1).border_color(t.border).text_size(13)
      // Settings (detailed/compact) sit in the header (sheprd-chrome); the
      // filters stay here.
      .child(h_flex().px(10).pt(10).pb(6).gap(4).flex_none().items_center().min_w(0)
        .children(FILTERS.map(chip)))
      .when(Boolean(this.problem), (el) => el.child(div().px(12).py(6).text_size(11.5).text_color(t.blocked).child(this.problem)))
      .when(list.length === 0, (el) => el.child(div().px(12).py(8).text_size(12).text_color(t.dim)
        .child(rows.length === 0 ? "No agents yet. They show up here when they report their status." : "Nothing here with this filter.")))
      .child(v_flex().id("sp-list").flex_1().min_h(0).px(8).overflow_y_scrollbar()
        .on_mouse_up("left", (_e, cx) => this.dropProject(cx))
        // Released outside the list: no mouse-up reaches it, so leaving ends the drag.
        .on_hover((on, cx) => {
          if (!on && this.drag) {
            this.drag = null;
            cx.notify();
          }
        })
        .children(list.map((g) => this.dropTarget(t, g, [header(g), ...(g.project.collapsed ? [] : g.rows.map(row))])))
        .when(v.hidden.length > 0, (el) => el.children(this.hiddenSection(t, v, everyRow))));
  }
}
