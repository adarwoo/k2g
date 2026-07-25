//! Job "Rack" view — the cross-step rack schedule as a matrix of physical slots
//! (rows) × machining steps (columns). Each cell is the tool in that slot for that
//! step, colour-coded by whether the operator must change it: fixed (toolset-pinned),
//! load (must swap in before this step), kept (carried over), or empty. Tools reused
//! across steps keep the same slot, so only genuine changes are highlighted.

use dioxus::prelude::*;

use crate::runtime::tooling::{plan_tooling, SlotChange};
use crate::runtime::AppCtx;

/// The rack-schedule matrix. Falls back to the plan note when there is no rack to show
/// (no toolset, no holes, or nothing selected).
#[component]
pub fn RackView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let plan = plan_tooling(&snapshot);

    let Some(schedule) = plan.rack_schedule else {
        return rsx! {
            div { class: "screen single centered",
                p { class: "diag-status",
                    {plan.note.unwrap_or_else(|| "No rack to plan — add a toolset and some holes.".to_string())}
                }
            }
        };
    };

    // Per-step count of slots the operator must change (Load cells in that column).
    let change_counts: Vec<usize> = (0..schedule.steps.len())
        .map(|col| {
            schedule
                .slots
                .iter()
                .filter(|row| matches!(row.cells.get(col).map(|c| c.status), Some(SlotChange::Load)))
                .count()
        })
        .collect();

    rsx! {
        div { class: "screen single rack-view",
            h3 { "Rack schedule" }
            p { class: "field-hint",
                "One column per machining step. A slot is highlighted only when its tool must change before that step; reused tools keep their slot."
            }

            div { class: "rack-legend",
                span { class: "rack-legend-item", span { class: "rack-swatch rack-fixed" } "Fixed (pinned)" }
                span { class: "rack-legend-item", span { class: "rack-swatch rack-load" } "Load / change" }
                span { class: "rack-legend-item", span { class: "rack-swatch rack-kept" } "Kept" }
                span { class: "rack-legend-item", span { class: "rack-swatch rack-empty" } "Empty" }
            }

            div { class: "table-wrap",
                table { class: "rack-matrix",
                    thead {
                        tr {
                            th { class: "rack-slot-col", "Slot" }
                            for (i , name) in schedule.steps.iter().enumerate() {
                                th {
                                    div { class: "rack-step-head",
                                        span { class: "rack-step-title", "Step {i + 1}: {name}" }
                                        span {
                                            class: if change_counts[i] > 0 { "rack-step-changes has-changes" } else { "rack-step-changes" },
                                            if change_counts[i] > 0 {
                                                "{change_counts[i]} change(s)"
                                            } else {
                                                "no changes"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    tbody {
                        for row in schedule.slots.iter() {
                            tr {
                                th { class: "rack-slot-col",
                                    span { "{row.slot}" }
                                    if row.fixed {
                                        span { class: "rack-pin", title: "Pinned by the toolset", " \u{1F4CC}" }
                                    }
                                }
                                for cell in row.cells.iter() {
                                    td { class: cell_class(cell.status),
                                        match cell.tool.as_deref() {
                                            Some(tool) => rsx! { "{tool}" },
                                            None => rsx! { span { class: "rack-dash", "\u{2014}" } },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The CSS class for a cell, keyed by its change status (the colour code).
fn cell_class(status: SlotChange) -> &'static str {
    match status {
        SlotChange::Fixed => "rack-cell rack-fixed",
        SlotChange::Load => "rack-cell rack-load",
        SlotChange::Kept => "rack-cell rack-kept",
        SlotChange::Empty => "rack-cell rack-empty",
    }
}
