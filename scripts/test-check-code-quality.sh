#!/usr/bin/env bash
# Self-check for check-code-quality.sh: pipes representative PostToolUse payloads
# and asserts the hook's contract. No framework — one runnable check.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$HERE/check-code-quality.sh"
HANDBOOK_URL="https://handbook.osbrjp.com/"
fails=0

check() { # <name> <condition-desc> <cmd...>
  local name="$1"; shift
  if "$@"; then
    echo "ok   - $name"
  else
    echo "FAIL - $name"
    fails=$((fails + 1))
  fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# --- Case 1: a clean code file → exit 0, valid JSON, handbook URL injected ---
f="$tmp/sample.ts"
printf 'export const x = 1\n' > "$f"
payload="$(python3 -c 'import json,sys; print(json.dumps({"hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"file_path":sys.argv[1]}}))' "$f")"
out="$(printf '%s' "$payload" | bash "$HOOK")"; rc=$?

check "clean file exits 0" test "$rc" -eq 0
check "output is valid JSON" bash -c 'printf "%s" "$1" | python3 -c "import sys,json; json.load(sys.stdin)"' _ "$out"
check "additionalContext carries handbook URL" bash -c \
  'printf "%s" "$1" | python3 -c "import sys,json; c=json.load(sys.stdin)[\"hookSpecificOutput\"][\"additionalContext\"]; sys.exit(0 if \"$2\" in c else 1)"' _ "$out" "$HANDBOOK_URL"
check "hookEventName is PostToolUse" bash -c \
  'printf "%s" "$1" | python3 -c "import sys,json; sys.exit(0 if json.load(sys.stdin)[\"hookSpecificOutput\"][\"hookEventName\"]==\"PostToolUse\" else 1)"' _ "$out"

# --- Case 2: empty file_path → no-op, exit 0 ---
payload='{"hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{}}'
printf '%s' "$payload" | bash "$HOOK" >/dev/null; rc=$?
check "missing file_path exits 0 (no-op)" test "$rc" -eq 0

# --- Case 3: a language with no configured tool → still exit 0 with context ---
f="$tmp/notes.txt"; printf 'hello\n' > "$f"
payload="$(python3 -c 'import json,sys; print(json.dumps({"hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{"file_path":sys.argv[1]}}))' "$f")"
out="$(printf '%s' "$payload" | bash "$HOOK")"; rc=$?
check "untooled language exits 0" test "$rc" -eq 0
check "untooled language still injects context" bash -c 'printf "%s" "$1" | python3 -c "import sys,json; json.load(sys.stdin)"' _ "$out"

echo
if [ "$fails" -eq 0 ]; then
  echo "All checks passed."
else
  echo "$fails check(s) failed." >&2
  exit 1
fi
