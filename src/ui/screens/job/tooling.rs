//! Job "Tooling" view — the tooling plan for the **selected** machining step
//! (Specification.md §3 "Tooling plan"): the resolved rack (T1..Tn) and every machining
//! requirement with its count and resolved tool. A step with no solution renders its
//! diagnostics as an error.
//!
//! One step at a time because a step is an autonomous setup with its own toolset; which
//! step is showing is the step chips' business (see [`super::JobViewPanel`]), so nothing
//! here names it.

use dioxus::prelude::*;

use crate::runtime::tooling::{plan_tooling, StepOutcome};
use crate::runtime::AppCtx;
use crate::ui::bindings::{revert_stock_tool, StockField, StockForm};

/// The tooling plan for the selected machining step.
#[component]
pub fn ToolingView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let plan = plan_tooling(&snapshot, snapshot.stitched_board_data.as_ref());
    let has_steps = !plan.steps.is_empty();
    let selected = snapshot.selected_step.min(plan.steps.len().saturating_sub(1));

    // The rack tool being edited, by stock id. Held here rather than pushed into `AppCtx`
    // for the reason the 3D view's hidden set gives: this is a dialog being open, not job
    // state, and putting it in the context would clone the world on every keystroke.
    let mut editing_tool_id = use_signal(|| None::<String>);
    // Resolved the way the Stock screen resolves its own detail panel: the schema-driven
    // form is addressed by position in `/tools`, and the id is what survives the list being
    // re-read. `None` when the tool has left stock since the plan was made.
    let editing = editing_tool_id.read().clone().map(|tool_id| {
        let index = snapshot.tools.iter().position(|tool| tool.id == tool_id);
        (tool_id, index)
    });

    rsx! {
        div { class: "screen single tooling-view",
            if let Some(note) = plan.note.as_ref() {
                p { class: "diag-status", "{note}" }
            }

            // One step: which one is the step chips' business, and with a single step
            // there is nothing to choose, so no heading names it here either.
            if let Some(step) = plan.steps.get(selected) {
                div { class: "tooling-step",

                    match &step.outcome {
                        StepOutcome::Empty => rsx! {
                            p { class: "diag-status", "Nothing to machine in this step." }
                        },
                        StepOutcome::Failed(messages) => rsx! {
                            div { class: "tooling-error",
                                div { class: "tooling-error-title", "No tooling solution" }
                                ul {
                                    for message in messages.iter() {
                                        li { "{message}" }
                                    }
                                }
                            }
                        },
                        StepOutcome::Resolved(resolved) => rsx! {
                            h4 { class: "tooling-subtitle", "Tool selection" }
                            p { class: "diag-status", "{resolved.summary}" }
                            if resolved.rack.is_empty() {
                                p { class: "diag-status", "No tools assigned." }
                            } else {
                                div { class: "table-wrap",
                                    table { class: "tooling-table",
                                        thead {
                                            tr {
                                                th { class: "tooling-slot-col", "Slot" }
                                                th { "Tool" }
                                                th { class: "tooling-edit-col" }
                                            }
                                        }
                                        tbody {
                                            for row in resolved.rack.iter() {
                                                tr {
                                                    td { class: "tooling-slot", "{row.slot}" }
                                                    td { "{row.tool}" }
                                                    // The rack is where a wrong bit is
                                                    // noticed — the slot is named, the
                                                    // diameter is beside it, and until now
                                                    // the only way to change either was to
                                                    // leave for the Stock screen and find
                                                    // the tool again.
                                                    td { class: "tooling-edit-col",
                                                        button {
                                                            class: "text-button",
                                                            title: "Edit this tool's properties in stock",
                                                            onclick: {
                                                                let tool_id = row.tool_id.clone();
                                                                move |_| editing_tool_id.set(Some(tool_id.clone()))
                                                            },
                                                            "Edit"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            h4 { class: "tooling-subtitle", "Machining requirements" }
                            div { class: "table-wrap",
                                table { class: "tooling-table",
                                    thead {
                                        tr {
                                            th { "Requirement" }
                                            th { class: "tooling-count-col", "Count" }
                                            th { class: "tooling-slot-col", "Tool" }
                                            th { class: "tooling-slot-col", "Ø" }
                                            th { class: "tooling-slot-col", "Δ" }
                                        }
                                    }
                                    tbody {
                                        for row in resolved.requirements.iter() {
                                            tr {
                                                class: if row.tools.iter().any(|tool| tool.routed) { "tooling-req-routed" } else { "" },
                                                td { "{row.label}" }
                                                td { class: "tooling-count", "{row.count}" }
                                                td { class: "tooling-slot",
                                                    for tool in row.tools.iter() {
                                                        div { class: "tooling-tool-line",
                                                            span { "{tool.slot}" }
                                                            if let Some(role) = tool.role {
                                                                span { class: "tooling-role", " {role}" }
                                                            }
                                                            if tool.routed {
                                                                span { class: "tooling-routed-badge", "routed" }
                                                            }
                                                        }
                                                    }
                                                }
                                                td { class: "tooling-slot",
                                                    for tool in row.tools.iter() {
                                                        div { class: "tooling-tool-line", "{tool.diameter}" }
                                                    }
                                                }
                                                td {
                                                    for tool in row.tools.iter() {
                                                        div { class: "tooling-tool-line {tool.delta_class}", "{tool.delta_text}" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            if !resolved.warnings.is_empty() {
                                div { class: "tooling-warnings",
                                    for warning in resolved.warnings.iter() {
                                        p { class: "tooling-warning", "⚠ {warning}" }
                                    }
                                }
                            }
                        },
                    }
                }
            }

            if !has_steps && plan.note.is_none() {
                p { class: "diag-status", "No machining steps to plan." }
            }

            // The tool editor, over the stock singleton.
            //
            // Mounted only while open, and last in the view so it overlays it: the same two
            // rules the Settings dialog follows, for the same reasons — a dialog kept warm
            // behind a hidden element still runs its subscriptions, and one nested earlier
            // in the tree is clipped by whatever scrolls above it.
            if let Some((tool_id, index)) = editing {
                div { class: "wizard-overlay",
                    div { class: "wizard-dialog tool-edit-dialog",
                        h2 { "Edit tool" }

                        match index {
                            // Editing here writes to **stock**, and stock is one list. The
                            // same bit is the same bit in every profile that loads it, and
                            // the job re-plans the moment it changes — which is the point
                            // (a wrong diameter is wrong everywhere) and is also invisible
                            // from a dialog opened out of one rack row. So it says so.
                            Some(index) => rsx! {
                                p { class: "diag-status",
                                    "This edits the tool in stock, so it changes everywhere the "
                                    "tool is used and re-plans the job."
                                }

                                div { class: "stock-detail-form",
                                    StockForm { ptr: format!("/tools/{index}/overrides") }
                                    StockField { ptr: format!("/tools/{index}/availability") }
                                    StockField { ptr: format!("/tools/{index}/preference") }
                                }

                                div { class: "wizard-actions",
                                    button {
                                        class: "btn btn-secondary",
                                        title: "Reset every edited field back to its original catalog value",
                                        onclick: move |_| revert_stock_tool(index),
                                        "Revert to catalog"
                                    }
                                    button {
                                        class: "btn btn-primary",
                                        onclick: move |_| editing_tool_id.set(None),
                                        "Done"
                                    }
                                }
                            },
                            // A plan can outlive the stock it was made from. Saying which
                            // tool is missing beats an empty form that looks broken.
                            None => rsx! {
                                p { class: "diag-status",
                                    "This tool is no longer in stock ({tool_id}), so there is "
                                    "nothing to edit. The plan still names it until the job is "
                                    "re-planned against what is there now."
                                }
                                div { class: "wizard-actions",
                                    button {
                                        class: "btn btn-primary",
                                        onclick: move |_| editing_tool_id.set(None),
                                        "Close"
                                    }
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}
