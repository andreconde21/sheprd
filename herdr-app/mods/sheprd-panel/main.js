// sheprd's work panel: the focused agent's cards (status, summary, decisions),
// its open questions with answer buttons, and what it printed last; or every
// agent's cards at once. Cards come from each machine's `sheprd-cards`
// herdr-core document through the `cards` CLI (herdr-app/CONTRACT.md in the
// sheprd repo). Answers are staged per question, then sent as one line with
// `cards send`, which types them into the agent with `hc send`.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { agents, machines, focused, run, theme, shared, setShared, exec, hc } from "herdr";

const OUTPUT_LINES = 40;
const OUTPUT_EVERY_MS = 3000;
// The sidebar loads cards every 5 s; the panel loads them itself only when that stops.
const CARDS_STALE_MS = 12000;

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

const live = (q) => q.state === "open" || q.state === "staged" || q.state === "sending";

export default class SheprdPanel extends View {
  init(_props, cx) {
    this.mode = localStorage.getItem("mode") ?? "agent";
    this.output = "";
    this.outputFor = null;
    this.openDecisions = new Set();
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
        const got = await Promise.all(list.filter((m) => m.layout !== "unavailable").map(async (m) => {
          try {
            const r = await exec("cards", ["json"], { on: m.name });
            return r.code === 0 ? [m.name ?? "local", JSON.parse(r.stdout)] : null;
          } catch {
            return null; // not installed there, or the machine went away
          }
        }));
        for (const entry of got) if (entry) docs[entry[0]] = entry[1];
        setShared("sheprd.cards", docs);
        if (full) setShared("sheprd.cards.at", Date.now());
      } finally {
        if (full) this.loading = false;
      }
      cx.notify();
    });
  }

  readOutput(cx) {
    const address = focused()?.agent;
    if (!address || this.mode !== "agent") return;
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
        const r = await exec("cards", args, { on });
        this.note = r.code === 0 ? null : (r.stderr || r.stdout).trim().replace(/^cards: /, "");
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

  question(t, address, card, q) {
    const key = `${address}/${card.id}/${q.n}`;
    const editing = this.editing?.key === key;
    return v_flex().gap(6).p(8).rounded(8).bg(t.background).border_1().border_color(q.state === "staged" ? t.accent : t.border)
      .child(h_flex().gap(6).items_center()
        .child(div().flex_none().text_size(11).font_semibold().text_color(t.blocked).child(`Q${q.n}`))
        .child(div().flex_1().min_w(0).text_size(12.5).child(q.text))
        .child(this.button(t, `qd-${key}`, "✕", (cx) => this.act(cx, address, ["dismiss", split(address).terminal, card.id, "--question", String(q.n)]), { dim: true })))
      .child(h_flex().gap(6).flex_wrap()
        .children(q.options.map((o, i) => this.button(t, `qo-${key}-${i}`, q.recommend === i + 1 ? `★ ${o}` : o,
          (cx) => this.stage(cx, address, card, q, o), { active: q.state === "staged" && q.answer === o })))
        .child(this.button(t, `qe-${key}`, "✎", (cx) => {
          this.editing = { key, address, card, q };
          this.input.set_value(q.state === "staged" && !q.options.includes(q.answer) ? q.answer : "");
          this.input.focus();
          cx.notify();
        }, { active: editing })))
      .when(editing, (el) => el.child(Input.new(this.input).h(30).px(8).rounded(6).border_1().border_color(t.accent).bg(t.chrome).text_size(12.5)))
      .when(q.state === "staged" && !q.options.includes(q.answer) && !editing, (el) => el.child(div().text_size(11.5).text_color(t.accent).child(`→ ${q.answer}`)))
      .when(q.state === "sending", (el) => el.child(div().text_size(11.5).text_color(t.dim)
        .child(`Sending → ${q.answer}. If this stays for 2 minutes the send died: choose the answer again to resend.`)));
  }

  card(t, address, card) {
    const qs = card.questions.filter(live);
    const key = `${address}/${card.id}`;
    const showDecisions = this.openDecisions.has(key);
    return v_flex().gap(6).p(10).rounded(10).bg(t.surface).border_1().border_color(t.border)
      .when(card.done, (el) => el.opacity(0.7))
      .child(h_flex().gap(6).items_center()
        .child(div().flex_none().text_size(11).font_semibold().text_color(t.accent).child(card.id))
        .child(div().flex_1().min_w(0).truncate().text_size(12.8).font_medium().child(card.title ?? ""))
        .when(Boolean(card.status), (el) => el.child(div().flex_none().px(6).h(18).rounded(5).bg(t.raised).text_size(10.5).text_color(t.muted).flex().items_center().child(card.status))))
      .when(Boolean(card.summary), (el) => el.child(div().text_size(12).text_color(t.muted).child(card.summary)))
      .when(Boolean(card.link), (el) => el.child(div().id(`ln-${key}`).text_size(11.5).text_color(t.accent).truncate().cursor_pointer()
        .on_click(() => exec("xdg-open", [card.link]).catch(() => {})).child(card.link)))
      .children(qs.map((q) => this.question(t, address, card, q)))
      .when(card.decisions.length > 0, (el) => el.child(div().id(`dc-${key}`).text_size(11).text_color(t.dim).cursor_pointer()
        .on_click((_e, cx) => { showDecisions ? this.openDecisions.delete(key) : this.openDecisions.add(key); cx.notify(); })
        .child(`${showDecisions ? "▾" : "▸"} ${card.decisions.length} decision${card.decisions.length === 1 ? "" : "s"}`)))
      .when(showDecisions, (el) => el.children(card.decisions.map((d, i) => div().id(`d-${key}-${i}`).pl(10).text_size(11.5).text_color(t.muted).child(`• ${d.text}`))))
      .when(Boolean(card.writeback), (el) => el.child(div().text_size(10.5).text_color(card.writeback.ok ? t.dim : t.blocked).child(`${card.writeback.ok ? "✓" : "!"} ${card.writeback.message}`)));
  }

  // One agent's cards, with its Send button when answers are staged.
  agentBlock(t, address, cards, compact) {
    const staged = cards.flatMap((c) => c.questions.filter((q) => q.state === "staged")).length;
    const a = agents().find((x) => x.address === address);
    return v_flex().gap(8)
      .when(compact, (el) => el.child(h_flex().id(`ab-${address}`).gap(6).items_center().cursor_pointer()
        .on_click(() => a && run("pane.focus", { pane: a.pane }))
        .child(div().text_size(12).font_semibold().child(a ? `${a.space_name} · ${a.tab_name}` : address))
        .child(div().text_size(10.5).text_color(t.dim).child(address))))
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
        ? div().text_size(12).text_color(t.dim).child("No open cards. Agents add them with `cards update`.")
        : v_flex().gap(14).children(entries.map(([addr, cards]) => this.agentBlock(t, addr, cards, true)));
    } else if (!address) {
      body = div().text_size(12).text_color(t.dim).child("Focus an agent to see its cards and output.");
    } else {
      const cards = visible(byAddress.get(address) ?? []);
      const lines = this.outputFor === address && this.output ? this.output.split("\n") : [];
      body = v_flex().gap(10)
        .child(h_flex().gap(6).items_center()
          .child(div().flex_1().min_w(0).truncate().text_size(13).font_semibold().child(agent ? `${agent.space_name} · ${agent.tab_name}` : address))
          .child(div().text_size(10.5).text_color(t.dim).child(address)))
        .child(cards.length ? this.agentBlock(t, address, cards, false) : div().text_size(11.5).text_color(t.dim).child("No cards from this agent."))
        .child(div().pt(6).text_size(11).font_medium().text_color(t.dim).child("Last output"))
        .child(v_flex().p(8).rounded(8).bg(t.background).border_1().border_color(t.border).text_size(11).text_color(t.muted)
          .children(lines.length ? lines.map((l, i) => div().id(`o-${i}`).truncate().child(l || " ")) : [div().text_color(t.dim).child("…")]));
    }

    return v_flex().size_full().min_h(0).bg(t.chrome).text_color(t.foreground).text_size(13)
      .child(tabs)
      .when(Boolean(this.note), (el) => el.child(div().id("note").mx(10).mb(4).p(6).rounded(6).bg(t.blocked + "1f").text_size(11.5).text_color(t.blocked)
        .cursor_pointer().on_click((_e, cx) => { this.note = null; cx.notify(); }).child(this.note)))
      .child(v_flex().id("panel-body").flex_1().min_h(0).px(10).pb(10).overflow_y_scrollbar().child(body));
  }
}
