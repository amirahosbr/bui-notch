//! Finding the Claude Code OAuth token the usage module probes with.
//!
//! Four places, in order: the environment, a token written here by hand, the
//! macOS keychain, then `~/.claude/.credentials.json`. Claude Code itself keeps
//! one in the keychain, so on a machine that runs Claude Code this usually needs
//! no setup at all.
//!
//! Nothing here ever logs or returns the token in an error message.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Result};
use serde::Deserialize;

const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Where a hand-written token is kept (local only, never git).
pub fn token_path() -> PathBuf {
    crate::config_dir().join("token")
}

/// A token written to [`token_path`], or `None`.
pub fn stored() -> Option<String> {
    let s = fs::read_to_string(token_path()).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Writes a token with owner-only permissions.
pub fn save(token: &str) -> Result<()> {
    let path = token_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, token.trim())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The token to probe with, from wherever it can be found.
pub fn find() -> Result<String> {
    if let Ok(t) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    // A token put here by hand wins over the keychain, so it can be overridden.
    stored()
        .or_else(keychain)
        .or_else(credentials_file)
        .ok_or_else(|| {
            anyhow!("no Claude OAuth token in the keychain or ~/.claude/.credentials.json")
        })
}

fn keychain() -> Option<String> {
    let user = std::env::var("USER").ok()?;
    let out = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            &user,
            "-w",
        ])
        .output()
        .ok()?;
    out.status.success().then(|| parse(&out.stdout))?
}

fn credentials_file() -> Option<String> {
    let home = dirs::home_dir()?;
    let data = fs::read(home.join(".claude").join(".credentials.json")).ok()?;
    parse(&data)
}

#[derive(Deserialize)]
struct CredsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OauthBlob>,
}

#[derive(Deserialize)]
struct OauthBlob {
    #[serde(rename = "accessToken")]
    access_token: String,
}

/// Pulls `claudeAiOauth.accessToken` specifically — the credentials blob also
/// carries an `mcpOAuth` section holding a different, wrong token.
fn parse(data: &[u8]) -> Option<String> {
    let f: CredsFile = serde_json::from_slice(data).ok()?;
    let t = f.claude_ai_oauth?.access_token;
    (!t.is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_prefers_the_claude_ai_oauth_token() {
        let blob =
            br#"{"claudeAiOauth":{"accessToken":"good"},"mcpOAuth":{"accessToken":"wrong"}}"#;
        assert_eq!(parse(blob).as_deref(), Some("good"));
    }

    #[test]
    fn parse_rejects_what_it_cannot_use() {
        assert!(parse(b"{}").is_none(), "no oauth section");
        assert!(
            parse(br#"{"claudeAiOauth":{"accessToken":""}}"#).is_none(),
            "an empty token is no token"
        );
        assert!(parse(b"not json").is_none());
        assert!(
            parse(br#"{"mcpOAuth":{"accessToken":"wrong"}}"#).is_none(),
            "the mcp token must never be used"
        );
    }

    #[test]
    fn the_error_never_carries_a_token() {
        // `find` may succeed on this machine; when it fails, the message must be
        // safe to print in a HUD.
        if let Err(e) = find() {
            let msg = e.to_string();
            assert!(!msg.contains("sk-"), "leaked a token: {msg}");
            assert!(msg.contains("no Claude OAuth token"));
        }
    }
}
