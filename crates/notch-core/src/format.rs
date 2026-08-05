//! Shared human-friendly formatting: the "resets in / at" durations and the
//! truncation the panel's one-line rows need.

use chrono::{DateTime, Local, Utc};

/// Formats the wall-clock time of `t`: `9:20 PM` if it's today, else
/// `Sat 11:59 PM`. Empty for a missing time.
pub fn clock_at(t: Option<DateTime<Utc>>) -> String {
    let Some(t) = t.map(|t| t.with_timezone(&Local)) else {
        return String::new();
    };
    if t.date_naive() == Local::now().date_naive() {
        t.format("%-I:%M %p").to_string()
    } else {
        t.format("%a %-I:%M %p").to_string()
    }
}

/// Formats the time left until `t` as `2h14m`, `5d`, or `now`.
pub fn until(t: Option<DateTime<Utc>>) -> String {
    until_from(t, Utc::now())
}

/// [`until`] measured from `now`, so it can be tested at a fixed instant.
fn until_from(t: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(t) = t else {
        return "now".to_string();
    };
    let secs = (t - now).num_seconds();
    if secs <= 0 {
        "now".to_string()
    } else if secs >= 24 * 3600 {
        format!("{}d", secs / (24 * 3600))
    } else if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}m", secs / 60)
    }
}

/// Shortens `s` to `max` characters, adding an ellipsis if it was cut.
pub fn trunc(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    chars[..max.saturating_sub(1)].iter().collect::<String>() + "…"
}

/// How long ago `secs` was, as `8s` / `4m` / `1h20m`. Short enough for a 9px
/// column.
pub fn ago(secs: i64) -> String {
    if secs < 0 {
        return String::new();
    }
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        let (h, m) = (secs / 3600, (secs % 3600) / 60);
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h{m}m")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(unix: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(unix, 0).expect("valid timestamp")
    }

    #[test]
    fn until_counts_down_in_useful_units() {
        let now = at(1_000_000);
        assert_eq!(until_from(Some(at(1_000_000 + 90)), now), "1m");
        assert_eq!(until_from(Some(at(1_000_000 + 3_600)), now), "1h0m");
        assert_eq!(until_from(Some(at(1_000_000 + 8_040)), now), "2h14m");
        assert_eq!(until_from(Some(at(1_000_000 + 5 * 86_400)), now), "5d");
    }

    #[test]
    fn until_reads_as_now_when_it_is_past_or_missing() {
        let now = at(1_000_000);
        assert_eq!(until_from(None, now), "now");
        assert_eq!(until_from(Some(at(999_000)), now), "now");
        assert_eq!(until_from(Some(now), now), "now");
    }

    #[test]
    fn clock_at_handles_a_missing_time() {
        assert_eq!(clock_at(None), "");
    }

    #[test]
    fn clock_at_names_the_day_when_it_is_not_today() {
        // Some day in 2001, which is certainly not today.
        let s = clock_at(Some(at(1_000_000_000)));
        assert!(s.contains(':'), "has a clock time: {s}");
        assert!(
            s.split(' ').count() == 3,
            "names the weekday when it is not today: {s}"
        );
    }

    #[test]
    fn trunc_adds_an_ellipsis_only_when_it_cuts() {
        assert_eq!(trunc("short", 22), "short");
        assert_eq!(trunc("abcdef", 4), "abc…");
        assert_eq!(trunc("abcd", 4), "abcd", "exactly the limit is left alone");
    }

    #[test]
    fn trunc_counts_characters_not_bytes() {
        // Naive byte slicing would panic here, or cut a character in half.
        assert_eq!(trunc("日本語のテキスト", 4), "日本語…");
    }

    #[test]
    fn ago_scales() {
        assert_eq!(ago(8), "8s");
        assert_eq!(ago(240), "4m");
        assert_eq!(ago(4_800), "1h20m");
        assert_eq!(ago(7_200), "2h", "a round hour drops the minutes");
        assert_eq!(ago(-1), "", "a clock skew is not a duration");
    }
}
