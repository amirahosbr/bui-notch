# bui-notch

A Dynamic Island for the Mac notch.

A black panel hangs from the top of your screen and merges with the physical
notch. Collapsed it fills the menu-bar row either side of the notch with a few
glanceable readings — Claude session usage, to-dos due today, commits today, live
agents, and how long until the session limit resets. Rest the cursor on it and it
unfurls into a small panel; move away and it shrinks back. Click it to hold it
open, and a green mark appears to say it's held.

It deliberately shows **no clock and no battery**: macOS already puts both in the
menu bar a few hundred points to the right, and two bare percentages side by side
can't be told apart. The Day card inside the panel has both, along with the date
and how much of the day is gone.

It draws *over* the menu bar, follows you across Spaces, and lets clicks through
everywhere except the sliver itself, so it never gets in the way of the menu bar
underneath.

Requires macOS. A notch is not required — without one the sliver falls back to a
fixed width and sits in the menu-bar row all the same.

## Install

You need [Rust](https://rustup.rs) (`brew install rustup && rustup default stable`).

```bash
git clone https://github.com/amirahosbr/bui-notch
cd bui-notch
cargo install --path crates/notch-app   # the panel
cargo install --path crates/notch       # the CLI that configures it
```

Then run it:

```bash
notch-app
```

The panel appears immediately. There is no dock icon and no menu-bar icon — the
panel is the whole interface.

### Keep it running

```bash
./scripts/install-launchagent.sh
```

Starts it at login and brings it back if it crashes. Logs go to
`~/Library/Logs/notch-app.log`. Remove it with `--uninstall`.

## Use it

Hover the notch to open the panel. Five tabs:

* **Overview** — Claude usage, the clock/date/day progress/battery, a
  contributions strip, and the newest few coding sessions.
* **To-do** — today, this week, in progress, done.
* **Usage** — session and weekly limits in detail, plus one row per reset window.
* **Sessions** — every live Claude Code session.
* **Git** — the full 90-day contribution grid, totals, and recent pushes.

Rest the cursor on a tab pill to switch to it (or click it). Click the sliver to
pin the panel open so it stops following your cursor; click again to release it,
or use `notch pin`.

Only the clock/battery module is on out of the box — everything else needs
something you may not have, so it ships off. Turn on what you want:

```bash
notch module usage on      # needs a Claude Code OAuth token
notch module git on        # needs `gh`, authenticated
notch module sessions on   # needs ~/.claude/projects
notch module todos on      # needs a producer writing todos.json
```

## Is it working?

```bash
notch --version
notch doctor         # every integration, and what to run about each one
notch doctor --json  # same, for scripts (exits 1 on a real failure)
```

Most of what the modules need lives outside this app — a running panel, the
LaunchAgent, a token, `gh` on a PATH launchd doesn't provide, a producer writing a
file. Each fails quietly and separately, so a blank card looks the same whichever
one it was. `doctor` asks all of them at once, skips the checks belonging to
switched-off modules, and names the command that fixes each failure.

## The to-do briefing

The `todos` module renders a JSON file and nothing more. It holds no credentials and
never talks to Slack, Gmail, or any tracker — so this app never has to ask anyone for
access to their messages.

Something else writes that file. Shipped here is `/todo-brief`, a Claude Code
command that reads **Slack and Gmail** through the connectors your own session
already has, picks out the week's action items, sorts them into today / this week /
in progress / done, and writes the briefing:

```bash
notch module todos on
# then, in a Claude Code session with the Slack and Gmail connectors:
/todo-brief
```

Any producer will do, as long as it writes the right shape in the right place:

```bash
notch todos           # print the briefing, and how old it is
notch todos path      # the file to write
notch todos schema    # an example document to copy
notch todos clear      # delete it
```

### It cannot be scheduled

`/todo-brief` only works in a session that **actually has the connectors**. A
headless `claude -p` has no Slack or Gmail tools at all, so cron cannot drive it —
it would either fail or, worse, invent items to fill the sections. The command
checks for the tools first and refuses rather than guessing.

This is a limitation of the design, not a bug: keeping the credentials out of the
HUD is the whole point, and the price is that the briefing is produced by hand from
a session that has them. If only one connector is present the command carries on
with that source alone and records which one it used in `source`, so the HUD never
claims coverage it didn't have.

The HUD shows the briefing's age and marks it **stale after 36 hours**, so an old
briefing announces itself rather than quietly passing as today's.

## What leaves your machine

Three of the five modules read nothing but your own computer. Two make network
calls, and it's worth knowing which:

| Module | Where its data comes from |
| --- | --- |
| `day` | The system clock, and `pmset` for the battery. Local. |
| `sessions` | The transcript files in `~/.claude/projects`. Local. |
| `todos` | A JSON file another process writes. Local — the HUD itself talks to neither Slack nor Gmail. |
| `usage` | **A request to `api.anthropic.com`.** |
| `git` | **`gh api` calls to GitHub.** |

**`usage`** sends a throwaway 1-token request to `/v1/messages` and reads the
rate-limit *response headers* — that is where the percentages and reset times come
from, so they are Anthropic's numbers rather than a guess. Your token is only read
locally (the macOS keychain, then `~/.claude/.credentials.json`) and is never
logged or included in an error message. Each reading is appended to
`usage-history.jsonl` beside your settings, which is what the reset-window list is
built from; nothing uploads it.

Note that this probe **spends a little of the quota it is measuring**. It's cached
for 60 seconds, so at most one request a minute while the module is on.

**`git`** shells out to `gh`, which reads your contribution calendar, your recent
events and your open PRs from GitHub. Nothing is read from your local repositories
— the calendar is used precisely because it counts commits on branches that were
never merged, which neither a local scan nor GitHub's commit search would show.

Neither module runs at all while it is switched off, and both are off by default.

## Configure it

```bash
notch                       # what's on, and every switch you have
notch off                   # hide the panel (the app keeps running)
notch on
notch toggle

notch delay 0               # open the moment the cursor touches the notch
notch delay 600             # or make it wait 600ms (the default)

notch module day off        # turn a module off
notch module day on

notch pin on                # hold the panel open
notch pin off
```

Settings live in `~/Library/Application Support/bui-notch/notch.json`. A running
app picks up changes within about five seconds, so you never need to restart it.

## How it works

Three crates:

```
crates/
  notch-core/   settings, the modules, and the JSON payload the panel renders
  notch/        the `notch` CLI, which only ever writes the settings file
  notch-app/    the panel itself (Tauri v2) — window mechanics + the web UI
```

Each module is a `payload()` returning `available: false` with a reason rather than
failing the whole document, so one broken integration costs one card.

Two macOS details make it behave like a notch app rather than a floating window:
the window level is raised to `NSStatusWindowLevel` so it draws over the menu bar
instead of under it, and its collection behaviour marks it stationary and joinable
on all Spaces.

Some less obvious choices, each of which took a while to arrive at:

* **The window never resizes.** A window resize can't be animated — every step is
  a discrete jump, and the webview relayouts on each one. So the window is fixed
  at the open size and CSS animates the black shape inside it, on the GPU.
* **Hover is sampled, not evented.** WebKit's tracking areas are scoped to the key
  window, and this panel deliberately never takes focus, so `mouseenter` would
  only fire after you clicked it. A worker thread samples the cursor through
  CoreGraphics instead — fast near the top edge, slow elsewhere, because idle
  wake-ups are what cost battery.
* **Clicks pass through except on the sliver.** Collapsed, the window covers a
  large transparent area; if it accepted clicks it would swallow menu-bar clicks.
  So it ignores the cursor until the cursor is actually over the sliver.
* **Opening waits for the cursor to settle.** Crossing the notch on the way to the
  menu bar shouldn't pop the panel open — only stopping there should.

## Develop

```bash
cargo run -p notch-app          # run the panel
cargo run -p notch -- <cmd>     # run the CLI (note the `--`)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
npx prettier --write "crates/notch-app/ui/*.{html,js}"
```

`NOTCH_DEBUG=1` traces the panel's geometry and every open/close to stderr, which
is the only way to see what the hover watcher is thinking.

## Licence

MIT
