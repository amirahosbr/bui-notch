//! The HUD's settings: which modules are on, whether the panel shows at all,
//! and how long the cursor must rest on the notch before it opens.
//!
//! Stored as JSON next to nothing else, so a running app and a terminal can both
//! read it. Every change returns a new value — [`save`] is the only side effect,
//! and it lives at the edge.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Which modules the HUD draws. Absent fields fall back to [`Default`], so an
/// older config file keeps working when a module is added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotchConfig {
    /// Whether the panel is shown at all. Off hides it without quitting the app.
    pub enabled: bool,
    /// Clock, date, day and week progress, battery. Needs nothing external.
    pub day: bool,
    /// Claude session and weekly limits. Needs an OAuth token.
    pub usage: bool,
    /// GitHub contributions. Needs the `gh` CLI, authenticated.
    pub git: bool,
    /// Live Claude Code sessions. Needs `~/.claude/projects`.
    pub sessions: bool,
    /// A to-do briefing. Needs a producer writing to `todos.json`.
    pub todos: bool,
    /// Hold the panel open, ignoring the cursor leaving.
    ///
    /// Persisted rather than kept in memory so a terminal can set it: without a
    /// local server there is no other way to reach the running app, and "leave it
    /// open" is a reasonable thing to want to stay set.
    pub pinned: bool,
    /// How long the cursor must rest in the sliver before the panel opens, in
    /// milliseconds. Without this, merely crossing the notch on the way
    /// somewhere else pops the HUD open.
    pub open_delay_ms: u64,
}

/// Default dwell before opening — long enough to ignore a cursor passing
/// through, short enough not to feel laggy when you mean it.
pub const DEFAULT_OPEN_DELAY_MS: u64 = 600;
/// Upper bound, so a typo can't make the HUD look broken.
pub const MAX_OPEN_DELAY_MS: u64 = 5_000;

/// Every module a user can name, in the order the HUD lays them out.
pub const MODULES: [(&str, &str); 5] = [
    ("day", "Clock, date, day progress, battery"),
    ("usage", "Claude session + weekly limits"),
    ("git", "GitHub contributions"),
    ("sessions", "Live Claude Code sessions"),
    ("todos", "To-do briefing"),
];

impl Default for NotchConfig {
    /// Only `day` is on: it is the one module that needs nothing outside itself.
    /// The rest each assume a token, a CLI, or a producer that a fresh install
    /// won't have, and a card reading "unavailable" on first run looks broken
    /// rather than optional. `notch module <name> on` turns them on.
    fn default() -> Self {
        Self {
            enabled: true,
            day: true,
            usage: false,
            git: false,
            sessions: false,
            todos: false,
            pinned: false,
            open_delay_ms: DEFAULT_OPEN_DELAY_MS,
        }
    }
}

/// Path to the settings file.
pub fn config_path() -> PathBuf {
    crate::config_dir().join("notch.json")
}

/// The stored settings, or the defaults when the file is missing or unreadable.
///
/// A corrupt file reads as defaults rather than failing: the HUD losing its
/// settings is a smaller problem than the HUD refusing to draw.
pub fn load() -> NotchConfig {
    fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes the settings out. The one side effect in this module.
pub fn save(cfg: &NotchConfig) -> Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

impl NotchConfig {
    /// Reads one switch by name — a module, or `"enabled"` for the panel itself.
    pub fn get(&self, key: &str) -> Result<bool> {
        match key {
            "enabled" => Ok(self.enabled),
            "day" => Ok(self.day),
            "usage" => Ok(self.usage),
            "git" => Ok(self.git),
            "sessions" => Ok(self.sessions),
            "todos" => Ok(self.todos),
            "pinned" => Ok(self.pinned),
            other => anyhow::bail!("unknown notch module: {other}"),
        }
    }

    /// The same settings with one switch set. The original is untouched.
    pub fn with(&self, key: &str, on: bool) -> Result<Self> {
        let mut next = self.clone();
        match key {
            "enabled" => next.enabled = on,
            "day" => next.day = on,
            "usage" => next.usage = on,
            "git" => next.git = on,
            "sessions" => next.sessions = on,
            "todos" => next.todos = on,
            "pinned" => next.pinned = on,
            other => anyhow::bail!("unknown notch module: {other}"),
        }
        Ok(next)
    }

    /// The same settings with one switch flipped.
    pub fn toggled(&self, key: &str) -> Result<Self> {
        self.with(key, !self.get(key)?)
    }

    /// The same settings with a new dwell, clamped to something sane.
    pub fn with_open_delay(&self, ms: u64) -> Self {
        Self {
            open_delay_ms: ms.min(MAX_OPEN_DELAY_MS),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_show_the_panel_with_only_the_self_contained_module() {
        let cfg = NotchConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.day, "the one module needing nothing external");
        assert_eq!(cfg.open_delay_ms, DEFAULT_OPEN_DELAY_MS);
    }

    #[test]
    fn every_module_needing_setup_is_off_by_default() {
        // A fresh install must not show cards that read "unavailable".
        let cfg = NotchConfig::default();
        for key in ["usage", "git", "sessions", "todos"] {
            assert!(!cfg.get(key).unwrap(), "{key} should ship off");
        }
    }

    #[test]
    fn each_module_switches_independently() {
        let cfg = NotchConfig::default().with("git", true).unwrap();
        assert!(cfg.git);
        assert!(!cfg.usage, "turning one on leaves the others alone");
        assert!(!cfg.sessions);
        assert!(!cfg.todos);
        assert!(cfg.day, "and does not disturb day");
        assert!(cfg.enabled);
    }

    #[test]
    fn with_leaves_the_original_alone() {
        let cfg = NotchConfig::default();
        let off = cfg.with("day", false).unwrap();
        assert!(!off.day, "the new value is set");
        assert!(cfg.day, "the original is untouched");
    }

    #[test]
    fn toggled_flips_one_switch_only() {
        let cfg = NotchConfig::default();
        let flipped = cfg.toggled("day").unwrap();
        assert!(!flipped.day);
        assert_eq!(flipped.enabled, cfg.enabled);
        assert_eq!(flipped.open_delay_ms, cfg.open_delay_ms);
    }

    #[test]
    fn unknown_module_is_an_error_not_a_panic() {
        let cfg = NotchConfig::default();
        assert!(cfg.get("solat").is_err());
        assert!(cfg.with("solat", true).is_err());
        assert!(cfg.toggled("solat").is_err());
    }

    #[test]
    fn open_delay_is_clamped() {
        let cfg = NotchConfig::default();
        assert_eq!(
            cfg.with_open_delay(0).open_delay_ms,
            0,
            "0 opens on contact"
        );
        assert_eq!(cfg.with_open_delay(250).open_delay_ms, 250);
        assert_eq!(
            cfg.with_open_delay(99_999).open_delay_ms,
            MAX_OPEN_DELAY_MS,
            "a typo cannot make the HUD look broken"
        );
    }

    #[test]
    fn every_listed_module_is_readable() {
        let cfg = NotchConfig::default();
        for (key, _) in MODULES {
            assert!(cfg.get(key).is_ok(), "{key} is listed but not readable");
        }
    }

    #[test]
    fn an_unreadable_file_reads_as_defaults() {
        // `load` swallows a missing or corrupt file on purpose; whatever is on
        // this machine, it must hand back something usable.
        let cfg = load();
        assert!(cfg.open_delay_ms <= MAX_OPEN_DELAY_MS);
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = NotchConfig {
            enabled: false,
            day: true,
            usage: true,
            git: false,
            sessions: true,
            todos: false,
            pinned: true,
            open_delay_ms: 120,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert_eq!(serde_json::from_str::<NotchConfig>(&json).unwrap(), cfg);
    }

    #[test]
    fn a_partial_file_fills_in_defaults() {
        let cfg: NotchConfig = serde_json::from_str(r#"{"enabled":false}"#).unwrap();
        assert!(!cfg.enabled, "what was written wins");
        assert!(cfg.day, "what was omitted falls back");
        assert_eq!(cfg.open_delay_ms, DEFAULT_OPEN_DELAY_MS);
    }
}
