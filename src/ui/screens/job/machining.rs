//! Job "Machining" view — renders the in-memory [`MachiningPlan`](crate::gcode::plan)
//! the operation-planner builds: per machining step, the ordered drill-phase tool
//! blocks (one per tool, small→large) and, within each, the atomic ops in the order
//! the TSP chose. Pending work (routing, oblong slots, locating pins) surfaces as
//! per-step notes. Routing joins this view once the stitcher rework lands.

use dioxus::prelude::*;

use units::{Length, UserUnitDisplay, UserUnitSystem};

use crate::gcode::plan::{StepPlan, ToolBlock};
use crate::runtime::machining_plan::cached_plan;
use crate::runtime::{AppCtx, MIN_MACHINING_SPLIT};
use crate::ui::screens::job::machining_3d::Machining3dView;

/// Max ops listed per tool block before collapsing the tail into a "+N more" row.
///
/// Forty when the list had no scrollbar of its own and lived at the bottom of a page that
/// scrolled as one — at which point a longer table was not so much dense as unreachable.
/// The list has its own pane now, so the number is sized against the DOM instead: five
/// hundred rows scroll without complaint, and several thousand make the whole view sluggish
/// whether or not anyone scrolls to them. The `+N more` row stays as the valve for a board
/// that goes past it.
const OP_LIST_CAP: usize = 500;

/// Which atomic op the operator has picked out, within the step on screen.
///
/// Positional, because an op has no identity of its own — it is an entry in a block's list,
/// and the block is an entry in the step's. That is fine for a *selection*, which is a
/// statement about what is on screen right now, and it is why this is checked against the
/// plan on every render rather than trusted: a step change or a machining-profile edit can
/// reorder the blocks under it, and an index that still resolves would then point at
/// whichever op inherited the position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpRef {
    pub block: usize,
    pub op: usize,
}

/// `selected`, but only if it still names an op of `step`.
///
/// The one place a stale reference is turned back into "nothing selected". Returning the
/// ref rather than a bool so the caller cannot accidentally use the unchecked one.
fn resolve_selection(step: Option<&StepPlan>, selected: Option<OpRef>) -> Option<OpRef> {
    let at = selected?;
    let block = step?.blocks.get(at.block)?;
    (at.op < block.ops.len()).then_some(at)
}

/// Every op the list actually shows, in the order the step runs them.
///
/// Bounded by [`OP_LIST_CAP`], and that bound is the point: it is what keeps the arrow keys
/// and the table agreeing. Walking the plan instead would step the selection onto ops past
/// the `+N more` row — off the end of what is on screen, with a highlight moving on the
/// canvas and no row to show for it.
fn listed_ops(step: &StepPlan) -> impl Iterator<Item = OpRef> + '_ {
    step.blocks.iter().enumerate().flat_map(|(block, tools)| {
        (0..tools.ops.len().min(OP_LIST_CAP)).map(move |op| OpRef { block, op })
    })
}

/// The op one step `forward` (or back) from `at`, **crossing tool blocks**.
///
/// Crossing them is what makes the arrow keys worth having: the sequence an operator wants
/// to watch is the step's, and a tool change is a moment in it rather than a wall. So the
/// last op of one block is followed by the first of the next.
///
/// `None` from `at` means "nowhere yet", and the first key press picks the end the operator
/// is heading away from — down selects the first op, up selects the last. `None` *returned*
/// means the end of the list, where the caller leaves the selection alone rather than
/// clearing it: arriving at the last op and pressing down again should not deselect it.
fn step_selection(step: &StepPlan, at: Option<OpRef>, forward: bool) -> Option<OpRef> {
    let ops: Vec<OpRef> = listed_ops(step).collect();
    let Some(at) = at else {
        return if forward { ops.first().copied() } else { ops.last().copied() };
    };
    let index = ops.iter().position(|op| *op == at)?;
    if forward {
        ops.get(index + 1).copied()
    } else {
        index.checked_sub(1).and_then(|prev| ops.get(prev).copied())
    }
}

/// The DOM id of an op's row, so the keyboard can bring it back into view.
///
/// The list scrolls, and stepping through a step of any size walks straight off the bottom
/// of it within a dozen presses — at which point the highlight is moving on the canvas and
/// the table is showing a stretch of the plan nobody is looking at.
fn op_row_id(at: OpRef) -> String {
    format!("k2g-op-{}-{}", at.block, at.op)
}

/// The machining view: the operation plan for the selected step.
#[component]
pub fn MachiningView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let plan = cached_plan(&snapshot);
    let unit = snapshot.unit_system;
    let total_ops = plan.total_ops();

    let steps: Vec<StepVm> = plan.steps.iter().map(|step| step_vm(&snapshot, unit, step)).collect();
    let has_steps = !steps.is_empty();
    let selected = snapshot.selected_step.min(steps.len().saturating_sub(1));
    // Counting steps is only worth saying when there is more than one — a job with a
    // single step must not mention that steps exist at all.
    let multi_step = steps.len() > 1;

    // The op the operator has clicked, shared with the 3D view beside it — which is the
    // whole point, and the reason this is lifted here rather than held in either half.
    //
    // Component state, so it clears when the view is remounted (a tab switch, the dock
    // opening). That is right for a selection: it is a question being asked of the job on
    // screen now, not a preference to carry forward — the opposite of the legend's hidden
    // tools, which are held in a `static` precisely so they survive the same remounting.
    let mut picked = use_signal(|| None::<OpRef>);
    let picked_op = resolve_selection(plan.steps.get(selected), *picked.read());

    // The divider, tracked live during a drag and written to settings once on release.
    // Both halves of that are copied from the Job dock's handle, and for its reasons: see
    // the notes in `ui::screens`.
    let stored_split = snapshot.machining_split_height;
    let mut dragging = use_signal(|| false);
    // The column's top edge in client space, measured once when a drag starts.
    //
    // The divider tracks the pointer **absolutely** — the pane is however far the pointer
    // is below this — rather than accumulating deltas the way the Job dock's handle does.
    // That is not a refinement, it is what makes the drag possible at all: an incremental
    // handle needs a height to add its delta to, and until the operator has dragged there
    // is no height, only "half the column". Absolute needs no starting value, and it cannot
    // drift away from the pointer over a long drag either.
    let mut column_top = use_signal(|| 0.0_f64);
    // The column element, kept so the drag can ask it where it is. Measured per drag rather
    // than once at mount: the shell above it changes height when the diagnostics banner
    // appears, and a top measured before that would put the divider out by its height.
    let mut column = use_signal(|| None::<Event<MountedData>>);
    // The op list pane, kept so a click on a row can put the keyboard focus on it. Clicking
    // a `tabindex` element does focus it, but only once the pointer has landed inside — and
    // a row is a child of a child, so this makes it explicit rather than relying on where
    // the browser decides the click landed.
    let mut oplist = use_signal(|| None::<Event<MountedData>>);
    // `None` until the divider is touched, here as in the setting — and the two mean the
    // same thing, so a live value is only ever a number once there is one to be.
    let mut live_split = use_signal(|| stored_split);
    // The stored value wins whenever no drag is in flight, so a settings change — or the
    // remount this view goes through constantly — does not leave a stale local copy on
    // screen.
    if !*dragging.read() && *live_split.read() != stored_split {
        live_split.set(stored_split);
    }
    // The property is **omitted** rather than given a number when nothing has been chosen,
    // so the stylesheet's own `50%` fallback is what applies. Passing a pixel default here
    // would be the same picture on this window and the wrong one on the next: half follows
    // a resize and a height does not.
    let split_style = match *live_split.read() {
        Some(height) => format!("--machining-split: {height}px;"),
        None => String::new(),
    };

    rsx! {
        div {
            class: if *dragging.read() {
                "screen single tooling-view machining-split is-dragging"
            } else {
                "screen single tooling-view machining-split"
            },
            style: "{split_style}",
            onmounted: move |evt| column.set(Some(evt)),
            // Client space, because that is the frame the measured top is in. The pointer
            // leaves the thin handle almost at once, so anything element-relative would
            // jump the moment the target under it changed.
            onmousemove: move |evt| {
                if !*dragging.read() {
                    return;
                }
                // Half the handle, so the divider sits centred under the pointer rather
                // than hanging below it.
                let height = evt.client_coordinates().y - *column_top.read() - 4.0;
                live_split.set(Some((height.max(MIN_MACHINING_SPLIT as f64)) as i64));
            },
            onmouseup: move |_| {
                if !*dragging.read() {
                    return;
                }
                dragging.set(false);
                if let Some(height) = *live_split.read() {
                    super::super::mutate_ctx(
                        state,
                        |ctx| ctx.app.set_machining_split_height(height),
                    );
                }
            },
            // A drag that left the window is a drag that has ended: without this the
            // divider follows the pointer back in as if the button were still down.
            onmouseleave: move |_| dragging.set(false),

            // The toolpath render. Above the list rather than replacing it — the list is
            // still the only way to read exact coordinates, and it is now also how one of
            // those coordinates is pointed at on the canvas.
            //
            // Handed the signal rather than `picked_op`: the view memoises its scene, and a
            // memo only re-runs for what it reads. See the note on the prop.
            Machining3dView { state, picked }

            div {
                class: if *dragging.read() { "split-handle is-dragging" } else { "split-handle" },
                title: "Drag to resize the 3D view",
                onmousedown: move |_| {
                    // Measured here and not at mount, and awaited before the drag opens:
                    // until the top is known every pointer position would be read against
                    // zero, which on the first frame throws the divider to the top of the
                    // screen.
                    let column = column.read().clone();
                    spawn(async move {
                        if let Some(column) = column {
                            if let Ok(rect) = column.get_client_rect().await {
                                column_top.set(rect.origin.y);
                            }
                        }
                        dragging.set(true);
                    });
                },
            }

            div {
                class: "machining-oplist",
                // Focusable, because a key press has to be *for* something. Zero rather
                // than a positive index: this joins the tab order where it sits, which is
                // after the 3D view's own controls, and does not jump the queue.
                tabindex: "0",
                onmounted: move |evt| oplist.set(Some(evt)),
                // Walking the step with the arrow keys, which is how the sequence is
                // actually read: press down repeatedly and the highlight moves through the
                // plan on the canvas in the order the machine will run it.
                onkeydown: move |evt| {
                    let key = evt.key().to_string().to_ascii_lowercase();
                    let forward = match key.as_str() {
                        "arrowdown" => true,
                        "arrowup" => false,
                        _ => return,
                    };
                    // Ours now: without this the pane also scrolls natively, which fights
                    // the row this is about to scroll into view.
                    evt.prevent_default();
                    let Some(step) = plan.steps.get(selected) else {
                        return;
                    };
                    // From the *resolved* selection, so a stale one starts the walk from the
                    // top rather than from an op that is no longer there.
                    let Some(next) = step_selection(step, picked_op, forward) else {
                        return; // at the end of the list; leave the selection where it is
                    };
                    picked.set(Some(next));
                    let id = op_row_id(next);
                    spawn(async move {
                        // `nearest`, so a row already on screen does not shunt the list
                        // about — only one that has gone off an edge is brought back, and
                        // only as far as it has to be.
                        let _ = document::eval(&format!(
                            "var row = document.getElementById('{id}'); \
                             if (row) row.scrollIntoView({{ block: 'nearest' }});"
                        ))
                        .await;
                    });
                },

            div { class: "machining-summary",
                if multi_step {
                    div { class: "impact-item",
                        div { class: "impact-name", "Machining steps" }
                        div { class: "impact-state", "{steps.len()}" }
                    }
                }
                div { class: "impact-item",
                    div { class: "impact-name", "Atomic operations" }
                    div { class: "impact-state", "{total_ops}" }
                }
            }

            if let Some(note) = plan.note.as_ref() {
                p { class: "diag-status", "{note}" }
            }

            if let Some(step) = steps.get(selected) {
                div { class: "tooling-step",
                    p { class: "diag-status", "{step.summary}" }

                    if step.blocks.is_empty() {
                        if step.notes.is_empty() {
                            p { class: "diag-status", "Nothing to machine in this step." }
                        }
                    } else {
                        for (block_index , block) in step.blocks.iter().enumerate() {
                            h4 { class: "tooling-subtitle", "{block.header}" }
                            div { class: "table-wrap",
                                table { class: "tooling-table machining-op-table",
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
                                        for (op_index , op) in block.ops.iter().enumerate() {
                                            tr {
                                                key: "{block_index}:{op_index}",
                                                // What the arrow keys scroll back into view.
                                                id: op_row_id(OpRef { block: block_index, op: op_index }),
                                                class: if picked_op == Some(OpRef { block: block_index, op: op_index }) {
                                                    "machining-op-row is-selected"
                                                } else {
                                                    "machining-op-row"
                                                },
                                                title: "Show this operation in the 3D view — then arrow up and down to walk the step",
                                                // Clicking the selected row clears it, so the
                                                // highlight can be put away without hunting for
                                                // somewhere neutral to click.
                                                onclick: move |_| {
                                                    let at = OpRef { block: block_index, op: op_index };
                                                    let next = (*picked.read() != Some(at)).then_some(at);
                                                    picked.set(next);
                                                    // Clicking a row is how the keyboard walk
                                                    // is started, so the click has to leave the
                                                    // focus somewhere the arrows are heard.
                                                    let pane = oplist.read().clone();
                                                    spawn(async move {
                                                        if let Some(pane) = pane {
                                                            let _ = pane.set_focus(true).await;
                                                        }
                                                    });
                                                },
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
}

/// A step, flattened for rendering. It carries no identity: which step this is, and
/// what it is called, are the step chips' business.
struct StepVm {
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
    StepVm { summary, blocks, notes: step.notes.clone() }
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

#[cfg(test)]
mod selection_tests {
    use super::*;
    use crate::gcode::plan::{AtomicOp, OpKind, Phase, Point, ZProfile};

    fn point(x: f64) -> Point {
        Point::new(Length::from_mm(x), Length::from_mm(0.0))
    }

    fn atomic() -> AtomicOp {
        AtomicOp {
            phase: Phase::Drill,
            kind: OpKind::Drill,
            tool_id: "t1".into(),
            entry: point(0.0),
            exit: point(0.0),
            z: ZProfile {
                z_bottom: Length::from_mm(-1.8),
                z_retract: Length::from_mm(2.0),
                z_feed: None,
            },
            primitive: "drill",
            source: "pth#0".into(),
        }
    }

    /// A step of `blocks` tool blocks, holding `ops` ops each.
    fn step(blocks: usize, ops: usize) -> StepPlan {
        StepPlan {
            index: 0,
            name: "s".into(),
            blocks: (0..blocks)
                .map(|n| ToolBlock {
                    tool_id: format!("t{n}"),
                    slot: Some(n as u8 + 1),
                    diameter: Length::from_mm(0.8),
                    ops: (0..ops).map(|_| atomic()).collect(),
                    travel_mm: 0.0,
                })
                .collect(),
            notes: Vec::new(),
        }
    }


    /// A block holding a given number of ops, so a step can mix full and empty ones.
    fn step_of(sizes: &[usize]) -> StepPlan {
        StepPlan {
            index: 0,
            name: "s".into(),
            blocks: sizes
                .iter()
                .enumerate()
                .map(|(n, count)| ToolBlock {
                    tool_id: format!("t{n}"),
                    slot: Some(n as u8 + 1),
                    diameter: Length::from_mm(0.8),
                    ops: (0..*count).map(|_| atomic()).collect(),
                    travel_mm: 0.0,
                })
                .collect(),
            notes: Vec::new(),
        }
    }

    fn at(block: usize, op: usize) -> Option<OpRef> {
        Some(OpRef { block, op })
    }

    /// Down walks forward, up walks back, one op at a time.
    #[test]
    fn the_arrows_walk_the_step_one_op_at_a_time() {
        let plan = step_of(&[3]);

        assert_eq!(step_selection(&plan, at(0, 0), true), at(0, 1));
        assert_eq!(step_selection(&plan, at(0, 1), true), at(0, 2));
        assert_eq!(step_selection(&plan, at(0, 2), false), at(0, 1));
    }

    /// **Why this crosses blocks.** The sequence the operator wants to watch is the
    /// *step's*, and a tool change is a moment in it rather than a wall — so the last op of
    /// one block is followed by the first of the next, and stepping back returns.
    #[test]
    fn the_walk_crosses_tool_blocks() {
        let plan = step_of(&[2, 2]);

        assert_eq!(step_selection(&plan, at(0, 1), true), at(1, 0), "into the next tool");
        assert_eq!(step_selection(&plan, at(1, 0), false), at(0, 1), "and back out of it");
    }

    /// A block with no ops is stepped straight over rather than landing on it — there is no
    /// row there to select, and nothing on the canvas to highlight.
    #[test]
    fn an_empty_block_is_stepped_over() {
        let plan = step_of(&[1, 0, 1]);

        assert_eq!(step_selection(&plan, at(0, 0), true), at(2, 0));
        assert_eq!(step_selection(&plan, at(2, 0), false), at(0, 0));
    }

    /// Nothing selected yet: the first press picks the end the operator is heading away
    /// from, so down starts at the top and up starts at the bottom.
    #[test]
    fn the_first_press_picks_an_end() {
        let plan = step_of(&[2, 2]);

        assert_eq!(step_selection(&plan, None, true), at(0, 0));
        assert_eq!(step_selection(&plan, None, false), at(1, 1));
    }

    /// At either end the walk yields nothing, and the caller leaves the selection alone.
    /// Pressing down on the last op must not deselect it — the operator is holding the key
    /// to watch the sequence, and losing the highlight at the end is a worse answer than
    /// stopping on it.
    #[test]
    fn the_ends_of_the_step_hold() {
        let plan = step_of(&[2]);

        assert_eq!(step_selection(&plan, at(0, 1), true), None, "past the last");
        assert_eq!(step_selection(&plan, at(0, 0), false), None, "before the first");
    }

    /// **The walk stops where the table does.** The list caps each block at
    /// `OP_LIST_CAP` and says so with a `+N more` row, so stepping past that would move the
    /// highlight on the canvas to an op with no row on screen — the table showing one
    /// stretch of the plan while the selection is somewhere else entirely.
    #[test]
    fn the_walk_stops_where_the_list_does() {
        let plan = step_of(&[OP_LIST_CAP + 50]);

        assert_eq!(
            step_selection(&plan, at(0, OP_LIST_CAP - 1), true),
            None,
            "the last listed op is the last the arrows reach",
        );
        assert_eq!(
            step_selection(&plan, None, false),
            at(0, OP_LIST_CAP - 1),
            "and stepping up from nowhere lands on it, not on the last op of the plan",
        );
    }

    /// A step with nothing in it has nowhere to go, from either end.
    #[test]
    fn an_empty_step_has_nothing_to_walk() {
        assert_eq!(step_selection(&step_of(&[]), None, true), None);
        assert_eq!(step_selection(&step_of(&[0, 0]), None, false), None);
    }

    /// A selection the plan no longer holds cannot be walked from — there is no position to
    /// step away from. The view resolves the selection before asking, so this is the
    /// belt-and-braces half.
    #[test]
    fn a_stale_selection_walks_nowhere() {
        assert_eq!(step_selection(&step_of(&[2]), at(9, 0), true), None);
    }

    /// The row ids the arrows scroll to have to be the ids the rows carry, and there is
    /// nothing but this to keep them so — a drift makes the scroll silently stop working
    /// while everything else still does.
    #[test]
    fn a_row_id_names_its_op() {
        assert_eq!(op_row_id(OpRef { block: 0, op: 0 }), "k2g-op-0-0");
        assert_eq!(op_row_id(OpRef { block: 3, op: 41 }), "k2g-op-3-41");

        let source = include_str!("machining.rs");
        assert!(
            source.contains("id: op_row_id(OpRef { block: block_index, op: op_index }),"),
            "the table must give each row the id this builds, or the scroll finds nothing",
        );
    }

    /// The ordinary case: a selection that names an op the step actually has survives.
    #[test]
    fn a_selection_that_still_resolves_is_kept() {
        let plan = step(2, 5);
        let at = OpRef { block: 1, op: 4 };

        assert_eq!(resolve_selection(Some(&plan), Some(at)), Some(at));
    }

    /// **Why this is checked every render rather than trusted.** An op has no identity of
    /// its own — it is an index into a block, and the block is an index into the step. A
    /// machining-profile edit reorders or removes blocks, and a step change replaces them
    /// wholesale, so an index that happens to still resolve would be pointing at whichever
    /// op inherited the position. Out of range is the case that can be caught; this is the
    /// one that cannot, which is why the selection is cleared rather than remapped.
    #[test]
    fn a_selection_past_the_end_resolves_to_nothing() {
        let plan = step(2, 5);

        assert_eq!(
            resolve_selection(Some(&plan), Some(OpRef { block: 1, op: 5 })),
            None,
            "one past the last op",
        );
        assert_eq!(
            resolve_selection(Some(&plan), Some(OpRef { block: 2, op: 0 })),
            None,
            "a block the step no longer has",
        );
    }

    /// A step that lost its blocks entirely — a profile edited down to nothing — must
    /// clear the selection rather than index into an empty list.
    #[test]
    fn an_empty_step_clears_the_selection() {
        assert_eq!(resolve_selection(Some(&step(0, 0)), Some(OpRef { block: 0, op: 0 })), None);
        assert_eq!(resolve_selection(Some(&step(1, 0)), Some(OpRef { block: 0, op: 0 })), None);
    }

    /// No plan and no selection are both simply nothing, not a panic. The view renders
    /// before a plan exists on every launch.
    #[test]
    fn nothing_selected_and_nothing_planned_are_both_none() {
        assert_eq!(resolve_selection(None, Some(OpRef { block: 0, op: 0 })), None);
        assert_eq!(resolve_selection(Some(&step(1, 1)), None), None);
        assert_eq!(resolve_selection(None, None), None);
    }
}
