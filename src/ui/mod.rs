pub mod bindings;
pub mod help;
pub mod navigation;
pub mod screens;
pub mod show_when;
pub mod theme;
pub mod window_state;

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

    // Reopen the window the last session left. This reads the ctx, so it must follow
    // `initialize_ctx`; the matching recorder is mounted by `screens::AppRoot`.
    let (size, maximized) = window_state::launch_geometry();
    let window = dioxus::desktop::WindowBuilder::new()
        .with_title("k2g - KiCAD to GCode")
        .with_inner_size(size)
        .with_maximized(maximized)
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

/// Edge length of the icon handed to the window manager.
///
/// Not cosmetic, and not arbitrary. On X11 the window icon reaches the desktop as the
/// `_NET_WM_ICON` property, and GTK installs it only up to a point: measured on
/// WebKitGTK/GTK3 here, 256×256 lands, while 512×512 and the master's own 1024×1024 are
/// dropped — no error, no warning, and the property simply absent. The taskbar and the
/// Alt-Tab switcher then have nothing to draw, which is exactly the blank icon this
/// solves. Windows was unaffected throughout, because it does not go through
/// `_NET_WM_ICON` at all.
///
/// 256 is also the largest size Windows itself draws, so scaling here costs nothing on
/// either platform and saves shipping a second asset.
const WINDOW_ICON_EDGE: u32 = 256;

/// Point the platform's own widgets at the same light or dark as the rest of k2g.
///
/// Nearly all of the UI is drawn by the webview from `theme.rs`, so it follows the app's
/// setting for free. A `<select>`'s open list is the exception: WebKitGTK does not draw it
/// at all, it hands it to GTK, which paints it from the *desktop* theme. Set k2g to dark
/// on a light desktop and the list opens white — a bright rectangle in the middle of a
/// dark window, and no amount of CSS reaches it (`select option { background }` is simply
/// ignored, which is worth knowing before trying it).
///
/// This asks GTK for the **dark variant of whatever theme the user has**, rather than
/// forcing a specific one the way `GTK_THEME=Adwaita:dark` would: their desktop stays
/// theirs, and it follows the in-app toggle rather than needing a restart.
///
/// Linux-only by nature. Windows draws its own controls from the system setting and was
/// never affected.
#[cfg(target_os = "linux")]
pub fn apply_platform_theme(dark: bool) {
    use gtk::prelude::GtkSettingsExt;

    // `None` before GTK is up. Callers run from the rendered UI, by which point it is.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn apply_platform_theme(_dark: bool) {}

fn load_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let icon_bytes = include_bytes!("../../assets/icons/icon.png");
    // The master is square, so `resize_exact` cannot distort it.
    let image = image::load_from_memory(icon_bytes)
        .ok()?
        .resize_exact(
            WINDOW_ICON_EDGE,
            WINDOW_ICON_EDGE,
            image::imageops::FilterType::Lanczos3,
        )
        .into_rgba8();
    let (width, height) = image.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(image.into_raw(), width, height).ok()
}

pub fn boot_data() -> &'static UiLaunchData {
    BOOT_DATA
        .get()
        .expect("UI launch data must be initialized before launch")
}
