pub mod app;
pub mod data_bind;
pub mod theme;
pub mod unit_service;

use std::sync::OnceLock;

pub use crate::domain::UiLaunchData;

static BOOT_DATA: OnceLock<UiLaunchData> = OnceLock::new();

pub fn launch(data: UiLaunchData) {
    let _ = BOOT_DATA.set(data);
    crate::app_state_impl::initialize_ctx(boot_data().clone());

    // Initialize the new datastore-backed store alongside the legacy context
    // (Phase 1: available to migrated screens; legacy remains the source of
    // truth until each screen is switched over).
    for problem in crate::data::init_appdata() {
        log::warn!("AppData load: {problem}");
    }

    let window = dioxus::desktop::WindowBuilder::new()
        .with_title("k2g - KiCAD to GCode")
        .with_window_icon(load_window_icon());

    let cfg = dioxus::desktop::Config::default()
        .with_menu(None)
        .with_window(window);

    dioxus::prelude::LaunchBuilder::desktop()
        .with_cfg(cfg)
        .launch(app::AppRoot);
}

fn load_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let icon_bytes = include_bytes!("../icons/icon.png");
    let image = image::load_from_memory(icon_bytes).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(image.into_raw(), width, height).ok()
}

pub fn boot_data() -> &'static UiLaunchData {
    BOOT_DATA
        .get()
        .expect("UI launch data must be initialized before launch")
}
