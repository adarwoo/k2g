//! The **machining plan** — the OperationPlanner's output
//! ([`schemas/docs/operation-planner.md`] §1). An ordered set of *atomic
//! operations*, grouped into tool blocks per machining step, held in memory as the
//! single structured description of what the job machines.
//!
//! Two consumers read it (op-planner §1): the **Machining view** renders it (tool
//! blocks, op counts, travel), and — later — the **Coder** walks it to emit GCode.
//! Keeping it as typed data (not only rendered GCode text) is the whole point: the
//! view can show the plan before a single line of GCode exists.
//!
//! This is the drill-phase shape. Routing adds op kinds (contour/slot/helical) and
//! a `Route` phase once the stitcher preserves typed segments (op-planner §3, §9.6);
//! the enums below are built to grow into it.

use units::{FeedRate, Length};

/// A 2D point in **machine coordinates** (millimetres), as produced by
/// [`super::placement::Placement`]. Ops carry machine-space points so the ordering
/// TSP minimises *physical* travel (op-planner §6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: Length,
    pub y: Length,
}

impl Point {
    pub const fn new(x: Length, y: Length) -> Self {
        Self { x, y }
    }

    /// Straight-line distance to another point, in millimetres.
    pub fn distance_mm(&self, other: &Point) -> f64 {
        let dx = self.x.as_mm() - other.x.as_mm();
        let dy = self.y.as_mm() - other.y.as_mm();
        (dx * dx + dy * dy).sqrt()
    }
}

/// The machining phase an op belongs to. Phases run in this fixed,
/// rigidity-decreasing order (op-planner §4): all drilling completes while the board
/// is fully attached and flat, before any routing releases it. `Engrave` is reserved
/// for the future copper phase (op-planner §9.5); ordering is by `derive(Ord)`, so
/// the variant order *is* the phase order.
///
/// `Engrave` and `Route` are not emitted by the drill phase yet, but their ordinal
/// positions define the precedence (`derive(Ord)`) the planner is built around, so
/// they are declared now (op-planner §4, §9.5).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Engrave,
    Drill,
    Route,
}

/// What an atomic op physically does — the discriminant the view and labels read.
/// The GTL primitive that renders it is [`AtomicOp::primitive`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    /// A point drill (`drill` primitive, G81). `entry == exit`.
    Drill,
}

/// The Z parameters an op cuts at, in machine Z. `z_bottom` is the deepest cutting
/// height and `z_retract` the R-plane the tool clears to between features.
///
/// Sign/reference note: the view treats machine Z0 as the board top surface, so
/// `z_bottom` is a **negative depth**. The definitive work-coordinate origin is set
/// in the `initialise` primitive; the Coder maps these onto it when generation is
/// wired (op-planner §6, §7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZProfile {
    pub z_bottom: Length,
    pub z_retract: Length,
    /// Plunge feed; `None` renders with the primitive/template default.
    pub z_feed: Option<FeedRate>,
}

/// One **atomic machining operation** (op-planner §1). Flat: exactly one
/// `entry`/`exit`. Any internal iteration (a multi-pass route, a whole contour path)
/// is hidden inside the op's rendering and never leaks into the op list — that
/// invariant keeps the ordering TSP and phase precedence tractable.
#[derive(Clone, Debug, PartialEq)]
pub struct AtomicOp {
    pub phase: Phase,
    pub kind: OpKind,
    /// Stock-tool id performing the op (the block it lands in binds it to a slot).
    pub tool_id: String,
    /// Where the tool arrives to begin.
    pub entry: Point,
    /// Where the tool leaves (`== entry` for a point drill).
    pub exit: Point,
    pub z: ZProfile,
    /// The GTL primitive that renders this op (op-planner §7).
    pub primitive: &'static str,
    /// The feature this op came from (hole/edge id), for the view + diagnostics.
    pub source: String,
}

/// A contiguous run of ops sharing one tool (op-planner §4.2) — the unit that costs
/// exactly one tool change. Ordered within by the planner's TSP.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolBlock {
    pub tool_id: String,
    /// Rack slot, when the assignment placed the tool on one.
    pub slot: Option<u8>,
    pub diameter: Length,
    pub ops: Vec<AtomicOp>,
    /// Total straight-line XY travel across the block, from the block's start point
    /// through every op in order (millimetres) — the quantity the TSP minimises.
    pub travel_mm: f64,
}

impl ToolBlock {
    pub fn op_count(&self) -> usize {
        self.ops.len()
    }
}

/// The plan for **one machining step** — its ordered tool blocks (already phase- and
/// tool-grouped, TSP-ordered within each block). One program is rendered per step
/// (op-planner §9.2), so the plan is naturally per-step.
#[derive(Clone, Debug, PartialEq)]
pub struct StepPlan {
    pub index: usize,
    pub name: String,
    pub blocks: Vec<ToolBlock>,
    /// Human-facing notes about what this step's plan does *not* yet cover (e.g.
    /// routing awaiting the stitcher rework, oblongs, locating pins).
    pub notes: Vec<String>,
}

impl StepPlan {
    pub fn op_count(&self) -> usize {
        self.blocks.iter().map(ToolBlock::op_count).sum()
    }
}

/// The whole job's plan: one [`StepPlan`] per machining step, in order. Held in
/// memory (this type *is* the "primitives in memory") and rendered by the Machining
/// view; the Coder will later walk each step to a standalone program.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MachiningPlan {
    pub steps: Vec<StepPlan>,
    /// A top-level note when there is nothing to plan (no profile / no board).
    pub note: Option<String>,
}

impl MachiningPlan {
    /// Total atomic ops across every step — a quick headline for the view.
    pub fn total_ops(&self) -> usize {
        self.steps.iter().map(StepPlan::op_count).sum()
    }
}
