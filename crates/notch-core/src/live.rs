//! Real Claude usage, read from Anthropic's rate-limit response headers.
//!
//! It sends a 1-token throwaway request to `/v1/messages`; the response *headers*
//! carry the session (5h) and weekly (7d) utilisation. The body is discarded —
//! the headers are the whole point.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::token;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

const HDR_SESSION_UTIL: &str = "anthropic-ratelimit-unified-5h-utilization";
const HDR_SESSION_RESET: &str = "anthropic-ratelimit-unified-5h-reset";
const HDR_WEEK_UTIL: &str = "anthropic-ratelimit-unified-7d-utilization";
const HDR_WEEK_RESET: &str = "anthropic-ratelimit-unified-7d-reset";
const HDR_STATUS: &str = "anthropic-ratelimit-unified-status";

/// The limit picture from Anthropic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// 0..100.
    pub session_pct: f64,
    /// 0..100.
    pub week_pct: f64,
    pub session_reset: Option<DateTime<Utc>>,
    pub week_reset: Option<DateTime<Utc>>,
    /// allowed / allowed_warning / rejected.
    pub status: String,
    pub at: DateTime<Utc>,
}

/// Performs one probe and returns the usage it reported.
pub fn fetch() -> Result<Usage> {
    let token = token::find()?;
    let body =
        r#"{"model":"claude-haiku-4-5","max_tokens":1,"messages":[{"role":"user","content":"."}]}"#;

    let resp = reqwest::blocking::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()?
        .post(API_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .body(body)
        .send()?;

    let status = resp.status();
    if status.as_u16() == 401 {
        return Err(anyhow!(
            "auth failed (the token has likely expired) — use Claude Code once to refresh it"
        ));
    }
    if !status.is_success() {
        return Err(anyhow!("Anthropic returned status {}", status.as_u16()));
    }

    let headers = resp.headers();
    let get = |k: &str| {
        headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    if get(HDR_SESSION_UTIL).is_empty() {
        return Err(anyhow!("the response carried no rate-limit headers"));
    }

    let usage = Usage {
        session_pct: pct(&get(HDR_SESSION_UTIL)),
        week_pct: pct(&get(HDR_WEEK_UTIL)),
        session_reset: epoch(&get(HDR_SESSION_RESET)),
        week_reset: epoch(&get(HDR_WEEK_RESET)),
        status: get(HDR_STATUS),
        at: Utc::now(),
    };
    // Best-effort: a failed write must never affect the live reading.
    crate::history::record(&usage);
    Ok(usage)
}

/// Turns Anthropic's utilisation (`"0.26"`) into a percentage (26). Also tolerates
/// an already-percent value in case the format ever changes.
fn pct(v: &str) -> f64 {
    let Ok(parsed) = v.parse::<f64>() else {
        return 0.0;
    };
    let scaled = if parsed <= 1.5 {
        parsed * 100.0
    } else {
        parsed
    };
    scaled.clamp(0.0, 100.0)
}

fn epoch(v: &str) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(v.parse().ok()?, 0)
}

// --- cache ----------------------------------------------------------------

static CACHE: Mutex<Option<(Usage, Instant)>> = Mutex::new(None);

/// A recent [`Usage`], probing only when the cached value is older than `ttl`.
///
/// On a failed probe the last good value is served instead, so a dropped network
/// blanks the card rather than the whole panel. This is what lets the panel poll
/// every few seconds without spending a request each time.
pub fn cached(ttl: Duration) -> Result<Usage> {
    let mut guard = CACHE.lock().map_err(|e| anyhow!("usage cache: {e}"))?;
    if let Some((usage, at)) = guard.as_ref() {
        if at.elapsed() < ttl {
            return Ok(usage.clone());
        }
    }
    match fetch() {
        Ok(usage) => {
            *guard = Some((usage.clone(), Instant::now()));
            Ok(usage)
        }
        Err(e) => match guard.as_ref() {
            Some((stale, _)) => Ok(stale.clone()),
            None => Err(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_converts_a_fraction() {
        assert_eq!(pct("0.26"), 26.0);
        assert_eq!(pct("0.5"), 50.0);
        assert_eq!(pct("1.0"), 100.0);
    }

    #[test]
    fn pct_tolerates_an_already_percent_value() {
        // Above 1.5 it cannot be a fraction, so it is taken as a percentage.
        assert_eq!(pct("26"), 26.0);
        assert_eq!(pct("2.0"), 2.0);
    }

    #[test]
    fn pct_clamps_and_survives_junk() {
        assert_eq!(pct("150"), 100.0);
        assert_eq!(pct("-5"), 0.0, "a negative reading is not shown as one");
        assert_eq!(pct(""), 0.0);
        assert_eq!(pct("nonsense"), 0.0);
    }

    #[test]
    fn epoch_parses_unix_seconds() {
        assert_eq!(epoch("0"), DateTime::from_timestamp(0, 0));
        assert!(epoch("not-a-number").is_none());
        assert!(epoch("").is_none());
    }
}
