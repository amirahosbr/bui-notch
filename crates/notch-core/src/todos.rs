//! The `todos` module: a to-do briefing written by something else.
//!
//! This app does not talk to Slack, or to any tracker, and does not decide what
//! counts as an action item. A producer writes a briefing to [`path`] and this
//! renders it. That keeps credentials and judgement out of a HUD, and means the
//! same tab works for any producer that can write a file.
//!
//! The schema is deliberately forgiving, because its author may well be a language
//! model: every item may be a bare string or an object, and every section may be
//! absent.
//!
//! ```json
//! {
//!   "generated_at": "2026-08-04T01:30:00Z",
//!   "source": "slack",
//!   "today":       [{ "text": "Reply to the tariff thread", "channel": "#eon", "who": "Kenji" }],
//!   "week":        ["Draft the Q3 plan"],
//!   "in_progress": [],
//!   "done":        []
//! }
//! ```

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

/// A briefing older than this is probably not today's.
pub const STALE_AFTER_HOURS: i64 = 36;

/// One action item. Accepts either `"some text"` or
/// `{ "text": …, "channel": …, "who": …, "url": … }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl<'de> Deserialize<'de> for Item {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Full {
                #[serde(alias = "title", alias = "task", alias = "item")]
                text: String,
                #[serde(default)]
                channel: Option<String>,
                #[serde(default, alias = "from", alias = "assigned_by")]
                who: Option<String>,
                #[serde(default, alias = "link", alias = "permalink")]
                url: Option<String>,
            },
        }

        Ok(match Raw::deserialize(d)? {
            Raw::Text(text) => Item {
                text,
                channel: None,
                who: None,
                url: None,
            },
            Raw::Full {
                text,
                channel,
                who,
                url,
            } => Item {
                text,
                channel,
                who,
                url,
            },
        })
    }
}

/// A whole briefing: the four sections a daily prompt produces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Briefing {
    /// When it was produced. Missing means "unknown", not "now".
    pub generated_at: Option<DateTime<Utc>>,
    /// Free-form note about where it came from, e.g. `slack`.
    pub source: Option<String>,
    #[serde(alias = "todays_todos")]
    pub today: Vec<Item>,
    #[serde(alias = "this_week", alias = "weeks_todos")]
    pub week: Vec<Item>,
    #[serde(alias = "inprogress", alias = "in-progress")]
    pub in_progress: Vec<Item>,
    pub done: Vec<Item>,
}

impl Briefing {
    /// Hours since it was produced, or `None` if it didn't say.
    pub fn age_hours(&self) -> Option<i64> {
        self.generated_at
            .map(|t| (Utc::now() - t).num_hours().max(0))
    }

    /// Whether it's too old to be trusted as today's briefing.
    pub fn stale(&self) -> bool {
        self.age_hours().is_none_or(|h| h >= STALE_AFTER_HOURS)
    }

    /// Items wanting action now — what the collapsed strip counts.
    pub fn open_today(&self) -> usize {
        self.today.len()
    }
}

/// Where the briefing is expected.
pub fn path() -> PathBuf {
    crate::config_dir().join("todos.json")
}

/// Reads the briefing. `Ok(None)` means none has been written yet, which is a
/// normal state and not an error.
pub fn load() -> Result<Option<Briefing>> {
    let p = path();
    let raw = match fs::read_to_string(&p) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", p.display())),
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", p.display()))?,
    ))
}

/// Everything the to-do module renders.
pub fn payload() -> Value {
    match load() {
        Ok(Some(b)) => json!({
            "available": true,
            "generated_at": b.generated_at,
            "age_hours": b.age_hours(),
            "stale": b.stale(),
            "source": b.source,
            "today_count": b.open_today(),
            "today": b.today,
            "week": b.week,
            "in_progress": b.in_progress,
            "done": b.done,
        }),
        // Nothing has run yet, which is worth telling the user how to fix.
        Ok(None) => json!({ "available": false, "missing": true, "path": path() }),
        Err(e) => json!({ "available": false, "error": e.to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_bare_strings_and_objects() {
        // r##"…"## because a channel name contains `"#`, which would close r#"…
        let json = r##"{
            "generated_at": "2026-08-04T01:30:00Z",
            "today": ["plain string", {"text": "with detail", "channel": "#eng", "who": "Kenji"}],
            "week": [],
            "done": [{"task": "aliased field"}]
        }"##;
        let b: Briefing = serde_json::from_str(json).unwrap();
        assert_eq!(b.today.len(), 2);
        assert_eq!(b.today[0].text, "plain string");
        assert_eq!(b.today[0].channel, None);
        assert_eq!(b.today[1].channel.as_deref(), Some("#eng"));
        assert_eq!(b.today[1].who.as_deref(), Some("Kenji"));
        assert_eq!(b.done[0].text, "aliased field", "`task` aliases `text`");
        assert!(b.in_progress.is_empty(), "absent sections default to empty");
    }

    #[test]
    fn an_empty_document_is_valid() {
        let b: Briefing = serde_json::from_str("{}").unwrap();
        assert_eq!(b.open_today(), 0);
        assert!(b.stale(), "no timestamp cannot be trusted as today's");
        assert_eq!(b.age_hours(), None);
    }

    #[test]
    fn age_and_staleness_track_the_timestamp() {
        let fresh = Briefing {
            generated_at: Some(Utc::now() - chrono::Duration::hours(2)),
            ..Default::default()
        };
        assert_eq!(fresh.age_hours(), Some(2));
        assert!(!fresh.stale());

        let old = Briefing {
            generated_at: Some(Utc::now() - chrono::Duration::hours(STALE_AFTER_HOURS + 1)),
            ..Default::default()
        };
        assert!(old.stale());
    }

    #[test]
    fn a_future_timestamp_is_not_a_negative_age() {
        let skewed = Briefing {
            generated_at: Some(Utc::now() + chrono::Duration::hours(3)),
            ..Default::default()
        };
        assert_eq!(
            skewed.age_hours(),
            Some(0),
            "a clock skew reads as just now"
        );
    }

    #[test]
    fn section_aliases_are_accepted() {
        let b: Briefing =
            serde_json::from_str(r#"{"this_week": ["a"], "in-progress": ["b"]}"#).unwrap();
        assert_eq!(b.week.len(), 1);
        assert_eq!(b.in_progress.len(), 1);
    }

    #[test]
    fn payload_says_what_is_wrong_when_there_is_no_briefing() {
        let v = payload();
        assert!(v["available"].is_boolean());
        if v["available"] == json!(false) && v["missing"] == json!(true) {
            assert!(
                v["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("todos.json")),
                "it names the file a producer should write: {v}"
            );
        }
    }
}
