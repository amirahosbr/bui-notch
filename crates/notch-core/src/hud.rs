//! The HUD's data payload — one JSON document holding everything the panel
//! renders, so the webview asks for a single thing instead of one call per
//! module.
//!
//! A module switched off in [`config::NotchConfig`] is present as `null` rather
//! than missing, so the panel can hide its card without guessing whether the
//! module failed or was never asked for.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::{config, day};

/// Pinned open until something says otherwise — a click on the sliver. Lives
/// here rather than in the app so the payload and the panel read one flag.
static PINNED: AtomicBool = AtomicBool::new(false);

/// Whether the panel is pinned open.
pub fn pinned() -> bool {
    PINNED.load(Ordering::Relaxed)
}

/// Sets the pin, returning the new state.
pub fn set_pinned(on: bool) -> bool {
    PINNED.store(on, Ordering::Relaxed);
    on
}

/// Flips the pin, returning the new state.
pub fn toggle_pinned() -> bool {
    set_pinned(!pinned())
}

/// Everything the HUD needs, with disabled modules present as `null`.
pub fn payload() -> Value {
    let cfg = config::load();
    assemble(&cfg, cfg.day.then(day::payload), pinned())
}

/// The payload, with every reading handed in. Pure, so the shape the panel
/// depends on can be tested without a clock, a battery or a config file.
fn assemble(cfg: &config::NotchConfig, day: Option<Value>, pinned: bool) -> Value {
    json!({
        "config": cfg,
        "pill": pill(day.as_ref()),
        "day": day,
        "pinned": pinned,
    })
}

/// The few values the collapsed sliver shows, pre-reduced so the strip never has
/// to dig through the full payload. Any of them may be absent.
fn pill(day: Option<&Value>) -> Value {
    json!({
        "clock": day.and_then(|d| d["clock"].as_str()).unwrap_or_default(),
        "meridiem": day.and_then(|d| d["meridiem"].as_str()).unwrap_or_default(),
        "day_pct": day.and_then(|d| d["progress"].as_f64()),
        "battery_pct": day.and_then(|d| d["battery"]["percent"].as_u64()),
        "charging": day
            .and_then(|d| d["battery"]["charging"].as_bool())
            .unwrap_or(false),
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
    fn an_enabled_module_is_present() {
        let v = assemble(&NotchConfig::default(), Some(a_day()), false);
        assert_eq!(v["day"]["clock"], json!("2:40"));
        assert_eq!(v["pinned"], json!(false));
        assert_eq!(v["config"]["enabled"], json!(true));
    }

    #[test]
    fn a_disabled_module_is_null_not_missing() {
        let cfg = NotchConfig::default().with("day", false).unwrap();
        let v = assemble(&cfg, None, false);
        assert!(v["day"].is_null(), "the panel can tell it was switched off");
        assert!(
            v.as_object().unwrap().contains_key("day"),
            "the key stays, so the shape never changes"
        );
    }

    #[test]
    fn the_pill_reduces_the_day_module() {
        let p = pill(Some(&a_day()));
        assert_eq!(p["clock"], json!("2:40"));
        assert_eq!(p["meridiem"], json!("PM"));
        assert_eq!(p["day_pct"], json!(61.1));
        assert_eq!(p["battery_pct"], json!(95));
        assert_eq!(p["charging"], json!(false));
    }

    #[test]
    fn the_pill_tolerates_a_missing_module() {
        let p = pill(None);
        assert_eq!(p["clock"], json!(""), "an empty string, not null");
        assert_eq!(p["meridiem"], json!(""));
        assert!(p["day_pct"].is_null());
        assert!(p["battery_pct"].is_null());
        assert_eq!(p["charging"], json!(false));
    }

    #[test]
    fn the_pill_tolerates_a_module_with_no_battery() {
        let day = json!({ "clock": "9:00", "meridiem": "AM", "progress": 37.5, "battery": null });
        let p = pill(Some(&day));
        assert_eq!(p["clock"], json!("9:00"));
        assert!(p["battery_pct"].is_null(), "a desktop has no battery");
        assert_eq!(p["charging"], json!(false));
    }

    #[test]
    fn payload_has_every_key_the_panel_reads() {
        let v = payload();
        for key in ["config", "pill", "day", "pinned"] {
            assert!(
                v.as_object().unwrap().contains_key(key),
                "payload is missing {key}"
            );
        }
    }
}
