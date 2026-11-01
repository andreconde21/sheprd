// sheprd's sidebar: every agent on every machine in one list, grouped by project
// (the same repo on two machines is one project), with a status filter, an
// active-only switch and marks. The layout lives in the hub's `sheprd-view`
// herdr-core document (see herdr-app/CONTRACT.md in the sheprd repo), read and
// written through the `cards` CLI; cards come from each machine's `sheprd-cards`.
//
// Opened as a popup with props { menu } (a row's right-click menu) or
// { newProject } (name a new project).

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { agents, spaces, machines, run, theme, shared, setShared, openOverlay, closeOverlay, props, exec } from "herdr";

const RANK = { blocked: 4, done: 3, working: 2, idle: 1 };
const FILTERS = [
  ["needs-you", "Needs you"],
  ["working", "Working"],
  ["all", "All"],
];
// Active only: idle agents drop out this long after their last change, unless kept.
const RECENT_MS = 8 * 3600000;
const CARDS_EVERY_MS = 5000;

const EMPTY_VIEW = { projects: [], hidden: [], filter: "all", active_only: false, other_collapsed: false, marks: { unread: [], kept: [], dismissed: [] } };
const view = () => ({ ...EMPTY_VIEW, .../** @type {object} */ (shared("sheprd.view") ?? {}) });
const marks = () => ({ unread: [], kept: [], dismissed: [], ...(view().marks ?? {}) });

// Changes show at once; the hub's document follows. A failed write leaves the
// screen ahead of the document until the next start, which is the lesser harm.
function saveView(patch) {
  setShared("sheprd.view", { ...view(), ...patch });
  exec("cards", ["view-set", JSON.stringify(patch)]).catch(() => {});
}

function saveMarks(change) {
  saveView({ marks: change(marks()) });
}

// A space's key in projects' `members`: its machine (`local` for this one) and its name.
const spaceKey = (s) => `${s?.machine ?? "local"}/${s?.name ?? ""}`;

function projectOf(space, projects) {
  if (!space) return null;
  const key = spaceKey(space).toLowerCase();
  const byMember = projects.find((p) => (p.members ?? []).some((m) => m.toLowerCase() === key));
  if (byMember) return byMember;
  const hay = `${space.name} ${space.folder ?? ""}`.toLowerCase();
  return projects.find((p) => (p.match ?? []).some((m) => m && hay.includes(m.toLowerCase()))) ?? null;
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

const openQuestions = (cards) => (cards ?? []).filter((c) => !c.dismissed).flatMap((c) => c.questions.filter((q) => q.state === "open" || q.state === "staged"));

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
    // Each agent's last state and when it changed (ms), kept across restarts.
    this.changed = JSON.parse(localStorage.getItem("changed") ?? "{}");
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

  // Every reachable machine's cards, into shared state for the panel too.
  loadCards(cx) {
    cx.spawn(async (cx) => {
      const docs = {};
      for (const m of machines()) {
        if (m.layout === "unavailable") continue;
        try {
          const r = await exec("cards", ["json"], { on: m.name });
          if (r.code === 0) docs[m.name ?? "local"] = JSON.parse(r.stdout);
        } catch {
          // Not installed there, or the machine went away: no cards from it.
        }
      }
      setShared("sheprd.cards", docs);
      setShared("sheprd.cards.at", Date.now());
      cx.notify();
    });
  }

  // Stamps agents whose state changed; done and idle count as one.
  track(list, now) {
    let dirty = false;
    for (const a of list) {
      const s = a.status === "done" ? "idle" : a.status;
      if (this.changed[a.address]?.s === s) continue;
      this.changed[a.address] = { s, at: now };
      dirty = true;
    }
    if (!dirty) return;
    const listed = new Set(list.map((a) => a.address));
    for (const [k, c] of Object.entries(this.changed)) if (!listed.has(k) && now - c.at > 30 * 86400000) delete this.changed[k];
    localStorage.setItem("changed", JSON.stringify(this.changed));
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
    const projects = view().projects.map((x) => ({ ...x, members: (x.members ?? []).filter((m) => m !== p.newProject.key) }));
    projects.push({ name, members: [p.newProject.key], match: [] });
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
    const { address, seq, key } = p.menu;
    const m = marks();
    const kept = m.kept.includes(address);
    const projects = view().projects;
    const item = (id, label, action, dim = false) => h_flex().id(id).h(28).px(8).rounded(6).items_center()
      .text_color(dim ? t.muted : t.foreground).hover((el) => el.bg(t.hover)).cursor_pointer()
      .on_click(() => { closeOverlay(p.overlay); action(); }).child(label);
    const moveTo = (name) => {
      const list = projects.map((x) => {
        const members = (x.members ?? []).filter((mm) => mm !== key);
        return x.name === name ? { ...x, members: [...members, key] } : { ...x, members };
      });
      saveView({ projects: list });
    };
    return v_flex().w_full().p(5).bg(t.popover).text_size(12.8)
      .child(item("m-read", "Mark read", () => saveMarks((x) => ({ ...x, dismissed: [...x.dismissed.filter((d) => !d.startsWith(`${address}@`)), `${address}@${seq}`].slice(-200) }))))
      .child(item("m-keep", kept ? "Stop keeping in view" : "Keep in view", () => saveMarks((x) => ({ ...x, kept: kept ? x.kept.filter((k) => k !== address) : [...x.kept, address] }))))
      .child(div().pt(4).pb(4).child(div().h(1).bg(t.border)))
      .child(div().px(8).pt(2).pb(4).text_size(11).text_color(t.dim).child(`Move ${key} to`))
      .children(projects.map((x, i) => item(`m-p${i}`, x.name, () => moveTo(x.name))))
      .child(item("m-other", "Other", () => moveTo(null), true))
      .child(item("m-new", "New project…", () => openOverlay({ mod: "sheprd-sidebar", kind: "dialog", width: 360, height: 190, props: { newProject: { key } } })))
      .child(div().pt(4).pb(4).child(div().h(1).bg(t.border)))
      .child(item("m-hide", "Hide this space", () => saveView({ hidden: [...view().hidden, key] }), true));
  }

  // ---- the list -----------------------------------------------------------

  status(t, s, questions = 0) {
    if (questions > 0) return div().flex_none().px(5).h(17).rounded(5).bg(t.blocked + "26").text_size(10.5).font_semibold().text_color(t.blocked).flex().items_center().child(`?${questions}`);
    if (s === "working") return svg("mods/workspace-chrome/spinner.svg").size(12).mx(1).flex_none().text_color(t.working).spin(900);
    if (s === "blocked") return div().size(14).flex_none().rounded(4).bg(t.blocked + "26").text_size(10.5).font_bold().text_color(t.blocked).flex().items_center().justify_center().child("!");
    if (s === "idle") return div().size(7).mx(3).flex_none().rounded(4).border(1.5).border_color(t.idle ?? t.dim);
    return div().size(7).mx(3).flex_none().rounded(4).bg(s === "done" ? t.done : t.dim + "00");
  }

  render(cx) {
    const t = theme();
    const p = props() ?? {};
    if (p.newProject) return this.promptView(t);
    if (p.menu) return this.menuView(t);

    const v = view();
    const m = marks();
    const now = Date.now();
    const all = agents();
    this.track(all, now);
    const spaceById = new Map(spaces().map((s) => [s.id, s]));
    const cards = cardsByAddress();
    const hidden = new Set(v.hidden);
    const dismissed = new Set(m.dismissed);

    const rows = all
      .map((a) => {
        const space = spaceById.get(a.space);
        const questions = openQuestions(cards.get(a.address)).length;
        const isDismissed = dismissed.has(`${a.address}@${a.seq}`);
        const needs = !isDismissed && (a.status === "blocked" || a.status === "done" || questions > 0 || m.unread.includes(a.address));
        return { a, space, key: spaceKey(space), questions, needs, rank: needs ? Math.max(RANK[a.status] ?? 0, 3) : RANK[a.status] ?? 0 };
      })
      .filter((r) => !hidden.has(r.key));

    const counts = {
      "needs-you": rows.filter((r) => r.needs).length,
      working: rows.filter((r) => r.a.status === "working").length,
      all: rows.length,
    };
    const shown = rows.filter((r) => {
      if (v.filter === "needs-you" && !r.needs) return false;
      if (v.filter === "working" && r.a.status !== "working") return false;
      if (v.active_only && !r.needs && r.a.status === "idle" && !r.a.focused && !r.a.pinned && !m.kept.includes(r.a.address)) {
        return now - (this.changed[r.a.address]?.at ?? 0) < RECENT_MS;
      }
      return true;
    });

    // Projects in their saved order, pinned first; Other last.
    const order = [...v.projects.filter((x) => x.pinned), ...v.projects.filter((x) => !x.pinned)];
    const groups = new Map(order.map((x) => [x.name, { project: x, rows: [] }]));
    const other = { project: { name: "Other", collapsed: v.other_collapsed }, rows: [], other: true };
    for (const r of shown) (groups.get(projectOf(r.space, v.projects)?.name)?.rows ?? other.rows).push(r);
    const list = [...groups.values(), other].filter((g) => g.rows.length > 0);
    for (const g of list) g.rows.sort((x, y) => y.rank - x.rank || (this.changed[y.a.address]?.at ?? 0) - (this.changed[x.a.address]?.at ?? 0) || y.a.seq - x.a.seq);

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
        .child(svg(`mods/workspace-chrome/chevron-${open ? "down" : "right"}.svg`).size(11).flex_none().text_color(t.dim))
        .child(div().truncate().text_color(waiting ? t.blocked : t.muted).child(g.project.name))
        .child(div().text_color(t.dim).child(String(g.rows.length)))
        .when(Boolean(g.project.note), (el) => el.child(div().flex_1().min_w(0).truncate().text_color(t.dim).child(`· ${g.project.note}`)))
        .child(div().flex_1())
        .when(!open && top.rank >= 3, (el) => el.child(this.status(t, top.a?.status ?? "done", top.questions ?? 0)));
    };

    const row = (r) => {
      const a = r.a;
      const machine = a.address.includes("/") ? a.address.split("/")[0] : null;
      const when = this.changed[a.address]?.at;
      const quiet = a.status === "idle" && !a.focused && !r.needs;
      const detail = a.tab_name && !/^\d+$/.test(a.tab_name) ? a.tab_name : a.title || a.program?.name || "";
      return h_flex().id(`ag-${a.address}`).px(8).h(30).flex_none().rounded(8).items_center().gap(8)
        .when(a.focused, (el) => el.bg(t.selected))
        .cursor_pointer().hover((s) => s.bg(t.hover))
        .on_click(() => {
          run("pane.focus", { pane: a.pane });
          if (m.unread.includes(a.address)) saveMarks((x) => ({ ...x, unread: x.unread.filter((u) => u !== a.address) }));
        })
        .on_mouse_down("right", (_e, cx) => {
          cx.stop_propagation();
          openOverlay({ mod: "sheprd-sidebar", kind: "popup", width: 230, height: 360, props: { menu: { address: a.address, seq: a.seq, key: r.key } } });
        })
        .child(h_flex().flex_1().min_w(0).gap(5).items_center()
          .child(div().min_w(0).max_w(130).truncate().text_color(quiet ? t.muted : t.foreground).child(a.space_name || detail))
          .when(Boolean(detail) && detail !== a.space_name, (el) => el.child(div().flex_1().min_w(0).truncate().text_color(t.dim).child(`· ${detail}`))))
        .when(Boolean(machine), (el) => el.child(div().flex_none().text_size(10).text_color(t.dim).child(machine)))
        .when(Boolean(when), (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child(duration(now - when))))
        .child(this.status(t, a.status, r.questions));
    };

    const chip = ([id, label]) => {
      const on = v.filter === id;
      return h_flex().id(`f-${id}`).h(22).px(8).gap(4).rounded(6).items_center().text_size(11)
        .bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.dim)
        .cursor_pointer().hover((s) => s.text_color(t.foreground))
        .on_click(() => saveView({ filter: id }))
        .child(label)
        .child(div().text_color(id === "needs-you" && counts[id] ? t.blocked : t.dim).child(String(counts[id])));
    };

    return v_flex().size_full().min_h(0).bg(t.chrome).border_t(1).border_color(t.border).text_size(13)
      .child(h_flex().px(10).pt(10).pb(6).gap(4).flex_none().items_center()
        .children(FILTERS.map(chip))
        .child(div().flex_1())
        .child(div().id("f-active").h(22).px(7).rounded(6).flex().items_center().text_size(11)
          .bg(v.active_only ? t.selected : t.chrome).text_color(v.active_only ? t.foreground : t.dim)
          .cursor_pointer().tooltip("Active only: hide agents idle for 8 hours, unless kept")
          .on_click(() => saveView({ active_only: !v.active_only }))
          .child("Active")))
      .when(Boolean(this.problem), (el) => el.child(div().px(12).py(6).text_size(11.5).text_color(t.blocked).child(this.problem)))
      .when(list.length === 0, (el) => el.child(div().px(12).py(8).text_size(12).text_color(t.dim)
        .child(rows.length === 0 ? "No agents yet. They show up here when they report their status." : "Nothing here with this filter.")))
      .child(v_flex().id("sp-list").flex_1().min_h(0).px(8).overflow_y_scrollbar()
        .children(list.flatMap((g) => [header(g), ...(g.project.collapsed ? [] : g.rows.map(row))])));
  }
}
