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

Hover the notch to open the panel. Two tabs:

* **Overview** — the clock, today's date, how much of the day is gone, the
  battery, and how far into the week you are.
* **Day** — the same readings written out as a list.

Rest the cursor on a tab pill to switch to it (or click it). Click the sliver to
pin the panel open so it stops following your cursor; click again to release it.

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
```

Settings live in `~/Library/Application Support/bui-notch/notch.json`. A running
app picks up changes within about five seconds, so you never need to restart it.

## How it works

Three crates:

```
crates/
  notch-core/   settings, the day module, and the JSON payload the panel renders
  notch/        the `notch` CLI, which only ever writes the settings file
  notch-app/    the panel itself (Tauri v2) — window mechanics + the web UI
```

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
