//! Window geometry persistence: the app reopens the window the user last left.
//!
//! The two halves of one contract, kept together. [`launch_geometry`] feeds the
//! `WindowBuilder` before the event loop exists; [`use_window_geometry`] — mounted once,
//! by the root component — records what the user then does to the window.
//!
//! The values themselves live in `AppState` and ride to disk in `global.setting.yaml`
//! with the rest of the settings. That is not incidental: the settings document is
//! written *whole* (see `AppState::make_global_settings_payload`), so a key written
//! behind AppState's back would be dropped by the next unrelated settings write.
//!
//! One thing here is not about geometry: the `CloseRequested` arm is also where the
//! session's **navigation** state is written (`persist_settings_now`). It lives here
//! because this is the only place in the application that knows the window is going away,
//! and because both are the same kind of value — a fact about the workspace, saved once
//! on the way out rather than as the user works.

use std::time::{Duration, Instant};

use dioxus::desktop::tao::event::{Event, WindowEvent};
use dioxus::desktop::{use_wry_event_handler, window, DesktopContext, LogicalSize};
use dioxus::prelude::*;

use crate::runtime::{store_window_geometry, with_ctx};

/// How often a size change may reach the settings file while the user is dragging the
/// window edge.
///
/// A drag delivers a `Resized` per frame and each save re-serializes the whole settings
/// document. The final size is written on close regardless, so this interval only
/// governs how much of a long drag would survive the app being killed rather than
/// closed.
const SAVE_INTERVAL: Duration = Duration::from_millis(750);

/// The window size (logical pixels) and maximized state the last session left.
pub fn launch_geometry() -> (LogicalSize<f64>, bool) {
    with_ctx(|ctx| {
        (
            LogicalSize::new(ctx.app.window_width as f64, ctx.app.window_height as f64),
            ctx.app.window_maximized,
        )
    })
}

/// Keeps the persisted geometry in step with the live window. Call once, from the root
/// component; the handler it registers lives as long as that component does.
pub fn use_window_geometry() {
    let desktop = use_hook(window);

    // A stored size can outlive the monitor it was chosen on — unplug the large screen
    // and the window reopens bigger than the display, with its edges (and possibly its
    // title bar) out of reach. The `WindowBuilder` cannot ask about monitors, there
    // being no event loop yet, so the correction happens here on the first render, once
    // there is a window to measure.
    use_hook(|| clamp_to_monitor(&desktop));

    // Restoring *maximized* takes a second attempt, on top of the builder's. Dioxus
    // creates the window hidden and shows it only once the webview has loaded, and a
    // debug build additionally moves it to the last session's position on the way — both
    // of which drop a maximized state set at creation (verified: the builder flag alone
    // reopens as an ordinary window). So the flag is re-applied on the first event the
    // shown window delivers, which is the earliest point it sticks.
    let mut pending_maximize = with_ctx(|ctx| ctx.app.window_maximized);
    let mut last_saved: Option<Instant> = None;
    let desktop = desktop.clone();

    use_wry_event_handler(move |event, _target| {
        let Event::WindowEvent { event, .. } = event else {
            return;
        };
        match event {
            // Whichever of the two the platform sends first once the window is up.
            WindowEvent::Resized(_) | WindowEvent::Focused(true) if pending_maximize => {
                pending_maximize = false;
                desktop.set_maximized(true);
            }
            // Rate-limited: see `SAVE_INTERVAL`. A save whose geometry is unchanged
            // costs nothing beyond the read — the runtime drops it.
            WindowEvent::Resized(_) => {
                if last_saved.is_none_or(|at| at.elapsed() >= SAVE_INTERVAL) {
                    save(&desktop);
                    last_saved = Some(Instant::now());
                }
            }
            // The last word on the session's geometry, and the only chance to get it to
            // disk: the write is queued on a background thread the process is about to
            // exit out from under, hence the flush.
            //
            // The order is load-bearing. `save` puts the final geometry into `AppState`
            // first, so the settings write that follows carries the geometry *and* the
            // navigation state in one document; the flush goes last, once nothing else
            // will queue.
            WindowEvent::CloseRequested => {
                save(&desktop);
                crate::runtime::persist_settings_now();
                crate::data::flush_appdata();
            }
            _ => {}
        }
    });
}

/// Reads the window's current geometry and hands it to the runtime, which persists it
/// only if it actually changed.
fn save(desktop: &DesktopContext) {
    // A minimized window measures 0×0 and describes nothing worth reopening.
    if desktop.is_minimized() {
        return;
    }

    let maximized = desktop.is_maximized();
    // While maximized, the inner size is the screen's, not the user's choice; the stored
    // restore size is left alone (see `AppState::set_window_geometry`).
    let size = (!maximized)
        .then(|| {
            let size = desktop.inner_size().to_logical::<f64>(desktop.scale_factor());
            (size.width.round() as i64, size.height.round() as i64)
        })
        .filter(|(width, height)| *width > 0 && *height > 0);

    store_window_geometry(size, maximized);
}

/// Shrinks the window to fit the monitor it opened on, when the restored size no longer
/// does. Maximized windows are left alone — the platform has already fitted those.
fn clamp_to_monitor(desktop: &DesktopContext) {
    if desktop.is_maximized() {
        return;
    }
    let Some(monitor) = desktop.current_monitor() else {
        return;
    };

    let scale = desktop.scale_factor();
    let available = monitor.size().to_logical::<f64>(scale);
    let current = desktop.inner_size().to_logical::<f64>(scale);

    let width = current.width.min(available.width);
    let height = current.height.min(available.height);
    if width < current.width || height < current.height {
        desktop.set_inner_size(LogicalSize::new(width, height));
    }
}
