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

/// One plunge to drill: where it is (board space), which tool drills it, and the plunge
/// depth the assigner computed for that tool/hole. A round hole is one target; an oblong
/// made by drilling is a chain of them (see [`DrillTarget::chain`]).
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
    /// Chain id when this plunge is one of a slot's drill chain, whose order the caller
    /// has already chosen ([`crate::gcode::oblong::chain_order`]) so no bit is ever
    /// loaded on one flank only.
    ///
    /// Consecutive targets sharing an id are one **run**: the TSP orders runs, never
    /// their contents. Without this the tour would shorten a chain into a left-to-right
    /// sweep — the exact order the chain geometry exists to avoid. `None` for a round
    /// hole, which is a run of one.
    pub chain: Option<String>,
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
        chain: Option<String>,
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
            chain: target.chain.clone(),
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
            // Order runs, not plunges: a drill chain's internal order is already fixed,
            // so the TSP sees each chain as one node at its first plunge.
            let runs = contiguous_runs(placed.len(), |i| placed[i].chain.as_deref());
            let heads: Vec<Point> = runs.iter().map(|run| placed[run.start].entry).collect();
            let run_order = tsp_order(start, &heads);
            let order: Vec<usize> = run_order
                .iter()
                .flat_map(|&r| runs[r].clone())
                .collect();

            let points: Vec<Point> = placed.iter().map(|p| p.entry).collect();
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

/// What a [`RouteTarget`] mills — the two feature shapes a router makes.
#[derive(Clone, Debug, PartialEq)]
pub enum RouteShape {
    /// A round hole, spiralled from its centre out to `hole_diameter`.
    Hole { hole_diameter: Length },
    /// An oblong slot whose medial axis runs from the target's `at` to `far` (both board
    /// coordinates) and which is `width` across. `from_solid` is `false` when a drill
    /// chain has already opened the channel, leaving only the wall lap.
    Slot { far: pcb::BoardPoint, width: Length, from_solid: bool },
}

/// One feature to mill with a router: a round hole no drill can make (the assigner's
/// route-fallback — too big to drill, or a drill point that would reach the bed), or an
/// oblong slot whose strategy calls for a router. Not a drill target.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteTarget {
    /// Feature id (board hole id or a synthesised index), carried onto the op.
    pub source: String,
    /// Board coordinates of where the cut begins: a hole's centre, or a slot's near
    /// medial-axis end centre.
    pub at: pcb::BoardPoint,
    /// The router performing the cut (the block's tool).
    pub tool_id: String,
    /// Router diameter — the block diameter and the toolpath's cutter width.
    pub tool_diameter: Length,
    /// The feature's geometry.
    pub shape: RouteShape,
    /// Plunge past the top surface (`T + m`) — a positive distance; the op stores it as
    /// a negative machine-Z depth.
    pub z_bottom: Length,
}

/// Plans the route-hole phase: one block per router, small→large, TSP-ordered within.
/// Mirrors [`plan_drilling`] but emits [`OpKind::RouteHole`] ops in the `Route` phase —
/// each expands to a spiral pocket at render time (`super::routing`). Runs after
/// drilling so the board stays rigid while every drilled hole is made (op-planner §4).
pub fn plan_routing(
    targets: &[RouteTarget],
    placement: &Placement,
    start: Point,
    slots: &BTreeMap<String, u8>,
) -> Vec<ToolBlock> {
    struct Placed {
        entry: Point,
        /// Where the tool leaves: the same point for a hole, the far medial end for a
        /// slot. Placed here so the slot's orientation is transformed exactly once.
        exit: Point,
        z_bottom: Length,
        kind: OpKind,
        source: String,
    }
    let mut by_tool: BTreeMap<String, (Length, Vec<Placed>)> = BTreeMap::new();
    for target in targets {
        let entry = placement.xy(&target.at);
        let (exit, kind) = match &target.shape {
            RouteShape::Hole { hole_diameter } => {
                (entry, OpKind::RouteHole { hole_diameter: *hole_diameter })
            }
            RouteShape::Slot { far, width, from_solid } => (
                placement.xy(far),
                OpKind::RouteSlot { width: *width, from_solid: *from_solid },
            ),
        };
        let slot_entry = by_tool
            .entry(target.tool_id.clone())
            .or_insert_with(|| (target.tool_diameter, Vec::new()));
        slot_entry.1.push(Placed {
            entry,
            exit,
            z_bottom: target.z_bottom,
            kind,
            source: target.source.clone(),
        });
    }

    let mut ordered: Vec<(String, Length, Vec<Placed>)> = by_tool
        .into_iter()
        .map(|(tool_id, (diameter, placed))| (tool_id, diameter, placed))
        .collect();
    ordered.sort_by(|a, b| micron(a.1).cmp(&micron(b.1)).then_with(|| a.0.cmp(&b.0)));

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
                        phase: Phase::Route,
                        primitive: match p.kind {
                            OpKind::RouteSlot { .. } => "route_slot",
                            _ => "route_hole",
                        },
                        kind: p.kind.clone(),
                        tool_id: tool_id.clone(),
                        entry: p.entry,
                        exit: p.exit,
                        z: ZProfile {
                            // Negative machine-Z depth (board top is Z0; op-planner §6).
                            z_bottom: Length::from_mm(-p.z_bottom.as_mm()),
                            z_retract: placement.z_retract(),
                            z_feed: None,
                        },
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

/// One uninterrupted span of the board outline to cut.
///
/// Unlike a drill or a hole target, a span arrives **already placed**: the contour has to
/// be offset in board space (before the CNC's per-axis scaling, which would otherwise turn
/// a constant kerf into a varying one), and each offset point is then mapped through the
/// placement. So the caller does the placing and the planner only orders.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineSpan {
    /// Feature id — the contour and span it came from, for the view + diagnostics.
    pub source: String,
    /// Cutter-centre polyline in **machine** coordinates.
    pub path: Vec<Point>,
}

/// Plans the board-outline phase: one block for the outline router, its spans ordered by
/// travel between their start points.
///
/// One block, not one per contour, because the outline is cut by a single router — the
/// step should pay one tool change for the whole outline whatever it is made of. Returns
/// `None` when there is nothing to cut, so the caller adds no empty block (and so no
/// pointless tool change) to the step.
///
/// Span order is a pure travel optimisation. It does not affect how well the board is held:
/// the tabs between spans are what hold it, and they survive until the operator breaks
/// them, so no span order can release the board early.
pub fn plan_outline(
    spans: &[OutlineSpan],
    tool_id: &str,
    tool_diameter: Length,
    z_bottom: Length,
    z_retract: Length,
    start: Point,
    slots: &BTreeMap<String, u8>,
) -> Option<ToolBlock> {
    let usable: Vec<&OutlineSpan> = spans.iter().filter(|s| s.path.len() >= 2).collect();
    if usable.is_empty() {
        return None;
    }

    let entries: Vec<Point> = usable.iter().map(|s| s.path[0]).collect();
    let order = tsp_order(start, &entries);
    let travel_mm = route_length(start, &entries, &order);

    let ops: Vec<AtomicOp> = order
        .iter()
        .map(|&i| {
            let span = usable[i];
            AtomicOp {
                phase: Phase::Route,
                kind: OpKind::RouteContour { path: span.path.clone() },
                tool_id: tool_id.to_string(),
                entry: span.path[0],
                exit: span.path[span.path.len() - 1],
                z: ZProfile { z_bottom, z_retract, z_feed: None },
                primitive: "route_contour",
                source: span.source.clone(),
            }
        })
        .collect();

    Some(ToolBlock {
        slot: slots.get(tool_id).copied(),
        tool_id: tool_id.to_string(),
        diameter: tool_diameter,
        ops,
        travel_mm,
    })
}

/// A length quantised to whole micrometres (matches the assigner's precision), for
/// deterministic diameter ordering.
fn micron(length: Length) -> i64 {
    length.as_um().round() as i64
}

/// A deterministic visit order for `points`, starting from `start`: nearest-neighbour
/// seeding, then 2-opt refinement. Ties break on the lower index, so the result is a
/// total function of the inputs.
/// Splits `0..len` into the runs the TSP may reorder: maximal spans of consecutive
/// indices sharing the same `Some` chain id. Anything with no chain id is a run of one,
/// so a board of plain round holes gets exactly the per-hole ordering it had before.
///
/// Chained targets arrive consecutively because one slot's chain is pushed in one go; a
/// second chain with the same id (impossible today — ids are per-hole) would simply
/// become a second run rather than silently merging.
fn contiguous_runs<'a>(
    len: usize,
    chain_of: impl Fn(usize) -> Option<&'a str>,
) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    let mut i = 0;
    while i < len {
        let mut end = i + 1;
        if let Some(id) = chain_of(i) {
            while end < len && chain_of(end) == Some(id) {
                end += 1;
            }
        }
        runs.push(i..end);
        i = end;
    }
    runs
}

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
        Placement::new(&crate::gcode::placement::PlacementSpec {
            bounds: None,
            orientation_deg: 0.0,
            origin: Default::default(),
            margin: Default::default(),
            flip: None,
            scale_x: 1.0,
            scale_y: 1.0,
            z_retract: Length::from_mm(2.0),
            z_safe: Length::from_mm(5.0),
        })
    }

    fn target(source: &str, x: f64, y: f64, tool: &str, dia: f64) -> DrillTarget {
        DrillTarget {
            source: source.to_string(),
            at: pcb::BoardPoint { x: Length::from_mm(x), y: Length::from_mm(y) },
            tool_id: tool.to_string(),
            diameter: Length::from_mm(dia),
            z_bottom: Length::from_mm(2.4),
            chain: None,
        }
    }

    /// A plunge belonging to a named drill chain.
    fn chained(chain: &str, n: usize, x: f64, y: f64, tool: &str, dia: f64) -> DrillTarget {
        DrillTarget { chain: Some(chain.to_string()), ..target(&format!("{chain}.{n}"), x, y, tool, dia) }
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

    /// A drill chain's order is chosen for tool safety, not travel, so the TSP must
    /// place the chain as a unit and leave its interior untouched. Left to itself the
    /// tour would sort these collinear points into a sweep — the very order the chain
    /// geometry exists to avoid.
    #[test]
    fn the_tsp_never_resequences_a_drill_chain() {
        // Ends first, then the middle — the bisection order, deliberately not sorted.
        let targets = vec![
            chained("slot", 0, 0.0, 0.0, "t", 0.4),
            chained("slot", 1, 4.0, 0.0, "t", 0.4),
            chained("slot", 2, 2.0, 0.0, "t", 0.4),
        ];
        let blocks = plan_drilling(
            &targets,
            &placement_identity(),
            Point::new(Length::from_mm(0.0), Length::from_mm(0.0)),
            &BTreeMap::new(),
        );
        let sources: Vec<&str> = blocks[0].ops.iter().map(|op| op.source.as_str()).collect();
        assert_eq!(sources, vec!["slot.0", "slot.1", "slot.2"], "chain order preserved verbatim");
    }

    /// Chains are still ordered *against each other*, and against loose holes, by
    /// travel — only their interiors are fixed.
    #[test]
    fn chains_are_ordered_among_themselves_by_travel() {
        // A far chain listed first, a near loose hole listed last.
        let targets = vec![
            chained("far", 0, 50.0, 0.0, "t", 0.4),
            chained("far", 1, 52.0, 0.0, "t", 0.4),
            target("near", 1.0, 0.0, "t", 0.4),
        ];
        let blocks = plan_drilling(
            &targets,
            &placement_identity(),
            Point::new(Length::from_mm(0.0), Length::from_mm(0.0)),
            &BTreeMap::new(),
        );
        let sources: Vec<&str> = blocks[0].ops.iter().map(|op| op.source.as_str()).collect();
        assert_eq!(
            sources,
            vec!["near", "far.0", "far.1"],
            "the near hole is visited first, then the chain intact"
        );
    }

    #[test]
    fn slot_is_carried_through_when_known() {
        let targets = vec![target("h1", 0.0, 0.0, "t", 1.0)];
        let mut slots = BTreeMap::new();
        slots.insert("t".to_string(), 3u8);
        let blocks = plan_drilling(&targets, &placement_identity(), Point::new(Length::from_mm(0.0), Length::from_mm(0.0)), &slots);
        assert_eq!(blocks[0].slot, Some(3));
    }

    fn route_target(source: &str, at: (f64, f64), shape: RouteShape) -> RouteTarget {
        RouteTarget {
            source: source.to_string(),
            at: pcb::BoardPoint { x: Length::from_mm(at.0), y: Length::from_mm(at.1) },
            tool_id: "router".to_string(),
            tool_diameter: Length::from_mm(1.0),
            shape,
            z_bottom: Length::from_mm(2.1),
        }
    }

    #[test]
    fn route_holes_land_in_the_route_phase_carrying_the_hole_diameter() {
        let targets = vec![route_target(
            "h1",
            (3.0, 4.0),
            RouteShape::Hole { hole_diameter: Length::from_mm(3.2) },
        )];
        let blocks = plan_routing(&targets, &placement_identity(), Point::new(Length::from_mm(0.0), Length::from_mm(0.0)), &BTreeMap::new());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].diameter.as_mm(), 1.0, "block diameter is the router");
        let op = &blocks[0].ops[0];
        assert_eq!(op.phase, Phase::Route);
        assert_eq!(op.kind, OpKind::RouteHole { hole_diameter: Length::from_mm(3.2) });
        assert_eq!(op.primitive, "route_hole");
        assert_eq!(op.entry, op.exit, "a spiralled hole ends where it began");
        assert!(op.z.z_bottom.as_mm() < 0.0, "cut depth is below the surface");
    }

    /// A slot's two medial-axis ends both go through the placement, so the op carries the
    /// orientation as placed points — there is no board-space angle left to misread.
    #[test]
    fn a_routed_slot_carries_its_axis_as_placed_entry_and_exit() {
        let targets = vec![route_target(
            "slot1",
            (2.0, 5.0),
            RouteShape::Slot {
                far: pcb::BoardPoint { x: Length::from_mm(6.0), y: Length::from_mm(5.0) },
                width: Length::from_mm(1.6),
                from_solid: true,
            },
        )];
        let blocks = plan_routing(&targets, &placement_identity(), Point::new(Length::from_mm(0.0), Length::from_mm(0.0)), &BTreeMap::new());
        let op = &blocks[0].ops[0];
        assert_eq!(
            op.kind,
            OpKind::RouteSlot { width: Length::from_mm(1.6), from_solid: true }
        );
        assert_eq!(op.primitive, "route_slot");
        assert_eq!(op.entry, Point::new(Length::from_mm(2.0), Length::from_mm(5.0)));
        assert_eq!(op.exit, Point::new(Length::from_mm(6.0), Length::from_mm(5.0)));
    }

    /// Every outline span lands in one block on the outline router — one tool change for
    /// the whole outline — and each op carries the span it cuts, entry to exit.
    #[test]
    fn outline_spans_share_one_block_and_carry_their_own_path() {
        let span = |source: &str, from: (f64, f64), to: (f64, f64)| OutlineSpan {
            source: source.to_string(),
            path: vec![
                Point::new(Length::from_mm(from.0), Length::from_mm(from.1)),
                Point::new(Length::from_mm(to.0), Length::from_mm(to.1)),
            ],
        };
        let spans = vec![
            span("outer#0.span0", (50.0, 0.0), (60.0, 0.0)),
            span("outer#0.span1", (1.0, 0.0), (10.0, 0.0)),
            // A degenerate span is dropped rather than emitted as a zero-length cut.
            OutlineSpan { source: "outer#0.span2".into(), path: vec![] },
        ];
        let block = plan_outline(
            &spans,
            "router",
            Length::from_mm(2.0),
            Length::from_mm(-2.1),
            Length::from_mm(5.0),
            Point::new(Length::from_mm(0.0), Length::from_mm(0.0)),
            &BTreeMap::new(),
        )
        .expect("there is an outline to cut");

        assert_eq!(block.op_count(), 2, "the empty span is dropped");
        assert_eq!(block.ops[0].source, "outer#0.span1", "the nearer span is cut first");
        assert_eq!(block.ops[0].phase, Phase::Route);
        assert_eq!(block.ops[0].primitive, "route_contour");
        assert_eq!(block.ops[0].entry, spans[1].path[0]);
        assert_eq!(block.ops[0].exit, spans[1].path[1], "the op ends where its span does");
        assert!(matches!(&block.ops[0].kind, OpKind::RouteContour { path } if *path == spans[1].path));
    }

    /// Nothing to cut means no block at all — an empty block would still cost a tool
    /// change, which is the one thing block grouping exists to avoid.
    #[test]
    fn an_outline_with_no_cuttable_spans_makes_no_block() {
        assert!(plan_outline(
            &[],
            "router",
            Length::from_mm(2.0),
            Length::from_mm(-2.1),
            Length::from_mm(5.0),
            Point::new(Length::from_mm(0.0), Length::from_mm(0.0)),
            &BTreeMap::new(),
        )
        .is_none());
    }

    /// Holes and slots milled by the same router share one block, so the step pays for a
    /// single tool change rather than one per shape.
    #[test]
    fn holes_and_slots_on_one_router_share_a_single_block() {
        let targets = vec![
            route_target("h1", (0.0, 0.0), RouteShape::Hole { hole_diameter: Length::from_mm(3.2) }),
            route_target(
                "slot1",
                (10.0, 0.0),
                RouteShape::Slot {
                    far: pcb::BoardPoint { x: Length::from_mm(12.0), y: Length::from_mm(0.0) },
                    width: Length::from_mm(1.6),
                    from_solid: false,
                },
            ),
        ];
        let blocks = plan_routing(&targets, &placement_identity(), Point::new(Length::from_mm(0.0), Length::from_mm(0.0)), &BTreeMap::new());
        assert_eq!(blocks.len(), 1, "one router, one block");
        assert_eq!(blocks[0].op_count(), 2);
    }
}
