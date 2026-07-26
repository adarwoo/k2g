pub mod bindings;
pub mod help;
pub mod navigation;
pub mod screens;
pub mod show_when;
pub mod theme;

use std::sync::OnceLock;

pub use navigation::UiLaunchData;

static BOOT_DATA: OnceLock<UiLaunchData> = OnceLock::new();

/// The vendored three.js bundle (see `assets/vendor/README.md`).
///
/// Compiled into the binary and injected into the document head at launch, because the
/// app has no asset server at runtime — every asset here is an `include_*!`. It exposes
/// one global, `window.K2G_THREE`.
const THREE_BUNDLE: &str = include_str!("../../assets/vendor/three.bundle.js");

/// Records the last uncaught error from the WebView so a failure is visible.
///
/// A broken script in a WebView fails *silently* — a blank canvas and nothing in the
/// terminal. WebView2's devtools have not been reliable on this project, so the page
/// reports its own errors instead: this handler stashes them where the Rust side can
/// pick them up and log them (see [`crate::ui::webview_error`]).
const ERROR_TRAP: &str = r#"
window.__k2g_errors = window.__k2g_errors || [];
window.addEventListener('error', function (e) {
  window.__k2g_errors.push(
    (e.message || 'script error') + ' @ ' + (e.filename || '?') + ':' + (e.lineno || 0)
  );
});
window.addEventListener('unhandledrejection', function (e) {
  window.__k2g_errors.push('unhandled promise rejection: ' + e.reason);
});
"#;

/// Drains the errors the page has collected since the last call, as log lines.
///
/// Returns the script to evaluate; the caller awaits it and logs whatever comes back.
/// Kept here beside [`ERROR_TRAP`] so the two halves of the contract stay together.
pub const DRAIN_ERRORS: &str = r#"
const drained = window.__k2g_errors || [];
window.__k2g_errors = [];
dioxus.send(drained);
"#;

pub fn launch(data: UiLaunchData) {
    let _ = BOOT_DATA.set(data);
    // `initialize_ctx` initializes the AppData store and hydrates the legacy
    // context from it (AppData is the single reader/writer of persisted state).
    crate::runtime::initialize_ctx(boot_data().clone());

    let window = dioxus::desktop::WindowBuilder::new()
        .with_title("k2g - KiCAD to GCode")
        .with_window_icon(load_window_icon());

    // The error trap goes in first, so it is already listening while three.js parses.
    let head = format!("<script>{ERROR_TRAP}</script>\n<script>{THREE_BUNDLE}</script>");

    let cfg = dioxus::desktop::Config::default()
        .with_menu(None)
        .with_custom_head(head)
        .with_window(window);

    dioxus::prelude::LaunchBuilder::desktop()
        .with_cfg(cfg)
        .launch(screens::AppRoot);
}

fn load_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let icon_bytes = include_bytes!("../../assets/icons/icon.png");
    let image = image::load_from_memory(icon_bytes).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(image.into_raw(), width, height).ok()
}

pub fn boot_data() -> &'static UiLaunchData {
    BOOT_DATA
        .get()
        .expect("UI launch data must be initialized before launch")
}
