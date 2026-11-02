// sheprd's leader key for the Herdr app, which has no prefix keys: ctrl+space
// (keys.json: "ctrl-space": "mod.open:sheprd-leader") opens this, and the next
// key runs the same action as sheprd's prefix+<key>. Esc or any other key closes.

import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { run, theme, props, closeOverlay, layout, shared, setShared, window } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sheprd layout's sidebar goes to its rail and back (as sheprd-chrome does);
// another layout's sidebar just hides.
function toggleSidebar() {
  const sidebar = /** @type {any} */ (find(layout(), "sidebar"));
  if (!sidebar) return;
  if (shared("layout") !== "sheprd") return run("layout.toggle", { id: "sidebar" });
  const narrow = (sidebar.size?.px ?? Infinity) < 160;
  if (!narrow) setShared("sidebar.width", sidebar.size?.px);
  setShared("sidebar.collapsed", !narrow);
  run("layout.resize", { id: "sidebar", px: narrow ? (shared("sidebar.width") ?? 300) : window()?.platform === "macos" ? 76 : 44 });
}

// key (shifted letters in capitals) -> [label, action], grouped as shown.
/** @type {[string, [string, string, (() => unknown) | null][]][]} */
const GROUPS = [
  ["Tabs", [
    ["c", "new tab", () => run("tab.new")],
    ["r", "rename tab", () => run("tab.rename")],
    ["k", "close tab", () => run("tab.close")],
    ["n", "next tab", () => run("tab.next")],
    ["p", "previous tab", () => run("tab.prev")],
    ["1-9", "tab N", null],
  ]],
  ["Spaces", [
    ["C", "new space", () => run("space.new")],
    ["R", "rename space", () => run("space.rename")],
    ["K", "close space", () => run("space.close")],
    ["N", "next space", () => run("space.next")],
    ["P", "previous space", () => run("space.prev")],
  ]],
  ["Panes", [
    ["h", "split right", () => run("pane.split", { direction: "right" })],
    ["v", "split down", () => run("pane.split", { direction: "down" })],
    ["x", "close pane", () => run("pane.close")],
    ["z", "zoom pane", () => run("pane.zoom")],
  ]],
  ["Agents and app", [
    ["a", "next agent", () => run("agent.next")],
    ["A", "previous agent", () => run("agent.prev")],
    ["g", "go to (picker)", () => run("mod.open", { name: "picker" })],
    ["f", "go to (picker)", () => run("mod.open", { name: "picker" })],
    ["b", "sidebar", toggleSidebar],
    ["s", "settings", () => run("app.settings")],
    ["?", "help", () => run("app.help")],
  ]],
];
const ACTIONS = new Map(GROUPS.flatMap(([, keys]) => keys.filter(([, , act]) => act).map(([k, , act]) => [k, act])));

export default class SheprdLeader extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    this.handle.focus();
  }

  render() {
    const t = theme();
    const close = () => closeOverlay(props()?.overlay);
    const entry = ([k, label, _act]) => h_flex().h(22).gap(10).items_center()
      .child(div().w(34).flex_none().px(5).h(18).rounded(4).bg(t.raised).text_size(11).font_semibold().text_color(t.foreground).flex().items_center().justify_center().child(k))
      .child(div().text_size(12).text_color(t.muted).child(label));
    return v_flex().id("sheprd-leader").size_full().p(12).gap(2).bg(t.popover).text_color(t.foreground)
      .track_focus(this.handle)
      .on_key_down((e) => {
        if (e.modifiers.control || e.modifiers.platform || e.modifiers.alt) return;
        let key = e.key;
        if (key.length === 1 && e.modifiers.shift) key = key === "/" ? "?" : key.toUpperCase();
        close();
        if (/^[1-9]$/.test(key)) return run("tab.select", { index: Number(key) });
        ACTIONS.get(key)?.();
      })
      .child(div().pb(4).text_size(11.5).text_color(t.dim).child("ctrl+space, then a key (Esc closes)"))
      .children(GROUPS.flatMap(([title, keys]) => [
        div().pt(6).text_size(10.5).font_medium().text_color(t.dim).child(title),
        ...keys.filter(([k], i, all) => !(k === "f" && all.some(([x]) => x === "g"))).map(entry),
      ]));
  }
}
