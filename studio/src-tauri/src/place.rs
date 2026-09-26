//! Where the main window was: put back when the app starts, remembered as it moves and
//! resizes, and written with the rest of the preferences when the window closes.
//!
//! The rectangle is in the window system's own (physical) pixels, the one coordinate space
//! every monitor shares, and it is put back only if enough of its title strip lands on a
//! monitor that is still there ([`WindowPlace::reachable`]); otherwise the window opens
//! centred at its configured size, as it always did. The decisions are in
//! `inkvec_studio_core::interface` and tested there; this module only reads and moves the
//! window.

use tauri::{Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow, Window};

use crate::settings::{self, WindowPlace};
use crate::AppState;

/// The window the place belongs to.
pub const MAIN: &str = "main";

fn screens<R: Runtime>(w: &WebviewWindow<R>) -> Vec<(f64, f64, f64, f64)> {
    w.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let (p, s) = (m.position(), m.size());
            (p.x as f64, p.y as f64, s.width as f64, s.height as f64)
        })
        .collect()
}

/// Size and move the (still hidden) main window to where it was, if that can be reached.
/// Maximising waits for [`show`]: on some platforms it would show the window at once,
/// under the splash.
pub fn restore<R: Runtime>(main: &WebviewWindow<R>, place: Option<WindowPlace>) {
    let Some(p) = place else { return };
    if !p.reachable(&screens(main)) {
        return;
    }
    let _ = main.set_size(PhysicalSize::new(
        p.width.round() as u32,
        p.height.round() as u32,
    ));
    let _ = main.set_position(PhysicalPosition::new(
        p.x.round() as i32,
        p.y.round() as i32,
    ));
}

/// Show the main window, maximised if it was.
pub fn show<R: Runtime>(main: &WebviewWindow<R>, place: Option<WindowPlace>) {
    let _ = main.show();
    if place.is_some_and(|p| p.maximized) {
        let _ = main.maximize();
    }
}

/// Note the main window's place after it moved or was resized. Minimised, nothing is
/// noted; maximised, only the flag, so the rectangle stays the one it un-maximises to.
pub fn record<R: Runtime>(window: &Window<R>) {
    if window.label() != MAIN || window.is_minimized().unwrap_or(true) {
        return;
    }
    let state = window.state::<AppState>();
    let Ok(mut prefs) = state.prefs.lock() else {
        return;
    };
    let maximized = window.is_maximized().unwrap_or(false);
    let next = match (maximized, prefs.window) {
        (true, Some(old)) => Some(WindowPlace {
            maximized: true,
            ..old
        }),
        (true, None) => None,
        (false, _) => match (window.outer_position(), window.inner_size()) {
            (Ok(pos), Ok(size)) => WindowPlace {
                x: pos.x as f64,
                y: pos.y as f64,
                width: size.width as f64,
                height: size.height as f64,
                maximized: false,
            }
            .sanitised(),
            _ => prefs.window,
        },
    };
    prefs.window = next;
}

/// The window is closing: write the preferences, so its place (and anything else only
/// held in memory) is there next time.
pub fn save<R: Runtime>(window: &Window<R>) {
    if window.label() != MAIN {
        return;
    }
    record(window);
    let state = window.state::<AppState>();
    let prefs = match state.prefs.lock() {
        Ok(p) => p.clone(),
        Err(_) => return,
    };
    let _ = settings::save(&prefs);
}
