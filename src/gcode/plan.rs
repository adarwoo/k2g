//! The **machining plan** — the OperationPlanner's output
//! ([`docs/design/operation-planner.md`] §1). An ordered set of *atomic
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
/// rigidity-decreasing order (op-planner §4): the copper is engraved while the board is whole,
/// flat and undrilled, then all drilling completes while it is still fully attached, before any
/// routing releases it. Ordering is by `derive(Ord)`, so the variant order *is* the phase order.
///
/// All three are emitted. Note that the ordering is not *enforced* through this type: the
/// planner pushes blocks in the right sequence and `block_order_tests` in
/// `crate::runtime::machining_plan` guards that at the source, because nothing here would fail
/// to compile if two pushes were swapped.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Engrave,
    Drill,
    Route,
}

/// What an atomic op physically does — the discriminant the view and body renderer
/// read. For a [`OpKind::Drill`] the single [`AtomicOp::primitive`] renders it; a
/// [`OpKind::RouteHole`] expands into a sequence of moves (rapid/plunge/arc), so its
/// `primitive` is only a display label.
#[derive(Clone, Debug, PartialEq)]
pub enum OpKind {
    /// A point drill (`drill` primitive, G81). `entry == exit`.
    Drill,
    /// A hole milled by spiralling a router from the centre outward — the assigner's
    /// route-fallback, when no drill can make the hole (too big, or its point would
    /// reach the bed). Carries the finished hole diameter; the router diameter comes
    /// from the enclosing block. Expanded by the body renderer via `super::routing`.
    RouteHole { hole_diameter: Length },
    /// An oblong slot milled by a router (the `route`, `drill_ends_then_route` and
    /// `drill_chain_then_route` oblong strategies).
    ///
    /// The op's [`AtomicOp::entry`] and [`AtomicOp::exit`] are the slot's **medial-axis
    /// end centres**, so those two placed points carry the slot's orientation and no
    /// board-space angle survives into machine space. `width` is the slot across its
    /// short axis; `from_solid` is `false` when a drill chain has already opened the
    /// channel and only the wall lap remains. Expanded by `super::routing::slot_route`.
    RouteSlot { width: Length, from_solid: bool },
    /// One span of the board outline: a cutter-centre polyline, already offset onto the
    /// waste side of the edge and already in machine coordinates.
    ///
    /// This is the one op kind that **carries** its geometry rather than deriving it,
    /// because a contour's shape cannot be reconstructed from two points. It is still one
    /// atomic op — the whole span is a single uninterrupted cut, which is exactly the unit
    /// the ordering and phase rules want (op-planner §1). Retaining tabs are the *gaps*
    /// between spans, so they need no representation of their own.
    RouteContour { path: Vec<Point> },
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
    /// This op picks up exactly where the **previous** op's cutter left off — no
    /// lead-in rapid/plunge, and (via the previous op's own lookahead) no lead-out
    /// retract for the shared seam between them.
    ///
    /// Set only by [`plan_engrave`](super::planner::plan_engrave) for the second and
    /// later members of a same-net chain (see `machining_plan::plan_engrave_spans`):
    /// pieces of one net's isolation loop that the ladder split apart only because the
    /// channel had to narrow, and which meet at an exact, verified-coincident point —
    /// never guessed, never across nets. Every other op leaves this `false`, which is
    /// today's independent-retract behaviour and the correct default for anything that
    /// is not a verified continuation.
    pub continues_from_previous: bool,
}

/// An operator stop, and everything the program needs to make one.
///
/// Emitted **after the block's first op**, so it is the depth test cut the operator is being
/// asked to look at — see [`ToolBlock::verify_stop`].
///
/// The text is split in two because a controller's message word carries one line: `advice` goes
/// out as comments, which a machine with no comment word simply drops, and `prompt` is the one
/// line that appears at the stop itself.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifyStop {
    /// Where the tool lifts to before the spindle stops — the fixture's safe height, clear of
    /// clamps and fixture hardware. The operator is about to put their hands and a loupe next to
    /// the cut, so this is not the retract plane.
    pub z_clear: Length,
    /// Lines shown before the stop, through the machine's own `comment` primitive. What the
    /// channel should measure, and what to do in each direction.
    ///
    /// **Plain ASCII, and no parentheses:** every bundled profile renders a comment as
    /// `( {text} )`, so a bracket inside the text closes the comment early and feeds the rest of
    /// the sentence to the parser. Formatted in millimetres and never through the operator's
    /// display-unit preference — the emitted program is a machine fact and must not change
    /// because someone switched the UI to inches.
    pub advice: Vec<String>,
    /// The one line shown at the stop itself.
    pub prompt: String,
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
    /// An operator stop after this block's **first** op, and what to say at it.
    ///
    /// Set only on an isolation block whose step asked for a depth test cut, in which case
    /// `ops[0]` **is** that cut — put there by [`plan_engrave`](super::planner::plan_engrave)
    /// ahead of the TSP rather than ordered by it, because a test cut that happens second has
    /// already been cut at a depth nobody looked at.
    ///
    /// The two halves have to agree and the type cannot say so, which is the price of keeping
    /// the cut in `ops` where the 3D view, the op table and the op count all find it for free.
    /// There is one construction site and one consumption site, and a test pins the pairing.
    ///
    /// Note that the test cut is counted by [`Self::op_count`] like any other op, even though it
    /// machines nothing of the board. That is deliberate: it is real motion at real depth, and a
    /// count that hid it would disagree with the program.
    pub verify_stop: Option<VerifyStop>,
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
