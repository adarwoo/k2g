//! Job "Tooling" view — the per-step tooling plan (Specification.md §3 "Tooling
//! plan"). Runs the tool-selection assigner for each machining step and shows, per
//! step, the resolved rack (T1..Tn) and every machining requirement with its count
//! and resolved tool. A step with no solution renders its diagnostics as an error.

use dioxus::prelude::*;

use crate::runtime::tooling::{plan_tooling, StepOutcome};
use crate::runtime::AppCtx;

/// The tooling-plan view: one section per machining step, separated by a rule.
#[component]
pub fn ToolingView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let plan = plan_tooling(&snapshot);
    let has_steps = !plan.steps.is_empty();

    rsx! {
        div { class: "screen single tooling-view",
            if let Some(note) = plan.note.as_ref() {
                p { class: "diag-status", "{note}" }
            }

            for (position , step) in plan.steps.iter().enumerate() {
                if position > 0 {
                    hr { class: "tooling-separator" }
                }

                div { class: "tooling-step",
                    h3 { class: "tooling-step-title", "Step {step.index + 1}: {step.name}" }

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
                            if resolved.rack.is_empty() {
                                p { class: "diag-status", "No tools assigned." }
                            } else {
                                div { class: "table-wrap",
                                    table { class: "tooling-table",
                                        thead {
                                            tr {
                                                th { class: "tooling-slot-col", "Slot" }
                                                th { "Tool" }
                                            }
                                        }
                                        tbody {
                                            for row in resolved.rack.iter() {
                                                tr {
                                                    td { class: "tooling-slot", "{row.slot}" }
                                                    td { "{row.tool}" }
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
                                        }
                                    }
                                    tbody {
                                        for row in resolved.requirements.iter() {
                                            tr {
                                                td { "{row.label}" }
                                                td { class: "tooling-count", "{row.count}" }
                                                td { class: "tooling-slot", "{row.resolved}" }
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
        }
    }
}
