//! The `day` module: clock, date, how much of the day and week is gone, and the
//! battery.
//!
//! Everything here is read locally — the clock from the system, the battery from
//! `pmset` — so the HUD has something real to draw with no account, key or
//! network anywhere.

use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Local, Timelike};
use serde_json::{json, Value};

/// How long a `pmset` reading is reused. The panel polls every few seconds;
/// shelling out that often for a number that moves in percent is waste.
const BATTERY_TTL: Duration = Duration::from_secs(60);

const SECS_PER_DAY: f64 = 24.0 * 3600.0;

/// Everything the day module renders.
pub fn payload() -> Value {
    at(Local::now(), battery())
}

/// The module as of `now`, with the battery reading handed in. Both inputs are
/// arguments rather than reads so this stays pure and testable at a fixed
/// instant — [`payload`] is where the clock and `pmset` are actually consulted.
fn at(now: DateTime<Local>, battery: Value) -> Value {
    let elapsed = now.num_seconds_from_midnight() as f64;
    let left = (SECS_PER_DAY - elapsed).max(0.0) as i64;
    // Monday-based, which is how a working week is usually counted.
    let week_elapsed = now.weekday().num_days_from_monday() as f64 * SECS_PER_DAY + elapsed;

    json!({
        // 12-hour, with the meridiem kept separate so it can be styled smaller.
        "clock": now.format("%-I:%M").to_string(),
        "meridiem": now.format("%p").to_string(),
        "date": now.format("%a, %-d %b").to_string(),
        "progress": round1(elapsed / SECS_PER_DAY * 100.0),
        "remaining": format!("{}h {:02}m left", left / 3600, (left % 3600) / 60),
        "week_progress": round1(week_elapsed / (7.0 * SECS_PER_DAY) * 100.0),
        "battery": battery,
    })
}

/// One decimal place, which is all a 4px-tall meter can show.
fn round1(pct: f64) -> f64 {
    (pct * 10.0).round() / 10.0
}

static BATTERY_CACHE: Mutex<Option<(Value, Instant)>> = Mutex::new(None);

/// Battery percentage and charge state, or `null` off macOS and on a desktop
/// with no battery.
fn battery() -> Value {
    if let Ok(guard) = BATTERY_CACHE.lock() {
        if let Some((cached, at)) = guard.as_ref() {
            if at.elapsed() < BATTERY_TTL {
                return cached.clone();
            }
        }
    }
    let fresh = read_battery();
    if let Ok(mut guard) = BATTERY_CACHE.lock() {
        *guard = Some((fresh.clone(), Instant::now()));
    }
    fresh
}

fn read_battery() -> Value {
    let Ok(out) = Command::new("pmset").args(["-g", "batt"]).output() else {
        return Value::Null;
    };
    parse_battery(&String::from_utf8_lossy(&out.stdout))
}

/// Pulls `95%; discharging` out of `pmset -g batt` output.
fn parse_battery(text: &str) -> Value {
    let Some(line) = text.lines().find(|l| l.contains('%')) else {
        return Value::Null;
    };
    let Some((before, after)) = line.split_once('%') else {
        return Value::Null;
    };
    let digits: String = before
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let Ok(percent) = digits.parse::<u32>() else {
        return Value::Null;
    };
    let state = after
        .trim_start_matches(';')
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    json!({
        "percent": percent,
        "state": state,
        "charging": state == "charging" || state == "AC attached" || state == "finishing charge",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Local time at a fixed wall-clock reading, whatever zone the test runs in.
    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("unambiguous local time")
    }

    #[test]
    fn parses_discharging_battery() {
        let out = "Now drawing from 'Battery Power'\n \
                   -InternalBattery-0 (id=22937699)\t95%; discharging; 7:21 remaining present: true";
        let v = parse_battery(out);
        assert_eq!(v["percent"], json!(95));
        assert_eq!(v["state"], json!("discharging"));
        assert_eq!(v["charging"], json!(false));
    }

    #[test]
    fn parses_charging_battery() {
        let out = "Now drawing from 'AC Power'\n \
                   -InternalBattery-0 (id=1)\t100%; charging; 0:00 remaining present: true";
        let v = parse_battery(out);
        assert_eq!(v["percent"], json!(100));
        assert_eq!(v["charging"], json!(true));
    }

    #[test]
    fn ac_attached_counts_as_charging() {
        let out = "-InternalBattery-0\t100%; AC attached; not charging present: true";
        assert_eq!(parse_battery(out)["charging"], json!(true));
    }

    #[test]
    fn no_battery_is_null() {
        assert_eq!(parse_battery("Now drawing from 'AC Power'"), Value::Null);
        assert_eq!(parse_battery(""), Value::Null);
        assert_eq!(parse_battery("nothing useful here"), Value::Null);
    }

    #[test]
    fn clock_is_twelve_hour_with_a_meridiem() {
        let v = at(local(2026, 8, 5, 14, 40), Value::Null);
        assert_eq!(v["clock"], json!("2:40"), "afternoon reads as 2, not 14");
        assert_eq!(v["meridiem"], json!("PM"));
    }

    #[test]
    fn midnight_reads_as_twelve() {
        let v = at(local(2026, 8, 5, 0, 5), Value::Null);
        assert_eq!(v["clock"], json!("12:05"));
        assert_eq!(v["meridiem"], json!("AM"));
    }

    #[test]
    fn progress_tracks_the_day() {
        assert_eq!(
            at(local(2026, 8, 5, 0, 0), Value::Null)["progress"],
            json!(0.0)
        );
        assert_eq!(
            at(local(2026, 8, 5, 12, 0), Value::Null)["progress"],
            json!(50.0)
        );
        assert_eq!(
            at(local(2026, 8, 5, 18, 0), Value::Null)["progress"],
            json!(75.0)
        );
    }

    #[test]
    fn remaining_counts_down_to_midnight() {
        assert_eq!(
            at(local(2026, 8, 5, 22, 30), Value::Null)["remaining"],
            json!("1h 30m left")
        );
        assert_eq!(
            at(local(2026, 8, 5, 0, 0), Value::Null)["remaining"],
            json!("24h 00m left")
        );
    }

    #[test]
    fn week_progress_starts_on_monday() {
        // 3 Aug 2026 is a Monday.
        assert_eq!(
            at(local(2026, 8, 3, 0, 0), Value::Null)["week_progress"],
            json!(0.0)
        );
        // Thursday noon is 3.5 of 7 days.
        assert_eq!(
            at(local(2026, 8, 6, 12, 0), Value::Null)["week_progress"],
            json!(50.0)
        );
    }

    #[test]
    fn the_battery_reading_passes_straight_through() {
        let reading = json!({ "percent": 42, "state": "discharging", "charging": false });
        let v = at(local(2026, 8, 5, 9, 0), reading.clone());
        assert_eq!(v["battery"], reading);
    }

    #[test]
    fn payload_stays_in_range_right_now() {
        let v = payload();
        let day = v["progress"].as_f64().expect("progress is a number");
        let week = v["week_progress"]
            .as_f64()
            .expect("week_progress is a number");
        assert!((0.0..=100.0).contains(&day), "day out of range: {day}");
        assert!((0.0..=100.0).contains(&week), "week out of range: {week}");
        assert!(v["remaining"].as_str().unwrap().ends_with("left"));
        assert!(!v["date"].as_str().unwrap().is_empty());
    }

    #[test]
    fn payload_battery_is_an_object_or_null() {
        // Whatever this machine is, the shape has to be one the panel can read.
        let b = &payload()["battery"];
        assert!(
            b.is_null() || b["percent"].is_u64(),
            "unexpected battery shape: {b}"
        );
    }
}
