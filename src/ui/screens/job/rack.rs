//! Job "Rack" view — the physical rack for the **selected** machining step: each slot,
//! the tool in it, and whether the operator has to change that slot before running the
//! step. Colour-coded fixed (toolset-pinned), load (must swap in), kept (carried over
//! from an earlier step) or empty.
//!
//! A step view because a rack belongs to a machine and a step names its own machine: two
//! steps of one job may run on two CNCs, and then there are two racks, each with its own
//! schedule (see [`RackSchedule`](crate::runtime::tooling::RackSchedule)). The schedule
//! behind a rack is still computed across every step that runs on that machine, because
//! "kept" is a statement about the steps before this one; this view projects one step out
//! of the rack that step runs against.
//!
//! With a single step on the rack there is nothing to carry over, so every loaded slot
//! would read "load" and the status column would say nothing. It collapses to Slot / Tool.
//!
//! A step whose machine has no tool changer has no rack at all. It says so here rather
//! than letting the view fall through to something else — being dropped onto the board
//! view for clicking a step chip is how this was found.

use dioxus::prelude::*;

use crate::runtime::tooling::{plan_tooling, step_headers, SlotChange, StepOutcome, ToolingPlan};
use crate::runtime::AppCtx;

/// The selected step's rack. Falls back to an explanation when there is no rack to show
/// (a machine with no tool changer, no toolset, no holes, or nothing selected).
#[component]
pub fn RackView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let plan = plan_tooling(&snapshot, snapshot.stitched_board_data.as_ref());
    let step_count = plan.steps.len();
    let selected = snapshot.selected_step.min(step_count.saturating_sub(1));

    let Some(rack) = plan.rack_for_step(selected) else {
        // Say which of the reasons it is, most specific first. The plan's own note wins:
        // with no board or no profile there is no step to say anything about at all.
        let note = plan
            .note
            .clone()
            .or_else(|| unplanned_note(&plan, selected))
            .or_else(|| no_changer_note(&snapshot, selected))
            .unwrap_or_else(|| "This step loads no tools — nothing to rack.".to_string());
        return rsx! {
            div { class: "screen single centered",
                p { class: "diag-status", "{note}" }
            }
        };
    };

    // Only meaningful once a previous step *on this machine* could have left something
    // in its rack — a step on another CNC leaves this one exactly as it was.
    let show_status = rack.step_count > 1;
    let changes = rack
        .slots
        .iter()
        .filter(|row| row.status == SlotChange::Load)
        .count();
    // Named, because a job that runs on two machines has two racks, and the slot numbers
    // alone do not say which machine is being loaded.
    let title = if rack.cnc_name.trim().is_empty() {
        "Rack".to_string()
    } else {
        format!("Rack — {}", rack.cnc_name)
    };
    let unplaced = rack.unplaced.join(", ");
    let clipped = rack.clipped_slots;

    rsx! {
        div { class: "screen single rack-view",
            h3 { "{title}" }
            if show_status {
                p { class: "field-hint",
                    "Highlighted slots must be changed before this step; tools carried over from an earlier step on this machine are not."
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
                        for row in rack.slots.iter() {
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

            // What the rack cannot do, under the rack it can. Both are facts about *this*
            // machine holding *this* toolset, which is exactly what the operator standing
            // at it needs told.
            if !unplaced.is_empty() {
                div { class: "tooling-warnings",
                    p { class: "tooling-warning",
                        "\u{26A0} This step needs more tools at once than the rack holds. No slot is free for: {unplaced}."
                    }
                }
            }
            if clipped > 0 {
                div { class: "tooling-warnings",
                    p { class: "tooling-warning",
                        "\u{26A0} The toolset defines {clipped} slot(s) beyond this machine's tool changer, which cannot be loaded here."
                    }
                }
            }
        }
    }
}

/// Why the selected step has no rack when the reason is the step itself: it has nothing
/// to machine, or nothing that can machine it. A rack listing is downstream of a tooling
/// solution, so it says so and points at the tab that carries the detail.
///
/// `None` when the step planned normally — the caller then looks at its machine.
fn unplanned_note(plan: &ToolingPlan, step_index: usize) -> Option<String> {
    match &plan.steps.get(step_index)?.outcome {
        StepOutcome::Empty => {
            Some("Nothing to machine in this step, so nothing to rack.".to_string())
        }
        StepOutcome::Failed(_) => Some(
            "This step has no tooling solution, so there is no rack to load — the Tooling \
             tab lists what is missing."
                .to_string(),
        ),
        StepOutcome::Resolved(_) => None,
    }
}

/// Why the selected step has no rack when the reason is its machine: a CNC with no tool
/// changer has no carousel to load, so its tools go into the spindle one at a time.
///
/// `None` when the machine is not the reason — the caller then falls back to the generic
/// "nothing to rack" wording, which covers a step that resolved nothing or loads nothing.
fn no_changer_note(ctx: &AppCtx, step_index: usize) -> Option<String> {
    let step = step_headers(ctx).into_iter().nth(step_index)?;
    if step.is_atc {
        return None;
    }
    let machine = if step.cnc_name.trim().is_empty() {
        "This step's machine".to_string()
    } else {
        step.cnc_name
    };
    Some(format!(
        "{machine} has no tool changer, so this step has no rack — its tools are loaded \
         one at a time. The Tooling tab lists them in the order they are called."
    ))
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
