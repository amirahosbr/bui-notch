//! The `usage` module: Claude session and weekly limits.
//!
//! Needs a Claude Code OAuth token (see [`crate::token`]). Without one the module
//! reports itself unavailable and says why, rather than drawing zeroes that look
//! like real headroom.

use std::time::Duration;

use serde_json::{json, Value};

use crate::{format, history, live};

/// How long Anthropic's headers are reused.
const TTL: Duration = Duration::from_secs(60);
/// How much history the Usage tab summarises.
const WINDOW_SPAN: Duration = Duration::from_secs(7 * 24 * 3600);

/// Everything the usage module renders.
pub fn payload() -> Value {
    match live::cached(TTL) {
        Ok(u) => json!({
            "available": true,
            "session": {
                "percent": u.session_pct,
                "resets_in": format::until(u.session_reset),
                "resets_at": format::clock_at(u.session_reset),
            },
            "week": {
                "percent": u.week_pct,
                "resets_in": format::until(u.week_reset),
                "resets_at": format::clock_at(u.week_reset),
            },
            "status": u.status,
        }),
        Err(e) => json!({ "available": false, "error": e.to_string() }),
    }
}

/// The reset windows the Usage tab lists, with the peaks across them.
///
/// Separate from [`payload`] because it reads a growing log: the panel asks for it
/// only while the Usage tab is actually open.
pub fn windows() -> Value {
    match history::windows(Some(WINDOW_SPAN)) {
        Ok((windows, peaks)) => json!({
            "available": true,
            "windows": windows,
            "session_peak": peaks.session,
            "week_peak": peaks.week,
        }),
        Err(e) => json!({ "available": false, "error": e.to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_always_says_whether_it_is_available() {
        // Whether this machine has a token or not, the panel must be able to tell.
        let v = payload();
        assert!(v["available"].is_boolean(), "got {v}");
        if v["available"] == json!(true) {
            assert!(v["session"]["percent"].is_number());
            assert!(v["week"]["percent"].is_number());
        } else {
            assert!(
                v["error"].as_str().is_some_and(|e| !e.is_empty()),
                "an unavailable module must say why"
            );
        }
    }

    #[test]
    fn an_unavailable_payload_never_reports_a_percentage() {
        // Zeroes would read as "plenty of headroom left", which is the opposite of
        // "we could not find out".
        let v = payload();
        if v["available"] == json!(false) {
            assert!(v["session"].is_null());
            assert!(v["week"].is_null());
        }
    }

    #[test]
    fn windows_are_a_list_even_with_no_history() {
        let v = windows();
        assert_eq!(v["available"], json!(true), "a missing log is not an error");
        assert!(v["windows"].is_array());
        assert!(v["session_peak"].is_number());
        assert!(v["week_peak"].is_number());
    }
}
