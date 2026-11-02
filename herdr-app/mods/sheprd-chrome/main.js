// The sheprd layout's sidebar header: Herdr's workspace-chrome with the sheprd
// name and mark, and every sidebar control as an icon next to the sidebar
// toggle: Search, Settings, detailed/compact rows, Add machine. The layout has
// no footer.
// Those live in the hub's `sheprd-view` document (herdr-app/CONTRACT.md),
// written through the `cards` CLI like the sidebar does.
//
// Based on workspace-chrome from herdr-app (Apache-2.0).

import { View, div, image, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { theme, run, window, shared, setShared, updates, layout, exec } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sidebar's rail goes by its width, not a flag: dragging its edge narrows it too.
const narrow = () => (find(layout(), "sidebar")?.size?.px ?? Infinity) < 160;

// To the rail (on macOS wide enough for the traffic lights), or back to the width it had.
function toggleSidebar() {
  const width = find(layout(), "sidebar")?.size?.px;
  if (!narrow()) setShared("sidebar.width", width);
  setShared("sidebar.collapsed", !narrow());
  run("layout.resize", { id: "sidebar", px: narrow() ? (shared("sidebar.width") ?? 300) : window()?.platform === "macos" ? 76 : 44 });
}

// The same shared view the sheprd-sidebar mod shows; the change shows at once
// and the hub's document follows, one write at a time.
const view = () => /** @type {any} */ (shared("sheprd.view") ?? {});
let writes = Promise.resolve();
function saveView(patch) {
  setShared("sheprd.view", { ...view(), ...patch });
  writes = writes.then(() => exec("cards", ["view-set", JSON.stringify(patch)])).then(() => {}, () => {});
}

export default class SheprdChrome extends View {
  button(id, label, action, tip, on = false) {
    const t = theme();
    return div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6)
      .text_color(on ? t.foreground : t.muted).when(on, el => el.bg(t.selected))
      .hover(s => s.bg(t.hover).text_color(t.foreground)).tooltip(tip)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).on_click(action).child(label);
  }
  render() {
    const t = theme();
    const collapsed = narrow();
    const v = view();
    const detailed = v.density !== "compact";
    const mac = window()?.platform === "macos" && !window()?.fullscreen;
    const ready = Boolean(updates().app.ready);
    const toggle = this.button("sidebar-toggle", svg("mods/workspace-chrome/sidebar.svg").size(15), toggleSidebar, collapsed ? "Expand the sidebar" : "Collapse the sidebar");
    // On the macOS rail the traffic lights fill the row; the sidebar's » unfolds it.
    if (collapsed) {
      return h_flex().size_full().bg(t.chrome).pl(6).pr(6).items_center().justify_center()
        .when(!(window()?.platform === "macos"), el => el.child(toggle));
    }
    // Most needed last-to-go: the toggle, Search, Settings, then rows, then Add
    // machine. As the sidebar narrows the name goes first, then the least needed
    // buttons, so nothing gets cut.
    const buttons = [
      this.button("search", svg("mods/workspace-chrome/search.svg").size(15), () => run("mod.open", { name: "picker" }), "Search"),
      this.button("settings", h_flex().relative().child(svg("mods/workspace-chrome/settings.svg").size(15))
          .when(ready, b => b.child(div().absolute().top(-2).right(-2).size(6).rounded_full().bg(t.accent))),
        () => run("app.settings"), ready ? "Settings (an update is ready)" : "Settings"),
      this.button("density", svg(`mods/sheprd-chrome/${detailed ? "detailed" : "compact"}.svg`).size(15),
        () => saveView({ density: detailed ? "compact" : "detailed" }),
        detailed ? "Detailed rows (two lines). Click for compact" : "Compact rows (one line). Click for detailed"),
      this.button("machine", svg("mods/workspace-chrome/plus.svg").size(15), () => run("mod.open", { name: "machine-add" }), "Add machine"),
    ];
    const width = find(layout(), "sidebar")?.size?.px ?? 300;
    const room = width - (mac ? 76 : 14) - 10 - 28 - 28; // padding, logo, toggle
    const named = room - 28 * buttons.length >= 72;
    const fits = Math.max(0, Math.min(buttons.length, Math.floor((room - (named ? 72 : 0)) / 28)));
    // Shown in reading order: Add machine, rows, Settings, Search, toggle.
    const shown = buttons.slice(0, fits).reverse();
    return h_flex().size_full().bg(t.chrome).pl(14).pr(10).gap(2).items_center().text_size(13.5)
      .when(mac, el => el.pl(76))
      .child(h_flex().flex_1().min_w(0).gap(8).items_center()
        .child(image("mods/sheprd-chrome/sheprd.png").size(20).rounded(5).flex_none())
        .when(named, el => el.child(div().min_w(0).truncate().font_semibold().text_color(t.foreground).child("sheprd"))))
      .children(shown)
      .child(toggle);
  }
}
