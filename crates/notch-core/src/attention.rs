//! "An agent needs you" — the signal that makes the panel open by itself.
//!
//! Transcripts cannot tell us this reliably. A tool call awaiting *permission* and
//! a tool call that is simply *running* look identical on disk: an assistant
//! `tool_use` with no result yet. Guessing from how long it has been would mean
//! popping the panel open every time a build took half a minute.
//!
//! So Claude Code tells us instead. Its `Notification` hook fires exactly when it
//! wants the user — permission prompts and idle waits — and pipes its payload to
//! `notch attention`, which lands here. `scripts/install-claude-hook.sh` wires it.
//!
//! ## Why a file
//!
//! bui kept this in memory and took it over HTTP, because it had a local server
//! running. This app deliberately has none, so the hook and the panel are separate
//! processes with no socket between them — a file is the one channel they share.
//!
//! That makes the signal persistent, which it should not be: a request for
//! attention only means anything while the agent is still waiting, and one left
//! over from before a reboot would open the panel for nothing. [`TTL`] is what
//! keeps that honest — anything older is ignored and treated as gone.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long a request stays live. Claude Code re-notifies while it keeps waiting,
/// so this only has to outlast the gap between notifications.
pub const TTL: Duration = Duration::from_secs(180);

/// Where the hook drops its payload.
pub fn path() -> PathBuf {
    crate::config_dir().join("attention.json")
}

/// One agent waiting on the user, as written by the hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attention {
    /// When it was raised. Absent means "unknown", which reads as expired.
    pub raised_at: Option<DateTime<Utc>>,
    /// Claude Code's session id, so the panel can point at the right row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Directory basename — what the session list calls the project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The notification text, e.g. "Claude needs your permission to use Bash".
    pub message: String,
    /// The transcript to read the pending ask out of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
}

impl Attention {
    /// Seconds since it was raised, or `None` if it never said.
    pub fn age_secs(&self) -> Option<i64> {
        self.raised_at
            .map(|t| (Utc::now() - t).num_seconds().max(0))
    }

    /// Whether it is old enough to be treated as gone.
    fn expired(&self) -> bool {
        self.age_secs().is_none_or(|s| s as u64 >= TTL.as_secs())
    }

    /// What distinguishes one waiting agent from another, for deciding whether the
    /// panel has already opened for this one.
    fn key(&self) -> String {
        self.session_id
            .clone()
            .unwrap_or_else(|| self.message.clone())
    }
}

/// Turns a `Notification` hook payload into an [`Attention`].
///
/// Unknown shapes are tolerated: the only thing that really matters is that
/// *something* wants attention, so a payload with no recognised field at all still
/// counts rather than being dropped.
pub fn from_hook(payload: &Value, now: DateTime<Utc>) -> Attention {
    let text = |k: &str| {
        payload[k]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };

    let cwd = text("cwd");
    Attention {
        raised_at: Some(now),
        session_id: text("session_id").or_else(|| text("sessionId")),
        project: cwd.as_deref().and_then(|c| {
            Path::new(c)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        }),
        message: text("message")
            .or_else(|| text("notification"))
            .unwrap_or_else(|| "An agent needs you".to_string()),
        transcript: text("transcript_path").or_else(|| text("transcriptPath")),
    }
}

/// Records a request for attention, replacing any previous one.
pub fn raise(a: &Attention) -> Result<()> {
    let p = path();
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&p, serde_json::to_string_pretty(a)?)
        .with_context(|| format!("writing {}", p.display()))
}

/// The live request, if one was raised within [`TTL`]. A missing or stale file is
/// `None` rather than an error — no agent waiting is the normal state.
pub fn current() -> Option<Attention> {
    let a: Attention = serde_json::from_str(&fs::read_to_string(path()).ok()?).ok()?;
    (!a.expired()).then_some(a)
}

/// The live request together with what it is asking, which is read from the
/// transcript rather than stored — the ask can change while the agent waits.
pub fn current_with_prompt() -> Option<Value> {
    let a = current()?;
    let pending = a
        .transcript
        .as_deref()
        .and_then(|t| crate::prompt::pending(Path::new(t)));

    let mut v = serde_json::to_value(&a).ok()?;
    v["age_secs"] = a.age_secs().into();
    v["prompt"] = serde_json::to_value(pending).ok()?;
    Some(v)
}

/// Forgets the current request — the agent stopped waiting, or the user looked.
pub fn clear() -> Result<()> {
    match fs::remove_file(path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path().display())),
    }
}

/// The last request the panel opened for, so it opens once and not every tick.
/// In-process on purpose: "have I already shown this" is the panel's business, and
/// the hook writing the file has no opinion on it.
static LAST_SHOWN: Mutex<Option<String>> = Mutex::new(None);

/// Claims the job of opening the panel: returns the request exactly once, so the
/// panel opens on arrival rather than on every poll.
///
/// A repeat notification for the same session does not re-open, because Claude Code
/// keeps notifying while it waits and a panel you just dismissed should stay
/// dismissed. Once the request expires or is cleared, the same session may open it
/// again — by then it is a fresh ask.
pub fn take_unshown() -> Option<Attention> {
    let mut last = LAST_SHOWN.lock().ok()?;
    let Some(a) = current() else {
        // Nothing waiting, so the next arrival — even from the same session —
        // deserves the panel.
        *last = None;
        return None;
    };
    let key = a.key();
    if last.as_deref() == Some(key.as_str()) {
        return None;
    }
    *last = Some(key);
    Some(a)
}

/// Whether the file has changed since `seen`, and its new stamp.
///
/// The hover watcher asks several times a second; parsing JSON that often for
/// something that is almost never there would be waste, so it gates on one `stat`
/// and only reads when the file has actually moved.
pub fn changed_since(seen: Option<SystemTime>) -> (bool, Option<SystemTime>) {
    let stamp = fs::metadata(path()).and_then(|m| m.modified()).ok();
    (stamp != seen, stamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The store is one file and one process-wide static, so these tests would
    /// otherwise clobber each other under cargo's default parallelism.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset() {
        let _ = clear();
        if let Ok(mut last) = LAST_SHOWN.lock() {
            *last = None;
        }
    }

    fn at(secs_ago: i64) -> DateTime<Utc> {
        Utc::now() - chrono::Duration::seconds(secs_ago)
    }

    #[test]
    fn a_hook_payload_becomes_a_request() {
        let a = from_hook(
            &json!({
                "session_id": "abc",
                "cwd": "/Users/a/dev/bui-notch",
                "message": "Claude needs your permission to use Bash",
            }),
            Utc::now(),
        );
        assert_eq!(a.session_id.as_deref(), Some("abc"));
        assert_eq!(a.project.as_deref(), Some("bui-notch"));
        assert!(a.message.contains("permission"));
    }

    #[test]
    fn a_bare_payload_still_counts() {
        // The one thing that matters is that something wants attention.
        let a = from_hook(&json!({}), Utc::now());
        assert_eq!(a.message, "An agent needs you");
        assert_eq!(a.project, None);
        assert_eq!(a.session_id, None);
    }

    #[test]
    fn camel_case_fields_are_accepted_too() {
        let a = from_hook(
            &json!({ "sessionId": "s1", "transcriptPath": "/tmp/t.jsonl" }),
            Utc::now(),
        );
        assert_eq!(a.session_id.as_deref(), Some("s1"));
        assert_eq!(a.transcript.as_deref(), Some("/tmp/t.jsonl"));
    }

    #[test]
    fn blank_fields_are_treated_as_absent() {
        let a = from_hook(&json!({ "session_id": "  ", "message": "" }), Utc::now());
        assert_eq!(a.session_id, None);
        assert_eq!(a.message, "An agent needs you");
    }

    #[test]
    fn a_fresh_request_is_live_and_a_stale_one_is_not() {
        let fresh = Attention {
            raised_at: Some(at(5)),
            ..from_hook(&json!({}), Utc::now())
        };
        assert!(!fresh.expired());

        let stale = Attention {
            raised_at: Some(at(TTL.as_secs() as i64 + 1)),
            ..from_hook(&json!({}), Utc::now())
        };
        assert!(stale.expired(), "an old request must not open the panel");

        let undated = Attention {
            raised_at: None,
            ..from_hook(&json!({}), Utc::now())
        };
        assert!(undated.expired(), "no timestamp cannot be trusted as now");
    }

    #[test]
    fn raise_then_read_round_trips() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset();
        raise(&from_hook(
            &json!({ "session_id": "s1", "message": "waiting on you" }),
            Utc::now(),
        ))
        .unwrap();
        let a = current().expect("just raised, so it is live");
        assert_eq!(a.session_id.as_deref(), Some("s1"));
        assert_eq!(a.message, "waiting on you");
        assert!(a.age_secs().is_some_and(|s| s < 5));
        reset();
    }

    #[test]
    fn a_stale_file_reads_as_nothing_waiting() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset();
        raise(&Attention {
            raised_at: Some(at(TTL.as_secs() as i64 + 10)),
            ..from_hook(&json!({ "session_id": "old" }), Utc::now())
        })
        .unwrap();
        assert!(
            current().is_none(),
            "a request left over from before a restart must be ignored"
        );
        reset();
    }

    #[test]
    fn opens_once_then_stops_claiming() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset();
        raise(&from_hook(&json!({ "session_id": "s1" }), Utc::now())).unwrap();
        assert!(take_unshown().is_some(), "the first look claims it");
        assert!(take_unshown().is_none(), "the second must not re-open");
        assert!(current().is_some(), "still live, just already shown");
        reset();
    }

    #[test]
    fn a_repeat_for_the_same_session_does_not_reopen() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset();
        raise(&from_hook(&json!({ "session_id": "s1" }), Utc::now())).unwrap();
        take_unshown();
        // Claude Code re-notifies while it waits.
        raise(&from_hook(
            &json!({ "session_id": "s1", "message": "still waiting" }),
            Utc::now(),
        ))
        .unwrap();
        assert!(
            take_unshown().is_none(),
            "a still-waiting agent must not fight a dismissal"
        );
        reset();
    }

    #[test]
    fn a_different_session_reopens() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset();
        raise(&from_hook(&json!({ "session_id": "s1" }), Utc::now())).unwrap();
        take_unshown();
        raise(&from_hook(&json!({ "session_id": "s2" }), Utc::now())).unwrap();
        assert!(take_unshown().is_some(), "a new agent deserves the panel");
        reset();
    }

    #[test]
    fn the_same_session_may_open_again_once_the_request_has_gone() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset();
        raise(&from_hook(&json!({ "session_id": "s1" }), Utc::now())).unwrap();
        take_unshown();
        clear().unwrap();
        assert!(take_unshown().is_none(), "nothing is waiting");

        raise(&from_hook(&json!({ "session_id": "s1" }), Utc::now())).unwrap();
        assert!(
            take_unshown().is_some(),
            "after it went away, the same session is a fresh ask"
        );
        reset();
    }

    #[test]
    fn clearing_twice_is_safe() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset();
        assert!(clear().is_ok());
        assert!(clear().is_ok(), "already gone is still success");
        assert!(current().is_none());
    }

    #[test]
    fn the_mtime_gate_notices_a_new_request() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset();
        let (_, none_yet) = changed_since(None);
        raise(&from_hook(&json!({ "session_id": "s1" }), Utc::now())).unwrap();
        let (changed, stamp) = changed_since(none_yet);
        assert!(changed, "writing the file must be noticed");
        let (again, _) = changed_since(stamp);
        assert!(!again, "an unchanged file must not be re-read");
        reset();
    }

    #[test]
    fn a_request_with_no_transcript_still_reports() {
        let _serial = TEST_LOCK.lock().unwrap();
        reset();
        raise(&from_hook(
            &json!({ "session_id": "s1", "message": "needs you" }),
            Utc::now(),
        ))
        .unwrap();
        let v = current_with_prompt().expect("live");
        assert_eq!(v["message"], json!("needs you"));
        assert!(v["prompt"].is_null(), "nothing to read the ask from");
        assert!(v["age_secs"].is_i64());
        reset();
    }
}
