// sheprd's work panel: the focused agent's cards, each with its summary first,
// then its latest decisions, then its open questions with answer buttons; what
// the agent printed last stays folded until asked for. Or every agent's cards. Cards come from each machine's `sheprd-cards`
// herdr-core document through the `cards` CLI (herdr-app/CONTRACT.md in the
// sheprd repo). Answers are staged per question, then sent as one line with
// `cards send`, which types them into the agent with `hc send`.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { agents, machines, focused, run, theme, shared, setShared, exec, hc } from "herdr";

const OUTPUT_LINES = 15;
// Decisions shown before "N more".
const DECISIONS_SHOWN = 4;
const OUTPUT_EVERY_MS = 3000;
// The sidebar loads cards every 5 s; the panel loads them itself only when that stops.
const CARDS_STALE_MS = 12000;

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

// `dev/t7` → { on: "dev", terminal: "t7" }; `t7` → { on: null, terminal: "t7" }.
function split(address) {
  const i = address.indexOf("/");
  return i < 0 ? { on: null, terminal: address } : { on: address.slice(0, i), terminal: address.slice(i + 1) };
}

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

// The sidebar's short machine tag (CONTRACT.md: machine_tags, else the name's
// first part, cut to 3 letters when longer than 5).
function machineTag(name) {
  const tag = /** @type {any} */ (shared("sheprd.view") ?? {}).machine_tags?.[name];
  if (tag) return tag;
  const first = name.split(/[-_.\s]/)[0] || name;
  return first.length > 5 ? first.slice(0, 3) : first;
}

const live = (q) => q.state === "open" || q.state === "staged" || q.state === "sending";

export default class SheprdPanel extends View {
  init(_props, cx) {
    this.mode = localStorage.getItem("mode") ?? "agent";
    this.output = "";
    this.outputFor = null;
    this.openDecisions = new Set();
    this.showOutput = localStorage.getItem("output") === "1";
    this.editing = null;
    this.note = null;
    this.input = InputState.new({ value: "" });
    this.input.on("submit", (_e, cx) => this.submitEdit(cx));
    cx.timer.every(OUTPUT_EVERY_MS, (cx) => this.readOutput(cx));
    cx.timer.every(CARDS_STALE_MS, (cx) => {
      const at = /** @type {number} */ (shared("sheprd.cards.at") ?? 0);
      if (Date.now() - at > CARDS_STALE_MS) this.loadCards(cx);
    });
    this.loadCards(cx);
  }

  // `only`: just that machine (after an action there), even while a full load runs.
  loadCards(cx, only) {
    const full = only === undefined;
    if (full && this.loading) return;
    if (full) this.loading = true;
    cx.spawn(async (cx) => {
      try {
        const list = machines().filter((m) => full || (m.name ?? null) === only);
        const docs = { .../** @type {object} */ (shared("sheprd.cards") ?? {}) };
        const errs = { .../** @type {object} */ (shared("sheprd.cards.err") ?? {}) };
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
        setShared("sheprd.cards", docs);
        setShared("sheprd.cards.err", errs);
        if (full) setShared("sheprd.cards.at", Date.now());
      } finally {
        if (full) this.loading = false;
      }
      cx.notify();
    });
  }

  readOutput(cx) {
    const address = focused()?.agent;
    if (!address || this.mode !== "agent" || !this.showOutput) return;
    cx.spawn(async (cx) => {
      try {
        const r = await hc(["read", address, "--lines", String(OUTPUT_LINES)]);
        const text = r.code === 0 ? r.stdout.replace(/\s+$/, "") : "";
        if (text !== this.output || address !== this.outputFor) {
          this.output = text;
          this.outputFor = address;
          cx.notify();
        }
      } catch {
        // The machine went away; keep the last screen.
      }
    });
  }

  // Runs `cards <args>` on the agent's machine, then reloads that machine's cards.
  act(cx, address, args) {
    const { on } = split(address);
    cx.spawn(async (cx) => {
      try {
        const r = await runCards(args, on);
        this.note = r && r.code === 0 ? null : cardsFailure(r).replace(/^cards: /, "");
      } catch (err) {
        this.note = String(err);
      }
      this.loadCards(cx, on);
      cx.notify();
    });
  }

  stage(cx, address, card, q, answer) {
    this.act(cx, address, ["stage", split(address).terminal, card.id, String(q.n), answer]);
  }

  submitEdit(cx) {
    const e = this.editing;
    const text = this.input.value().trim();
    this.editing = null;
    this.input.set_value("");
    if (e && text) this.stage(cx, e.address, e.card, e.q, text);
    cx.notify();
  }

  // Machines whose cards could not be read, as text to show next to (or instead of) an empty list.
  failures() {
    const errs = /** @type {Record<string, string>} */ (shared("sheprd.cards.err") ?? {});
    const list = Object.entries(errs).map(([machine, why]) => `cards not available on ${machine === "local" ? "this machine" : machine} (${why})`);
    return list.length ? ` ${list.join("; ")}.` : "";
  }

  noCards(address) {
    const errs = /** @type {Record<string, string>} */ (shared("sheprd.cards.err") ?? {});
    const machine = split(address).on ?? "local";
    return errs[machine]
      ? `cards not available on ${machine === "local" ? "this machine" : machine} (${errs[machine]})`
      : "No cards from this agent.";
  }

  // ---- pieces ---------------------------------------------------------------

  button(t, id, label, action, { primary = false, active = false, dim = false } = {}) {
    return h_flex().id(id).h(24).px(8).rounded(6).items_center().text_size(11.5)
      .bg(primary ? t.solid : active ? t.selected : t.raised)
      .text_color(primary ? t.solid_foreground : dim ? t.dim : t.foreground)
      .border_1().border_color(active ? t.accent : primary ? t.solid : t.border)
      .cursor_pointer().hover((s) => s.opacity(0.85))
      .on_click((_e, cx) => { cx.stop_propagation(); action(cx); })
      .child(label);
  }

  // An answer option: as wide as the panel, as tall as its text.
  option(t, id, label, action, active) {
    return h_flex().id(id).w_full().min_h(24).px(8).py(4).rounded(6).items_start().text_size(11.5)
      .bg(active ? t.selected : t.raised).text_color(t.foreground)
      .border_1().border_color(active ? t.accent : t.border)
      .cursor_pointer().hover((s) => s.opacity(0.85))
      .on_click((_e, cx) => { cx.stop_propagation(); action(cx); })
      .child(div().flex_1().min_w(0).child(label));
  }

  question(t, address, card, q) {
    const key = `${address}/${card.id}/${q.n}`;
    const editing = this.editing?.key === key;
    return v_flex().gap(6).p(8).rounded(8).bg(t.background).border_1().border_color(q.state === "staged" ? t.accent : t.border)
      .child(h_flex().gap(6).items_center()
        .child(div().flex_none().text_size(11).font_semibold().text_color(t.blocked).child(`Q${q.n}`))
        .child(div().flex_1().min_w(0).text_size(12.5).child(q.text))
        .child(this.button(t, `qd-${key}`, "✕", (cx) => this.act(cx, address, ["dismiss", split(address).terminal, card.id, "--question", String(q.n)]), { dim: true })))
      // One option per row, its text wrapping: a long option used to run past the panel's edge.
      .child(v_flex().gap(4)
        .children(q.options.map((o, i) => this.option(t, `qo-${key}-${i}`, q.recommend === i + 1 ? `★ ${o}` : o,
          (cx) => this.stage(cx, address, card, q, o), q.state === "staged" && q.answer === o)))
        .child(h_flex().child(this.button(t, `qe-${key}`, "✎", (cx) => {
          this.editing = { key, address, card, q };
          this.input.set_value(q.state === "staged" && !q.options.includes(q.answer) ? q.answer : "");
          this.input.focus();
          cx.notify();
        }, { active: editing }))))
      .when(editing, (el) => el.child(Input.new(this.input).h(30).px(8).rounded(6).border_1().border_color(t.accent).bg(t.chrome).text_size(12.5)))
      .when(q.state === "staged" && !q.options.includes(q.answer) && !editing, (el) => el.child(div().text_size(11.5).text_color(t.accent).child(`→ ${q.answer}`)))
      .when(q.state === "sending", (el) => el.child(div().text_size(11.5).text_color(t.dim)
        .child(`Sending → ${q.answer}. If this stays for 2 minutes the send died: choose the answer again to resend.`)));
  }

  card(t, address, card) {
    const qs = card.questions.filter(live);
    const key = `${address}/${card.id}`;
    const allDecisions = this.openDecisions.has(key);
    // Newest first; the rest behind "N more".
    const decisions = [...card.decisions].reverse();
    const shownDecisions = allDecisions ? decisions : decisions.slice(0, DECISIONS_SHOWN);
    const label = (text) => div().pt(2).text_size(10.5).font_medium().text_color(t.dim).child(text);
    return v_flex().gap(6).p(10).rounded(10).bg(t.surface).border_1().border_color(qs.length ? t.blocked + "66" : t.border)
      .when(card.done, (el) => el.opacity(0.7))
      .child(h_flex().gap(6).items_center()
        .child(div().flex_none().text_size(11).font_semibold().text_color(t.accent).child(card.id))
        .child(div().flex_1())
        .when(Boolean(card.status), (el) => el.child(div().flex_none().px(6).h(18).rounded(5).bg(t.raised).text_size(10.5).text_color(t.muted).flex().items_center().child(card.status))))
      .when(Boolean(card.title), (el) => el.child(div().text_size(13).font_semibold().child(card.title)))
      .when(Boolean(card.summary), (el) => el.child(div().text_size(12.5).text_color(t.foreground).child(card.summary)))
      .when(decisions.length > 0, (el) => el
        .child(label("Decisions"))
        .children(shownDecisions.map((d, i) => h_flex().id(`d-${key}-${i}`).gap(6).items_start().text_size(11.5).text_color(t.muted)
          .child(div().flex_none().child("•")).child(div().flex_1().min_w(0).child(d.text))))
        .when(decisions.length > DECISIONS_SHOWN, (el) => el.child(div().id(`dc-${key}`).text_size(11).text_color(t.dim).cursor_pointer()
          .hover((s) => s.text_color(t.foreground))
          .on_click((_e, cx) => { allDecisions ? this.openDecisions.delete(key) : this.openDecisions.add(key); cx.notify(); })
          .child(allDecisions ? "Fewer" : `${decisions.length - DECISIONS_SHOWN} more`))))
      .when(qs.length > 0, (el) => el.child(label(qs.length === 1 ? "Question" : `${qs.length} questions`)))
      .children(qs.map((q) => this.question(t, address, card, q)))
      .when(Boolean(card.link), (el) => el.child(div().id(`ln-${key}`).text_size(11.5).text_color(t.accent).truncate().cursor_pointer()
        .on_click(() => exec("xdg-open", [card.link]).catch(() => {})).child(card.link)))
      .when(Boolean(card.writeback), (el) => el.child(div().text_size(10.5).text_color(card.writeback.ok ? t.dim : t.blocked).child(`${card.writeback.ok ? "✓" : "!"} ${card.writeback.message}`)));
  }

  // One agent's cards, with its Send button when answers are staged.
  agentBlock(t, address, cards, compact) {
    const staged = cards.flatMap((c) => c.questions.filter((q) => q.state === "staged")).length;
    const a = agents().find((x) => x.address === address);
    return v_flex().gap(8)
      .when(compact, (el) => el.child(h_flex().id(`ab-${address}`).gap(6).items_center().cursor_pointer()
        .on_click(() => a && run("pane.focus", { pane: a.pane }))
        .child(div().text_size(12).font_semibold().child(a?.space_name || address))
        .child(div().text_size(10.5).text_color(t.dim).child(split(address).on ? machineTag(split(address).on) : "here"))))
      .children(cards.map((c) => this.card(t, address, c)))
      .when(staged > 0, (el) => el.child(h_flex().justify_end()
        .child(this.button(t, `send-${address}`, `Send ${staged} answer${staged === 1 ? "" : "s"}`,
          (cx) => this.act(cx, address, ["send", split(address).terminal]), { primary: true }))));
  }

  render(cx) {
    const t = theme();
    const byAddress = cardsByAddress();
    const visible = (cards) => cards.filter((c) => !c.dismissed).sort((x, y) => (x.done - y.done) || y.updated_at.localeCompare(x.updated_at));
    const address = focused()?.agent ?? null;
    const agent = address ? agents().find((x) => x.address === address) : null;

    const tabs = h_flex().px(10).pt(10).pb(6).gap(4).flex_none().items_center()
      .child(this.button(t, "mode-agent", "This agent", (cx) => { this.mode = "agent"; localStorage.setItem("mode", "agent"); this.readOutput(cx); cx.notify(); }, { active: this.mode === "agent" }))
      .child(this.button(t, "mode-all", "All cards", (cx) => { this.mode = "all"; localStorage.setItem("mode", "all"); cx.notify(); }, { active: this.mode === "all" }));

    let body;
    if (this.mode === "all") {
      const entries = [...byAddress.entries()]
        .map(([addr, cards]) => [addr, visible(cards).filter((c) => !c.done || c.questions.some(live))])
        .filter(([, cards]) => cards.length > 0)
        .sort(([, x], [, y]) => y.flatMap((c) => c.questions.filter(live)).length - x.flatMap((c) => c.questions.filter(live)).length);
      body = entries.length === 0
        ? div().text_size(12).text_color(t.dim).child(`No open cards. Agents add them with \`cards update\`.${this.failures()}`)
        : v_flex().gap(14).children(entries.map(([addr, cards]) => this.agentBlock(t, addr, cards, true)))
          .when(this.failures() !== "", (el) => el.child(div().text_size(11).text_color(t.dim).child(this.failures().trim())));
    } else if (!address) {
      body = div().text_size(12).text_color(t.dim).child("Focus an agent to see its cards and output.");
    } else {
      const cards = visible(byAddress.get(address) ?? []);
      const lines = this.outputFor === address && this.output ? this.output.split("\n") : [];
      body = v_flex().gap(10)
        .child(h_flex().gap(6).items_center()
          .child(div().flex_1().min_w(0).truncate().text_size(13).font_semibold().child(agent?.space_name || address))
          .child(div().flex_none().text_size(10.5).text_color(t.dim).child(split(address).on ? machineTag(split(address).on) : "here")))
        .child(cards.length ? this.agentBlock(t, address, cards, false) : div().text_size(11.5).text_color(t.dim).child(this.noCards(address)))
        .child(div().id("out-toggle").pt(6).text_size(11).font_medium().text_color(t.dim).cursor_pointer().hover((s) => s.text_color(t.foreground))
          .on_click((_e, cx) => {
            this.showOutput = !this.showOutput;
            localStorage.setItem("output", this.showOutput ? "1" : "0");
            this.readOutput(cx);
            cx.notify();
          })
          .child(`${this.showOutput ? "▾" : "▸"} Last output`))
        .when(this.showOutput, (el) => el.child(v_flex().p(8).rounded(8).bg(t.background).border_1().border_color(t.border).text_size(11).text_color(t.muted)
          .children(lines.length ? lines.filter((l, i) => l.trim() || (i > 0 && lines[i - 1].trim())).map((l, i) => div().id(`o-${i}`).truncate().child(l || " ")) : [div().text_color(t.dim).child("…")])));
    }

    return v_flex().size_full().min_h(0).bg(t.chrome).text_color(t.foreground).text_size(13)
      .child(tabs)
      .when(Boolean(this.note), (el) => el.child(div().id("note").mx(10).mb(4).p(6).rounded(6).bg(t.blocked + "1f").text_size(11.5).text_color(t.blocked)
        .cursor_pointer().on_click((_e, cx) => { this.note = null; cx.notify(); }).child(this.note)))
      .child(v_flex().id("panel-body").flex_1().min_h(0).px(10).pb(10).overflow_y_scrollbar().child(body));
  }
}
