//! Job "Machining" view — renders the in-memory [`MachiningPlan`](crate::gcode::plan)
//! the operation-planner builds: per machining step, the ordered drill-phase tool
//! blocks (one per tool, small→large) and, within each, the atomic ops in the order
//! the TSP chose. Pending work (routing, oblong slots, locating pins) surfaces as
//! per-step notes. Routing joins this view once the stitcher rework lands.

use dioxus::prelude::*;

use units::{Length, UserUnitDisplay, UserUnitSystem};

use crate::gcode::plan::{StepPlan, ToolBlock};
use crate::runtime::machining_plan::plan_machining;
use crate::runtime::AppCtx;

/// Max ops listed per tool block before collapsing the tail into a "+N more" row —
/// keeps a dense board (hundreds of holes) from flooding the DOM.
const OP_LIST_CAP: usize = 40;

/// The machining view: the operation plan, step by step.
#[component]
pub fn MachiningView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let plan = plan_machining(&snapshot);
    let unit = snapshot.unit_system;
    let total_ops = plan.total_ops();

    let steps: Vec<StepVm> = plan.steps.iter().map(|step| step_vm(&snapshot, unit, step)).collect();
    let has_steps = !steps.is_empty();

    rsx! {
        div { class: "screen single tooling-view",
            div { class: "machining-summary",
                div { class: "impact-item",
                    div { class: "impact-name", "Machining steps" }
                    div { class: "impact-state", "{steps.len()}" }
                }
                div { class: "impact-item",
                    div { class: "impact-name", "Atomic operations" }
                    div { class: "impact-state", "{total_ops}" }
                }
            }

            if let Some(note) = plan.note.as_ref() {
                p { class: "diag-status", "{note}" }
            }

            for (position , step) in steps.iter().enumerate() {
                if position > 0 {
                    hr { class: "tooling-separator" }
                }

                div { class: "tooling-step",
                    h3 { class: "tooling-step-title", "Step {step.index + 1}: {step.name}" }
                    p { class: "diag-status", "{step.summary}" }

                    if step.blocks.is_empty() {
                        if step.notes.is_empty() {
                            p { class: "diag-status", "Nothing to machine in this step." }
                        }
                    } else {
                        for block in step.blocks.iter() {
                            h4 { class: "tooling-subtitle", "{block.header}" }
                            div { class: "table-wrap",
                                table { class: "tooling-table",
                                    thead {
                                        tr {
                                            th { class: "tooling-count-col", "#" }
                                            th { "Feature" }
                                            th { class: "tooling-slot-col", "X" }
                                            th { class: "tooling-slot-col", "Y" }
                                            th { class: "tooling-slot-col", "Z" }
                                        }
                                    }
                                    tbody {
                                        for op in block.ops.iter() {
                                            tr {
                                                td { class: "tooling-count", "{op.order}" }
                                                td { "{op.source}" }
                                                td { class: "tooling-slot", "{op.x}" }
                                                td { class: "tooling-slot", "{op.y}" }
                                                td { class: "tooling-slot", "{op.z}" }
                                            }
                                        }
                                        if block.more > 0 {
                                            tr {
                                                td { class: "diag-status", colspan: "5", "+{block.more} more op(s)…" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if !step.notes.is_empty() {
                        div { class: "tooling-warnings",
                            for note in step.notes.iter() {
                                p { class: "tooling-warning", "⚠ {note}" }
                            }
                        }
                    }
                }
            }

            if !has_steps && plan.note.is_none() {
                p { class: "diag-status", "No machining steps to plan." }
            }
        }
    }
}

/// A step, flattened for rendering.
struct StepVm {
    index: usize,
    name: String,
    summary: String,
    blocks: Vec<BlockVm>,
    notes: Vec<String>,
}

/// One tool block, flattened for rendering.
struct BlockVm {
    header: String,
    ops: Vec<OpVm>,
    /// Ops beyond [`OP_LIST_CAP`], collapsed.
    more: usize,
}

/// One atomic op row.
struct OpVm {
    order: usize,
    source: String,
    x: String,
    y: String,
    z: String,
}

/// Builds a step's view model: summary line, per-block op tables, and notes.
fn step_vm(ctx: &AppCtx, unit: UserUnitSystem, step: &StepPlan) -> StepVm {
    let travel: f64 = step.blocks.iter().map(|b| b.travel_mm).sum();
    let summary = format!(
        "{} op(s) · {} tool block(s) · {:.1} mm travel",
        step.op_count(),
        step.blocks.len(),
        travel,
    );
    let blocks = step.blocks.iter().map(|block| block_vm(ctx, unit, block)).collect();
    StepVm { index: step.index, name: step.name.clone(), summary, blocks, notes: step.notes.clone() }
}

/// Builds a block's view model: a header line and the capped, ordered op list.
fn block_vm(ctx: &AppCtx, unit: UserUnitSystem, block: &ToolBlock) -> BlockVm {
    let slot = block.slot.map(|n| format!("T{n}")).unwrap_or_else(|| "—".into());
    let tool_name = ctx
        .tools
        .iter()
        .find(|t| t.id == block.tool_id)
        .map(|t| t.display_name())
        .unwrap_or_else(|| block.tool_id.clone());
    let header = format!(
        "{slot} · {tool_name} ⌀{} · {} op(s) · {:.1} mm travel",
        fmt_len(unit, block.diameter),
        block.op_count(),
        block.travel_mm,
    );

    let ops: Vec<OpVm> = block
        .ops
        .iter()
        .take(OP_LIST_CAP)
        .enumerate()
        .map(|(i, op)| OpVm {
            order: i + 1,
            source: op.source.clone(),
            x: fmt_len(unit, op.entry.x),
            y: fmt_len(unit, op.entry.y),
            z: fmt_len(unit, op.z.z_bottom),
        })
        .collect();
    let more = block.ops.len().saturating_sub(ops.len());

    BlockVm { header, ops, more }
}

/// Formats a length in the user's preferred unit.
fn fmt_len(unit: UserUnitSystem, length: Length) -> String {
    length.unit_display(unit).user
}
