#!/bin/bash
#
# Wires Claude Code's Notification hook to `notch attention`, so a waiting agent
# opens the notch panel by itself.
#
#   ./scripts/install-claude-hook.sh             # install (or reinstall)
#   ./scripts/install-claude-hook.sh --uninstall  # remove just this hook
#
# The Notification hook fires exactly when Claude Code wants you — a permission
# prompt, or a question. That is information a transcript cannot give us: a tool
# call awaiting approval and one merely running look identical on disk.

set -euo pipefail

SETTINGS="$HOME/.claude/settings.json"
MARKER="notch attention"

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is needed to edit $SETTINGS safely" >&2
  exit 1
fi

BIN="$(command -v notch || true)"
if [[ -z "$BIN" && "${1:-}" != "--uninstall" ]]; then
  echo "error: notch is not on PATH — run: cargo install --path crates/notch" >&2
  exit 1
fi

mkdir -p "$(dirname "$SETTINGS")"
[[ -f "$SETTINGS" ]] || echo '{}' > "$SETTINGS"

# Edited through a JSON parser rather than sed: this file is the user's own Claude
# Code configuration, and a half-applied text substitution would break every hook
# they have, not just ours.
UNINSTALL=0
[[ "${1:-}" == "--uninstall" ]] && UNINSTALL=1

BIN="$BIN" SETTINGS="$SETTINGS" MARKER="$MARKER" UNINSTALL="$UNINSTALL" python3 <<'PY'
import json, os, shutil, sys

path = os.environ["SETTINGS"]
marker = os.environ["MARKER"]
uninstall = os.environ["UNINSTALL"] == "1"
command = f'{os.environ["BIN"]} attention'

with open(path) as f:
    try:
        settings = json.load(f)
    except json.JSONDecodeError as e:
        sys.exit(f"error: {path} is not valid JSON ({e}) — fix it before running this")

hooks = settings.setdefault("hooks", {})
entries = hooks.setdefault("Notification", [])

# Drop any hook of ours that is already there, so reinstalling cannot stack two.
kept = []
for entry in entries:
    inner = [h for h in entry.get("hooks", []) if marker not in h.get("command", "")]
    if inner:
        kept.append({**entry, "hooks": inner})
    elif not entry.get("hooks"):
        kept.append(entry)

if not uninstall:
    kept.append({"hooks": [{"type": "command", "command": command}]})

if kept:
    hooks["Notification"] = kept
else:
    hooks.pop("Notification", None)
if not hooks:
    settings.pop("hooks", None)

shutil.copyfile(path, path + ".notch-backup")
with open(path, "w") as f:
    json.dump(settings, f, indent=2)
    f.write("\n")

print("removed" if uninstall else f"installed: {command}")
print(f"  settings: {path}")
print(f"  backup:   {path}.notch-backup")
PY

if [[ "$UNINSTALL" == "0" ]]; then
  echo
  echo "restart any running Claude Code session to pick the hook up."
  echo "check it with: notch attention show"
fi
