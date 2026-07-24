//! The **OperationPlanner** — decomposition + ordering (operation-planner.md §3–§5).
//! Turns the step's resolved demand + tool assignment into an ordered
//! [`MachiningPlan`](super::plan) of atomic ops.
//!
//! **This module is the drill phase.** Round holes and vias become point-drill ops,
//! grouped into one contiguous block per tool (op-planner §4.2), ordered
//! small→large diameter (§4.4), with a deterministic nearest-neighbour + 2-opt TSP
//! *within* each block (§4.3). Routing (contours, oblong slots, helical holes) is a
//! separate phase that needs the stitcher to preserve typed segments (§3, §9.6); it
//! is added once that lands. The types in [`super::plan`] are built to grow into it.
//!
//! Everything here is a **pure, deterministic** function of its inputs (op-planner
//! §8): no clock, no RNG, no hash-map iteration order. That is what makes the plan
//! snapshot-testable and the rendered GCode reproducible.

use std::collections::BTreeMap;

use units::Length;

use super::placement::Placement;
use super::plan::{AtomicOp, OpKind, Phase, Point, ToolBlock, ZProfile};

/// Ordering tolerance: a 2-opt swap is accepted only if it shortens the route by
/// more than this (millimetres), so floating-point noise never flips a decision.
const IMPROVE_EPS_MM: f64 = 1e-9;

/// A cap on 2-opt passes — PCB hole counts converge in a handful; the bound only
/// guards against a pathological non-converging input.
const MAX_TWO_OPT_PASSES: usize = 8;

/// One round hole to drill: where it is (board space), which tool drills it, and the
/// plunge depth the assigner computed for that tool/hole. Oblongs and routes are not
/// drill targets — they decompose in the (future) route phase.
#[derive(Clone, Debug, PartialEq)]
pub struct DrillTarget {
    /// Feature id (board hole id or a synthesised index), carried onto the op.
    pub source: String,
    /// Hole centre in board coordinates.
    pub at: pcb::BoardPoint,
    pub tool_id: String,
    pub diameter: Length,
    /// Plunge past the top surface (`T + Lp + m`) from the assignment — a positive
    /// distance; the op stores it as a negative machine-Z depth.
    pub z_bottom: Length,
}

/// Plans the drill phase: one tool block per tool, small→large, TSP-ordered within.
///
/// `start` is each block's virtual start node — the spindle position after its tool
/// change (op-planner §9.1); v1 uses the same park position for every block. `slots`
/// maps a tool id to its rack slot (for display); a tool absent from it renders with
/// no slot.
pub fn plan_drilling(
    targets: &[DrillTarget],
    placement: &Placement,
    start: Point,
    slots: &BTreeMap<String, u8>,
) -> Vec<ToolBlock> {
    // Group targets by tool, preserving a placed (machine-space) point per target.
    // A BTreeMap keeps grouping deterministic regardless of input order.
    struct Placed {
        entry: Point,
        z_bottom: Length,
        source: String,
    }
    let mut by_tool: BTreeMap<String, (Length, Vec<Placed>)> = BTreeMap::new();
    for target in targets {
        let entry = placement.xy(&target.at);
        let slot_entry = by_tool
            .entry(target.tool_id.clone())
            .or_insert_with(|| (target.diameter, Vec::new()));
        slot_entry.1.push(Placed {
            entry,
            z_bottom: target.z_bottom,
            source: target.source.clone(),
        });
    }

    // Order the tool blocks small→large diameter, then by tool id for a total,
    // deterministic order (op-planner §4.4).
    let mut ordered: Vec<(String, Length, Vec<Placed>)> = by_tool
        .into_iter()
        .map(|(tool_id, (diameter, placed))| (tool_id, diameter, placed))
        .collect();
    ordered.sort_by(|a, b| {
        micron(a.1)
            .cmp(&micron(b.1))
            .then_with(|| a.0.cmp(&b.0))
    });

    ordered
        .into_iter()
        .map(|(tool_id, diameter, placed)| {
            let points: Vec<Point> = placed.iter().map(|p| p.entry).collect();
            let order = tsp_order(start, &points);
            let travel_mm = route_length(start, &points, &order);

            let ops: Vec<AtomicOp> = order
                .iter()
                .map(|&i| {
                    let p = &placed[i];
                    AtomicOp {
                        phase: Phase::Drill,
                        kind: OpKind::Drill,
                        tool_id: tool_id.clone(),
                        entry: p.entry,
                        exit: p.entry, // a point drill leaves where it entered
                        z: ZProfile {
                            // Machine Z0 is the board top for the view, so the cutting
                            // bottom is a negative depth (op-planner §6 sign note).
                            z_bottom: Length::from_mm(-p.z_bottom.as_mm()),
                            z_retract: placement.z_retract(),
                            z_feed: None,
                        },
                        primitive: "drill",
                        source: p.source.clone(),
                    }
                })
                .collect();

            ToolBlock {
                slot: slots.get(&tool_id).copied(),
                tool_id,
                diameter,
                ops,
                travel_mm,
            }
        })
        .collect()
}

/// A length quantised to whole micrometres (matches the assigner's precision), for
/// deterministic diameter ordering.
fn micron(length: Length) -> i64 {
    length.as_um().round() as i64
}

/// A deterministic visit order for `points`, starting from `start`: nearest-neighbour
/// seeding, then 2-opt refinement. Ties break on the lower index, so the result is a
/// total function of the inputs.
fn tsp_order(start: Point, points: &[Point]) -> Vec<usize> {
    let n = points.len();
    if n <= 1 {
        return (0..n).collect();
    }
    let mut order = nearest_neighbour(start, points);
    two_opt(start, points, &mut order);
    order
}

/// Greedy nearest-neighbour tour from `start`. At each step picks the unvisited point
/// with the smallest `(distance, index)` — the index tie-break keeps it deterministic.
fn nearest_neighbour(start: Point, points: &[Point]) -> Vec<usize> {
    let n = points.len();
    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut current = start;
    for _ in 0..n {
        let mut best: Option<(f64, usize)> = None;
        for (i, point) in points.iter().enumerate() {
            if visited[i] {
                continue;
            }
            let key = (current.distance_mm(point), i);
            if best.map(|b| key < b).unwrap_or(true) {
                best = Some(key);
            }
        }
        let (_, idx) = best.expect("at least one unvisited point remains");
        visited[idx] = true;
        order.push(idx);
        current = points[idx];
    }
    order
}

/// 2-opt refinement on an **open** path `start → order…`. Repeatedly reverses the
/// sub-tour `order[a..=b]` when doing so shortens the total, scanning in a fixed order
/// and accepting only strict improvements, so it is deterministic and terminating.
fn two_opt(start: Point, points: &[Point], order: &mut Vec<usize>) {
    let n = order.len();
    if n < 3 {
        return;
    }
    let dist = |a: usize, b: usize| points[a].distance_mm(&points[b]);
    let mut passes = 0;
    let mut improved = true;
    while improved && passes < MAX_TWO_OPT_PASSES {
        improved = false;
        passes += 1;
        for a in 0..n - 1 {
            for b in a + 1..n {
                // Edges the reversal of order[a..=b] would replace, on an open path:
                //   (pre → order[a])  becomes  (pre → order[b])
                //   (order[b] → post) becomes  (order[a] → post)   [post may not exist]
                let cur_before = match a {
                    0 => start.distance_mm(&points[order[0]]),
                    _ => dist(order[a - 1], order[a]),
                };
                let new_before = match a {
                    0 => start.distance_mm(&points[order[b]]),
                    _ => dist(order[a - 1], order[b]),
                };
                let (cur_after, new_after) = if b + 1 < n {
                    (dist(order[b], order[b + 1]), dist(order[a], order[b + 1]))
                } else {
                    (0.0, 0.0)
                };
                let delta = (new_before + new_after) - (cur_before + cur_after);
                if delta < -IMPROVE_EPS_MM {
                    order[a..=b].reverse();
                    improved = true;
                }
            }
        }
    }
}

/// Total straight-line length of the open path `start → points[order[0]] → …`.
fn route_length(start: Point, points: &[Point], order: &[usize]) -> f64 {
    let mut total = 0.0;
    let mut prev = start;
    for &i in order {
        total += prev.distance_mm(&points[i]);
        prev = points[i];
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement_identity() -> Placement {
        Placement::new(None, 0.0, 1.0, 1.0, Length::from_mm(2.0), Length::from_mm(5.0))
    }

    fn target(source: &str, x: f64, y: f64, tool: &str, dia: f64) -> DrillTarget {
        DrillTarget {
            source: source.to_string(),
            at: pcb::BoardPoint { x: Length::from_mm(x), y: Length::from_mm(y) },
            tool_id: tool.to_string(),
            diameter: Length::from_mm(dia),
            z_bottom: Length::from_mm(2.4),
        }
    }

    #[test]
    fn groups_into_one_block_per_tool_small_to_large() {
        let targets = vec![
            target("h1", 0.0, 0.0, "big", 1.0),
            target("h2", 5.0, 0.0, "small", 0.6),
            target("h3", 1.0, 0.0, "small", 0.6),
        ];
        let blocks = plan_drilling(&targets, &placement_identity(), Point::new(Length::from_mm(0.0), Length::from_mm(0.0)), &BTreeMap::new());
        assert_eq!(blocks.len(), 2, "one block per distinct tool");
        assert_eq!(blocks[0].tool_id, "small", "smallest diameter first");
        assert_eq!(blocks[0].diameter.as_mm(), 0.6);
        assert_eq!(blocks[1].tool_id, "big");
        assert_eq!(blocks[0].op_count(), 2);
        assert_eq!(blocks[1].op_count(), 1);
    }

    #[test]
    fn a_drill_op_enters_and_exits_at_the_same_point_below_the_surface() {
        let targets = vec![target("h1", 3.0, 4.0, "t", 1.0)];
        let blocks = plan_drilling(&targets, &placement_identity(), Point::new(Length::from_mm(0.0), Length::from_mm(0.0)), &BTreeMap::new());
        let op = &blocks[0].ops[0];
        assert_eq!(op.entry, op.exit, "a point drill returns where it started");
        assert_eq!(op.primitive, "drill");
        assert_eq!(op.kind, OpKind::Drill);
        assert!(op.z.z_bottom.as_mm() < 0.0, "the cutting bottom is below the surface");
        assert_eq!(op.entry.x.as_mm(), 3.0);
        assert_eq!(op.entry.y.as_mm(), 4.0);
    }

    #[test]
    fn tsp_is_no_worse_than_input_order_and_is_deterministic() {
        // Points along a line handed in a poor order; the tour must not be longer than
        // visiting them as given, and must be identical across runs.
        let xs = [0.0, 2.0, 4.0, 1.0, 3.0];
        let targets: Vec<DrillTarget> =
            xs.iter().enumerate().map(|(i, &x)| target(&format!("h{i}"), x, 0.0, "t", 1.0)).collect();
        let start = Point::new(Length::from_mm(0.0), Length::from_mm(0.0));
        let placement = placement_identity();

        let naive_travel: f64 = {
            let pts: Vec<Point> = xs.iter().map(|&x| Point::new(Length::from_mm(x), Length::from_mm(0.0))).collect();
            route_length(start, &pts, &(0..pts.len()).collect::<Vec<_>>())
        };

        let a = plan_drilling(&targets, &placement, start, &BTreeMap::new());
        let b = plan_drilling(&targets, &placement, start, &BTreeMap::new());
        assert_eq!(a, b, "identical inputs yield an identical plan");
        assert!(a[0].travel_mm <= naive_travel + 1e-9, "ordering is no worse than input order");
        // The optimal tour over 0..4 from the origin is a straight sweep of length 4.
        assert!((a[0].travel_mm - 4.0).abs() < 1e-6, "sorts the collinear points, travel = 4mm");
    }

    #[test]
    fn slot_is_carried_through_when_known() {
        let targets = vec![target("h1", 0.0, 0.0, "t", 1.0)];
        let mut slots = BTreeMap::new();
        slots.insert("t".to_string(), 3u8);
        let blocks = plan_drilling(&targets, &placement_identity(), Point::new(Length::from_mm(0.0), Length::from_mm(0.0)), &slots);
        assert_eq!(blocks[0].slot, Some(3));
    }
}
