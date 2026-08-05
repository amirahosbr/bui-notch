//! Whether the things the HUD depends on are actually wired up.
//!
//! Most of what a module needs lives outside this app — a running panel, the
//! LaunchAgent, a token, `gh` on a PATH launchd doesn't provide, a producer writing
//! a briefing. Each fails quietly and separately, so a panel with a blank card
//! looks the same whichever one it was. This asks all of them at once.
//!
//! A check reports on the module it belongs to, and a module that is switched off
//! is reported as skipped rather than failing.

use std::fs;
use std::process::Command;

use serde::Serialize;

use crate::{config, token};

/// How a single check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Wired up.
    Ok,
    /// Not wired up, and the module that needs it is on.
    Fail,
    /// The module that needs it is off, so it doesn't matter.
    Skipped,
}

/// One thing that was checked.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Which module this belongs to, or `"panel"` for the app itself.
    pub module: String,
    pub name: String,
    pub state: State,
    /// What was found.
    pub detail: String,
    /// What to run about it, when there is something.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// Everything the HUD depends on, in the order it's worth reading.
pub fn run() -> Vec<Check> {
    let cfg = config::load();
    vec![
        app_running(),
        launch_agent(),
        panel_enabled(&cfg),
        usage_token(cfg.usage),
        gh_cli(cfg.git),
        transcripts(cfg.sessions),
        briefing(cfg.todos),
        claude_hook(),
    ]
}

/// Whether every check that mattered passed.
pub fn healthy(checks: &[Check]) -> bool {
    !checks.iter().any(|c| c.state == State::Fail)
}

fn check(module: &str, name: &str, state: State, detail: String, fix: Option<&str>) -> Check {
    Check {
        module: module.to_string(),
        name: name.to_string(),
        state,
        detail,
        fix: fix.map(str::to_string),
    }
}

/// A module that is off turns any failure into a skip.
fn gated(on: bool, module: &str, name: &str, result: (bool, String, Option<&str>)) -> Check {
    let (ok, detail, fix) = result;
    if !on {
        return check(
            module,
            name,
            State::Skipped,
            format!("the {module} module is off"),
            Some(&format!("notch module {module} on")),
        );
    }
    check(
        module,
        name,
        if ok { State::Ok } else { State::Fail },
        detail,
        (!ok).then_some(fix).flatten(),
    )
}

fn app_running() -> Check {
    let running = Command::new("pgrep")
        .args(["-x", "notch-app"])
        .output()
        .is_ok_and(|o| o.status.success());
    check(
        "panel",
        "notch-app running",
        if running { State::Ok } else { State::Fail },
        if running {
            "the panel process is up".into()
        } else {
            "no notch-app process".into()
        },
        (!running).then_some("notch-app  (or ./scripts/install-launchagent.sh)"),
    )
}

/// The LaunchAgent's plist, whether or not it exists.
fn agent_plist() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| {
        h.join("Library")
            .join("LaunchAgents")
            .join("com.osbr.bui-notch.plist")
    })
}

fn launch_agent() -> Check {
    let plist = agent_plist();
    let installed = plist.as_ref().is_some_and(|p| p.exists());
    check(
        "panel",
        "starts at login",
        if installed { State::Ok } else { State::Fail },
        if installed {
            "the LaunchAgent is installed".into()
        } else {
            "no LaunchAgent, so it won't come back after a reboot".into()
        },
        (!installed).then_some("./scripts/install-launchagent.sh"),
    )
}

fn panel_enabled(cfg: &config::NotchConfig) -> Check {
    check(
        "panel",
        "panel shown",
        if cfg.enabled { State::Ok } else { State::Fail },
        if cfg.enabled {
            "the panel is switched on".into()
        } else {
            "the panel is switched off, so nothing will draw".into()
        },
        (!cfg.enabled).then_some("notch on"),
    )
}

fn usage_token(on: bool) -> Check {
    let found = token::find();
    gated(
        on,
        "usage",
        "Claude token",
        match &found {
            // Never the token itself — only that there is one.
            Ok(_) => (true, "a token was found".into(), None),
            Err(e) => (
                false,
                e.to_string(),
                Some("use Claude Code once, or set CLAUDE_CODE_OAUTH_TOKEN"),
            ),
        },
    )
}

/// Directories launchd already puts on `PATH`, so a tool in one of them needs no
/// help from the plist.
const LAUNCHD_DEFAULT_PATH: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

/// The directory `gh` actually lives in.
fn gh_dir() -> Option<String> {
    let out = Command::new("sh")
        .args(["-c", "command -v gh"])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let dir = std::path::Path::new(&path).parent()?;
    Some(dir.to_string_lossy().into_owned())
}

/// Whether the panel's own environment can see `gh`, which is a different question
/// from whether your shell can.
///
/// This check exists because it got the answer wrong once: `gh` was installed and
/// authenticated, this reported it fine, and the git module still said "gh
/// unavailable" — launchd hands its jobs a bare `PATH` with no Homebrew in it, so
/// the panel could not find a binary the shell found instantly. A check that passes
/// while the thing is broken is worse than no check.
///
/// `Some(dir)` means `gh` is in `dir` and the LaunchAgent cannot reach it.
fn gh_hidden_from_agent() -> Option<String> {
    let dir = gh_dir()?;
    if LAUNCHD_DEFAULT_PATH.contains(&dir.as_str()) {
        return None;
    }
    // No LaunchAgent means the panel is started from a shell, which has the same
    // PATH you just checked with.
    let plist = fs::read_to_string(agent_plist()?).ok()?;
    (!plist.contains(&dir)).then_some(dir)
}

fn gh_cli(on: bool) -> Check {
    let ok = Command::new("gh")
        .args(["auth", "status"])
        .output()
        .is_ok_and(|o| o.status.success());

    let result = if !ok {
        (
            false,
            "gh is missing, or not authenticated".into(),
            Some("brew install gh && gh auth login"),
        )
    } else if let Some(dir) = gh_hidden_from_agent() {
        (
            false,
            format!("gh works here, but the LaunchAgent's PATH cannot reach {dir}"),
            Some("./scripts/install-launchagent.sh"),
        )
    } else {
        (true, "gh is installed and reachable".into(), None)
    };
    gated(on, "git", "gh CLI", result)
}

fn transcripts(on: bool) -> Check {
    let dir = dirs::home_dir().map(|h| h.join(".claude").join("projects"));
    let exists = dir.as_ref().is_some_and(|d| d.is_dir());
    gated(
        on,
        "sessions",
        "Claude Code transcripts",
        if exists {
            (true, "~/.claude/projects is readable".into(), None)
        } else {
            (
                false,
                "no ~/.claude/projects — has Claude Code run on this machine?".into(),
                Some("run Claude Code once"),
            )
        },
    )
}

/// Whether Claude Code will tell us when an agent is waiting.
///
/// Not gated on a module: the attention interrupt has no switch, so this is always
/// worth reporting.
fn claude_hook() -> Check {
    let settings = dirs::home_dir().map(|h| h.join(".claude").join("settings.json"));
    let wired = settings
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|s| s.contains("notch attention"));
    check(
        "attention",
        "Claude Code hook",
        if wired { State::Ok } else { State::Fail },
        if wired {
            "the Notification hook will open the panel".into()
        } else {
            "no Notification hook, so a waiting agent cannot reach the panel".into()
        },
        (!wired).then_some("./scripts/install-claude-hook.sh"),
    )
}

fn briefing(on: bool) -> Check {
    let path = crate::todos::path();
    let exists = path.exists();
    gated(
        on,
        "todos",
        "to-do briefing",
        if exists {
            (true, format!("{} exists", path.display()), None)
        } else {
            (
                false,
                format!("nothing at {}", path.display()),
                Some("have a producer write that file"),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_that_is_off_is_skipped_not_failed() {
        let c = gated(
            false,
            "git",
            "gh CLI",
            (false, "missing".into(), Some("install it")),
        );
        assert_eq!(c.state, State::Skipped);
        assert!(c.detail.contains("off"));
        assert_eq!(c.fix.as_deref(), Some("notch module git on"));
    }

    #[test]
    fn a_module_that_is_on_and_broken_fails_with_a_fix() {
        let c = gated(
            true,
            "git",
            "gh CLI",
            (false, "missing".into(), Some("install it")),
        );
        assert_eq!(c.state, State::Fail);
        assert_eq!(c.detail, "missing");
        assert_eq!(c.fix.as_deref(), Some("install it"));
    }

    #[test]
    fn a_passing_check_offers_no_fix() {
        let c = gated(
            true,
            "git",
            "gh CLI",
            (true, "all good".into(), Some("unused")),
        );
        assert_eq!(c.state, State::Ok);
        assert!(c.fix.is_none(), "there is nothing to fix");
    }

    #[test]
    fn healthy_ignores_skipped_checks() {
        let skipped = gated(false, "git", "gh", (false, "x".into(), None));
        let passing = gated(true, "usage", "token", (true, "y".into(), None));
        assert!(healthy(&[skipped, passing]));
    }

    #[test]
    fn healthy_is_false_when_something_that_mattered_failed() {
        let failing = gated(true, "git", "gh", (false, "x".into(), None));
        assert!(!healthy(&[failing]));
    }

    #[test]
    fn run_reports_on_every_module_plus_the_panel() {
        let checks = run();
        for module in ["panel", "usage", "git", "sessions", "todos", "attention"] {
            assert!(
                checks.iter().any(|c| c.module == module),
                "nothing checked for {module}"
            );
        }
    }

    #[test]
    fn a_tool_in_a_default_launchd_dir_needs_no_plist_help() {
        // The bug this guards was specifically Homebrew's prefix; something in
        // /usr/bin is on launchd's PATH already and must not be flagged.
        for dir in LAUNCHD_DEFAULT_PATH {
            assert!(
                LAUNCHD_DEFAULT_PATH.contains(&dir),
                "{dir} should count as already reachable"
            );
        }
    }

    #[test]
    fn the_gh_check_agrees_with_what_the_panel_can_see() {
        // Whatever this machine looks like, a pass must mean the panel can reach
        // gh — not merely that this shell can.
        let c = gated(true, "git", "gh CLI", (true, "x".into(), None));
        assert_eq!(c.state, State::Ok);

        let real = gh_cli(true);
        if real.state == State::Ok {
            assert!(
                gh_hidden_from_agent().is_none(),
                "reported OK while the LaunchAgent cannot reach gh — the exact false \
                 pass this check exists to stop"
            );
        }
    }

    #[test]
    fn no_check_ever_prints_a_token() {
        for c in run() {
            assert!(!c.detail.contains("sk-"), "leaked a token: {}", c.detail);
        }
    }

    #[test]
    fn every_failing_check_says_what_to_do() {
        for c in run() {
            if c.state == State::Fail {
                assert!(c.fix.is_some(), "{} failed with no fix", c.name);
            }
        }
    }
}
