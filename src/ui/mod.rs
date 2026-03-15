pub mod app;
pub mod model;
pub mod theme;

use std::sync::OnceLock;

pub use model::UiLaunchData;

static BOOT_DATA: OnceLock<UiLaunchData> = OnceLock::new();

pub fn launch(data: UiLaunchData) {
    let _ = BOOT_DATA.set(data);
    dioxus::launch(app::AppRoot);
}

pub fn boot_data() -> &'static UiLaunchData {
    BOOT_DATA
        .get()
        .expect("UI launch data must be initialized before launch")
}
