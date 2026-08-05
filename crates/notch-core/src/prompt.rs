//! What the waiting agent is actually asking.
//!
//! [`crate::attention`] knows *that* an agent wants you; this reads its transcript
//! to find out *what for*, so the panel can show the real question and the real
//! options instead of a generic "needs your permission".
//!
//! The pending ask is the last `tool_use` with no `tool_result` yet. Two shapes
//! matter: `AskUserQuestion`, whose input carries the question and its labelled
//! options, and everything else, which is a permission prompt for that tool.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

/// Only the tail is read: a pending ask is by definition at the end.
const TAIL_BYTES: u64 = 256 * 1024;
/// Claude Code's own prompt shows at most four; more would not fit the banner.
const MAX_OPTIONS: usize = 4;
/// Enough of a command to recognise it, not enough to wrap the banner.
const MAX_DETAIL: usize = 120;

/// The ask an agent is blocked on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Pending {
    /// `AskUserQuestion` — a question with the labels the user would choose from.
    Question {
        question: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        header: Option<String>,
        options: Vec<String>,
    },
    /// Any other tool waiting on approval.
    Permission {
        tool: String,
        /// The command, path, or URL at issue, when the input has an obvious one.
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// Reads the pending ask out of a transcript, or `None` if nothing is waiting.
pub fn pending(transcript: &Path) -> Option<Pending> {
    let text = read_tail(transcript)?;
    let (tool, input) = unanswered(&text)?;

    if tool == "AskUserQuestion" {
        if let Some(q) = question_from(&input) {
            return Some(q);
        }
    }
    Some(Pending::Permission {
        detail: detail_from(&input),
        tool,
    })
}

/// The newest `tool_use` in `text` with no `tool_result`. Pure, so the walk can be
/// tested without a file.
fn unanswered(text: &str) -> Option<(String, Value)> {
    // Walk forward tracking which tool_use ids have been answered, so the ask left
    // over at the end is the one still blocking.
    let mut answered: HashSet<String> = HashSet::new();
    let mut open: Vec<(String, String, Value)> = Vec::new();

    for line in text.lines() {
        let Ok(rec) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(Value::Array(blocks)) = rec.pointer("/message/content") else {
            continue;
        };
        for b in blocks {
            match b["type"].as_str().unwrap_or_default() {
                "tool_use" => {
                    let id = b["id"].as_str().unwrap_or_default();
                    if !id.is_empty() {
                        open.push((
                            id.to_string(),
                            b["name"].as_str().unwrap_or_default().to_string(),
                            b["input"].clone(),
                        ));
                    }
                }
                "tool_result" => {
                    if let Some(id) = b["tool_use_id"].as_str() {
                        answered.insert(id.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    open.into_iter()
        .rev()
        .find(|(id, _, _)| !answered.contains(id))
        .map(|(_, tool, input)| (tool, input))
}

fn question_from(input: &Value) -> Option<Pending> {
    let q = input["questions"].as_array()?.first()?;
    let question = q["question"].as_str()?.trim().to_string();
    let options: Vec<String> = q["options"]
        .as_array()?
        .iter()
        .filter_map(|o| o["label"].as_str())
        .take(MAX_OPTIONS)
        .map(|l| l.trim().to_string())
        .collect();
    if question.is_empty() || options.is_empty() {
        return None;
    }
    Some(Pending::Question {
        question,
        header: q["header"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        options,
    })
}

/// The one field of a tool's input worth showing — a command beats a file path,
/// which beats a URL.
fn detail_from(input: &Value) -> Option<String> {
    for key in ["command", "file_path", "path", "url", "pattern"] {
        let Some(v) = input[key].as_str() else {
            continue;
        };
        let flat = v.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.is_empty() {
            continue;
        }
        return Some(if flat.chars().count() > MAX_DETAIL {
            format!("{}…", flat.chars().take(MAX_DETAIL).collect::<String>())
        } else {
            flat
        });
    }
    None
}

fn read_tail(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(TAIL_BYTES);
    if from > 0 {
        file.seek(SeekFrom::Start(from)).ok()?;
    }
    let mut buf = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if from == 0 {
        return Some(text);
    }
    // Started mid-line; drop the fragment.
    text.find('\n').map(|i| text[i + 1..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Folds each record onto one line — the fixtures below are wrapped across
    /// source lines for readability, and a transcript with a record split over
    /// several lines is not JSONL and would not parse.
    fn jsonl(records: &[&str]) -> String {
        records
            .iter()
            .map(|r| r.lines().map(str::trim).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn transcript(name: &str, records: &[&str]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("notch-prompt-{name}.jsonl"));
        let mut f = fs::File::create(&p).unwrap();
        writeln!(f, "{}", jsonl(records)).unwrap();
        p
    }

    #[test]
    fn reads_a_question_with_its_options() {
        let p = transcript(
            "q",
            &[r#"{"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"t1","name":"AskUserQuestion","input":{"questions":[
              {"question":"How much of the notch HUD should bui-notch take?",
               "header":"Scope","multiSelect":false,
               "options":[{"label":"Shell + local modules"},
                          {"label":"Shell only"},
                          {"label":"Full HUD, everything"}]}]}}]}}"#],
        );
        match pending(&p).expect("something is pending") {
            Pending::Question {
                question,
                header,
                options,
            } => {
                assert!(question.starts_with("How much of the notch HUD"));
                assert_eq!(header.as_deref(), Some("Scope"));
                assert_eq!(options.len(), 3);
                assert_eq!(options[0], "Shell + local modules");
            }
            other => panic!("expected a question, got {other:?}"),
        }
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn reads_a_permission_with_the_command() {
        let p = transcript(
            "perm",
            &[r#"{"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm install --include=dev"}}]}}"#],
        );
        assert_eq!(
            pending(&p),
            Some(Pending::Permission {
                tool: "Bash".into(),
                detail: Some("npm install --include=dev".into()),
            })
        );
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn an_answered_tool_is_not_pending() {
        let text = jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#,
        ]);
        assert!(unanswered(&text).is_none());
    }

    #[test]
    fn the_newest_unanswered_ask_wins() {
        let text = jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a/old.rs"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Write","input":{"file_path":"/a/new.rs"}}]}}"#,
        ]);
        let (tool, input) = unanswered(&text).expect("t2 is still open");
        assert_eq!(tool, "Write");
        assert_eq!(detail_from(&input).as_deref(), Some("/a/new.rs"));
    }

    #[test]
    fn a_malformed_question_degrades_to_a_permission() {
        let p = transcript(
            "bad",
            &[r#"{"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"t1","name":"AskUserQuestion","input":{"questions":[]}}]}}"#],
        );
        assert_eq!(
            pending(&p),
            Some(Pending::Permission {
                tool: "AskUserQuestion".into(),
                detail: None,
            })
        );
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn a_missing_transcript_is_not_an_error() {
        assert_eq!(pending(Path::new("/nope/does-not-exist.jsonl")), None);
    }

    #[test]
    fn a_command_too_long_for_the_banner_is_cut() {
        let long = "echo ".to_string() + &"x".repeat(MAX_DETAIL + 40);
        let input = serde_json::json!({ "command": long });
        let out = detail_from(&input).unwrap();
        assert_eq!(out.chars().count(), MAX_DETAIL + 1, "plus the ellipsis");
        assert!(out.ends_with('…'));
    }

    #[test]
    fn only_the_first_few_options_are_kept() {
        let input = serde_json::json!({
            "questions": [{
                "question": "pick one",
                "options": [
                    { "label": "a" }, { "label": "b" }, { "label": "c" },
                    { "label": "d" }, { "label": "e" }, { "label": "f" },
                ],
            }],
        });
        match question_from(&input).unwrap() {
            Pending::Question { options, .. } => assert_eq!(options.len(), MAX_OPTIONS),
            other => panic!("expected a question, got {other:?}"),
        }
    }

    #[test]
    fn a_detail_free_tool_is_still_a_permission() {
        let input = serde_json::json!({ "unexpected": "shape" });
        assert_eq!(detail_from(&input), None);
    }
}
