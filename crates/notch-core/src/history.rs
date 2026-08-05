//! A local JSONL log of usage readings, so the Usage tab can show what each
//! reset window peaked at even after it has reset.
//!
//! Best-effort throughout: a failed write never affects the live reading.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

use crate::live::Usage;

/// Collapse bursts when several callers probe within a short span.
const MIN_GAP: Duration = Duration::from_secs(30);
/// Cap the log (~2 weeks at one line a minute); older lines are trimmed.
const MAX_LINES: usize = 20_000;
/// Only scan for trimming once the file passes ~4MB.
const TRIM_THRESHOLD: u64 = 4 << 20;

/// One recorded reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub at: DateTime<Utc>,
    pub session_pct: f64,
    pub week_pct: f64,
    pub session_reset: Option<DateTime<Utc>>,
    pub week_reset: Option<DateTime<Utc>>,
    pub status: String,
}

/// One real reset window, collapsed from every sample taken inside it. A week of
/// per-minute samples is thousands of points but only ~40 windows, so anything
/// that wants the summary should ask for these.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Window {
    /// `Aug 4 · reset 02:40 AM`, in local time.
    pub label: String,
    /// Highest session % reached. A session starts each window near zero, so the
    /// low end carries no information.
    pub session_peak: f64,
    /// Weekly % spans windows, so its range is real signal.
    pub week_low: f64,
    pub week_high: f64,
}

/// The peaks across a set of windows, which the tab shows above the list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Peaks {
    pub session: f64,
    pub week: f64,
}

/// The JSONL log (local only, never git).
pub fn history_path() -> PathBuf {
    crate::config_dir().join("usage-history.jsonl")
}

struct Throttle {
    last_at: Option<Instant>,
    last_session_reset: Option<DateTime<Utc>>,
    last_week_reset: Option<DateTime<Utc>>,
}

static THROTTLE: Mutex<Throttle> = Mutex::new(Throttle {
    last_at: None,
    last_session_reset: None,
    last_week_reset: None,
});

/// Appends a reading, at most one per [`MIN_GAP`] — but always when a window has
/// reset, so the pre-reset peak and the reset moment are never lost.
pub fn record(usage: &Usage) {
    let Ok(mut throttle) = THROTTLE.lock() else {
        return;
    };

    let reset_changed = throttle.last_session_reset != usage.session_reset
        || throttle.last_week_reset != usage.week_reset;
    if !reset_changed {
        if let Some(last) = throttle.last_at {
            if last.elapsed() < MIN_GAP {
                return;
            }
        }
    }

    let snapshot = Snapshot {
        at: usage.at,
        session_pct: usage.session_pct,
        week_pct: usage.week_pct,
        session_reset: usage.session_reset,
        week_reset: usage.week_reset,
        status: usage.status.clone(),
    };
    if append(&snapshot).is_err() {
        return;
    }

    throttle.last_at = Some(Instant::now());
    throttle.last_session_reset = usage.session_reset;
    throttle.last_week_reset = usage.week_reset;
    trim(&history_path());
}

fn append(snapshot: &Snapshot) -> Result<()> {
    let path = history_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{}", serde_json::to_string(snapshot)?)?;
    Ok(())
}

/// Keeps only the most recent [`MAX_LINES`] once the file passes
/// [`TRIM_THRESHOLD`], rewriting through a temporary file so a crash mid-write
/// cannot leave a half-file.
fn trim(path: &Path) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() < TRIM_THRESHOLD {
        return;
    }
    let Ok(data) = fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = data.trim_end_matches('\n').split('\n').collect();
    if lines.len() <= MAX_LINES {
        return;
    }
    let kept = lines[lines.len() - MAX_LINES..].join("\n") + "\n";
    let tmp = path.with_extension("jsonl.tmp");
    if fs::write(&tmp, kept).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Readings newer than now-`since`, oldest first. `None` returns everything, and
/// a missing log is an empty list rather than an error.
pub fn load(since: Option<Duration>) -> Result<Vec<Snapshot>> {
    let file = match fs::File::open(history_path()) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };

    let cutoff = since
        .and_then(|d| chrono::Duration::from_std(d).ok())
        .map(|d| Utc::now() - d);

    Ok(BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        // A line the writer never finished is skipped, not fatal.
        .filter_map(|line| serde_json::from_str::<Snapshot>(&line).ok())
        .filter(|s| cutoff.is_none_or(|c| s.at >= c))
        .collect())
}

/// Groups the log into reset windows, newest first, with the peaks across them.
pub fn windows(since: Option<Duration>) -> Result<(Vec<Window>, Peaks)> {
    Ok(group(&load(since)?))
}

/// Groups readings into windows. Pure, so the grouping can be tested without a log.
///
/// Every reading carries the `session_reset` in force when it was taken, so
/// consecutive samples sharing that value belong to one window and a change marks
/// an actual reset — a fixed clock interval would not line up with it.
fn group(snapshots: &[Snapshot]) -> (Vec<Window>, Peaks) {
    struct Group {
        first_at: DateTime<Utc>,
        reset: Option<DateTime<Utc>>,
        session_peak: f64,
        week_low: f64,
        week_high: f64,
    }

    // Keyed by reset instant so the natural ordering is chronological; samples
    // taken before any reset was reported land under the empty key.
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut peaks = Peaks {
        session: 0.0,
        week: 0.0,
    };

    for s in snapshots {
        peaks.session = peaks.session.max(s.session_pct);
        peaks.week = peaks.week.max(s.week_pct);

        groups
            .entry(s.session_reset.map(|t| t.to_rfc3339()).unwrap_or_default())
            .and_modify(|g| {
                g.first_at = g.first_at.min(s.at);
                g.session_peak = g.session_peak.max(s.session_pct);
                g.week_low = g.week_low.min(s.week_pct);
                g.week_high = g.week_high.max(s.week_pct);
            })
            .or_insert(Group {
                first_at: s.at,
                reset: s.session_reset,
                session_peak: s.session_pct,
                week_low: s.week_pct,
                week_high: s.week_pct,
            });
    }

    let windows = groups
        .into_values()
        .rev() // newest first
        .map(|g| Window {
            label: match g.reset {
                Some(t) => format!("{} · reset {}", local_day(t), local_clock(t)),
                None => format!("{} · {}", local_day(g.first_at), local_clock(g.first_at)),
            },
            session_peak: g.session_peak,
            week_low: g.week_low,
            week_high: g.week_high,
        })
        .collect();

    (windows, peaks)
}

fn local_day(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%b %-d").to_string()
}

fn local_clock(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%I:%M %p").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(unix: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(unix, 0).expect("valid timestamp")
    }

    fn snap(secs: i64, session: f64, week: f64, reset: Option<i64>) -> Snapshot {
        Snapshot {
            at: at(secs),
            session_pct: session,
            week_pct: week,
            session_reset: reset.map(at),
            week_reset: None,
            status: "allowed".into(),
        }
    }

    #[test]
    fn no_readings_group_into_nothing() {
        let (windows, peaks) = group(&[]);
        assert!(windows.is_empty());
        assert_eq!(peaks.session, 0.0);
        assert_eq!(peaks.week, 0.0);
    }

    #[test]
    fn samples_sharing_a_reset_collapse_into_one_window() {
        let snaps = [
            snap(100, 10.0, 40.0, Some(9_000)),
            snap(200, 55.0, 42.0, Some(9_000)),
            snap(300, 30.0, 41.0, Some(9_000)),
        ];
        let (windows, _) = group(&snaps);
        assert_eq!(windows.len(), 1, "one reset instant is one window");
        assert_eq!(
            windows[0].session_peak, 55.0,
            "the peak, not the last value"
        );
        assert_eq!(windows[0].week_low, 40.0);
        assert_eq!(windows[0].week_high, 42.0);
    }

    #[test]
    fn a_changed_reset_starts_a_new_window() {
        let snaps = [
            snap(100, 80.0, 40.0, Some(9_000)),
            snap(200, 5.0, 41.0, Some(27_000)),
        ];
        let (windows, _) = group(&snaps);
        assert_eq!(windows.len(), 2);
    }

    #[test]
    fn windows_come_back_newest_first() {
        let snaps = [
            snap(100, 80.0, 40.0, Some(9_000)),
            snap(200, 5.0, 41.0, Some(27_000)),
        ];
        let (windows, _) = group(&snaps);
        assert_eq!(
            windows[0].session_peak, 5.0,
            "the later window leads the list"
        );
        assert_eq!(windows[1].session_peak, 80.0);
    }

    #[test]
    fn peaks_span_every_window() {
        let snaps = [
            snap(100, 80.0, 40.0, Some(9_000)),
            snap(200, 5.0, 91.0, Some(27_000)),
        ];
        let (_, peaks) = group(&snaps);
        assert_eq!(peaks.session, 80.0);
        assert_eq!(peaks.week, 91.0);
    }

    #[test]
    fn samples_taken_before_any_reset_was_reported_still_group() {
        let (windows, _) = group(&[snap(100, 12.0, 30.0, None)]);
        assert_eq!(windows.len(), 1);
        assert!(
            !windows[0].label.contains("reset"),
            "it cannot claim a reset time it never had: {}",
            windows[0].label
        );
    }

    #[test]
    fn a_window_label_names_the_day_and_the_reset() {
        let (windows, _) = group(&[snap(100, 12.0, 30.0, Some(9_000))]);
        assert!(windows[0].label.contains("reset"), "{}", windows[0].label);
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let s = snap(1_000, 12.5, 40.0, Some(9_000));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Snapshot>(&json).unwrap(), s);
    }

    #[test]
    fn a_missing_log_loads_as_empty() {
        // Whatever is on this machine, loading must not error.
        assert!(load(Some(Duration::from_secs(60))).is_ok());
    }
}
