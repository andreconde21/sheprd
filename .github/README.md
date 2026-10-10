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
sheprd's Claude Code hook (`scripts/sheprd-claude-hook`) reads each
session's transcript incrementally after every turn and attaches the session's
context size and per-day usage to its pane as herdr metadata, so it reaches the
sidebar from any machine without syncing files. sheprd keeps the history in
`~/.local/state/herdr/sheprd-usage.json` and attributes it to projects with the
same rules as the sidebar. Right-click a project for *Today* and *Last 7 days*
(active time · input + output + cache-write tokens; cache reads are excluded).

`sheprd setup` installs the hook on this machine and every saved herdr machine.

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

`prefix+b` cycles the sidebar through full, the rail, and hidden (zero columns);
the menu also has "mini sidebar" and "hide sidebar". While it is hidden, agents
that need you show as ` ● 2 ` at the right end of the tab bar.

### Right panel: what the focused agent has been doing
`prefix+shift+b` opens a panel on the right about the agent you're looking at, so
you don't have to read its transcript:

```
 claude · dev
 card CB-12 ↗                       ← the card its launcher reported
 ✗ PR #7 · checks ✗ 1 ↗             ← [[status]] lines (below)

 SUMMARY · 14:05
 Retry queue done, waiting on deploy approval
 · Moved retries to a queue
 WAITING ON YOU
 ? OK to deploy to dev?
 TO-DO 1/3
 ▸ wire the dead-letter alert
 TASKS
 CAL-1021 Bring PRs onto trunk
  dev ✓  adv ●  chr ✓ ↗
 TIMELINE
 13:58 fix the retry queue (+3 edits, 2 cmds)
```

- **Summary**: while the panel shows an agent, sheprd asks its machine for a
  short summary: the first time, then when the agent moved on and 10 minutes
  passed. Any agent works: a Claude Code agent is summarized from its
  transcript, any other (Codex, OpenCode, Gemini…) from what its pane shows.
  The summarizer is `summary_command` in `~/.config/herdr/sheprd-hook.toml` on
  that machine, any command that reads a prompt on stdin and prints the answer:

  ```toml
  summary_command = "ollama run llama3.2"      # or "codex exec -", "llm -m gpt-4o-mini"
  ```

  Without it, Claude Haiku (`claude -p --safe-mode`, your Claude subscription)
  when `claude` is installed; without either, the agent's last lines stand in.
  Nothing is spent on an agent that barely moved on.
  For agents other than Claude Code the same summary also reads the plan or
  to-do list they show (Codex, OpenCode…), which fills the panel's TO-DO.
- **Waiting on you**: the questions in its last message (for agents other than
  Claude Code, the ones the summary found on its screen). Click **↩ reply** to
  answer without switching to it: your text goes to the agent as a prompt (or is
  typed into its pane when it is on a dialog).
- **Tasks**: when it hands work to subagents (dev, adversarial review, a browser
  check…), one entry per task, grouped by the reference id in the subagent's
  description. Click the id to read the card (your `[[refs]]` command), a stage
  to read its final report, `↗` to open the page a browser stage last visited.
- **Card**: when the tool that started the workspace reported `card` and
  `card_link` workspace tokens (Cockpit Board does), click to open the link
  (obsidian://, https://).

The same key switches to a mini column (one mark per task) and hides it. It
reads the Claude Code hook's tokens; run `sheprd setup` again after updating.

### Remote machines that feel local
- **Typing** in a pane on another machine shows each character at once
  (underlined until the machine echoes it), like mosh. It waits for the first
  echo after Enter, so passwords and keys that don't echo (vim normal mode)
  never show stray characters, and it stays off when echo is already fast.
  Menu → **local echo** turns it off.
- **Switching** to a workspace shows its last screen immediately while the
  machine answers.
- **Reconnecting**: retries at most every 10 s, and at once after the laptop
  wakes from sleep.

### Move an agent to another machine
Right-click a Claude Code agent → **Move to…** → pick a machine, e.g. before you
close the laptop lid. sheprd checks the target first: the repository at the same
place (a path under your home maps to the same place under the target's home),
a clean checkout there, the session file here. If the agent has uncommitted
changes it asks: **commit and push** (the target pulls the branch) or **carry
them as a patch** (applied on the target, stashed here). Then it copies the
conversation over the machines' SSH link, opens a workspace on the target
running `claude --resume`, focuses it, and renames the old pane
"moved → <machine>". Nothing is deleted. Unpushed commits are pushed first (the
agent's own branch). A folder Claude Code has never opened on the target asks
once whether to trust it.

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
- Two separate controls per agent (right-click it):
  - **status**: **Mark unread** (a yellow `●` that counts as needing you until
    you visit it) or **Mark read** (clears a finished or blocked agent from the
    queue until its state changes again);
  - **the active view**: **Remove from active** (only under *all agents* until
    the agent does something new), **Keep active** (pinned there, ⚑) or
    **Stop keeping active**.
- **Hiding** is separate and per workspace. When something is hidden the header
  shows **N hidden**; click it (or `prefix+alt+h`) to show them, then
  right-click → **Unhide workspace**.
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

### Agent insights
With the Claude Code hook installed (`sheprd setup`), agent rows also show:
- a **to-do progress bar** (`▰▰▱▱▱ 5/12`); click it to expand the list
  under the agent;
- right-click → **Timeline**: each of your prompts with what followed;
- right-click → **Mentioned**: references the agent talked about (task ids,
  issue numbers…, see below); pick one to read it.

### References: plug in your tracker
Tell sheprd what your references look like and how to show one, in
`~/.config/herdr/sheprd-refs.toml`:

```toml
[[refs]]
name = "issues"
pattern = "#\\d+"                     # a whole reference, as a regex
command = "gh issue view {id}"         # prints its body (markdown works best)

[[refs]]
name = "tasks"
pattern = "[A-Z]{2,5}-\\d+"
ids_from = "~/notes/tasks"             # optional: only ids that exist as <ID>.md here
command = "cat ~/notes/tasks/{id}.md"
```

- **Ctrl+click** a reference in any agent's output to read it. Anything else
  falls through to normal link handling.
- It opens in a **reader**: front matter becomes the header (title, status…),
  markdown is rendered, wheel / arrows / PgUp-PgDn scroll, esc closes.
- The hook uses the same patterns (and `ids_from`) to list what an agent
  mentioned.
- A source can have `open = "xdg-open 'obsidian://…{id}'"` instead of
  `command`: the card then opens in that app rather than in the reader.

### Documents in your editor
**Ctrl+click a Markdown path** in any pane (`docs/plan.md`, `~/notes/report.md`,
`/abs/x.md:12`, `file:///abs/x.md`) and it opens in a vertical split next to that
pane, on the pane's machine, in your editor: `$SHEPRD_DOC_EDITOR`, else `$VISUAL`,
else `$EDITOR`, else `nvim`. Each workspace has at most one document split: the
next document opens in it. Quitting the editor closes the split.

Agents can do the same with `sheprd-doc <path>`; the `sheprd-doc` skill tells
Claude Code to open the plans and reports it writes for you. `sheprd setup`
installs both on every machine.

### Sharing the sidebar with other apps
Apps that mirror sheprd (Conductore Mobile, for one) can show your projects and
which agents need you, mark agents read or unread, and edit the layout (move a
workspace, hide it, create, rename, pin, reorder or delete projects) from there. It's off until
you add `share_view = true` to `sidebar.toml`. Then sheprd:
- writes `~/.local/state/sheprd/view.json` (layout, each agent's presence and
  marks, order, focus) when it changes and every 30 s;
- copies it to every saved machine over the relay's SSH connection;
- applies the marks and layout edits those apps queue in `view-updates.jsonl` on
  any machine, refusing an edit that conflicts with a newer change (the app shows
  why: `rejected` in view.json).

The file format is a small versioned contract (v1 marks, v2 layout edits); see
`docs/sheprd-view-sync.md` in conductore-mobile.

### PR, checks and deploy status
`[[status]]` entries in `~/.config/herdr/sheprd-refs.toml` run a command in each
workspace's folder, on that workspace's machine (over SSH for remote ones), when
the workspace has an agent and then every 3 minutes:

```toml
[[status]]
name = "github"
command = "sheprd-status-github"   # shipped; `sheprd setup` installs it on each machine
match = ["my-repo"]                # optional: workspace label or folder contains one of these
```

The command prints one JSON line,
`{"state":"ok|pending|fail|review|none","text":"PR #212 · checks ✗ 2","url":"…","details":"markdown"}`.
The work panel shows it under the card (click for the details, `↗` for the
page); a failing one counts as needing you (the counter, `prefix+u`, the board).
`sheprd-status-github` reads the branch's PR, checks and review with the GitHub
CLI. Write your own for any other forge, pipeline or deploy.

### Status board
`board` in the sidebar footer, `prefix+shift+u`, or the first menu entry opens one
page with every agent on every machine, grouped by project, those that need you
first (including red CI): its card, status lines, summary, what it is on
(to-do), its tasks' pipelines, and the questions that wait on you. Summaries are
the ones the right panel asked for; with `summaries = true` in
`~/.config/herdr/sheprd-hook.toml` every agent also gets one when it stops or
every 10 minutes while it works (more Haiku calls).

### Token format (for other agent integrations)
Any integration can feed the sidebar by reporting herdr pane metadata tokens
(`herdr pane report-metadata <pane> --source sheprd --token name=value`; values
≤ 80 chars, ≤ 16 tokens per report):

| Token | Value |
|---|---|
| `sheprd_ctx` | current context size in tokens |
| `sheprd_session` | the agent's session id |
| `sheprd_u_YYYYMMDD` | `input,output,cache_read,cache_write,active_minutes` for that day |
| `sheprd_todo` | `done/total` of the agent's to-do list |
| `sheprd_todo_1..12` | `✓ item` / `▸ item` (in progress) / `○ item` |
| `sheprd_tl_1..10` | timeline, oldest first: `09:12 the prompt (+3 edits, 2 cmds)` |
| `sheprd_refs` | references mentioned, comma-separated, newest first |
| `sheprd_w_1..10` | work panel tasks: `id|label|stage:s,stage:s` (s: `r` running, `d` done, `s` stopped, `f` failed; id may be empty) |
| `sheprd_wf` | path of a JSON details file on the agent's machine (`tasks[].stages[]` with `report`, `urls`) |
| `sheprd_q_1..3` | questions in the agent's last message that wait on the user |
| `sheprd_sum` / `sheprd_sum_1..3` | where the work stands / what got done recently (model summary) |

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
| `prefix+b` | sidebar: full / rail / hidden |
| `prefix+shift+b` | work panel: full / mini / hidden |
| `prefix+shift+u` | status board |
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
share_view = false                     # mirror to apps like Conductore (see above)
local_echo_off = false                 # turn predictive local echo off (menu: local echo)

[[group]]
name = "storefront"
pinned = true
match = ["storefront"]                 # name or folder substring
members = ["local/notes"]              # explicit members, in display order
```

sheprd shows its sidebar with one machine or many. Stock herdr ignores this file.
`~/.config/herdr/sheprd-refs.toml` holds `[[refs]]` and `[[status]]` (above),
`~/.config/herdr/sheprd-hook.toml` the summarizer (`summary_command`) and the
`summaries` switch.

## Agents talking to agents, across machines

herdr lets an agent drive panes on its own machine. sheprd adds `sheprd msg`, so an agent on one
machine can talk to an agent on another, for example "ask the agent on the server what it found and
tell the one on my laptop to wait for it".

```bash
sheprd msg list                          # every agent on every machine: machine/workspace, status, context
sheprd msg send dev/api "text"           # submit a prompt to that agent, marked as from another agent
sheprd msg read dev/api -n 80            # its recent output
```

- **Laptop → server** goes direct, over the SSH link sheprd already uses.
- **Server → laptop** works even when the server can't reach your laptop: the message waits in a
  queue on the server, and sheprd collects and delivers it while it's open. The server never gets
  access to your machine.
- **Any agent.** It's a shell command, and every delivered message carries the command to reply.
  A Claude skill is included.
- **Guardrails.** Messages tell the receiver they come from another agent, not from you, and to
  ask you before anything destructive, outward-facing or involving secrets. A per-target rate
  limit stops two agents ping-ponging.

Run `sheprd setup` once: it installs `sheprd-msg` (python3 only, no sheprd needed), the
skill and the Claude Code hook on this machine and on every saved herdr machine.

## Install
Linux (x86_64, aarch64; static) and macOS (Apple Silicon, Intel):

```bash
curl -fsSL https://raw.githubusercontent.com/andreconde21/sheprd/main/scripts/sheprd-install | bash
sheprd            # instead of `herdr`
```

It installs to `~/.local/share/sheprd/sheprd` and leaves your `herdr` install alone.
The macOS builds are not signed yet; installed this way (with `curl`) macOS runs them without a
Gatekeeper prompt.
Update later with `sheprd update`.
The server keeps running stock herdr; use the matching herdr version on each machine.

## Versioning
`sheprd-v<herdr version>-<n>`, e.g. `sheprd-v0.9.3-1` = herdr 0.9.3 + sheprd patch set 1.

## For maintainers of this fork
- Fork code lives in `src/client/shell/`: `projects.rs` (model), `sheprd_sidebar.rs`
  (the combined sidebar), `project_actions.rs` (menus, clicks, drag, keys),
  `work_panel.rs` (right panel), `board.rs` (status board), `status.rs`
  ([[status]] worker), `predict.rs` (local echo), `view_sync.rs` (sharing), plus
  `src/sheprd_msg.rs` and the bundled scripts in `scripts/` (`sheprd-msg`,
  `sheprd-claude-hook`, `sheprd-doc`, `sheprd-status-github`). Small hooks in
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
