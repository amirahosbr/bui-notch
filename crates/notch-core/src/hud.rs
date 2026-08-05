//! The HUD's data payload — one JSON document holding everything the panel
//! renders, so the webview asks for a single thing instead of one call per module.
//!
//! A module switched off in [`config::NotchConfig`] is present as `null` rather
//! than missing, so the panel can hide its card without guessing whether the
//! module failed or was never asked for. A module that is on but could not read
//! anything answers `available: false` with a reason.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::{config, day, git, sessions, todos, usage};

/// The live pin, mirroring `config.pinned`.
///
/// The hover watcher asks up to eleven times a second, which is far too often to
/// read a file, so the flag is cached here. [`config`] stays the source of truth:
/// every setter writes it, and [`sync_pin`] adopts a change made by a terminal.
static PINNED: AtomicBool = AtomicBool::new(false);

/// Whether the panel is pinned open.
pub fn pinned() -> bool {
    PINNED.load(Ordering::Relaxed)
}

/// Sets the pin and persists it, returning the new state.
///
/// A failed write still moves the live flag: the click the user just made should
/// take effect even if the settings file is unwritable.
pub fn set_pinned(on: bool) -> bool {
    PINNED.store(on, Ordering::Relaxed);
    let _ = config::save(&config::load().with("pinned", on).unwrap_or_default());
    on
}

/// Flips the pin, returning the new state.
pub fn toggle_pinned() -> bool {
    set_pinned(!pinned())
}

/// Adopts `config.pinned` into the live flag, so `notch pin` from a terminal lands.
///
/// Called on the app's reconcile tick. After a click the two already agree, so
/// this is a no-op and the click cannot be undone by it.
pub fn sync_pin(cfg: &config::NotchConfig) {
    PINNED.store(cfg.pinned, Ordering::Relaxed);
}

/// Every module's reading, with the disabled ones absent.
///
/// Grouped rather than passed positionally, since five `Option<Value>` in a row is
/// an easy thing to scramble.
#[derive(Default)]
pub struct Modules {
    pub day: Option<Value>,
    pub usage: Option<Value>,
    pub git: Option<Value>,
    pub sessions: Option<Value>,
    pub todos: Option<Value>,
}

/// Everything the HUD needs, with disabled modules present as `null`.
pub fn payload() -> Value {
    let cfg = config::load();
    let modules = Modules {
        day: cfg.day.then(day::payload),
        usage: cfg.usage.then(usage::payload),
        git: cfg.git.then(git::payload),
        sessions: cfg.sessions.then(sessions::payload),
        todos: cfg.todos.then(todos::payload),
    };
    assemble(&cfg, &modules, pinned())
}

/// The payload, with every reading handed in. Pure, so the shape the panel depends
/// on can be tested without a clock, a network or a config file.
fn assemble(cfg: &config::NotchConfig, m: &Modules, pinned: bool) -> Value {
    json!({
        "config": cfg,
        "pill": pill(m),
        "day": m.day,
        "usage": m.usage,
        "git": m.git,
        "sessions": m.sessions,
        "todos": m.todos,
        "pinned": pinned,
    })
}

/// The few values the collapsed sliver shows, pre-reduced so the strip never has
/// to dig through the full payload. Any of them may be absent.
fn pill(m: &Modules) -> Value {
    let available = |v: &Option<Value>| {
        v.as_ref()
            .filter(|v| v["available"] == json!(true))
            .cloned()
    };
    let usage = available(&m.usage);
    let git = available(&m.git);
    let todos = available(&m.todos);

    // No clock here on purpose: macOS already puts one in the menu bar a few
    // hundred points away, so the strip would only be printing it twice. The Day
    // card inside the panel still shows it.
    json!({
        "day_pct": m.day.as_ref().and_then(|d| d["progress"].as_f64()),
        "battery_pct": m.day.as_ref().and_then(|d| d["battery"]["percent"].as_u64()),
        "charging": m.day
            .as_ref()
            .and_then(|d| d["battery"]["charging"].as_bool())
            .unwrap_or(false),
        "session_pct": usage.as_ref().and_then(|u| u["session"]["percent"].as_f64()),
        "resets_in": usage
            .as_ref()
            .and_then(|u| u["session"]["resets_in"].as_str())
            .unwrap_or_default(),
        "commits": git.as_ref().and_then(|g| g["today"].as_u64()),
        "agents": m.sessions.as_ref().and_then(|s| s["active"].as_u64()),
        // Only what needs doing today; the rest is a tab away.
        "todos": todos.as_ref().and_then(|t| t["today_count"].as_u64()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NotchConfig;

    fn a_day() -> Value {
        json!({
            "clock": "2:40",
            "meridiem": "PM",
            "date": "Wed, 5 Aug",
            "progress": 61.1,
            "remaining": "9h 20m left",
            "week_progress": 37.3,
            "battery": { "percent": 95, "state": "discharging", "charging": false },
        })
    }

    fn an_hour_of_usage() -> Value {
        json!({
            "available": true,
            "session": { "percent": 42.0, "resets_in": "3h52m", "resets_at": "2:40 AM" },
            "week": { "percent": 18.0, "resets_in": "4d", "resets_at": "Mon 9:00 AM" },
            "status": "allowed",
        })
    }

    #[test]
    fn the_pin_round_trips() {
        // A process-wide flag, so put it back rather than leaving it set for
        // whichever test runs next on this thread pool.
        let before = pinned();
        assert!(set_pinned(true));
        assert!(pinned());
        assert!(!set_pinned(false));
        assert!(!pinned());
        assert!(toggle_pinned(), "toggle turns it back on");
        set_pinned(before);
    }

    #[test]
    fn every_module_key_is_present_even_when_off() {
        let v = assemble(&NotchConfig::default(), &Modules::default(), false);
        let obj = v.as_object().unwrap();
        for key in [
            "config", "pill", "day", "usage", "git", "sessions", "todos", "pinned",
        ] {
            assert!(obj.contains_key(key), "payload is missing {key}");
        }
        for key in ["day", "usage", "git", "sessions", "todos"] {
            assert!(v[key].is_null(), "{key} is off, so it must be null");
        }
    }

    #[test]
    fn an_enabled_module_is_present() {
        let m = Modules {
            day: Some(a_day()),
            ..Default::default()
        };
        let v = assemble(&NotchConfig::default(), &m, false);
        assert_eq!(v["day"]["clock"], json!("2:40"));
        assert_eq!(v["pinned"], json!(false));
    }

    #[test]
    fn the_pill_reduces_every_module() {
        let m = Modules {
            day: Some(a_day()),
            usage: Some(an_hour_of_usage()),
            git: Some(json!({ "available": true, "today": 7 })),
            sessions: Some(json!({ "available": true, "active": 2, "total": 5 })),
            todos: Some(json!({ "available": true, "today_count": 3 })),
        };
        let p = pill(&m);
        assert_eq!(p["day_pct"], json!(61.1));
        assert_eq!(p["battery_pct"], json!(95));
        assert_eq!(p["session_pct"], json!(42.0));
        assert_eq!(p["resets_in"], json!("3h52m"));
        assert_eq!(p["commits"], json!(7));
        assert_eq!(p["agents"], json!(2));
        assert_eq!(p["todos"], json!(3));
    }

    #[test]
    fn the_pill_tolerates_every_module_being_absent() {
        let p = pill(&Modules::default());
        assert_eq!(p["resets_in"], json!(""), "an empty string, not null");
        for key in [
            "day_pct",
            "battery_pct",
            "session_pct",
            "commits",
            "agents",
            "todos",
        ] {
            assert!(p[key].is_null(), "{key} should be null");
        }
        assert_eq!(p["charging"], json!(false));
    }

    #[test]
    fn the_pill_ignores_a_module_that_could_not_read_anything() {
        // An unavailable module must not contribute a number the strip would show
        // as if it were real.
        let m = Modules {
            usage: Some(json!({ "available": false, "error": "no token" })),
            git: Some(json!({ "available": false, "pending": true })),
            todos: Some(json!({ "available": false, "missing": true })),
            ..Default::default()
        };
        let p = pill(&m);
        assert!(p["session_pct"].is_null());
        assert_eq!(p["resets_in"], json!(""));
        assert!(p["commits"].is_null());
        assert!(p["todos"].is_null());
    }

    #[test]
    fn the_pill_carries_no_clock() {
        // macOS already shows one in the menu bar; the strip must not print it
        // twice, so the reduction should not offer it at all.
        let m = Modules {
            day: Some(a_day()),
            ..Default::default()
        };
        let p = pill(&m);
        assert!(p["clock"].is_null(), "the strip has no clock");
        assert!(p["meridiem"].is_null());
        assert_eq!(p["day_pct"], json!(61.1), "the meter still gets the day");
    }

    #[test]
    fn the_pill_shows_no_agents_when_none_are_active() {
        let m = Modules {
            sessions: Some(json!({ "available": true, "active": 0, "total": 4 })),
            ..Default::default()
        };
        assert_eq!(pill(&m)["agents"], json!(0));
    }

    #[test]
    fn payload_carries_the_config_the_panel_renders_against() {
        let v = payload();
        assert!(v["config"]["enabled"].is_boolean());
        assert!(v["config"]["open_delay_ms"].is_u64());
    }
}
