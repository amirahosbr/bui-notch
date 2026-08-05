#!/bin/bash
#
# Installs a LaunchAgent so the notch HUD starts at login and comes back if it
# ever crashes. macOS only.
#
#   ./scripts/install-launchagent.sh             # install (or reinstall) and start
#   ./scripts/install-launchagent.sh --uninstall # stop and remove
#
# Nothing here is hardcoded to one machine: the binary is resolved at install
# time and written into the plist.

set -euo pipefail

LABEL="com.osbr.bui-notch"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG="$HOME/Library/Logs/notch-app.log"
DOMAIN="gui/$(id -u)"

if [[ "${1:-}" == "--uninstall" ]]; then
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || launchctl unload "$PLIST" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed $LABEL"
  exit 0
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: LaunchAgents are macOS only" >&2
  exit 1
fi

BIN="$(command -v notch-app || true)"
if [[ -z "$BIN" ]]; then
  echo "error: notch-app is not on PATH — run: cargo install --path crates/notch-app" >&2
  exit 1
fi

# launchd hands processes a bare PATH (/usr/bin:/bin:/usr/sbin:/sbin) with no
# Homebrew in it. The git module shells out to `gh`, which Homebrew installs to
# /opt/homebrew/bin — so without pinning this, that one module reports "gh
# unavailable" while everything else looks fine, and `gh` works perfectly when you
# check it by hand in a shell that does have Homebrew on PATH.
BREW_BIN=""
if command -v brew >/dev/null 2>&1; then
  BREW_BIN="$(brew --prefix)/bin:"
fi
AGENT_PATH="${BREW_BIN}/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

mkdir -p "$(dirname "$PLIST")" "$(dirname "$LOG")"

cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$LABEL</string>

  <key>ProgramArguments</key>
  <array>
    <string>$BIN</string>
  </array>

  <key>RunAtLoad</key>
  <true/>

  <!-- Restart only on a crash. A plain KeepAlive would relaunch the app the
       instant it was asked to quit, since that exits cleanly. -->
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>

  <!-- Without this the git module cannot find gh; see the comment above. -->
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>$AGENT_PATH</string>
  </dict>

  <!-- It draws a UI, so it should not be throttled like a batch job. -->
  <key>ProcessType</key>
  <string>Interactive</string>

  <key>StandardOutPath</key>
  <string>$LOG</string>
  <key>StandardErrorPath</key>
  <string>$LOG</string>
</dict>
</plist>
PLIST_EOF

# Replace any running copy — two panels would sit on top of each other.
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
pkill -x notch-app 2>/dev/null || true
sleep 1

launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl kickstart "$DOMAIN/$LABEL" >/dev/null 2>&1 || true

echo "installed $PLIST"
echo "  binary: $BIN"
echo "  log:    $LOG"
echo
echo "starts at login; restarts on a crash but not when it exits cleanly."
echo "to remove: ./scripts/install-launchagent.sh --uninstall"
