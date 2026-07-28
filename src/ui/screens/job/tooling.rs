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

/// The tooling plan for the selected machining step.
#[component]
pub fn ToolingView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let plan = plan_tooling(&snapshot);
    let has_steps = !plan.steps.is_empty();
    let selected = snapshot.selected_step.min(plan.steps.len().saturating_sub(1));

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
        }
    }
}
