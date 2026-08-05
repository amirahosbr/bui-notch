#!/usr/bin/env bash
# PostToolUse code-quality hook backing script.
#
# A hook command is deterministic shell — it cannot itself judge a prose
# guideline. So it delivers "check code quality" in layers:
#   1. run the shared formatter/linter (prettier / ruff) on the changed file
#      when the tool is installed;
#   2. inject the handbook coding guideline as additionalContext so Claude
#      self-checks the code it just wrote against the authoritative standard;
#   3. for a substantial change, nudge a /code-review conformance pass.
#
# It NEVER blocks a code action: any missing tool/parser/file is a clean exit 0.
# Contract: https://code.claude.com/docs/en/hooks.md (PostToolUse)
set -euo pipefail

HANDBOOK_URL="https://handbook.osbrjp.com/"

payload="$(cat)"

# python3 parses the event JSON and emits valid JSON back (safe escaping).
# Absent → no-op; never fail a code action just because the parser is missing.
command -v python3 >/dev/null 2>&1 || exit 0

file_path="$(
  printf '%s' "$payload" | python3 -c \
    'import sys,json; print(json.load(sys.stdin).get("tool_input",{}).get("file_path",""))' \
    2>/dev/null || true
)"

[ -n "$file_path" ] || exit 0   # nothing to check
[ -f "$file_path" ] || exit 0   # e.g. a delete, or a relative path we can't resolve

# --- Layer 1: deterministic lint/format, only when the tool is installed ---
lint_note=""
case "$file_path" in
  *.js|*.jsx|*.ts|*.tsx|*.mjs|*.cjs|*.json|*.css|*.scss|*.html|*.md|*.yaml|*.yml)
    if command -v prettier >/dev/null 2>&1 && ! prettier --check "$file_path" >/dev/null 2>&1; then
      lint_note="prettier reports formatting issues (fix: prettier --write \"$file_path\"). "
    fi
    ;;
  *.py)
    if command -v ruff >/dev/null 2>&1 && ! ruff check "$file_path" >/dev/null 2>&1; then
      lint_note="ruff reports lint issues (see: ruff check \"$file_path\"). "
    fi
    ;;
esac

# --- Layer 3: escalation to /code-review for a substantial change or lint failure ---
# ponytail: 80-line file as the "non-trivial" heuristic; tune if it's too chatty.
review_note=""
lines="$(wc -l < "$file_path" 2>/dev/null || echo 0)"
if [ "$lines" -gt 80 ] || [ -n "$lint_note" ]; then
  review_note=" (3) This is a substantial change — run /code-review now to verify conformance against the guideline."
fi

# --- Layer 2: inject a directive instruction (always) ---
# A hook can only inject text; it cannot force these actions. Word it as an
# imperative so Claude acts on it, and cache the fetch across the session.
context="A code file was just edited: ${file_path}. ${lint_note}Enforce the OSBR handbook coding standard before continuing: (1) If you have not already this session, open the handbook at ${HANDBOOK_URL} and locate its coding style guide for this file's language (TypeScript / Go / Python), then read those rules. (2) Review the code you just wrote against the guide and fix any violations now.${review_note}"

python3 - "$context" <<'PY'
import sys, json
print(json.dumps({"hookSpecificOutput": {
    "hookEventName": "PostToolUse",
    "additionalContext": sys.argv[1],
}}))
PY
