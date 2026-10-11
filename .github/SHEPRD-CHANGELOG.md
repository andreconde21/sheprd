# sheprd changelog

What changed in each sheprd release (herdr's own changes are in `CHANGELOG.md`). The release
workflow publishes the section named after the tag as the GitHub release notes.

## Unreleased

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
