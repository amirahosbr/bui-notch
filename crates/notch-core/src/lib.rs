//! notch-core — the framework-agnostic half of the notch HUD.
//!
//! Nothing here knows about Tauri, AppKit or a terminal, so the CLI and the app
//! share one implementation:
//!   - [`config`] which modules are on, and how long the cursor must dwell
//!   - [`day`]    clock, date, day/week progress, battery
//!   - [`hud`]    the single JSON payload the panel renders, and the pin flag

pub mod config;
pub mod day;
pub mod hud;

use std::path::PathBuf;

/// Where the HUD keeps its settings.
/// On macOS: `~/Library/Application Support/bui-notch`.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("bui-notch"))
        .unwrap_or_else(|| PathBuf::from("bui-notch"))
}
