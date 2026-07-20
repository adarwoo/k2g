//! Job "Machining" view — a summary of the job's machining steps and a mock of
//! the operation-flow / tool-path canvas (visualization is a future enhancement).

use dioxus::prelude::*;

use crate::app_state_impl::AppCtx;

/// The machining-summary view: counts of selected operations and tools in the rack.
#[component]
pub fn MachiningView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let step_count = snapshot.project_config.selected_operations.len();
    let tools_in_rack = snapshot
        .rack_slots
        .iter()
        .filter(|(_, slot)| slot.tool_id.is_some())
        .count();

    rsx! {
        div { class: "screen single",
            div { class: "machining-summary",
                div { class: "impact-item",
                    div { class: "impact-name", "Machining steps" }
                    div { class: "impact-state", "{step_count} selected" }
                }
                div { class: "impact-item",
                    div { class: "impact-name", "Tools in rack" }
                    div { class: "impact-state", "{tools_in_rack}" }
                }
            }
            p { class: "diag-status",
                "A job can be made of several machining steps. Each step has a start and an end."
            }
            div { class: "canvas-mock", "Machining: operation flow + tool paths" }
        }
    }
}
