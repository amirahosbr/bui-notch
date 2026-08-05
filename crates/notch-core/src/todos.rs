//! The `todos` module: a to-do briefing written by something else.
//!
//! This app does not talk to Slack, Gmail, or any tracker, and does not decide
//! what counts as an action item. A producer writes a briefing to [`path`] and this
//! renders it. That keeps credentials and judgement out of a HUD, and means the
//! same tab works for any producer that can write a file.
//!
//! The producer shipped with this repo is `/todo-brief`, a Claude Code command that
//! reads Slack and Gmail through connectors the HUD never sees. [`schema`] is what
//! it is told to copy.
//!
//! The schema is deliberately forgiving, because its author may well be a language
//! model: every item may be a bare string or an object, and every section may be
//! absent.
//!
//! ```json
//! {
//!   "generated_at": "2026-08-04T01:30:00Z",
//!   "source": "slack+gmail",
//!   "today":       [{ "text": "Reply to the tariff thread", "channel": "#eon", "who": "Kenji" }],
//!   "week":        ["Draft the Q3 plan"],
//!   "in_progress": [],
//!   "done":        []
//! }
//! ```

use std::fs;
use std::path::{Path, PathBuf};

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
    load_at(&path())
}

/// [`load`] from an explicit file, so the reading can be tested without touching
/// the briefing on this machine.
fn load_at(p: &Path) -> Result<Option<Briefing>> {
    let raw = match fs::read_to_string(p) {
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

/// Removes the briefing. Missing already is success, not an error.
pub fn clear() -> Result<()> {
    clear_at(&path())
}

/// [`clear`] on an explicit file. Takes the path rather than reaching for it so a
/// test can never delete the real briefing.
fn clear_at(p: &Path) -> Result<()> {
    match fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", p.display())),
    }
}

/// The shape a producer should write, as an example document.
///
/// Handed out rather than described in prose so a producer can be pointed at
/// something it can copy — and so this file stays the one definition of the shape.
pub fn schema() -> Value {
    json!({
        "generated_at": "2026-08-05T04:00:00Z",
        "source": "slack+gmail",
        "today": [
            {
                "text": "Reply to the tariff thread",
                "channel": "#eon",
                "who": "Kenji",
            },
            "a bare string is also accepted",
        ],
        "week": [{ "text": "Draft the Q3 plan", "channel": "inbox", "who": "Sato" }],
        "in_progress": [],
        "done": [],
    })
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
    fn the_schema_we_hand_out_parses_back() {
        // A producer is told to copy this shape, so it has to survive the reader.
        // If they ever drift, every briefing written from the schema breaks.
        let b: Briefing = serde_json::from_value(schema()).expect("schema must parse");
        assert_eq!(b.today.len(), 2, "the object and the bare string");
        assert_eq!(b.today[0].text, "Reply to the tariff thread");
        assert_eq!(b.today[0].channel.as_deref(), Some("#eon"));
        assert_eq!(b.today[1].text, "a bare string is also accepted");
        assert_eq!(b.week.len(), 1);
        assert!(b.generated_at.is_some(), "the timestamp shape is valid");
    }

    #[test]
    fn the_schema_names_both_sources() {
        // The briefing is fed from Slack and Gmail, and `source` is what says so.
        assert_eq!(schema()["source"], json!("slack+gmail"));
    }

    #[test]
    fn a_briefing_round_trips_through_a_file() {
        let p = std::env::temp_dir().join("notch-todos-roundtrip.json");
        fs::write(&p, serde_json::to_string(&schema()).unwrap()).unwrap();
        let b = load_at(&p).unwrap().expect("a written briefing loads");
        assert_eq!(b.today.len(), 2);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn clearing_is_safe_to_repeat() {
        let p = std::env::temp_dir().join("notch-todos-clear.json");
        fs::write(&p, "{}").unwrap();
        assert!(clear_at(&p).is_ok());
        assert!(clear_at(&p).is_ok(), "already gone is still success");
        assert!(load_at(&p).unwrap().is_none());
    }

    #[test]
    fn an_absent_file_and_an_empty_one_both_read_as_no_briefing() {
        let missing = std::env::temp_dir().join("notch-todos-nope.json");
        let _ = fs::remove_file(&missing);
        assert!(load_at(&missing).unwrap().is_none());

        let blank = std::env::temp_dir().join("notch-todos-blank.json");
        fs::write(&blank, "   \n").unwrap();
        assert!(
            load_at(&blank).unwrap().is_none(),
            "whitespace is not a briefing"
        );
        let _ = fs::remove_file(&blank);
    }

    #[test]
    fn a_corrupt_briefing_is_an_error_naming_the_file() {
        // The tab should say the file is broken, not silently show nothing.
        let p = std::env::temp_dir().join("notch-todos-corrupt.json");
        fs::write(&p, "{not json").unwrap();
        let err = load_at(&p).unwrap_err();
        assert!(err.to_string().contains("notch-todos-corrupt"), "{err}");
        let _ = fs::remove_file(&p);
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
