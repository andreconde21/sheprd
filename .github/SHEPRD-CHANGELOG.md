# sheprd changelog

What changed in each sheprd release (herdr's own changes are in `CHANGELOG.md`). The release
workflow publishes the section named after the tag as the GitHub release notes.

## Unreleased

## sheprd-v0.9.5-4 (2026-10-20)

### Added
- Card ledger: agents keep a short record per card (SHE-12, CAL-40, CB-12, #123…) with its
  status, a summary, open questions with suggested answers, and decisions, through
  `sheprd card update | ask | decide | done | list | remove`. The right work panel
  (prefix+shift+b) shows them as CARDS rows, cards waiting on you first (`● ?2`); click a row to
  expand its summary, decisions and questions (★ marks the agent's recommendation). Ledgers are
  saved with the session and survive restarts and handoffs, and reach remote machines' panels too.
- The Claude hook fills in guessed cards from the subagent tasks and questions it already sees,
  in the background; a card an agent writes itself always wins over a guess. Run `sheprd setup`
  again on each machine to pick up the new hook.

## sheprd-v0.9.5-3 (2026-10-18)

### Fixed
- The message relay comes back after a quick sheprd restart. The new window's relay found the
  lock still held by the old one (which notices its sheprd quit within 5 s) and gave up, leaving
  the machine without a relay, and the shared view unsent, until the next restart. It now waits
  for the lock while its sheprd runs, so a second window also takes over when the first closes.

## sheprd-v0.9.5-2 (2026-10-18)

### Added
- The shared view (`share_view = true`) now carries each agent's sidebar row: its title, whether
  it is in the active view, its number, when it went idle, context size and to-dos (view sync
  contract v3). Conductore names agents the way sheprd does (no more rows all reading
  "Projects") and keeps recently idle agents in its active view.

### Fixed
- `sheprd promote` publishes the release under its `sheprd-v*` tag. It used to set
  `GITHUB_REF_NAME`, which GitHub ignores, so the release came out named `main` and
  `sheprd update` could not find it.

## sheprd-v0.9.5-1 (2026-10-17)

### Changed
- Based on herdr v0.9.5 (was v0.9.3): herdr's v0.9.4 and v0.9.5 fixes and features, such as
  crash-safe session saves, the server surviving the shell that started it, prefix keys with input
  methods, and Claude Code questions showing as blocked in named sessions.
- Clickable desktop notifications keep sheprd's notify-send + Hyprland focus on Linux; herdr's new
  native clickable notifications are used on Windows.

## sheprd-v0.9.3-37 (2026-10-11)

### Fixed
- New workspace on another machine (footer **new** → on Local while on a remote workspace): sheprd
  now waits for the workspace to exist and switches straight to it, instead of switching first and
  landing on that machine's last workspace (the v0.9.3-36 fix still raced).

## sheprd-v0.9.3-36 (2026-10-11)

### Added
- **Ctrl+click opens anything.** Any file path, not only Markdown: text and code open in the
  editor split at the line (`src/main.rs:42`); images and PDFs open in the desktop viewer, copied
  from the remote machine first when needed.
- **Remote localhost links.** Ctrl+click `http://localhost:<port>` in a remote pane opens it
  through an SSH tunnel to that machine (same port here when free; reused; closes after 12 h).
- **Hint mode** (`prefix+i`): labels every path, link and reference on screen; type a label to
  open it, in capitals to copy it.
- **Ctrl+Alt+click copies** the path, link or reference under the mouse (Ctrl+Shift+click too
  where the terminal passes Shift).
- **Reference preview**: hold Ctrl over a reference to see its title (`preview` command, or the
  title of `command`'s output).
- `{ssh}` and `{cwd}` placeholders in reference commands, for commands that must run where the
  agent is (e.g. `git show` of a commit hash). Examples for commits and PRs in the README.
- `sheprd-doc --line <n>`.

### Fixed
- A new workspace created on another machine (footer **new** → on Local, while on a remote
  workspace) now opens that workspace instead of the machine's last focused one.
