pub mod app;
pub mod model;
pub mod theme;

use std::sync::OnceLock;

pub use model::UiLaunchData;
use crate::config::PersistenceState;

static BOOT_DATA: OnceLock<UiLaunchData> = OnceLock::new();
static PERSISTENCE_STATE: OnceLock<PersistenceState> = OnceLock::new();

pub fn launch(data: UiLaunchData) {
    let _ = BOOT_DATA.set(data);
    use log::warn;
    use crate::user_path::ensure_app_dirs;
    use crate::config::load_all_configs;

    // Try to load persisted configurations
    if let Ok(app_dirs) = ensure_app_dirs() {
        if let Ok(persistence_state) = load_all_configs(&app_dirs, &app_dirs.schemas) {
            // Store persistence state for the app to use
            let _ = PERSISTENCE_STATE.set(persistence_state);
        } else {
            warn!("Could not load persisted configuration; will use defaults");
        }
    } else {
        warn!("Could not locate app directories; will use defaults");
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
    let icon_bytes = include_bytes!("../../resources/icons/icon.png");
    let image = image::load_from_memory(icon_bytes).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(image.into_raw(), width, height).ok()
}

pub fn boot_data() -> &'static UiLaunchData {
    BOOT_DATA
        .get()
        .expect("UI launch data must be initialized before launch")
}

#[allow(dead_code)]
pub fn persistence_state() -> Option<&'static PersistenceState> {
    PERSISTENCE_STATE.get()
}
