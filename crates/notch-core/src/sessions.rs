//! The `sessions` module: live Claude Code sessions, read from the transcripts in
//! `~/.claude/projects`. Which sessions are running, in which project, on which
//! model, and what they last said.
//!
//! Transcripts are append-only JSONL and can reach tens of megabytes, so this
//! never reads a whole file: the turn count is kept incrementally (only the bytes
//! appended since the last scan are read) and everything else comes from the last
//! [`TAIL_BYTES`] of the file.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use walkdir::WalkDir;

/// How much of each transcript's tail is parsed for title, preview and status.
const TAIL_BYTES: u64 = 256 * 1024;
/// Only transcripts touched inside this window are considered.
const WINDOW: Duration = Duration::from_secs(12 * 3600);
/// Most sessions to return, newest first.
const LIMIT: usize = 12;
/// Activity newer than this counts as a live session.
const ACTIVE_WITHIN_SECS: i64 = 90;
/// Preview text is truncated to this many characters.
const PREVIEW_CHARS: usize = 140;
/// How long a scan of the transcript tree is reused.
const TTL: Duration = Duration::from_secs(20);

/// What a session appears to be doing, inferred from its last records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// A tool call was issued and no result has landed — running, or waiting on
    /// approval.
    Tool,
    /// Wrote something in the last minute and a half.
    Active,
    /// Quiet for longer than that.
    Idle,
}

/// One Claude Code session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Session {
    pub id: String,
    /// Basename of the session's working directory.
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Custom title, else the generated one, else the last prompt, else the project.
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The most recent thing said, by either side.
    pub preview: String,
    /// Seconds since the last activity, or -1 if it never said.
    pub idle_secs: i64,
    /// Assistant turns in the whole transcript.
    pub messages: u64,
    pub status: Status,
    /// Whether the newest record belongs to a subagent rather than the main thread.
    pub sidechain: bool,
}

/// Everything the sessions module renders.
pub fn payload() -> Value {
    match cached(TTL) {
        Ok(list) => json!({
            "available": true,
            "active": list.iter().filter(|s| s.status != Status::Idle).count(),
            "total": list.len(),
            "list": list,
        }),
        Err(e) => json!({ "available": false, "error": e.to_string() }),
    }
}

/// `~/.claude/projects`, where Claude Code keeps its transcripts.
fn projects_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".claude").join("projects"))
        .unwrap_or_else(|| PathBuf::from("projects"))
}

/// Sessions whose transcript was touched inside `window`, newest first, capped at
/// `limit`. An unreadable transcript is skipped rather than failing the scan.
fn recent(window: Duration, limit: usize) -> Result<Vec<Session>> {
    let now = SystemTime::now();

    // Cheap pass first: mtime only, so a long history costs one stat per file.
    let mut candidates: Vec<(PathBuf, SystemTime)> = WalkDir::new(projects_dir())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            let fresh = now.duration_since(modified).unwrap_or(Duration::ZERO) <= window;
            fresh.then(|| (e.path().to_path_buf(), modified))
        })
        .collect();

    candidates.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    candidates.truncate(limit);

    Ok(candidates
        .iter()
        .filter_map(|(path, _)| read_session(path))
        .collect())
}

/// A memoised scan and when it was taken.
type Memo = Mutex<Option<(Vec<Session>, Instant)>>;

static CACHE: OnceLock<Memo> = OnceLock::new();

/// [`recent`] memoised for `ttl`, so a polling panel doesn't re-walk the tree
/// every tick.
fn cached(ttl: Duration) -> Result<Vec<Session>> {
    let cell = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cell.lock() {
        if let Some((sessions, at)) = guard.as_ref() {
            if at.elapsed() < ttl {
                return Ok(sessions.clone());
            }
        }
    }
    let fresh = recent(WINDOW, LIMIT)?;
    if let Ok(mut guard) = cell.lock() {
        *guard = Some((fresh.clone(), Instant::now()));
    }
    Ok(fresh)
}

// --- per-file reading -----------------------------------------------------

/// Where the incremental turn counter left off in one transcript.
#[derive(Clone, Copy)]
struct Counted {
    /// Bytes consumed so far, always ending on a line boundary.
    offset: u64,
    assistant: u64,
}

static COUNTS: OnceLock<Mutex<HashMap<PathBuf, Counted>>> = OnceLock::new();

/// Counts assistant turns in `path`, reading only the bytes appended since the
/// last call. A shrunk file (rotated or replaced) is rescanned from the start.
fn assistant_turns(path: &Path, len: u64) -> u64 {
    const NEEDLE: &str = "\"type\":\"assistant\"";

    let cell = COUNTS.get_or_init(|| Mutex::new(HashMap::new()));
    let prev = cell
        .lock()
        .ok()
        .and_then(|m| m.get(path).copied())
        .filter(|c| c.offset <= len)
        .unwrap_or(Counted {
            offset: 0,
            assistant: 0,
        });

    if prev.offset == len {
        return prev.assistant;
    }

    let Ok(file) = fs::File::open(path) else {
        return prev.assistant;
    };
    let mut reader = BufReader::new(file);
    if prev.offset > 0 && reader.seek(SeekFrom::Start(prev.offset)).is_err() {
        return prev.assistant;
    }

    // read_line rather than lines(), so a half-written trailing line stays out of
    // both the count and the saved offset and is picked up next tick.
    let mut counted = prev;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(n) => {
                if !line.ends_with('\n') {
                    break; // an incomplete tail line is not committed
                }
                counted.offset += n as u64;
                if line.contains(NEEDLE) {
                    counted.assistant += 1;
                }
            }
            Err(_) => break,
        }
    }

    if let Ok(mut m) = cell.lock() {
        m.insert(path.to_path_buf(), counted);
    }
    counted.assistant
}

/// What one pass over a transcript's tail found.
#[derive(Default)]
struct Scanned {
    id: Option<String>,
    cwd: String,
    branch: Option<String>,
    custom_title: Option<String>,
    generated_title: Option<String>,
    last_prompt: Option<String>,
    model: Option<String>,
    preview: String,
    last_activity: Option<DateTime<Utc>>,
    sidechain: bool,
    /// tool_use ids still waiting on a tool_result.
    pending: HashSet<String>,
    saw_turn: bool,
}

/// Builds a `Session` from one transcript, or `None` if it holds no real turns.
fn read_session(path: &Path) -> Option<Session> {
    let len = fs::metadata(path).ok()?.len();
    let messages = assistant_turns(path, len);
    let tail = read_tail(path, len)?;
    let fallback_id = path.file_stem().map(|s| s.to_string_lossy().into_owned())?;

    let found = scan(&tail);
    if !found.saw_turn {
        return None; // a stub transcript: a queued prompt that never ran
    }

    let idle_secs = found
        .last_activity
        .map(|t| (Utc::now() - t).num_seconds().max(0))
        .unwrap_or(i64::MAX);

    // Transcripts with a prompt but no assistant turn are the shells Claude Code
    // leaves behind; they would otherwise crowd out real work. One just written is
    // kept, since it may be a session mid-first-reply.
    if messages == 0 && idle_secs > ACTIVE_WITHIN_SECS {
        return None;
    }

    let project = Path::new(&found.cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "(unknown)".to_string());

    Some(Session {
        id: found.id.unwrap_or(fallback_id),
        title: found
            .custom_title
            .clone()
            .or(found.generated_title.clone())
            .or(found.last_prompt.as_deref().and_then(clean_text))
            .unwrap_or_else(|| project.clone()),
        project,
        branch: found.branch,
        model: found.model,
        preview: found.preview,
        idle_secs: if idle_secs == i64::MAX { -1 } else { idle_secs },
        messages,
        status: status_of(&found.pending, idle_secs),
        sidechain: found.sidechain,
    })
}

/// What the session is doing. Pure, so every branch can be checked directly.
fn status_of(pending: &HashSet<String>, idle_secs: i64) -> Status {
    if idle_secs > ACTIVE_WITHIN_SECS {
        Status::Idle
    } else if pending.is_empty() {
        Status::Active
    } else {
        Status::Tool
    }
}

/// Reads every record in `tail`, newest value winning for each field.
fn scan(tail: &str) -> Scanned {
    let mut found = Scanned::default();

    for line in tail.lines() {
        let Ok(rec) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        if let Some(s) = str_field(&rec, "sessionId") {
            found.id = Some(s);
        }

        match rec["type"].as_str().unwrap_or_default() {
            "custom-title" => {
                found.custom_title = str_field(&rec, "customTitle");
                continue;
            }
            "ai-title" => {
                found.generated_title = str_field(&rec, "aiTitle");
                continue;
            }
            "last-prompt" => {
                found.last_prompt = str_field(&rec, "lastPrompt");
                continue;
            }
            "assistant" | "user" => {}
            _ => continue,
        }

        found.saw_turn = true;
        if let Some(c) = rec["cwd"].as_str() {
            found.cwd = c.to_string();
        }
        if let Some(b) = rec["gitBranch"].as_str() {
            found.branch = (!b.is_empty()).then(|| b.to_string());
        }
        found.sidechain = rec["isSidechain"].as_bool().unwrap_or(false);
        if let Some(t) = rec["timestamp"]
            .as_str()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        {
            found.last_activity = Some(t.with_timezone(&Utc));
        }

        let msg = &rec["message"];
        if let Some(m) = msg["model"].as_str() {
            if !m.is_empty() && m != "<synthetic>" {
                found.model = Some(pretty_model(m));
            }
        }

        match &msg["content"] {
            // A plain-string user message.
            Value::String(s) => {
                if let Some(t) = clean_text(s) {
                    found.preview = t;
                }
            }
            Value::Array(blocks) => {
                for b in blocks {
                    match b["type"].as_str().unwrap_or_default() {
                        "text" => {
                            if let Some(t) = b["text"].as_str().and_then(clean_text) {
                                found.preview = t;
                            }
                        }
                        "tool_use" => {
                            if let Some(id) = b["id"].as_str() {
                                found.pending.insert(id.to_string());
                            }
                        }
                        "tool_result" => {
                            if let Some(id) = b["tool_use_id"].as_str() {
                                found.pending.remove(id);
                            }
                        }
                        // Thinking blocks, images and friends are not previewable.
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    found
}

/// The last [`TAIL_BYTES`] of `path` as text, with a leading partial line dropped.
fn read_tail(path: &Path, len: u64) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let from = len.saturating_sub(TAIL_BYTES);
    if from > 0 {
        file.seek(SeekFrom::Start(from)).ok()?;
    }
    let mut buf = Vec::with_capacity(TAIL_BYTES.min(len) as usize);
    file.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if from == 0 {
        return Some(text);
    }
    // Started mid-line: everything before the first newline is a fragment.
    Some(match text.find('\n') {
        Some(i) => text[i + 1..].to_string(),
        None => String::new(),
    })
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Collapses a message to one previewable line, or `None` for the wrapped
/// command and reminder plumbing the user never typed.
fn clean_text(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t.starts_with('<') {
        return None;
    }
    let flat = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PREVIEW_CHARS {
        return Some(flat);
    }
    let cut: String = flat.chars().take(PREVIEW_CHARS).collect();
    Some(format!("{}…", cut.trim_end()))
}

/// `claude-opus-5` → `Opus 5`; `claude-haiku-4-5-20251001` → `Haiku 4.5`.
fn pretty_model(model: &str) -> String {
    let stripped = model.strip_prefix("claude-").unwrap_or(model);
    // Drop a trailing date suffix like `-20251001`.
    let name = match stripped.rfind("-20") {
        Some(i) if i > 0 => &stripped[..i],
        _ => stripped,
    };
    let parts: Vec<&str> = name.split('-').collect();
    match parts.len() {
        n if n >= 3 => format!("{} {}.{}", title_case(parts[0]), parts[1], parts[2]),
        2 => format!("{} {}", title_case(parts[0]), parts[1]),
        _ => model.to_string(),
    }
}

fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Writes `lines` to a temporary transcript and returns its path.
    fn transcript(name: &str, lines: &[&str]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("notch-sessions-{name}.jsonl"));
        let mut f = fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    fn pending(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reads_title_preview_and_active_status() {
        let now = Utc::now().to_rfc3339();
        let p = transcript(
            "active",
            &[
                &format!(
                    r#"{{"type":"user","sessionId":"s1","cwd":"/Users/a/dev/bui-notch","gitBranch":"main","timestamp":"{now}","message":{{"role":"user","content":"ship the notch"}}}}"#
                ),
                &format!(
                    r#"{{"type":"assistant","sessionId":"s1","cwd":"/Users/a/dev/bui-notch","timestamp":"{now}","message":{{"role":"assistant","model":"claude-opus-5","content":[{{"type":"text","text":"On it."}}]}}}}"#
                ),
                r#"{"type":"ai-title","aiTitle":"Notch HUD work","sessionId":"s1"}"#,
            ],
        );
        let s = read_session(&p).unwrap();
        assert_eq!(s.id, "s1");
        assert_eq!(s.project, "bui-notch");
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.title, "Notch HUD work");
        assert_eq!(s.model.as_deref(), Some("Opus 5"));
        assert_eq!(s.preview, "On it.");
        assert_eq!(s.status, Status::Active);
        assert_eq!(s.messages, 1);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn a_custom_title_outranks_the_generated_one() {
        let now = Utc::now().to_rfc3339();
        let p = transcript(
            "titles",
            &[
                &format!(
                    r#"{{"type":"assistant","sessionId":"s9","cwd":"/tmp/z","timestamp":"{now}","message":{{"role":"assistant","content":[{{"type":"text","text":"hi"}}]}}}}"#
                ),
                r#"{"type":"ai-title","aiTitle":"generated","sessionId":"s9"}"#,
                r#"{"type":"custom-title","customTitle":"mine","sessionId":"s9"}"#,
            ],
        );
        assert_eq!(read_session(&p).unwrap().title, "mine");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn a_session_with_no_title_falls_back_to_the_project() {
        let now = Utc::now().to_rfc3339();
        let p = transcript(
            "untitled",
            &[&format!(
                r#"{{"type":"assistant","sessionId":"s8","cwd":"/Users/a/dev/thing","timestamp":"{now}","message":{{"role":"assistant","content":[{{"type":"text","text":"hi"}}]}}}}"#
            )],
        );
        assert_eq!(read_session(&p).unwrap().title, "thing");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn an_unanswered_tool_use_reads_as_tool_status() {
        let now = Utc::now().to_rfc3339();
        let p = transcript(
            "tool",
            &[&format!(
                r#"{{"type":"assistant","sessionId":"s2","cwd":"/tmp/x","timestamp":"{now}","message":{{"role":"assistant","model":"claude-sonnet-5","content":[{{"type":"tool_use","id":"t1","name":"Bash"}}]}}}}"#
            )],
        );
        assert_eq!(read_session(&p).unwrap().status, Status::Tool);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn an_answered_tool_use_is_not_tool_status() {
        let now = Utc::now().to_rfc3339();
        let p = transcript(
            "answered",
            &[
                &format!(
                    r#"{{"type":"assistant","sessionId":"s3","cwd":"/tmp/x","timestamp":"{now}","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Bash"}}]}}}}"#
                ),
                &format!(
                    r#"{{"type":"user","sessionId":"s3","cwd":"/tmp/x","timestamp":"{now}","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1"}}]}}}}"#
                ),
            ],
        );
        assert_eq!(read_session(&p).unwrap().status, Status::Active);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn a_stale_transcript_is_idle() {
        let old = (Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
        let p = transcript(
            "idle",
            &[&format!(
                r#"{{"type":"assistant","sessionId":"s4","cwd":"/tmp/y","timestamp":"{old}","message":{{"role":"assistant","content":[{{"type":"text","text":"done"}}]}}}}"#
            )],
        );
        let s = read_session(&p).unwrap();
        assert_eq!(s.status, Status::Idle);
        assert!(s.idle_secs > 3_000);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn a_transcript_without_turns_is_skipped() {
        let p = transcript(
            "stub",
            &[r#"{"type":"last-prompt","lastPrompt":"hi","sessionId":"s5"}"#],
        );
        assert!(read_session(&p).is_none());
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn an_idle_session_with_a_pending_tool_is_still_idle() {
        // A tool call left hanging hours ago is not work in progress.
        assert_eq!(status_of(&pending(&["t1"]), 5_000), Status::Idle);
        assert_eq!(status_of(&pending(&["t1"]), 10), Status::Tool);
        assert_eq!(status_of(&HashSet::new(), 10), Status::Active);
        assert_eq!(status_of(&HashSet::new(), 5_000), Status::Idle);
    }

    #[test]
    fn wrapped_command_text_is_not_previewed() {
        assert_eq!(clean_text("<command-name>/clear</command-name>"), None);
        assert_eq!(clean_text("  hello   world \n"), Some("hello world".into()));
        assert_eq!(clean_text("   "), None);
    }

    #[test]
    fn a_long_preview_is_truncated() {
        let long = "x".repeat(PREVIEW_CHARS + 50);
        let out = clean_text(&long).unwrap();
        assert_eq!(out.chars().count(), PREVIEW_CHARS + 1, "plus the ellipsis");
        assert!(out.ends_with('…'));
    }

    #[test]
    fn model_names_are_made_readable() {
        assert_eq!(pretty_model("claude-opus-5"), "Opus 5");
        assert_eq!(pretty_model("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(pretty_model("claude-sonnet-5"), "Sonnet 5");
        assert_eq!(
            pretty_model("weird"),
            "weird",
            "an unknown shape is left alone"
        );
    }

    #[test]
    fn payload_reports_the_active_count() {
        let v = payload();
        assert_eq!(
            v["available"],
            json!(true),
            "a missing tree is not an error"
        );
        assert!(v["active"].is_u64());
        assert!(v["total"].is_u64());
        assert!(v["list"].is_array());
        assert!(
            v["active"].as_u64().unwrap() <= v["total"].as_u64().unwrap(),
            "active cannot exceed total: {v}"
        );
    }
}
