//! The notch panel — pinned to the top of the menu-bar screen so it hugs the
//! notch, showing a collapsed sliver by default and expanding to the module grid
//! on hover.
//!
//! Two macOS details make it behave like a notch app rather than a floating
//! window: the window level is raised to `NSStatusWindowLevel` (25) so it draws
//! *over* the menu bar instead of under it, and its collection behaviour marks it
//! stationary and joinable on all Spaces so it doesn't slide away when you switch
//! desktops. Everything else is plain Tauri.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

/// The panel's window label.
pub const LABEL: &str = "notch";

/// Content width either side of the physical notch when collapsed.
const COLLAPSED_SIDE: f64 = 134.0;
/// Collapsed width floor, for displays with no notch to flank.
const COLLAPSED_MIN_W: f64 = 268.0;
const EXPANDED_W: f64 = 680.0;
/// Tab bar, the overview cards, and the five session rows `notch.js` shows there
/// (`SESSION_ROWS`) plus the "+N older" line — change both together.
const EXPANDED_H: f64 = 462.0;
/// Menu-bar height to assume when macOS won't tell us (no notch, or non-macOS).
const MENUBAR_FALLBACK: f64 = 24.0;
/// Collapsed height floor, so the sliver stays hoverable on short menu bars.
const COLLAPSED_MIN_H: f64 = 26.0;

/// Geometry of the screen that owns the menu bar, in logical points.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub screen_w: f64,
    /// Height of the menu-bar row (the notch's height on notched displays).
    pub menubar_h: f64,
    /// Width of the physical notch, or 0 on displays without one.
    pub notch_w: f64,
}

impl Metrics {
    fn fallback() -> Self {
        Self {
            screen_w: 1440.0,
            menubar_h: MENUBAR_FALLBACK,
            notch_w: 0.0,
        }
    }

    /// Bounds of the visible sliver — also the region that triggers opening.
    fn collapsed_size(&self) -> LogicalSize<f64> {
        LogicalSize::new(
            (self.notch_w + 2.0 * COLLAPSED_SIDE).max(COLLAPSED_MIN_W),
            self.menubar_h.max(COLLAPSED_MIN_H),
        )
    }

    /// Bounds of the open panel — also the region that keeps it open.
    fn expanded_size(&self) -> LogicalSize<f64> {
        LogicalSize::new(EXPANDED_W, EXPANDED_H)
    }

    /// The window is always this big, whichever state the panel draws.
    ///
    /// Resizing a window can't be animated — every step is a discrete jump, and
    /// the webview relayouts on each one. So the window is fixed at the open size
    /// and CSS animates the black shape inside it, which composites on the GPU.
    /// While collapsed the surrounding area is transparent *and* ignores the
    /// cursor, so it doesn't swallow menu-bar clicks.
    fn window_size(&self) -> LogicalSize<f64> {
        self.expanded_size()
    }
}

static METRICS: OnceLock<Metrics> = OnceLock::new();

/// Screen geometry, read once. Call [`prime_metrics`] from the main thread during
/// setup; later calls reuse that reading.
pub fn metrics() -> Metrics {
    *METRICS.get_or_init(read_metrics)
}

/// Reads and caches the screen geometry. Must run on the main thread, which is
/// where Tauri's `setup` hook runs — AppKit refuses to answer anywhere else.
pub fn prime_metrics() -> Metrics {
    let m = metrics();
    eprintln!(
        "notch: screen {:.0}pt · menubar {:.1}pt · notch {:.1}pt",
        m.screen_w, m.menubar_h, m.notch_w
    );
    m
}

#[cfg(target_os = "macos")]
fn read_metrics() -> Metrics {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;

    // No marker means we're off the main thread and can't touch NSScreen.
    let Some(mtm) = MainThreadMarker::new() else {
        return Metrics::fallback();
    };
    // screens()[0] is by definition the screen carrying the menu bar.
    let Some(screen) = NSScreen::screens(mtm).firstObject() else {
        return Metrics::fallback();
    };

    let frame = screen.frame();
    let inset_top = screen.safeAreaInsets().top;
    let menubar_h = if inset_top > 0.0 {
        inset_top
    } else {
        // Unnotched: the menu bar is the gap between the full and visible frames.
        let gap = frame.size.height - screen.visibleFrame().size.height;
        if gap > 0.0 {
            gap
        } else {
            MENUBAR_FALLBACK
        }
    };

    // The two auxiliary areas are the usable strips left and right of the notch;
    // what they don't cover is the notch itself. Both are empty on unnotched
    // displays, so guard on the safe-area inset instead of the maths.
    let notch_w = if inset_top > 0.0 {
        let left = screen.auxiliaryTopLeftArea().size.width;
        let right = screen.auxiliaryTopRightArea().size.width;
        (frame.size.width - left - right).max(0.0)
    } else {
        0.0
    };

    Metrics {
        screen_w: frame.size.width,
        menubar_h,
        notch_w,
    }
}

#[cfg(not(target_os = "macos"))]
fn read_metrics() -> Metrics {
    Metrics::fallback()
}

/// Raises `win` above the menu bar and makes it follow the user across Spaces.
#[cfg(target_os = "macos")]
fn apply_panel_traits(win: &WebviewWindow) {
    use objc2_app_kit::{NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior};

    let Ok(ptr) = win.ns_window() else { return };
    if ptr.is_null() {
        return;
    }
    // Borrowed, not retained: Tauri owns this window for the app's lifetime.
    let ns: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
    ns.setLevel(NSStatusWindowLevel);
    ns.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    // The panel draws its own rounded shape; a system shadow would outline the
    // whole rectangle, including the transparent corners.
    ns.setHasShadow(false);
}

#[cfg(not(target_os = "macos"))]
fn apply_panel_traits(_win: &WebviewWindow) {}

/// Centres the panel on the menu-bar screen at its full size, flush with the top
/// edge. The window keeps this one size for its whole life — see [`expand`].
fn place(win: &WebviewWindow) {
    let m = metrics();
    let size = m.window_size();
    let x = ((m.screen_w - size.width) / 2.0).max(0.0);
    let _ = win.set_size(size);
    let _ = win.set_position(LogicalPosition::new(x, 0.0));
    trace(|| format!("placed {:.0}x{:.0} at x={:.0}", size.width, size.height, x));
}

/// Logs panel geometry changes when `NOTCH_DEBUG` is set — the panel's own
/// behaviour is otherwise invisible to anything but your eyes.
fn trace(msg: impl FnOnce() -> String) {
    if std::env::var_os("NOTCH_DEBUG").is_some() {
        eprintln!("notch: {}", msg());
    }
}

/// Creates the panel if needed and shows it collapsed. Never takes focus, so
/// whatever you were typing in keeps it.
pub fn show(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(win) = app.get_webview_window(LABEL) {
        collapse(app);
        place(&win);
        let _ = win.show();
        return Ok(win);
    }

    let m = metrics();
    let size = m.window_size();
    let win = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("notch.html".into()))
        .title("notch")
        .inner_size(size.width, size.height)
        .position(((m.screen_w - size.width) / 2.0).max(0.0), 0.0)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .focused(false)
        .visible(false)
        .build()?;

    apply_panel_traits(&win);
    // Click-through until the cursor is actually over the sliver — the watcher
    // takes the cursor back then, so a click can land, and again while the panel
    // is open or pinned so it can be clicked and scrolled.
    //
    // Clicking it does focus the app. Making it a truly non-activating window is
    // not available to us: `tauri-nspanel` is a git-only dependency, and doing it
    // by hand crashes — tao's `send_event` computes `superclass(self)` and sends a
    // super-message, so inserting any class into the chain makes that resolve to
    // tao's own implementation and recurse until the stack dies.
    let _ = win.set_ignore_cursor_events(true);
    let _ = win.show();
    Ok(win)
}

/// Hides the panel, leaving it built so the next show is instant.
pub fn hide(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(LABEL) {
        let _ = win.hide();
    }
}

/// Width and height of the collapsed sliver, which the page animates from.
pub fn collapsed_bounds() -> (f64, f64) {
    let s = metrics().collapsed_size();
    (s.width, s.height)
}

/// Whether the panel is currently on screen.
pub fn visible(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL)
        .map(|w| w.is_visible().unwrap_or(false))
        .unwrap_or(false)
}

/// Whether the panel is drawn open. The page reads this on load so a reload while
/// the cursor is already inside doesn't leave it stuck shut.
static OPEN: AtomicBool = AtomicBool::new(false);

/// Whether the panel is currently drawn open.
pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Opens the panel and starts accepting the cursor, so the tabs can be clicked.
pub fn expand(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(LABEL) {
        OPEN.store(true, Ordering::Relaxed);
        let _ = win.set_ignore_cursor_events(false);
        let _ = win.eval("window.notchHud?.setOpen(true)");
        trace(|| "open".to_string());
    }
}

/// Closes the panel and goes back to letting the cursor through, so the sliver's
/// transparent surroundings don't swallow menu-bar clicks.
pub fn collapse(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(LABEL) {
        OPEN.store(false, Ordering::Relaxed);
        let _ = win.eval("window.notchHud?.setOpen(false)");
        // Keep taking the cursor until the shape has finished shrinking, or the
        // window stops tracking mid-animation.
        let win = win.clone();
        thread::spawn(move || {
            thread::sleep(CLOSE_ANIM);
            let _ = win.set_ignore_cursor_events(true);
        });
        trace(|| "close".to_string());
    }
}

// --- hover ---------------------------------------------------------------

/// How often the cursor is sampled while it might matter: near the top edge, or
/// with the panel already open.
const HOVER_POLL_NEAR: Duration = Duration::from_millis(90);
/// How often it's sampled otherwise. The watcher only ever asks whether the
/// cursor is in the top strip, so polling eleven times a second while it sits at
/// the bottom of the screen is wasted wake-ups — and idle wake-ups are what cost
/// battery.
const HOVER_POLL_FAR: Duration = Duration::from_millis(350);
/// Within this many points of the top edge, sample at the fast rate. Generous
/// enough that the cursor is already being watched closely before it arrives.
const NEAR_TOP_PT: f64 = 140.0;
/// How long the cursor must stay outside before the panel shrinks again.
const COLLAPSE_GRACE: Duration = Duration::from_millis(260);
/// Must outlast the closing transition in notch.html.
const CLOSE_ANIM: Duration = Duration::from_millis(360);

/// How long to wait before looking at the cursor again.
///
/// Fast while the panel is open or pinned — a leaving cursor has to be noticed
/// promptly, and the page's hover feed comes from here. Fast also when the cursor
/// is near the top edge, so it's already being watched by the time it reaches the
/// sliver. Slow the rest of the time, which is most of the time.
fn poll_interval(expanded: bool) -> Duration {
    if expanded || notch_core::hud::pinned() {
        return HOVER_POLL_NEAR;
    }
    match cursor() {
        Some((_, y)) if y <= NEAR_TOP_PT => HOVER_POLL_NEAR,
        _ => HOVER_POLL_FAR,
    }
}

/// Cursor position in top-left-origin screen points, or `None` if unavailable.
///
/// Read through CoreGraphics rather than `NSEvent::mouseLocation` because this
/// runs on a worker thread and CG, unlike AppKit, is documented thread-safe.
#[cfg(target_os = "macos")]
fn cursor() -> Option<(f64, f64)> {
    use objc2_core_graphics::CGEvent;

    let event = CGEvent::new(None)?;
    let p = CGEvent::location(Some(&event));
    Some((p.x, p.y))
}

#[cfg(not(target_os = "macos"))]
fn cursor() -> Option<(f64, f64)> {
    None
}

/// How long the cursor must rest in the sliver before the panel opens.
/// `notch delay <ms>` (0 opens on contact).
fn open_dwell() -> Duration {
    Duration::from_millis(notch_core::config::load().open_delay_ms)
}

/// Whether the cursor is inside the panel's current bounds.
fn cursor_over_panel(expanded: bool) -> bool {
    let Some((cx, cy)) = cursor() else {
        return false;
    };
    let m = metrics();
    let size = if expanded {
        m.expanded_size()
    } else {
        m.collapsed_size()
    };
    inside(size, m.screen_w, cx, cy)
}

/// Whether `(cx, cy)` falls inside a centred panel of `size` on a screen of
/// `screen_w`. Pure, so the hit test can be checked without a cursor or a screen.
fn inside(size: LogicalSize<f64>, screen_w: f64, cx: f64, cy: f64) -> bool {
    let x0 = ((screen_w - size.width) / 2.0).max(0.0);
    cx >= x0 && cx <= x0 + size.width && cy >= 0.0 && cy <= size.height
}

/// The panel's own left edge, which is where the *window* starts. While open the
/// window and the panel are the same size, so this is the origin the page's
/// coordinates are relative to.
fn window_origin_x() -> f64 {
    let m = metrics();
    ((m.screen_w - m.window_size().width) / 2.0).max(0.0)
}

/// Tells the page where the cursor is, in its own CSS pixels.
///
/// Dispatched to the main thread: this runs on the watcher thread, and evaluating
/// script in a WKWebView from anywhere else doesn't take effect.
fn report_cursor(app: &AppHandle) {
    let Some((cx, cy)) = cursor() else { return };
    let x = cx - window_origin_x();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = handle.get_webview_window(LABEL) {
            let _ = win.eval(format!("window.notchHud?.cursor({x:.0},{cy:.0})"));
        }
    });
}

/// Starts the watcher that expands the panel on hover and collapses it after.
///
/// A webview's own `mouseenter` can't drive this: WebKit's tracking areas are
/// scoped to the key window, and this panel deliberately never takes focus, so
/// the events would only arrive after you clicked it. Sampling the cursor works
/// no matter which app is active.
pub fn spawn_hover_watcher(app: AppHandle) {
    thread::spawn(move || {
        let mut expanded = false;
        let mut outside_since: Option<Instant> = None;
        let mut inside_since: Option<Instant> = None;
        let mut dwell = open_dwell();
        // Mirrors the window's ignore_cursor_events, so it's only set on a change.
        let mut accepting_clicks = false;

        loop {
            thread::sleep(poll_interval(expanded));

            if !visible(&app) {
                // Hidden panels are re-shown collapsed, so drop any hover state.
                expanded = false;
                outside_since = None;
                inside_since = None;
                continue;
            }

            // Pinning is a request to see it, so open even if the cursor is
            // nowhere near.
            if notch_core::hud::pinned() && !expanded {
                expanded = true;
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || expand(&handle));
                trace(|| "open (pinned)".to_string());
            }

            // Collapsed, the window normally lets the cursor through so it can't
            // block the menu bar — but then a click on the sliver would go
            // straight past it. So take the cursor only while it's actually over
            // the sliver, which is exactly the region meant to be clickable.
            let over = cursor_over_panel(expanded);
            let want_clicks = over || expanded || notch_core::hud::pinned();
            if want_clicks != accepting_clicks {
                accepting_clicks = want_clicks;
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Some(win) = handle.get_webview_window(LABEL) {
                        let _ = win.set_ignore_cursor_events(!want_clicks);
                    }
                });
            }

            if over {
                outside_since = None;
                if !expanded {
                    // Wait for the cursor to settle: crossing the notch on the
                    // way to the menu bar shouldn't pop the panel open, only
                    // stopping there should.
                    let since = match inside_since {
                        Some(t) => t,
                        None => {
                            // Read the setting once per approach rather than on
                            // every 90ms sample.
                            dwell = open_dwell();
                            let now = Instant::now();
                            inside_since = Some(now);
                            now
                        }
                    };
                    if since.elapsed() < dwell {
                        continue;
                    }
                    expanded = true;
                    let handle = app.clone();
                    let _ = app.run_on_main_thread(move || expand(&handle));
                } else {
                    // Feed the page the cursor so it can light up what's under it
                    // and switch tabs. WebKit's own :hover can't be relied on
                    // here: its tracking areas are scoped to the key window, and
                    // this panel is never key.
                    report_cursor(&app);
                }
                continue;
            }
            inside_since = None;

            if !expanded {
                continue;
            }
            // Pinned panels don't close on their own; that's the whole point.
            if notch_core::hud::pinned() {
                outside_since = None;
                continue;
            }

            // Outside: shrink once the grace period has fully elapsed, so a
            // cursor grazing the edge doesn't make the panel flicker.
            match outside_since {
                None => outside_since = Some(Instant::now()),
                Some(since) if since.elapsed() >= COLLAPSE_GRACE => {
                    expanded = false;
                    outside_since = None;
                    let handle = app.clone();
                    let _ = app.run_on_main_thread(move || collapse(&handle));
                }
                Some(_) => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_flanks_the_notch() {
        let m = Metrics {
            screen_w: 1512.0,
            menubar_h: 37.0,
            notch_w: 200.0,
        };
        let s = m.collapsed_size();
        assert_eq!(s.width, 200.0 + 2.0 * COLLAPSED_SIDE);
        assert_eq!(s.height, 37.0);
    }

    #[test]
    fn collapsed_has_a_floor_without_a_notch() {
        let m = Metrics {
            screen_w: 1440.0,
            menubar_h: 12.0,
            notch_w: 0.0,
        };
        let s = m.collapsed_size();
        assert_eq!(s.width, COLLAPSED_MIN_W);
        assert_eq!(s.height, COLLAPSED_MIN_H, "stays hoverable");
    }

    #[test]
    fn the_window_never_changes_size() {
        // The shape animates in CSS precisely because the window cannot.
        let m = Metrics::fallback();
        assert_eq!(m.window_size().width, m.expanded_size().width);
        assert_eq!(m.window_size().height, m.expanded_size().height);
    }

    #[test]
    fn the_sliver_is_never_taller_than_the_open_panel() {
        let m = Metrics {
            screen_w: 1512.0,
            menubar_h: 37.0,
            notch_w: 200.0,
        };
        assert!(m.collapsed_size().height <= m.expanded_size().height);
        assert!(m.collapsed_size().width <= m.expanded_size().width);
    }

    #[test]
    fn the_hit_test_covers_the_panel_and_nothing_else() {
        // A 100pt-wide panel on a 1000pt screen sits from x=450 to x=550.
        let size = LogicalSize::new(100.0, 30.0);
        assert!(inside(size, 1000.0, 500.0, 15.0), "centre");
        assert!(inside(size, 1000.0, 450.0, 0.0), "top-left corner");
        assert!(inside(size, 1000.0, 550.0, 30.0), "bottom-right corner");
        assert!(!inside(size, 1000.0, 449.0, 15.0), "just left");
        assert!(!inside(size, 1000.0, 551.0, 15.0), "just right");
        assert!(!inside(size, 1000.0, 500.0, 31.0), "just below");
        assert!(!inside(size, 1000.0, 500.0, -1.0), "above the screen edge");
    }

    #[test]
    fn the_hit_test_survives_a_panel_wider_than_the_screen() {
        // The panel is clamped to x=0 rather than going negative.
        let size = LogicalSize::new(600.0, 30.0);
        assert!(inside(size, 400.0, 0.0, 0.0));
        assert!(inside(size, 400.0, 399.0, 10.0));
    }

    #[test]
    fn the_closing_grace_outlasts_the_animation() {
        // If the window stopped tracking the cursor before the shape finished
        // shrinking, the close would visibly stutter.
        assert!(
            CLOSE_ANIM > COLLAPSE_GRACE - Duration::from_millis(200),
            "the click-through hand-back must not land mid-animation"
        );
    }

    #[test]
    fn the_far_poll_is_slower_than_the_near_one() {
        assert!(
            HOVER_POLL_FAR > HOVER_POLL_NEAR,
            "idle wake-ups cost battery"
        );
    }
}
