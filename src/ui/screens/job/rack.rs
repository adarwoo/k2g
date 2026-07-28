//! Job "Rack" view — the physical rack for the **selected** machining step: each slot,
//! the tool in it, and whether the operator has to change that slot before running the
//! step. Colour-coded fixed (toolset-pinned), load (must swap in), kept (carried over
//! from an earlier step) or empty.
//!
//! A step view because a toolset is assigned per step — the rack is a property of the
//! setup, not of the job. The *schedule* behind it is still computed across every step,
//! because "kept" is a statement about the steps before this one; this view projects one
//! step out of it (`rack_for_step`).
//!
//! With a single step there is nothing to carry over, so every loaded slot would read
//! "load" and the status column would say nothing. It collapses to Slot / Tool.

use dioxus::prelude::*;

use crate::runtime::tooling::{plan_tooling, rack_for_step, SlotChange};
use crate::runtime::AppCtx;

/// The selected step's rack. Falls back to the plan note when there is no rack to show
/// (no toolset, no holes, or nothing selected).
#[component]
pub fn RackView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let plan = plan_tooling(&snapshot);
    let step_count = plan.steps.len();
    let selected = snapshot.selected_step.min(step_count.saturating_sub(1));

    let rows = plan
        .rack_schedule
        .as_ref()
        .and_then(|schedule| rack_for_step(schedule, selected));

    let Some(rows) = rows else {
        return rsx! {
            div { class: "screen single centered",
                p { class: "diag-status",
                    {plan.note.clone().unwrap_or_else(||
                        "This step loads no tools — nothing to rack.".to_string())}
                }
            }
        };
    };

    // Only meaningful once a previous step could have left something in the rack.
    let show_status = step_count > 1;
    let changes = rows.iter().filter(|row| row.status == SlotChange::Load).count();

    rsx! {
        div { class: "screen single rack-view",
            h3 { "Rack" }
            if show_status {
                p { class: "field-hint",
                    "Highlighted slots must be changed before this step; tools carried over from an earlier step are not."
                }
                div { class: "rack-legend",
                    span { class: "rack-legend-item", span { class: "rack-swatch rack-fixed" } "Fixed (pinned)" }
                    span { class: "rack-legend-item", span { class: "rack-swatch rack-load" } "Load / change" }
                    span { class: "rack-legend-item", span { class: "rack-swatch rack-kept" } "Kept" }
                    span { class: "rack-legend-item", span { class: "rack-swatch rack-empty" } "Empty" }
                }
                p {
                    class: if changes > 0 { "rack-step-changes has-changes" } else { "rack-step-changes" },
                    if changes > 0 { "{changes} change(s) before this step" } else { "No changes before this step" }
                }
            }

            div { class: "table-wrap",
                table { class: "rack-matrix",
                    thead {
                        tr {
                            th { class: "rack-slot-col", "Slot" }
                            th { "Tool" }
                            if show_status {
                                th { class: "rack-slot-col", "Status" }
                            }
                        }
                    }
                    tbody {
                        for row in rows.iter() {
                            tr { key: "{row.slot}",
                                th { class: "rack-slot-col",
                                    span { "{row.slot}" }
                                    if row.status == SlotChange::Fixed {
                                        span { class: "rack-pin", title: "Pinned by the toolset", " \u{1F4CC}" }
                                    }
                                }
                                td { class: cell_class(row.status),
                                    match row.tool.as_deref() {
                                        Some(tool) => rsx! { "{tool}" },
                                        None => rsx! { span { class: "rack-dash", "\u{2014}" } },
                                    }
                                }
                                if show_status {
                                    td { class: "rack-slot-col", {status_label(row.status)} }
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

/// What the operator has to do about this slot, in words — the colour alone is not an
/// instruction, and this is the column that tells them whether to touch the machine.
fn status_label(status: SlotChange) -> &'static str {
    match status {
        SlotChange::Fixed => "Fixed",
        SlotChange::Load => "Load",
        SlotChange::Kept => "Kept",
        SlotChange::Empty => "Empty",
    }
}
