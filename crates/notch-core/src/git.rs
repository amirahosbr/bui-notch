//! The `git` module: GitHub contributions via the `gh` CLI.
//!
//! Totals and a 90-day heatmap come from the contribution calendar, recent pushes
//! from the Events API, and open PRs from search. The calendar rather than commit
//! search, because search only indexes the default branch and so misses work still
//! sitting on a branch — the calendar counts it.
//!
//! Needs `gh` installed and authenticated. Every reading shells out three times
//! and hits the network, so nothing here is ever on the panel's critical path:
//! see [`payload`].

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use chrono::{Datelike, Local, NaiveDate};
use serde::Deserialize;
use serde_json::{json, Value};

/// How long a reading is reused.
const TTL: Duration = Duration::from_secs(300);
/// Days of heatmap the payload carries. The overview strip draws the last 30 and
/// the Git tab draws all of it.
const HEATMAP_DAYS: i64 = 90;
/// Cap on the pushes listed, which is more than the tab can show anyway.
const MAX_PUSHES: usize = 50;

const CAL_QUERY: &str = "{viewer{contributionsCollection{contributionCalendar{\
weeks{contributionDays{date contributionCount}}}}}}";

/// Everything the git module renders.
///
/// Never blocks. A cold cache reports itself pending and kicks off a background
/// refresh, so the numbers land on a later poll rather than holding up the other
/// modules for three `gh` round-trips.
pub fn payload() -> Value {
    match cached_if_fresh(TTL) {
        Some(v) => v,
        None => {
            refresh();
            json!({ "available": false, "pending": true })
        }
    }
}

// --- the gh calls ---------------------------------------------------------

#[derive(Deserialize)]
struct Resp {
    data: Data,
}
#[derive(Deserialize)]
struct Data {
    viewer: Viewer,
}
#[derive(Deserialize)]
struct Viewer {
    #[serde(rename = "contributionsCollection")]
    contributions: Contributions,
}
#[derive(Deserialize)]
struct Contributions {
    #[serde(rename = "contributionCalendar")]
    calendar: Calendar,
}
#[derive(Deserialize)]
struct Calendar {
    weeks: Vec<Week>,
}
#[derive(Deserialize)]
struct Week {
    #[serde(rename = "contributionDays")]
    days: Vec<Day>,
}

/// One day of the contribution calendar.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
struct Day {
    /// `YYYY-MM-DD`, which sorts and compares as a string.
    date: String,
    #[serde(rename = "contributionCount")]
    count: u32,
}

fn gh_json(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| anyhow!("the gh CLI is not available: {e}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// A year of per-day contribution counts, oldest first.
fn calendar_days() -> Result<Vec<Day>> {
    let query = format!("query={CAL_QUERY}");
    let raw = gh_json(&["api", "graphql", "-f", &query])?;
    let resp: Resp = serde_json::from_slice(&raw)?;
    Ok(resp
        .data
        .viewer
        .contributions
        .calendar
        .weeks
        .into_iter()
        .flat_map(|w| w.days)
        .collect())
}

static LOGIN: OnceLock<String> = OnceLock::new();

/// The authenticated login, read once — it cannot change under a running app.
fn login() -> Result<String> {
    if let Some(l) = LOGIN.get() {
        return Ok(l.clone());
    }
    let raw = gh_json(&["api", "user", "--jq", ".login"])?;
    let login = String::from_utf8_lossy(&raw).trim().to_string();
    if login.is_empty() {
        return Err(anyhow!("could not determine the GitHub login"));
    }
    let _ = LOGIN.set(login.clone());
    Ok(login)
}

/// Recent pushes across every repo and branch, public and private, from the Events
/// API — this is what surfaces branch work commit search cannot see.
fn recent_pushes(login: &str) -> Vec<Value> {
    (1..=3)
        .map_while(|page| {
            let path = format!("users/{login}/events?per_page=100&page={page}");
            let raw = gh_json(&["api", &path]).ok()?;
            let events: Value = serde_json::from_slice(&raw).ok()?;
            let arr = events.as_array()?.clone();
            (!arr.is_empty()).then_some(arr)
        })
        .flatten()
        .filter(|ev| ev["type"] == json!("PushEvent"))
        .map(|ev| {
            json!({
                "repo": ev["repo"]["name"].as_str().unwrap_or_default(),
                "branch": ev["payload"]["ref"]
                    .as_str()
                    .unwrap_or_default()
                    .strip_prefix("refs/heads/")
                    .unwrap_or_default(),
                "at": ev["created_at"].as_str().unwrap_or_default(),
                "private": !ev["public"].as_bool().unwrap_or(true),
            })
        })
        .take(MAX_PUSHES)
        .collect()
}

/// Open pull requests the user authored — work in flight.
fn open_prs(login: &str) -> Vec<Value> {
    let path = format!("search/issues?q=type:pr+is:open+author:{login}&per_page=30&sort=updated");
    let Ok(raw) = gh_json(&["api", &path]) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<Value>(&raw) else {
        return Vec::new();
    };
    v["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|pr| {
                    json!({
                        "repo": pr["repository_url"]
                            .as_str()
                            .and_then(|u| u.rsplit("/repos/").next())
                            .unwrap_or_default(),
                        "title": pr["title"].as_str().unwrap_or_default(),
                        "url": pr["html_url"].as_str().unwrap_or_default(),
                        "number": pr["number"].as_u64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One full reading: totals, heatmap, pushes, open PRs.
fn activity() -> Result<Value> {
    let days = calendar_days()?;
    let login = login()?;
    Ok(summarise(&days, Local::now().date_naive(), &login)
        .into_iter()
        .chain([
            ("recent_pushes".to_string(), json!(recent_pushes(&login))),
            ("open_prs".to_string(), json!(open_prs(&login))),
        ])
        .collect::<serde_json::Map<_, _>>()
        .into())
}

/// The counting half of a reading. Pure, so the totals can be tested without `gh`.
fn summarise(days: &[Day], today: NaiveDate, login: &str) -> serde_json::Map<String, Value> {
    let ymd = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let today_s = ymd(today);

    let count_on = |date: &str| days.iter().find(|d| d.date == date).map_or(0, |d| d.count);
    let sum_since = |from: NaiveDate| -> u32 {
        let from_s = ymd(from);
        days.iter()
            .filter(|d| d.date.as_str() >= from_s.as_str() && d.date.as_str() <= today_s.as_str())
            .map(|d| d.count)
            .sum()
    };

    let days_ago = |n: i64| today - chrono::Duration::days(n);
    // Monday of the current week (Mon = 0).
    let monday = days_ago(today.weekday().num_days_from_monday() as i64);
    let heatmap_from = ymd(days_ago(HEATMAP_DAYS - 1));

    let map = json!({
        "available": true,
        "login": login,
        "today": count_on(&today_s),
        "week": sum_since(monday),
        "last7d": sum_since(days_ago(6)),
        "last30d": sum_since(days_ago(29)),
        "year": days.iter().map(|d| d.count).sum::<u32>(),
        "heatmap": days
            .iter()
            .filter(|d| d.date.as_str() >= heatmap_from.as_str())
            .map(|d| json!({ "date": d.date, "count": d.count }))
            .collect::<Vec<_>>(),
    });
    map.as_object().cloned().unwrap_or_default()
}

// --- cache ----------------------------------------------------------------

static CACHE: Mutex<Option<(Value, Instant)>> = Mutex::new(None);
/// Guards against piling up refreshes while one is already in flight.
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// The cached reading if it is younger than `ttl`, never touching the network.
fn cached_if_fresh(ttl: Duration) -> Option<Value> {
    let guard = CACHE.lock().ok()?;
    let (v, at) = guard.as_ref()?;
    (at.elapsed() < ttl).then(|| v.clone())
}

/// Refreshes the cache on a background thread and returns immediately. A second
/// call while one is in flight is a no-op.
fn refresh() {
    if REFRESHING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let payload = match activity() {
            Ok(v) => v,
            // Cache the failure too, so a missing `gh` doesn't mean three new
            // subprocesses on every single poll.
            Err(e) => json!({ "available": false, "error": e.to_string() }),
        };
        if let Ok(mut guard) = CACHE.lock() {
            *guard = Some((payload, Instant::now()));
        }
        REFRESHING.store(false, Ordering::SeqCst);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(date: &str, count: u32) -> Day {
        Day {
            date: date.to_string(),
            count,
        }
    }

    fn on(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    #[test]
    fn totals_count_the_right_spans() {
        // 5 Aug 2026 is a Wednesday, so the week starts Monday the 3rd.
        let days = [
            day("2026-07-01", 100), // inside the year, outside 30 days
            day("2026-07-20", 7),   // inside 30 days, outside 7
            day("2026-08-02", 3),   // Sunday — the previous week
            day("2026-08-03", 5),   // Monday
            day("2026-08-05", 11),  // today
        ];
        let s = summarise(&days, on(2026, 8, 5), "someone");

        assert_eq!(s["today"], json!(11));
        assert_eq!(s["week"], json!(16), "Monday and today, not Sunday");
        assert_eq!(s["last7d"], json!(19), "the 2nd through today");
        assert_eq!(s["last30d"], json!(26), "adds 20 July");
        assert_eq!(s["year"], json!(126), "everything given");
    }

    #[test]
    fn a_day_with_no_contributions_reads_as_zero_not_missing() {
        let s = summarise(&[day("2026-08-04", 4)], on(2026, 8, 5), "someone");
        assert_eq!(s["today"], json!(0), "today is absent from the calendar");
    }

    #[test]
    fn the_heatmap_is_capped_to_its_window() {
        let days = [
            day("2026-01-01", 1), // far outside 90 days
            day("2026-08-04", 2),
            day("2026-08-05", 3),
        ];
        let s = summarise(&days, on(2026, 8, 5), "someone");
        let heatmap = s["heatmap"].as_array().unwrap();
        assert_eq!(heatmap.len(), 2, "January is dropped");
        assert_eq!(heatmap[0]["date"], json!("2026-08-04"));
    }

    #[test]
    fn a_monday_week_total_is_just_today() {
        let days = [day("2026-08-02", 9), day("2026-08-03", 4)];
        let s = summarise(&days, on(2026, 8, 3), "someone");
        assert_eq!(s["week"], json!(4), "Sunday belongs to the week before");
    }

    #[test]
    fn an_empty_calendar_summarises_to_zeroes() {
        let s = summarise(&[], on(2026, 8, 5), "someone");
        assert_eq!(s["today"], json!(0));
        assert_eq!(s["year"], json!(0));
        assert_eq!(s["heatmap"].as_array().unwrap().len(), 0);
        assert_eq!(
            s["available"],
            json!(true),
            "zero activity is still a reading"
        );
    }

    #[test]
    fn the_login_is_carried_through() {
        let s = summarise(&[], on(2026, 8, 5), "amirahosbr");
        assert_eq!(s["login"], json!("amirahosbr"));
    }

    #[test]
    fn a_calendar_response_parses() {
        let raw = br#"{"data":{"viewer":{"contributionsCollection":{"contributionCalendar":
            {"weeks":[{"contributionDays":[{"date":"2026-08-04","contributionCount":2}]},
                      {"contributionDays":[{"date":"2026-08-05","contributionCount":5}]}]}}}}}"#;
        let resp: Resp = serde_json::from_slice(raw).unwrap();
        let days: Vec<Day> = resp
            .data
            .viewer
            .contributions
            .calendar
            .weeks
            .into_iter()
            .flat_map(|w| w.days)
            .collect();
        assert_eq!(days, vec![day("2026-08-04", 2), day("2026-08-05", 5)]);
    }

    #[test]
    fn a_cold_cache_reports_pending_rather_than_blocking() {
        // The first call must return at once, whatever `gh` is doing.
        let v = payload();
        assert!(v["available"].is_boolean(), "got {v}");
        if v["available"] == json!(false) {
            assert!(
                v["pending"] == json!(true) || v["error"].is_string(),
                "unavailable must say pending or why: {v}"
            );
        }
    }
}
