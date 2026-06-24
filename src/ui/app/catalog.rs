use dioxus::prelude::*;

use super::super::model::*;
use super::setup_sections::CatalogManagementPanel;

#[component]
pub fn CatalogScreen(state: Signal<UiState>) -> Element {
    let import_feedback = use_signal(String::new);

    rsx! {
        div { class: "screen single",
            CatalogManagementPanel { state, import_feedback }
        }
    }
}
