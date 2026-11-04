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
// Opened as a popup with props { menu } (a row's right-click menu) or
// { newProject } (name a new project).

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { agents, spaces, machines, run, theme, shared, setShared, openOverlay, closeOverlay, props, exec, layout, window, playSound } from "herdr";

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
const MENU_FIXED = 3 * 28 + 9 + 22 + 10; // mark, read, hide, divider, label, padding
const menuHeight = (projects) => Math.min(MENU_FIXED + 28 * (projects + 3), 560);

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

// `cards json` from the given machines, merged over the last good result.
async function fetchCards(list) {
  const docs = { .../** @type {object} */ (shared("sheprd.cards") ?? {}) };
  const present = new Set(machines().map((m) => m.name ?? "local"));
  for (const key of Object.keys(docs)) if (!present.has(key)) delete docs[key];
  const got = await Promise.all(list.filter((m) => m.layout !== "unavailable").map(async (m) => {
    try {
      const r = await exec("cards", ["json"], { on: m.name });
      return r.code === 0 ? [m.name ?? "local", JSON.parse(r.stdout)] : null;
    } catch {
      return null; // not installed there, or the machine went away
    }
  }));
  for (const entry of got) if (entry) docs[entry[0]] = entry[1];
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
    cx.timer.every(CARDS_EVERY_MS, (cx) => this.loadCards(cx));
    cx.timer.every(30000, (cx) => cx.notify());
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
      return v_flex().w_full().p(5).bg(t.popover).text_size(12.8)
        .child(item("m-mute", muted ? "Unmute: sound and notify again" : "Mute: no sound or notification", () => setMuted(!muted)));
    }
    if (unhideOnly) {
      return v_flex().w_full().p(5).bg(t.popover).text_size(12.8)
        .child(item("m-unhide", "Unhide this space", unhide));
    }
    // Mark, read and hide first, so they never scroll away; the projects
    // scroll when the window is too short for all of them.
    return v_flex().size_full().p(5).bg(t.popover).text_size(12.8)
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
        .child(div().size(7).rounded(4).bg(colour(a.status)));
    };
    const block = (g) => {
      const top = g.rows.reduce((best, r) => ((RANK[r.a.status] ?? 0) > (RANK[best] ?? 0) ? r.a.status : best), "idle");
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

  // Hidden spaces, folded at the bottom: one line each (its agents count, a
  // click focuses the first one) with Unhide.
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
      return h_flex().id(`hd-${key}`).px(8).h(28).flex_none().rounded(8).items_center().gap(8)
        .cursor_pointer().hover((s) => s.bg(t.hover))
        .on_click(() => first && run("pane.focus", { pane: first.pane }))
        .on_mouse_down("right", (_e, cx) => {
          cx.stop_propagation();
          openOverlay({ mod: "sheprd-sidebar", kind: "popup", width: 200, height: 60, props: { menu: { key, space: agentsThere[0]?.space?.id, unhideOnly: true } } });
        })
        .child(div().flex_1().min_w(0).truncate().text_color(t.muted).child(name))
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

  render(cx) {
    const t = theme();
    const p = props() ?? {};
    if (p.newProject) return this.promptView(t);
    if (p.menu) return this.menuView(t);

    const v = view();
    const now = Date.now();
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
        return { a, space, key: spaceKey(space), questions, needs, summary, marked: m.marked.includes(a.address), rank: needs ? Math.max(RANK[a.status] ?? 0, 3) : RANK[a.status] ?? 0 };
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

    const fold = (g) => {
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
        .on_mouse_down("right", (_e, cx) => {
          cx.stop_propagation();
          openOverlay({ mod: "sheprd-sidebar", kind: "popup", width: 260, height: 50, props: { menu: { project: g.other ? null : g.project.name } } });
        })
        .child(svg(`mods/workspace-chrome/chevron-${open ? "down" : "right"}.svg`).size(11).flex_none().text_color(t.dim))
        .child(div().truncate().text_color(waiting ? t.blocked : t.muted).child(g.project.name))
        .child(div().text_color(t.dim).child(String(g.rows.length)))
        .when(g.other ? Boolean(v.other_muted) : Boolean(g.project.muted), (el) => el.child(div().text_color(t.dim).child("muted")))
        .when(Boolean(g.project.note), (el) => el.child(div().flex_1().min_w(0).truncate().text_color(t.dim).child(`· ${g.project.note}`)))
        .child(div().flex_1())
        .when(!open && top.rank >= 3, (el) => el.child(this.status(t, top.a?.status ?? "done", top.questions ?? 0)));
    };

    const detailed = v.density !== "compact";
    const row = (r) => {
      const a = r.a;
      const machine = a.address.includes("/") ? machineTag(a.address.split("/")[0]) : "";
      const when = this.changed[a.address]?.at;
      const quiet = (a.status === "idle" && !a.focused && !r.needs) || r.held;
      const space = a.space_name || a.tab_name || a.program?.name || a.address;
      // Its work: the card summary, else the terminal's title, else a named tab.
      const work = r.summary || a.title || (a.tab_name && !/^\d+$/.test(a.tab_name) ? a.tab_name : "") || space;
      const base = h_flex().id(`ag-${a.address}`).px(8).flex_none().rounded(8).items_center().gap(8)
        .when(a.focused, (el) => el.bg(t.selected))
        .cursor_pointer().hover((s) => s.bg(t.hover))
        .tooltip(`${space}${machine ? ` · ${machine}` : ""}${work !== space ? ` — ${work}` : ""}`)
        .on_hover((on, cx) => {
          if (on) this.hovered = a.address;
          else if (this.hovered === a.address) this.hovered = null;
          cx.notify();
        })
        .on_click(() => {
          if (v.filter === "needs-you" && !r.held) this.held = { address: a.address, ...placed(r) };
          run("pane.focus", { pane: a.pane });
          if (m.unread.includes(a.address)) saveMarks((x) => ({ ...x, unread: x.unread.filter((u) => u !== a.address) }));
        })
        .on_mouse_down("right", (_e, cx) => {
          cx.stop_propagation();
          openOverlay({ mod: "sheprd-sidebar", kind: "popup", width: 230, height: menuHeight(view().projects.length), props: { menu: { address: a.address, seq: a.seq, key: r.key, space: r.space?.id, needs: r.needs } } });
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
          .child(this.status(t, a.status, r.questions))
          .child(div().flex_1().min_w(0).truncate().text_color(quiet ? t.muted : t.foreground).when(r.needs, (el) => el.font_semibold()).child(space))
          .when(Boolean(machine), (el) => el.child(div().flex_none().text_size(10).text_color(t.dim).child(machine)))
          .child(age);
      }
      return base.py(4).items_start()
        .child(div().pt(3).flex_none().child(this.status(t, a.status, r.questions)))
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
        .children(list.flatMap((g) => [header(g), ...(g.project.collapsed ? [] : g.rows.map(row))]))
        .when(v.hidden.length > 0, (el) => el.children(this.hiddenSection(t, v, everyRow))));
  }
}
