// sheprd's Messages panel: what agents sent each other with `hc send`, which
// hc-inbox delivers in the background (one line in the agent's transcript
// instead of the whole message). Two views: the focused agent's messages, or
// every agent's on every machine, newest first. The header switch turns
// background delivery off (messages typed into the prompt again) or back on.
// Data: `hc-inbox log --json --all` on each machine (herdr-app/bin/hc-inbox).

import { View, div } from "gpui-kit";
import { v_flex, h_flex, TextView } from "gpui-base";
import { agents, terminals, machines, focused, run, theme, exec } from "herdr";

const EVERY_MS = 4000;
const LIMIT = 300; // per machine
const FOLD = 600; // characters shown before "more"

const stored = (key, fallback) => { try { return localStorage.getItem(key) ?? fallback; } catch { return fallback; } };
const store = (key, value) => { try { localStorage.setItem(key, value); } catch { /* not remembered */ } };

// hc-inbox lives in ~/.local/bin, often not on the PATH of a herdr-core reached over SSH.
async function inbox(args, on) {
  try {
    const r = await exec("sh", ["-c", 'p="$HOME/.local/bin/hc-inbox"; [ -x "$p" ] || p=hc-inbox; exec "$p" "$@"', "hc-inbox", ...args], { on });
    return r;
  } catch {
    return null;
  }
}

const failure = (r) => {
  if (!r) return "could not run it";
  if (r.code === 127 || /not found|no such file/i.test(r.stderr ?? "")) return "hc-inbox is not installed there";
  return (r.stderr || r.stdout || `exit ${r.code}`).trim().split("\n")[0].slice(0, 120);
};

// An address as the target's machine wrote it, as this app names it.
// `on`: the machine the log came from (null: this one); `names`: this app's machine names.
function appAddress(address, on, names) {
  if (!address) return "";
  const i = address.indexOf("/");
  if (i < 0) return on ? `${on}/${address}` : address;
  const machine = address.slice(0, i);
  // The prefix is the target machine's name for the sender's machine. If this app has no
  // machine of that name, the sender is on this app's own machine.
  return names.has(machine) ? address : address.slice(i + 1);
}

function when(ts) {
  const d = new Date(ts * 1000);
  const now = new Date();
  const hm = d.toTimeString().slice(0, 5);
  if (d.toDateString() === now.toDateString()) return hm;
  return `${d.getDate()}/${d.getMonth() + 1} ${hm}`;
}

export default class SheprdMessages extends View {
  init(_props, cx) {
    this.view = stored("view", "agent") === "all" ? "all" : "agent";
    this.byMachine = {}; // machine key -> { messages, mode }
    this.errors = {};
    this.open = new Set();
    this.seenAll = Number(stored("seenAll", "0")) || 0;
    this.loading = false;
    this.note = null;
    this.load(cx);
    cx.timer.every(EVERY_MS, (cx) => this.load(cx));
  }

  load(cx) {
    if (this.loading) return;
    this.loading = true;
    cx.spawn(async (cx) => {
      try {
        const list = machines().filter((m) => m.layout !== "unavailable");
        const got = await Promise.all(list.map(async (m) => {
          const key = m.name ?? "local";
          const r = await inbox(["log", "--json", "--all", "--limit", String(LIMIT)], m.name);
          if (r && r.code === 0) {
            try {
              return [key, JSON.parse(r.stdout), null];
            } catch {
              return [key, null, "unreadable answer"];
            }
          }
          return [key, null, failure(r)];
        }));
        const next = {};
        const errors = {};
        for (const [key, doc, err] of got) {
          if (doc) next[key] = doc;
          else errors[key] = err;
        }
        const changed = JSON.stringify(next) !== JSON.stringify(this.byMachine) || JSON.stringify(errors) !== JSON.stringify(this.errors);
        this.byMachine = next;
        this.errors = errors;
        if (changed) cx.notify();
      } finally {
        this.loading = false;
      }
    });
  }

  // Every message, newest first, with app-relative `to`/`from`.
  messages() {
    const names = new Set(machines().map((m) => m.name).filter(Boolean));
    const out = [];
    for (const [key, doc] of Object.entries(this.byMachine)) {
      const on = key === "local" ? null : key;
      for (const m of doc.messages ?? []) {
        out.push({ ...m, to: appAddress(m.to, on, names), from: appAddress(m.from, on, names), id: `${key}:${m.to}:${m.ts}:${out.length}` });
      }
    }
    return out.sort((a, b) => b.ts - a.ts);
  }

  // compact on every machine that answered, unless one is in full mode.
  mode() {
    const modes = Object.values(this.byMachine).map((d) => d.mode);
    return modes.includes("full") ? "full" : "compact";
  }

  setMode(cx, mode) {
    cx.spawn(async (cx) => {
      const list = machines().filter((m) => m.layout !== "unavailable");
      const results = await Promise.all(list.map((m) => inbox(["mode", mode], m.name).then((r) => [m.name ?? "here", r])));
      const bad = results.filter(([, r]) => !r || r.code !== 0).map(([n, r]) => `${n}: ${failure(r)}`);
      this.note = bad.length ? bad.join("; ") : null;
      this.load(cx);
      cx.notify();
    });
  }

  label(address) {
    if (!address) return "someone";
    const a = agents().find((x) => x.address === address);
    const name = terminals().find((x) => x.address === address)?.name;
    return name || a?.space_name || address;
  }

  focusLink(t, id, address) {
    const a = agents().find((x) => x.address === address);
    return div().id(id).font_semibold().text_size(12).truncate()
      .when(Boolean(a), (el) => el.cursor_pointer().hover((s) => s.text_color(t.accent)).on_click(() => run("pane.focus", { pane: a.pane })))
      .child(this.label(address));
  }

  row(t, m, showTo, unread) {
    const long = m.body.length > FOLD;
    const open = this.open.has(m.id);
    const body = long && !open ? m.body.slice(0, FOLD).replace(/\s+\S*$/, "") + " …" : m.body;
    return v_flex().id(`m-${m.id}`).gap(4).p(8).rounded(8).bg(t.background).border_1().border_color(unread ? t.accent + "88" : t.border)
      .child(h_flex().gap(5).items_center().min_w(0)
        .child(this.focusLink(t, `mf-${m.id}`, m.from))
        .when(showTo, (el) => el.child(div().text_size(11).text_color(t.dim).child("→")).child(this.focusLink(t, `mt-${m.id}`, m.to)))
        .child(div().flex_1())
        .child(div().flex_none().text_size(10.5).text_color(t.dim).child(when(m.ts))))
      .child(div().text_size(12).text_color(t.muted).child(TextView.markdown(`mb-${m.id}-${open ? 1 : 0}`, body).selectable(true)))
      .when(long, (el) => el.child(div().id(`mo-${m.id}`).text_size(11).text_color(t.dim).cursor_pointer().hover((s) => s.text_color(t.foreground))
        .on_click((_e, cx) => { open ? this.open.delete(m.id) : this.open.add(m.id); cx.notify(); })
        .child(open ? "less" : "more")));
  }

  render(cx) {
    const t = theme();
    const all = this.messages();
    const address = focused()?.agent ?? null;
    const newest = all[0]?.ts ?? 0;
    const unreadAll = all.filter((m) => m.ts > this.seenAll).length;
    if (this.view === "all" && newest > this.seenAll) {
      const before = this.seenAll;
      this.seenAll = newest;
      store("seenAll", String(newest));
      this.unreadBefore = before;
    }

    const tab = (id, label, view) => {
      const on = this.view === view;
      return div().id(id).px(4).pb(5).text_size(12.5).cursor_pointer()
        .font_medium().text_color(on ? t.foreground : t.dim).hover((s) => s.text_color(t.foreground))
        .border_b_2().border_color(on ? t.accent : t.chrome)
        .on_click((_e, cx) => { this.unreadBefore = this.seenAll; this.view = view; store("view", view); cx.notify(); })
        .child(label);
    };
    const full = this.mode() === "full";
    const toggle = div().id("mode").px(7).py(2).rounded(5).text_size(11).cursor_pointer().border_1()
      .border_color(full ? t.blocked + "88" : t.border).text_color(full ? t.blocked : t.dim)
      .hover((s) => s.text_color(t.foreground))
      .on_click((_e, cx) => this.setMode(cx, full ? "compact" : "full"))
      .child(full ? "Typed into prompts" : "In background");
    const tabs = h_flex().px(12).pt(10).mb(8).gap(14).flex_none().items_end().border_b_1().border_color(t.border)
      .child(tab("v-agent", "This agent", "agent"))
      .child(tab("v-all", unreadAll && this.view !== "all" ? `All ${unreadAll}` : "All", "all"))
      .child(div().flex_1())
      .child(div().pb(4).child(toggle));

    let body;
    const errors = Object.entries(this.errors);
    if (this.view === "agent" && !address) {
      body = div().text_size(12).text_color(t.dim).child("Focus an agent to see the messages other agents sent it.");
    } else {
      const shown = this.view === "agent" ? all.filter((m) => m.to === address || m.from === address) : all;
      const seen = this.unreadBefore ?? this.seenAll;
      body = v_flex().gap(8)
        .when(this.view === "agent", (el) => el.child(div().text_size(13).font_semibold().truncate().child(this.label(address))))
        .when(full, (el) => el.child(div().text_size(11.5).text_color(t.dim)
          .child("Background delivery is off: new messages are typed into the agents' prompts and show in their transcripts.")))
        .children(shown.length
          ? shown.map((m) => this.row(t, m, this.view === "all" || m.from === address, this.view === "all" && m.ts > seen))
          : [div().text_size(11.5).text_color(t.dim).child(this.view === "agent" ? "No messages for this agent yet." : "No messages yet.")])
        .when(errors.length > 0, (el) => el.child(v_flex().pt(6).gap(2).children(errors.map(([m, e]) =>
          div().id(`err-${m}`).text_size(10.5).text_color(t.dim).child(`${m === "local" ? "here" : m}: ${e}`)))));
    }

    return v_flex().size_full().min_h(0).bg(t.chrome).text_color(t.foreground).text_size(13)
      .child(tabs)
      .when(Boolean(this.note), (el) => el.child(div().id("note").mx(10).mb(4).p(6).rounded(6).bg(t.blocked + "1f").text_size(11.5).text_color(t.blocked)
        .cursor_pointer().on_click((_e, cx) => { this.note = null; cx.notify(); }).child(this.note)))
      .child(v_flex().id("messages-body").flex_1().min_h(0).px(10).pb(10).overflow_y_scrollbar().child(body));
  }
}
