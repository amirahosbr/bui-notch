//! notch-core — the framework-agnostic half of the notch HUD.
//!
//! Nothing here knows about Tauri, AppKit or a terminal, so the CLI and the app
//! share one implementation:
//!   - [`config`]   which modules are on, and how long the cursor must dwell
//!   - [`hud`]      the single JSON payload the panel renders, and the pin flag
//!   - [`doctor`]   whether the things the modules depend on are actually wired up
//!
//! One module per card, each answering with `available: false` and a reason rather
//! than failing the payload:
//!   - [`day`]      clock, date, day/week progress, battery
//!   - [`usage`]    Claude session and weekly limits
//!   - [`git`]      GitHub contributions
//!   - [`sessions`] live Claude Code sessions
//!   - [`todos`]    a to-do briefing written by something else

pub mod config;
pub mod day;
pub mod doctor;
pub mod format;
pub mod git;
pub mod history;
pub mod hud;
pub mod live;
pub mod sessions;
pub mod todos;
pub mod token;
pub mod usage;

use std::path::PathBuf;

/// Where the HUD keeps its settings.
/// On macOS: `~/Library/Application Support/bui-notch`.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("bui-notch"))
        .unwrap_or_else(|| PathBuf::from("bui-notch"))
}
