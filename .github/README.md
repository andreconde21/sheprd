<p align="center"><img src="sheprd-logo.svg" width="112" alt="sheprd logo"></p>

# sheprd

**A herdr fork for people who run many agents on more than one machine.**
A shepherd tends the herd: same runtime, smarter pasture.

sheprd is a *client-side* fork of [herdr](https://github.com/herdrdev/herdr) by
herdrdev, with its own logo and name so the two are never confused. The server is unchanged, so the `sheprd` client attaches to stock
`herdr` servers of the same version, locally and over SSH. Everything herdr does,
sheprd does; this page lists only the differences.

> Upstream does not accept outside pull requests, and sheprd does not send any.
> Please don't report sheprd behaviour to herdr.

## What sheprd adds

### One sidebar instead of two
herdr splits the sidebar into *machines* (workspaces per machine) and *agents*.
With several machines that means the same work appears twice, in two different
orders. sheprd replaces both with **one list**: your projects, each showing its
agents from **every** machine, then **Other** for everything not in a project.

```
 ○ all agents ● 2   detailed      ← filter · needs-you counter · view (click)
 ▾ ★ storefront
 ● Fix checkout rounding          ← agent topic
   gpu-box                        ← workspace (if ≠ project) · machine
 ▾ ★ billing
 ○ ⚑ Invoice PDF layout          ← ⚑ kept active
   billing-web
 ● Retry failed webhooks
   billing-web · gpu-box
 ▸ infra                   ○ 3    ← collapsed: worst status + count
 ▾ Other
 ○ Weekly notes               4
   notes
 new · Local              menu
```

The sidebar stays quiet: status and topic only. **Peek** (`prefix+space`)
reveals, for ten seconds (press again to hide): idle age, context size
(`ctx 581k`) and jump number per agent, today's time and tokens per project,
and each machine's latency.

### Time and tokens per project
A small Claude Code hook (`scripts/sheprd-usage-hook`, a Stop hook) reads each
session's transcript incrementally after every turn and attaches the session's
context size and per-day usage to its pane as herdr metadata, so it reaches the
sidebar from any machine without syncing files. sheprd keeps the history in
`~/.local/state/herdr/sheprd-usage.json` and attributes it to projects with the
same rules as the sidebar. Right-click a project for *Today* and *Last 7 days*
(active time · input + output + cache-write tokens; cache reads are excluded).

Install the hook on every machine where agents run. The claude-mods
`agent-insights` hook supersedes `scripts/sheprd-usage-hook` and adds to-dos,
timeline and task mentions.

- **Two views** (click the right header label): *detailed*, one row per agent
  with its topic, and *compact*, one line per workspace.
- **Filter** (click the left header label): *all agents* or *active*. Active
  keeps agents that are working, need you, are kept, or went idle less than
  24 h ago (`recent_hours`), so something you just read doesn't vanish.
  Older idle agents are dimmed in *all agents*.

### Collapsed sidebar: a project rail
Collapsed, the sidebar becomes a 3-column rail: the needs-you counter, then one
row per project (worst status + a 2-letter tag, e.g. `●TC`), with the project
you're in spelled downwards beneath its row. Click a row to jump to that
project's most urgent agent. Peek (`prefix+space`) shows the full sidebar over
the panes for a moment. Tags are derived from the name; set `short = "OP"` on a
project to choose your own.

### Projects across machines
- **Drag** any row onto a project header to move its workspace there. Drop it on
  **Other** to take it out, or on another row to place it just above that row.
  Right-click → `→ project` does the same without the mouse gesture.
- **Auto-assign**: a project's match rules catch workspaces whose *name or
  folder* contains the rule (`storefront` catches `~/code/storefront-api` on
  every machine), so new agents land in the right place with no clicks.
  Dragging something to Other overrides its rules.
- **Organise**: right-click a header → Collapse, Pin to top, Move up/down,
  Rename, Auto-match rules, Delete. Left-click a header to collapse it.
- **Hide** workspaces you rarely look at (right-click → Hide); `prefix+alt+h`
  shows them again, dimmed with ⊘.

### Attention you control
- `prefix+u` jumps to the **next agent that needs you**: blocked, finished and
  not looked at yet, or marked unread, in sidebar order. The `● 2` counter in
  the header shows how many there are; clicking it does the same.
- **Click a desktop notification** to raise the terminal and land on that agent.
- Right-click an agent → **Mark unread** (a yellow `●` status that counts as
  needing you until you visit it) or **Mark inactive** (drops a finished or
  blocked agent out of the queue until its state changes again).
- **Keep active** (right-click an agent): pins it to the active view (⚑) until
  you unpin it, for the thing you're still working on.
- **Jump numbers only when you want them**: `prefix+#` shows a number on every
  row; type it and sheprd jumps as soon as the number is unambiguous.

### New workspaces on any machine
- `prefix+alt+c` (or clicking **new** in the footer) asks which machine, then a
  name. The workspace joins the project you're in and starts in that project's
  folder on the chosen machine.
- Right-click a project → **New agent here** does the same and starts `cc` in it.

### Small fixes
- Workspaces are tracked by id, so two with the same name are independent and a
  rename keeps a workspace in its project.
- **Go To** (`prefix+g`) opens ready to type; arrows and Enter still pick, Left/
  Right still jump between workspaces while the search is empty.

### Find, peek, tidy
- **Filter** (`prefix+/`): type to narrow the sidebar to matching projects,
  workspaces, machines and agent topics; ↑↓ pick, Enter goes there;
  `alt+m` marks unread/inactive, `alt+k` keeps, `alt+h` hides the highlighted row.
- **Peek last lines** (right-click an agent): an agent's last 12 lines in a
  popup, from any machine, without switching to it.
- **Project note** (right-click a project → Note…): a dim line under the header,
  e.g. "waiting on client reply".
- **Quiet projects** (right-click → Notify): `all`, `blocked` (only when an
  agent waits on you) or `none`; applies to toasts and sounds.
- **Close idle workspaces** (right-click a project): lists the workspaces whose
  agents have been idle for 7+ days and closes them only when you confirm.

### Agent insights (with claude-mods)
With the [claude-mods](https://github.com/andreconde21/claude-mods)
`agent-insights` hook installed, agent rows also show:
- a **to-do progress bar** (`▰▰▱▱▱ 5/12`); click it to expand the list
  under the agent;
- right-click → **Timeline**: each of your prompts with what followed;
- right-click → **Tasks mentioned**: ids from your own task sources (pattern +
  command in `~/.config/claude-mods/tasks.toml`); pick one to read its body.

### Desktop status
While it runs, sheprd keeps `~/.local/state/herdr/sheprd-status.json` up to date
(who needs you, which projects, today's time and tokens). The
[Omarchy Herdr widget](https://github.com/andreconde21/omarchy-herdr) reads it
to show `● 3` in the bar; any other bar or script can too.

### Keys (defaults; no config needed)
| Key | Action |
|---|---|
| `prefix+space` | peek: idle age, context, numbers, usage, latency |
| `prefix+u` | next agent that needs you |
| `prefix+#` | show jump numbers, type one to jump |
| `prefix+alt+c` | new workspace on a machine you pick |
| `prefix+/` | filter the sidebar |
| `prefix+.` | project menu for the focused workspace |
| `prefix+alt+h` | show / conceal hidden workspaces |

## Configuration
Everything lives client-side in `~/.config/herdr/sidebar.toml`. The UI writes
it, and hand edits reload within a second:

```toml
compact = false                        # view: one row per agent / per workspace
active_only = false                    # filter: all agents / active ones
recent_hours = 24                      # idle agents stay "active" this long
hidden = ["gpu-box/scratch"]           # machine/workspace

[[group]]
name = "storefront"
pinned = true
match = ["storefront"]                 # name or folder substring
members = ["local/notes"]              # explicit members, in display order
```

The combined sidebar appears when the client is connected to 2+ machines. With
a single machine sheprd looks like herdr. Stock herdr ignores this file.

## Install
Linux x86_64, static binary:

```bash
curl -fsSL https://raw.githubusercontent.com/andreconde21/sheprd/main/scripts/sheprd-install | bash
sheprd            # instead of `herdr`
```

It installs to `~/.local/share/sheprd/sheprd` and leaves your `herdr` install alone.
Update later with `sheprd update`.
The server keeps running stock herdr; use the matching herdr version on each machine.

## Versioning
`sheprd-v<herdr version>-<n>`, e.g. `sheprd-v0.9.3-1` = herdr 0.9.3 + sheprd patch set 1.

## For maintainers of this fork
- Fork code lives in `src/client/shell/projects.rs` (model),
  `src/client/shell/sheprd_sidebar.rs` (the combined sidebar) and
  `src/client/shell/project_actions.rs` (menus, clicks, drag, keys). Small hooks in
  upstream files are tagged: `grep -rn "andreconde fork" src`.
- **Following herdr is automatic**: *sheprd rebase* runs daily. When herdr ships
  a new stable release it rebases sheprd onto it; if that's clean and the tests
  pass it pushes `rebase/<tag>` and opens a "ready to ship" issue, and on a
  conflict it opens an issue with the files and upstream commits involved,
  changing nothing. *sheprd promote* (Actions → Run workflow) ships a ready
  rebase. Conflict resolutions are remembered via `git rerere` (`.github/rr-cache`).
- Manual rebase: `git fetch origin --tags && git rebase --onto v<new> v<old> main`.
- Release: `git tag sheprd-v<ver>-<n> && git push fork sheprd-v<ver>-<n>` → the
  "sheprd release" workflow builds and publishes. Upstream workflows are disabled here.
- Local build needs Zig 0.16.0 (`cargo build --release`).

## License
Apache-2.0, same as herdr. herdr is © its authors; sheprd changes are © André Conde.
