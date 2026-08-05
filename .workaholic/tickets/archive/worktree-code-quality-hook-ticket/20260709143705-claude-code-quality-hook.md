---
created_at: 2026-07-09T14:37:05+09:00
author: po.ching.yu.alex@oz-design.jp
type: enhancement
layer: [Infrastructure, Config]
effort: 1h
commit_hash: 4870240
category: Added
depends_on:
mission:
---

# Claude Code quality hook referencing the handbook guideline

## Overview

Add a Claude Code **PostToolUse** hook to this repository (the `standard-repository`
template) that fires on every code-mutating tool action (`Edit`, `Write`,
`MultiEdit`) and drives a code-quality check against the published handbook
coding guideline (`https://osbrjp.github.io/handbook/`).

A hook command is deterministic shell — it cannot itself run an LLM to judge a
prose guideline. So the requested "check code quality" is delivered as **three
layered checks** in one backing script (all three were explicitly requested):

1. **Deterministic lint/format** — run the shared formatter/linter on the changed
   file by language: `prettier --check` (JS/TS/JSON/MD/… via `.prettierrc.json`)
   and `ruff check` (Python via `ruff.toml`), honoring `.editorconfig`. This
   tooling is introduced by **PR #32 "Add shared formatter/lint config"** (branch
   `work-20260709-152306`); the hook should call those same configs, not invent
   its own. Skip gracefully (exit 0) for a language with no configured tool — see
   Considerations for the #32 merge-ordering.
2. **Guideline context injection** — emit the handbook coding-guideline URL (and
   the relevant section for the file's language) back to Claude as
   `additionalContext`, so Claude self-checks the code it just wrote against the
   authoritative standard.
3. **Guideline-conformance review** — for non-trivial changes, the injected
   `additionalContext` instructs Claude to run a conformance review of the
   changed file against the guideline (escalating to `/code-review` when
   warranted). This is how "sub-agent review" is reached from a deterministic
   hook: the shell cannot spawn an agent, so it returns the instruction that
   makes Claude do it.

The hook wiring in `.claude/settings.json` stays a thin entry point; all logic
lives in a repo script under `scripts/`, so the same check is runnable by a
developer and (later) CI, not duplicated inline (`command-scripts` / `ci-cd`).

## Policies

The standard engineering policies (synced from qmu.co.jp into the `workaholic`
policy skills) that govern this ticket. The implementing session **MUST** read
each linked hard copy before writing code and keep every change defensible
against that policy's Goal (目標), Responsibility (責務), and Practices (実践).

- `workaholic:implementation` / `policies/directory-structure.md` — the backing
  logic belongs in `scripts/` as a pronounceable `[verb]-****.sh`; placement
  must be readable from structure, not hunted for.
- `workaholic:implementation` / `policies/coding-standards.md` — defines the
  actual quality bar the hook enforces (the guideline is the authority for what
  "quality" means); the hook must not invent its own ad-hoc rules.
- `workaholic:implementation` / `policies/command-scripts.md` — automation
  (the hook, and later CI) must CALL a single named script, never re-implement
  its logic inline in `.claude/settings.json`.
- `workaholic:implementation` / `policies/policy-conformance-audit.md` — a
  per-action quality hook is exactly the "automated checks as the first layer"
  of conformance auditing.
- `workaholic:operation` / `policies/ci-cd.md` — the same inspection command must
  run identically locally, from the hook, and in CI; inspection logic stays in
  repo scripts, not locked in hook/service config.

## Key Files

- `.claude/settings.json` - host for the new top-level `hooks.PostToolUse` block;
  currently only declares `extraKnownMarketplaces` (and only on branch
  `work-20260701-181748`), so this ticket adds/commits it on the work branch.
- `scripts/check-code-quality.sh` (new) - the backing script the hook invokes;
  reads the PostToolUse JSON payload on stdin, runs the three layered checks,
  emits JSON back to Claude.
- `.prettierrc.json`, `ruff.toml`, `.editorconfig` (added by **PR #32**, branch
  `work-20260709-152306`) - the shared formatter/lint config the hook's Layer 1
  invokes (`prettier` for JS/TS/JSON/MD, `ruff` for Python, editorconfig for
  whitespace/indent). The hook calls these; it does not define its own rules.
- `README.md` - already treats `https://osbrjp.github.io/handbook/` as the source
  of truth for the coding guideline; the hook cites the same URL.
- `.github/workflows/run-tests.yml` - existing CI stub (`echo "Build and run
  tests here"`); the same `scripts/check-code-quality.sh` can later back a real
  CI lint step (ci-cd policy), but wiring CI is out of scope here.

## Related History

No prior tickets, hooks, or code-quality automation exist in this repository —
`.workaholic/tickets/` does not yet exist and git history contains no related
work. The only adjacent item is the published "Coding Style Guide (TypeScript,
Go, Python)" in the handbook, which is the **reference target**, not overlapping
work.

## Implementation Steps

1. Create `scripts/check-code-quality.sh` (cwd-safe: resolve repo root, keep a
   clean working directory). It reads the PostToolUse event JSON from stdin and
   extracts `tool_name` and `tool_input.file_path`.
2. **Layer 1 — deterministic lint/format**: detect the file's language by
   extension and run the shared tool from PR #32 on `file_path` — `prettier
   --check` (JS/TS/JSON/MD/…, via `.prettierrc.json`), `ruff check` (Python, via
   `ruff.toml`), honoring `.editorconfig`. Surface tool output on failure. No-op
   (exit 0) for a language with no configured tool, and guard on the binary being
   present so the hook never fails a code action just because the tool isn't
   installed.
3. **Layer 2 — guideline context**: emit `hookSpecificOutput.additionalContext`
   containing the handbook coding-guideline URL and the section relevant to the
   file's language, instructing Claude to verify the just-written code against it.
4. **Layer 3 — conformance review**: when the change is non-trivial, include in
   the `additionalContext` an instruction to run a guideline-conformance review
   of the changed file (escalate to `/code-review` where warranted).
5. Wire the hook in `.claude/settings.json`:
   `hooks.PostToolUse = [{ "matcher": "Edit|Write|MultiEdit", "hooks": [{ "type":
   "command", "command": "bash scripts/check-code-quality.sh" }] }]`. Match the
   existing 2-space JSON style and preserve `extraKnownMarketplaces`.
6. **Verify the exact hook I/O contract against the current Claude Code hooks
   documentation before finalizing** (stdin field names, the PostToolUse JSON
   output schema for `additionalContext` / `decision` / exit codes) — do not rely
   on memory; confirm via the official docs / `claude-code-guide`.
7. Add `scripts/test-check-code-quality.sh` (or `.mjs`) that pipes representative
   PostToolUse payloads into the script and asserts: valid JSON out, exit 0 on a
   clean file, the handbook URL present in `additionalContext`, and graceful
   no-op when no linter is configured.
8. Commit `.claude/settings.json` + both scripts.

## Quality Gate

**Acceptance criteria** — the checkable conditions that must hold:

- `.claude/settings.json` contains a `hooks.PostToolUse` entry whose matcher is
  `Edit|Write|MultiEdit` and whose command invokes `scripts/check-code-quality.sh`;
  `extraKnownMarketplaces` is preserved; the file is valid JSON.
- A live `Edit`/`Write` in-session fires the hook (confirmed via `claude --debug`
  or equivalent hook trace) and the hook exits 0 on a clean change.
- The hook emits **valid JSON** whose `additionalContext` contains the handbook
  coding-guideline URL (`https://osbrjp.github.io/handbook/`).
- With PR #32's config in tree, a `.py` change routes through `ruff check` and a
  `.ts`/`.json`/`.md` change routes through `prettier --check`; the hook no-ops
  (exit 0) — never blocks a code action — when no tool/config exists for the
  changed file's language or the binary is not installed.
- `scripts/test-check-code-quality.sh` exists, is committed, and passes.

**Verification method** — the commands/tests/probes that prove them:

- `python3 -c 'import json; json.load(open(".claude/settings.json"))'` (or `jq .`)
  exits 0.
- Live: perform an `Edit`/`Write` with hook tracing on; observe the hook fire and
  its JSON output.
- Manual: `echo '<sample PostToolUse payload>' | bash scripts/check-code-quality.sh`
  returns valid JSON containing the handbook URL and exits 0.
- `bash scripts/test-check-code-quality.sh` is green (asserts all criteria above).

**Gate** — what must pass before approval:

- The test script is green, `settings.json` parses as valid JSON, and the hook is
  observed firing live in-session with the handbook URL present in its output.

## Considerations

- **Merge order vs PR #32** — Layer 1's tooling (`prettier`, `ruff`,
  `.editorconfig`) is added by PR #32 (branch `work-20260709-152306`), which is
  not yet merged. **#32 should merge before this hook lands** so Layer 1 has its
  configs in-tree; if this ships first, Layer 1 simply no-ops until #32 arrives.
  Either way the hook must guard on tool/config presence and never fail a code
  action when they're absent. #32's `.claude/settings.json` has no `hooks` block,
  so there is no settings.json conflict — but both touch that file, so rebase
  after #32 merges. (`.claude/settings.json`, `.prettierrc.json`, `ruff.toml`)
- **Cost of layer 3** — instructing a conformance review on *every* non-trivial
  code action adds latency/tokens. Consider a size/scope threshold so trivial
  edits only get the cheap context injection (layer 2), reserving the review for
  substantive changes. (`scripts/check-code-quality.sh`)
- **Reference source is external** — the guideline lives at
  `https://osbrjp.github.io/handbook/`, not in this tree. The hook cites the URL
  (offline runs simply can't fetch it, which is acceptable for a
  context-injection hook); if offline resilience is later needed, revisit a
  vendored copy. (`README.md`, `scripts/check-code-quality.sh`)
- **Template propagation** — this is the `standard-repository` template, so the
  hook becomes a default for repos scaffolded from it. Keep it self-contained and
  tooling-agnostic so it works in a fresh repo. (`.claude/settings.json`)
- **Hook I/O schema must be verified against current docs**, not memory — Claude
  Code's hook payload/output contract is the authority (Step 6). (`.claude/settings.json`)

## Final Report

Development completed as planned. Implemented `scripts/check-code-quality.sh`
(three layers: prettier/ruff by extension → guideline context injection →
/code-review nudge for substantial changes), wired it as `hooks.PostToolUse`
(matcher `Edit|Write|MultiEdit`) in `.claude/settings.json`, and added
`scripts/test-check-code-quality.sh` (7 assertions, all green). PR #32's shared
config (`.prettierrc.json`, `ruff.toml`, `.editorconfig`) was merged into this
branch so Layer 1 has its tooling in-tree.

### Discovered Insights

- **Insight**: PostToolUse hooks cannot block — the tool already ran. `decision:
  "block"` has no effect; the only way to feed Claude is exit 0 with
  `hookSpecificOutput.additionalContext` (verified against
  https://code.claude.com/docs/en/hooks.md).
  **Context**: shaped the design — the hook injects a self-check instruction and
  surfaces lint output as context, rather than trying to gate the edit.
- **Insight**: the "handbook coding guideline" is published externally
  (`https://osbrjp.github.io/handbook/`); it is NOT in this repo tree. This
  `standard-repository` is a template that links out to it.
  **Context**: Layer 1 (lint) is the only in-tree quality check; Layers 2–3 cite
  the external URL, so the hook degrades to a clean no-op offline.
- **Insight**: the hook command uses `bash "${CLAUDE_PROJECT_DIR:-.}/scripts/..."`
  so it resolves whether or not Claude Code exports `CLAUDE_PROJECT_DIR`, and
  guards on `python3`/`prettier`/`ruff` presence — it never fails a code action
  in a fresh template repo with no toolchain.
  **Context**: matters for template propagation — repos scaffolded from this one
  inherit a hook that works before any linters are installed.
