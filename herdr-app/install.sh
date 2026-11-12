#!/usr/bin/env bash
# Installs sheprd's pieces for the Herdr app (herdr-app + herdr-core):
#   cards                      -> ~/.local/bin/cards (every machine with agents)
#   hc-inbox + hc wrapper      -> ~/.local/bin, plus Stop/SessionStart hooks in ~/.claude/settings.json
#                                 (agent messages delivered in the background, not typed)
#   mods/sheprd-sidebar, -panel, -messages, -doc, -chrome, -tabs, -leader -> ~/.config/herdr-app/mods/ (the machine running the app)
#   layouts/sheprd.json         -> ~/.config/herdr-app/layouts/
#
#   ./install.sh               everything (run on the machine with the app)
#   ./install.sh --cards-only  just the CLI (servers the app reaches over SSH)
#   ./install.sh --link        symlink instead of copy, to edit mods from this checkout
#                              (images via image(), like the header logo, stay blank: not followed)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
app="${HERDR_APP_HOME:-$HOME/.config/herdr-app}"
cards_only=false
link=false
for arg in "$@"; do
  case "$arg" in
    --cards-only) cards_only=true ;;
    --link) link=true ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

place() {
  local from="$1" to="$2"
  mkdir -p "$(dirname "$to")"
  rm -rf "$to"
  if $link; then ln -s "$from" "$to"; else cp -R "$from" "$to"; fi
  echo "  $to"
}

command -v python3 >/dev/null || { echo "cards needs python3 (3.11+)" >&2; exit 1; }
command -v hc >/dev/null || [ -x "$HOME/.local/bin/hc" ] || echo "note: hc (herdr-core) is not installed here yet; cards needs it"

$link && ! $cards_only && echo "note: --link leaves image() assets (the header's sheprd mark) blank; use a copy install to see them"
echo "Installing:"
chmod +x "$here/bin/cards"
place "$here/bin/cards" "$HOME/.local/bin/cards"
# herdr-core started over SSH has a minimal PATH without ~/.local/bin, so the app would not find
# `cards` there. Also link it into /usr/local/bin when that is writable and has no other `cards`
# (the panel falls back to ~/.local/bin/cards when this is not possible).
if [ -d /usr/local/bin ] && [ -w /usr/local/bin ] && { [ ! -e /usr/local/bin/cards ] || [ -L /usr/local/bin/cards ]; }; then
  ln -sfn "$HOME/.local/bin/cards" /usr/local/bin/cards
  echo "  /usr/local/bin/cards -> $HOME/.local/bin/cards"
fi
# Agent messages in the background (hc-inbox): `hc send` to a Claude agent goes to its inbox,
# not its prompt box. The wrapper replaces ~/.local/bin/hc only when that is herdr-core's own
# symlink (or already our wrapper); `hc update` may put the symlink back: rerun this then.
chmod +x "$here/bin/hc-inbox" "$here/bin/hc"
place "$here/bin/hc-inbox" "$HOME/.local/bin/hc-inbox"
hc_path="$HOME/.local/bin/hc"
if [ -x "$HOME/.local/bin/herdr-core" ] && { [ ! -e "$hc_path" ] || [ "$(readlink "$hc_path")" = herdr-core ] \
     || [ "$(readlink -f "$hc_path")" = "$(readlink -f "$HOME/.local/bin/herdr-core")" ] \
     || head -c 200 "$hc_path" 2>/dev/null | command grep -q 'hc wrapper (sheprd)'; }; then
  rm -f "$hc_path"
  cp "$here/bin/hc" "$hc_path"
  echo "  $hc_path (wrapper: send -> hc-inbox, the rest -> herdr-core)"
else
  echo "  note: $hc_path is not herdr-core's symlink; left as is (agent messages keep being typed)"
fi
# sheprd-doc: the same script serves the TUI and the app (it hands off to `cards doc` there).
# Update the copy agents find first on PATH too, when it is sheprd's.
chmod +x "$here/../scripts/sheprd-doc"
place "$here/../scripts/sheprd-doc" "$HOME/.local/bin/sheprd-doc"
# Every sheprd copy on PATH (agents' shells may put the TUI's mise folder first).
IFS=: read -r -a path_dirs <<< "$PATH"
for dir in "${path_dirs[@]}" "$HOME/.local/share/mise/installs/herdr/latest"; do
  found="$dir/sheprd-doc"
  if [ -f "$found" ] && [ "$found" != "$HOME/.local/bin/sheprd-doc" ] && [ -w "$found" ] \
     && head -c 300 "$found" | command grep -q 'sheprd-doc' \
     && ! cmp -s "$found" "$here/../scripts/sheprd-doc"; then
    cp "$here/../scripts/sheprd-doc" "$found"
    echo "  $found"
  fi
done
if [ -d "$HOME/.claude" ]; then
  python3 "$HOME/.local/bin/hc-inbox" install-hooks "$HOME/.claude/settings.json" "$HOME/.local/bin/hc-inbox"
fi
# Agents learn the ledger (and that `cards` is the command in the app) from these skills.
if [ -d "$HOME/.claude" ]; then
  mkdir -p "$HOME/.claude/skills/sheprd-card" "$HOME/.claude/skills/sheprd-msg"
  cp "$here/../skills/sheprd-card/SKILL.md" "$HOME/.claude/skills/sheprd-card/SKILL.md"
  python3 - "$here/../scripts/sheprd-msg" "$HOME/.claude/skills/sheprd-msg/SKILL.md" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
open(sys.argv[2], "w").write(re.search(r'^SKILL = """(.*?)"""', src, re.S | re.M).group(1))
PY
  echo "  $HOME/.claude/skills/sheprd-card, sheprd-msg"
fi
if ! $cards_only; then
  place "$here/mods/sheprd-sidebar" "$app/mods/sheprd-sidebar"
  place "$here/mods/sheprd-panel" "$app/mods/sheprd-panel"
  place "$here/mods/sheprd-messages" "$app/mods/sheprd-messages"
  place "$here/mods/sheprd-doc" "$app/mods/sheprd-doc"
  place "$here/mods/sheprd-chrome" "$app/mods/sheprd-chrome"
  place "$here/mods/sheprd-tabs" "$app/mods/sheprd-tabs"
  place "$here/mods/sheprd-leader" "$app/mods/sheprd-leader"
  # ctrl+space opens the leader (sheprd's prefix), unless the key is already bound.
  python3 - "$app/keys.json" <<'PY'
import json, os, shutil, sys
path = sys.argv[1]
if not os.path.exists(path):
    # Herdr writes its full keys.json on first start; a partial one would replace it.
    print("  keys.json: not there yet (start the app once), then rerun to bind ctrl+space")
    sys.exit(0)
keys = json.load(open(path))
glob = keys.setdefault("global", {})
if "ctrl-space" in glob:
    print(f"  keys.json: ctrl-space already does {glob['ctrl-space']!r}; left as is")
else:
    shutil.copy2(path, path + ".pre-sheprd-leader")
    glob["ctrl-space"] = "mod.open:sheprd-leader"
    with open(path, "w") as f:
        json.dump(keys, f, indent=2)
        f.write("\n")
    print(f"  {path}: ctrl-space -> sheprd leader")
PY
  place "$here/layouts/sheprd.json" "$app/layouts/sheprd.json"
  if command -v herdr-app >/dev/null; then
    herdr-app check "$app/mods/sheprd-sidebar"
    herdr-app check "$app/mods/sheprd-panel"
    herdr-app check "$app/mods/sheprd-messages"
    herdr-app check "$app/mods/sheprd-doc"
    herdr-app check "$app/mods/sheprd-chrome"
    herdr-app check "$app/mods/sheprd-tabs"
    herdr-app check "$app/mods/sheprd-leader"
  fi
  echo "Switch to it: ctrl-shift-l cycles layouts, or: herdr-app do layout.switch '{\"name\":\"sheprd\"}'"
fi
