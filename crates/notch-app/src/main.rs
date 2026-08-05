//! notch-app — the notch HUD itself: a background macOS app whose only window is
//! the panel hugging the notch.
//!
//! No dock icon and no menu-bar icon: the panel *is* the interface. Turn it off
//! with `notch off` from a terminal, which the reconcile tick below picks up.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod notch;

use std::thread;
use std::time::Duration;

use notch_core::{config, hud};
use tauri::{AppHandle, Manager};

/// How often the app checks whether the settings file has changed under it.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // A background app: no dock icon, stays alive without a visible window.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Read the notch geometry while we're still on the main thread —
            // AppKit only answers there, and commands may run off it.
            notch::prime_metrics();
            if config::load().enabled {
                let _ = notch::show(app.handle());
            }
            notch::spawn_hover_watcher(app.handle().clone());

            // Reconcile the panel with its settings file, so `notch on|off` from a
            // terminal lands without restarting the app.
            let handle = app.handle().clone();
            thread::spawn(move || loop {
                reconcile(&handle);
                thread::sleep(RECONCILE_INTERVAL);
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            notch_payload,
            notch_metrics,
            notch_pin,
            notch_toggle,
        ])
        .run(tauri::generate_context!())
        .expect("error while running notch-app");
}

/// Brings the panel in line with its settings file. Window work has to happen on
/// the main thread; this runs on the reconcile thread.
fn reconcile(app: &AppHandle) {
    let want = config::load().enabled;
    if want == notch::visible(app) {
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if want {
            let _ = notch::show(&handle);
        } else {
            notch::hide(&handle);
        }
    });
}

// ==================== COMMANDS ====================

/// Everything the panel renders, in one document.
///
/// `pmset` is shelled out to behind a cache, so this can block briefly — keep it
/// off the main thread.
#[tauri::command]
async fn notch_payload() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(hud::payload)
        .await
        .map_err(|e| e.to_string())
}

/// Screen geometry the panel needs to lay itself out around the physical notch,
/// plus the collapsed bounds the black shape animates from and the current open
/// state (so a reload mid-hover doesn't leave the panel stuck shut).
#[tauri::command]
fn notch_metrics() -> serde_json::Value {
    let m = notch::metrics();
    let (collapsed_w, collapsed_h) = notch::collapsed_bounds();
    serde_json::json!({
        "screen_w": m.screen_w,
        "menubar_h": m.menubar_h,
        "notch_w": m.notch_w,
        "collapsed_w": collapsed_w,
        "collapsed_h": collapsed_h,
        "open": notch::is_open(),
    })
}

/// Holds the panel open, or lets it close again. Returns the new state.
#[tauri::command]
fn notch_pin(on: Option<bool>) -> bool {
    match on {
        Some(want) => hud::set_pinned(want),
        None => hud::toggle_pinned(),
    }
}

/// Flips one module (or `"enabled"`, which shows or hides the whole panel) and
/// returns its new value.
#[tauri::command]
fn notch_toggle(app: AppHandle, key: String) -> Result<bool, String> {
    let next = config::load().toggled(&key).map_err(|e| e.to_string())?;
    config::save(&next).map_err(|e| e.to_string())?;
    let now = next.get(&key).map_err(|e| e.to_string())?;

    if key == "enabled" {
        if now {
            notch::show(&app).map_err(|e| e.to_string())?;
        } else {
            notch::hide(&app);
        }
    } else if let Some(win) = app.get_webview_window(notch::LABEL) {
        // A module came or went, so the panel needs to re-render from scratch.
        let _ = win.eval("window.notchHud?.refresh()");
    }
    Ok(now)
}
