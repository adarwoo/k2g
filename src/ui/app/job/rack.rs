//! Job "Rack" (slot) view — a preview of the ATC tool rack, one card per slot.
//! Only meaningful when the selected CNC profile has an automatic tool changer.

use dioxus::prelude::*;

use crate::app_state_impl::AppCtx;

/// The rack-preview view: a card per rack slot (number, assigned tool, lock
/// state), or a note when the active machine has no ATC.
#[component]
pub fn RackView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let has_atc = snapshot.selected_machine_has_atc();

    rsx! {
        if has_atc {
            div { class: "screen single",
                h3 { "Rack preview" }
                div { class: "rack-grid",
                    for (slot_num , slot) in snapshot.rack_slots.iter() {
                        div { class: if slot.disabled { "rack-slot disabled" } else if slot.tool_id.is_some() { "rack-slot assigned" } else { "rack-slot" },
                            div { class: "rack-slot-title", "Slot #{slot_num}" }
                            p {
                                "Tool: "
                                {slot.tool_id.as_deref().unwrap_or("Empty")}
                            }
                            p {
                                "Locked: "
                                {if slot.locked { "Yes" } else { "No" }}
                            }
                        }
                    }
                }
            }
        } else {
            div { class: "screen single centered",
                p { "Rack view is only available when the selected CNC profile has ATC." }
            }
        }
    }
}
