// sheprd-doc: a document an agent wrote for you, rendered beside it. The agent
// runs `sheprd-doc <file>` (or `cards doc <file>`), which records the file in
// its machine's card ledger; the sidebar sees the new entry and splits the
// agent's pane with this mod (props { address }). While this pane is open, the
// agent's next document replaces this one here instead of opening another split.
// The file is re-read every few seconds, so edits show up.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, TextView } from "gpui-base";
import { props, shared, setShared, readTextFile, theme, run } from "herdr";

const EVERY_MS = 2500;

// `dev/t7` → { on: "dev", terminal: "t7" }; `t7` → { on: null, terminal: "t7" }.
function split(address) {
  const i = address.indexOf("/");
  return i < 0 ? { on: null, terminal: address } : { on: address.slice(0, i), terminal: address.slice(i + 1) };
}

export default class SheprdDoc extends View {
  init(_props, cx) {
    this.address = props()?.address ?? "";
    this.text = null;
    this.path = null;
    this.version = 0;
    this.error = null;
    this.reading = false;
    this.beat();
    this.load(cx);
    cx.timer.every(EVERY_MS, (cx) => { this.beat(); this.load(cx); });
  }

  // Tells the sidebar this agent's document pane is open (see sheprd-sidebar openDocs).
  beat() {
    if (!this.address) return;
    const open = { .../** @type {Record<string, number>} */ (shared("sheprd.doc.open") ?? {}) };
    open[this.address] = Date.now();
    setShared("sheprd.doc.open", open);
  }

  current() {
    const { on, terminal } = split(this.address);
    const doc = /** @type {any} */ (shared("sheprd.cards") ?? {})[on ?? "local"];
    return doc?.docs?.[terminal] ?? null;
  }

  load(cx) {
    const d = this.current();
    if (!d || this.reading) return;
    this.reading = true;
    const { on } = split(this.address);
    cx.spawn(async (cx) => {
      try {
        const text = await readTextFile(d.path, { on });
        if (text !== this.text || d.path !== this.path) {
          this.text = text;
          this.path = d.path;
          this.version += 1;
          this.error = null;
          cx.notify();
        }
      } catch (err) {
        const msg = `Could not read ${d.path}: ${String(err)}`;
        if (msg !== this.error) {
          this.error = msg;
          cx.notify();
        }
      } finally {
        this.reading = false;
      }
    });
  }

  render() {
    const t = theme();
    const path = this.path ?? this.current()?.path ?? "";
    const slash = path.lastIndexOf("/");
    const name = slash >= 0 ? path.slice(slash + 1) : path;
    const folder = slash > 0 ? path.slice(0, slash) : "";
    const { on } = split(this.address);
    const edit = div().id("doc-edit").flex_none().px(7).py(2).rounded(5).text_size(11).cursor_pointer()
      .border_1().border_color(t.border).text_color(t.dim).hover((s) => s.text_color(t.foreground))
      .on_click(() => path && run("terminal.open", {
        place: "down",
        folder: folder || undefined,
        argv: ["sh", "-c", 'exec ${VISUAL:-${EDITOR:-nvim}} "$1"', "sh", path],
      }))
      .child("Edit");
    const header = h_flex().px(12).py(8).gap(8).flex_none().items_center().border_b_1().border_color(t.border)
      .child(v_flex().flex_1().min_w(0)
        .child(div().text_size(12.5).font_semibold().truncate().child(name || "Document"))
        .child(div().text_size(10.5).text_color(t.dim).truncate().child((on ? `${on}: ` : "") + folder)))
      .when(Boolean(path), (el) => el.child(edit));
    const body = this.error
      ? div().text_size(12).text_color(t.blocked).child(this.error)
      : this.text === null
        ? div().text_size(12).text_color(t.dim).child(path ? `Loading ${name}…` : "No document yet.")
        : TextView.markdown(`doc-${this.version}`, this.text || "_(empty)_").selectable(true);
    return v_flex().size_full().min_h(0).bg(t.background).text_color(t.foreground).text_size(13)
      .child(header)
      .child(v_flex().id("doc-body").flex_1().min_h(0).px(14).py(10).overflow_y_scrollbar().child(body));
  }
}
