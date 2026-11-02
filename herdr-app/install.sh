#!/usr/bin/env bash
# Installs sheprd's pieces for the Herdr app (herdr-app + herdr-core):
#   cards                      -> ~/.local/bin/cards (every machine with agents)
#   mods/sheprd-sidebar, -panel, -chrome -> ~/.config/herdr-app/mods/ (the machine running the app)
#   layouts/sheprd.json         -> ~/.config/herdr-app/layouts/
#
#   ./install.sh               everything (run on the machine with the app)
#   ./install.sh --cards-only  just the CLI (servers the app reaches over SSH)
#   ./install.sh --link        symlink instead of copy, to edit mods from this checkout
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

echo "Installing:"
chmod +x "$here/bin/cards"
place "$here/bin/cards" "$HOME/.local/bin/cards"
if ! $cards_only; then
  place "$here/mods/sheprd-sidebar" "$app/mods/sheprd-sidebar"
  place "$here/mods/sheprd-panel" "$app/mods/sheprd-panel"
  place "$here/mods/sheprd-chrome" "$app/mods/sheprd-chrome"
  place "$here/layouts/sheprd.json" "$app/layouts/sheprd.json"
  if command -v herdr-app >/dev/null; then
    herdr-app check "$app/mods/sheprd-sidebar"
    herdr-app check "$app/mods/sheprd-panel"
    herdr-app check "$app/mods/sheprd-chrome"
  fi
  echo "Switch to it: ctrl-shift-l cycles layouts, or: herdr-app do layout.switch '{\"name\":\"sheprd\"}'"
fi
