# bui-notch

A Dynamic Island for the Mac notch.

A black panel hangs from the top of your screen and merges with the physical
notch. Collapsed it fills the menu-bar row either side of the notch with a clock
and a couple of readings. Rest the cursor on it and it unfurls into a small panel;
move away and it shrinks back. Click it to hold it open.

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
