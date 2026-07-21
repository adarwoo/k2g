pub mod bindings;
pub mod help;
pub mod navigation;
pub mod screens;
pub mod theme;

use std::sync::OnceLock;

pub use navigation::UiLaunchData;

static BOOT_DATA: OnceLock<UiLaunchData> = OnceLock::new();

pub fn launch(data: UiLaunchData) {
    let _ = BOOT_DATA.set(data);
    // `initialize_ctx` initializes the AppData store and hydrates the legacy
    // context from it (AppData is the single reader/writer of persisted state).
    crate::runtime::initialize_ctx(boot_data().clone());

    let window = dioxus::desktop::WindowBuilder::new()
        .with_title("k2g - KiCAD to GCode")
        .with_window_icon(load_window_icon());

    let cfg = dioxus::desktop::Config::default()
        .with_menu(None)
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
