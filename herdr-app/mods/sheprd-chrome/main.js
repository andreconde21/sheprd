// The sheprd layout's sidebar header and footer: Herdr's workspace-chrome with
// the sheprd name and mark, plus the sheprd sidebar's settings as icon buttons
// next to search and the sidebar toggle: detailed/compact rows and Active only.
// Those live in the hub's `sheprd-view` document (herdr-app/CONTRACT.md),
// written through the `cards` CLI like the sidebar does.
//
// Based on workspace-chrome from herdr-app (Apache-2.0).

import { View, div, image, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { theme, run, props, window, shared, setShared, updates, layout, exec } from "herdr";

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
    if (props()?.footer) {
      const row = (id, icon, label, dim, action) => h_flex().id(id).h(28).px(8).gap(9).items_center().rounded(8)
        .text_color(dim ? t.dim : t.muted).hover(s => s.bg(t.hover).text_color(t.foreground)).cursor_pointer()
        .on_click(action).child(svg(`mods/workspace-chrome/${icon}.svg`).size(14))
        .when(!collapsed, el => el.child(label));
      return v_flex().size_full().bg(t.chrome).border_t(1).border_color(t.border).px(8).pt(4).pb(8).text_size(13)
        .child(row("settings", "settings", "Settings", false, () => run("app.settings"))
          .when(Boolean(updates().app.ready), el => el.child(div().size(6).rounded_full().bg(t.accent))))
        .child(row("machine", "plus", "Add machine", true, () => run("mod.open", { name: "machine-add" })));
    }
    const v = view();
    const detailed = v.density !== "compact";
    return h_flex().size_full().bg(t.chrome).pl(collapsed ? 6 : 14).pr(10).gap(2).items_center().text_size(13.5)
      .when(window()?.platform === "macos" && !window()?.fullscreen && !collapsed, el => el.pl(76))
      .when(!collapsed, el => el.child(h_flex().flex_1().min_w(0).gap(8).items_center()
        .child(image("mods/sheprd-chrome/sheprd.svg").size(20).flex_none())
        .child(div().font_semibold().text_color(t.foreground).child("sheprd"))))
      .when(!collapsed, el => el
        .child(this.button("density", svg(`mods/sheprd-chrome/${detailed ? "detailed" : "compact"}.svg`).size(15),
          () => saveView({ density: detailed ? "compact" : "detailed" }),
          detailed ? "Detailed rows (two lines). Click for compact" : "Compact rows (one line). Click for detailed"))
        .child(this.button("active", svg("mods/sheprd-chrome/active.svg").size(15),
          () => saveView({ active_only: !v.active_only }),
          v.active_only ? "Active only: on. Idle 8 h+ agents are hidden unless kept" : "Active only: off. Click to hide agents idle 8 h+", Boolean(v.active_only)))
        .child(this.button("search", svg("mods/workspace-chrome/search.svg").size(15), () => run("mod.open", { name: "picker" }), "Search")))
      // On the macOS rail the traffic lights fill the row; the sidebar's » unfolds it.
      .when(!(collapsed && window()?.platform === "macos"), el => el.child(this.button("sidebar-toggle", svg("mods/workspace-chrome/sidebar.svg").size(15), toggleSidebar, collapsed ? "Expand the sidebar" : "Collapse the sidebar")));
  }
}
