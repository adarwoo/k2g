//! Job "Code" view — the generated G-code program: an editable text buffer plus
//! program statistics (save target, line/character counts, board thickness).

use dioxus::prelude::*;
use units::Length;

use crate::app_state_impl::AppCtx;
use crate::ui::unit_service;

/// The G-code program view: shows the generated program in an editable textarea
/// and a stat strip. Editing marks the program modified.
#[component]
pub fn CodeView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let board_thickness_pcb_label = snapshot
        .board
        .as_ref()
        .and_then(|board| board.thickness.as_ref())
        .map(|thickness| {
            unit_service::format_length_display(
                Length::from_mm(thickness.as_mm()),
                snapshot.unit_system,
            )
        });

    rsx! {
        div { class: "screen single",
            textarea {
                class: "gcode-editor",
                value: snapshot.gcode.clone(),
                oninput: move |evt| {
                    let value = evt.value();
                    state
                        .with_mut(|s| {
                            s.gcode = value;
                            s.gcode_modified = true;
                        });
                },
            }
            div { class: "program-stats",
                span { "Save target: {snapshot.save_filename}" }
                span { "Lines: {snapshot.gcode.lines().count()}" }
                span { "Characters: {snapshot.gcode.len()}" }
                span {
                    if let Some(v) = board_thickness_pcb_label.as_ref() {
                        "Board thickness (PCB): {v}"
                    } else {
                        "Board thickness (PCB): unavailable"
                    }
                }
            }
        }
    }
}
