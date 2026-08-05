# Programming Standard

All project code follows these rules. Libraries are exempt; project code is not.

## Pure functional, no mutation

- Write pure functions: same input → same output, no side effects.
- No classes. No mutation — no reassignment, no in-place edits. Return new values (`map`/`filter`/`reduce`, spread, immutable updates) instead of mutating.
- Push side effects (I/O, network, logging) to the edges; keep the core pure.

## KISS

Simplest thing that works. Boring over clever — clever is what someone decodes at 3am.

## DRY

Reuse what already exists before writing it. One source of truth per fact. But don't abstract two things that merely look alike — wait for the third real case.

## YAGNI

Build only what's needed now. No speculative abstraction, no config for a value that never changes, no "for later" scaffolding.

## SOLID, functionally

- **S** — Single Responsibility: one function, one job.
- **O** — Open/Closed: add behaviour by writing a new function, never by editing the old one. Two ways, both leave the original untouched:
  - a new function that calls the old one (default, simplest) — `const createSquare = (w) => createRectangle(w, w)`
  - a higher-order function, only when behaviour itself is a parameter — `const withLog = (f) => (x) => { log(x); return f(x) }`
- **L** — Liskov Substitution: a function must honour its type's contract; no surprise shapes, no throw-instead-of-return. Immutable values keep this free: a `{w, h}` square is a valid rectangle everywhere, because nothing can mutate it out of contract.
- **I** — Interface Segregation: narrow signatures. Take only the arguments you use, not a whole context object — `greet(name)`, not `greet(user)`.
- **D** — Dependency Inversion: depend on function parameters (injected), not hard-wired concrete implementations. Pass `insert` in; don't reach for `db` inside. Side effects go to the edges.

## Order of preference (before writing code)

1. Does it need to exist? If speculative, skip it.
2. Already in this codebase? Reuse it.
3. Stdlib / native feature does it? Use it.
4. Existing dependency solves it? Use it — don't add a new one for a few lines.
5. Can it be one line? One line.
6. Only then: the minimum code that works.
