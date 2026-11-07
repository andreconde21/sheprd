# sheprd for the Herdr app

sheprd's sidebar, work panel and card ledger, rebuilt for the Herdr desktop app
(herdr-app + herdr-core `hc`) as mods and one small CLI. Nothing here patches
Herdr: it survives Herdr updates.

| Piece | What it does |
|---|---|
| `mods/sheprd-sidebar` | Every agent on every machine in one list, grouped by project across machines, filter Needs you / Working / Marked / All, ★ marks, right-click: mark, mark read, move a space to a project, new project, hide. Plays the attention sound when an agent turns blocked or done, and a desktop notification when the app isn't in front; right-click a project header to mute it (turn Herdr's own sounds off in Settings so you don't hear both). Collapsed (the sidebar button, or under 160 px): a rail of project tags with their most urgent status and a needs-you count, and a dot per agent. |
| `mods/sheprd-panel` | The focused agent's cards: summary first, then its latest decisions, then open questions with answer buttons (★ = recommended, ✎ = your own text) and Send; its last output folded below. "All cards" shows every agent's open cards. |
| `mods/sheprd-chrome` | The sidebar's header: Herdr's own with the sheprd name and mark, with Search, Settings, detailed/compact rows and Add machine as icons next to the sidebar toggle; icons drop out as the sidebar narrows instead of being cut. No footer. |
| `mods/sheprd-tabs` | Herdr's tab bar with a third panel button, **Work** (the sheprd panel), next to Files and Changes. A space opens with Work until you choose another panel or close it. |
| `mods/sheprd-leader` | sheprd's prefix key, which Herdr doesn't have: ctrl+space opens a key list and the next key runs sheprd's `prefix+<key>` action (c new tab, x close pane, z zoom, n/p tabs, N/P spaces, a next agent, g go to, b sidebar, s settings, ?). `install.sh` binds ctrl+space in `keys.json` unless it's already bound. |
| `layouts/sheprd.json` | Herdr's main layout with the sidebar on the left and the panel on the right (Files and Changes stay in the panel stack). |
| `bin/cards` | The card ledger CLI agents use (`cards update/ask/decide/done/answers/list`, same arguments as `sheprd card`), plus the commands the mods call. |
| `CONTRACT.md` | The two herdr-core documents (`sheprd-cards`, `sheprd-view`) shared with Conductore. |

## Install

On the computer that runs the app:

```sh
./install.sh            # cards + mods + layout, copied; --link symlinks them to edit from this checkout
```

Use the default copy install. `--link` (symlinks into this checkout, for editing
mods) leaves images loaded with `image()` blank, such as the sheprd mark in the
header: Herdr's image loader doesn't follow symlinks. Icons drawn with `svg()`
are fine.

On every server the app reaches over SSH (where agents run), only the CLI:

```sh
./install.sh --cards-only
```

herdr-core started over SSH often has a minimal PATH without `~/.local/bin`. The installer
therefore also links `cards` into `/usr/local/bin` when that is writable; where it is not, the
panel and sidebar fall back to `~/.local/bin/cards`, and show "cards not available on <machine>"
when neither works.

Then switch layouts with ctrl-shift-l, or `herdr-app do layout.switch '{"name":"sheprd"}'`.

`cards` needs python3 (3.11+) and `hc` on the same machine. The sidebar imports
your sheprd projects from `~/.config/herdr/sidebar.toml` the first time it
runs (marks are not imported: they name sheprd's pane ids).

## Agents

The `sheprd-card` skill covers it: inside the Herdr app (`$HC_TERMINAL_ID`
set), agents run `cards …` instead of `sheprd card …`. `cards done` writes the
card back to its task file with the same `[ledger.writeback]` section of
`~/.config/herdr/config.toml` that sheprd uses.

## Not (yet) like sheprd

- Write-back covers status, decisions and the Done line; sheprd's
  `on_card_done` hook is not ported.
- The sidebar has no per-project notification setting.
- Cards refresh every 5 s (herdr-core has no doc events through `exec`).

The panel's chips (Needs me, Working, Review, Blocked, Done, All) filter by the card's status; the
mapping is one table at the top of `mods/sheprd-panel/main.js`, Done is hidden under All, and the
choice is remembered. Evidence images are shown as thumbnails drawn from a `data:` URI; where the
app's `image()` does not accept one, the label shows instead.
