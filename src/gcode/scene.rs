//! The 3D machining view's **scene** — one machining step turned into geometry.
//!
//! This is everything about the 3D view that can be decided in Rust, which is
//! deliberately as much as possible: the JavaScript downstream receives this and owns
//! only the camera. That split is what keeps the one untestable part of k2g small — the
//! renderer draws what it is given, so a wrong picture is a failing test here rather
//! than something to squint at.
//!
//! ## What it draws, and what it does not
//!
//! The scene is built from the **plan**, not from the emitted G-code. The plan is typed,
//! already in machine space, and already expanded through the same `routing`/`outline`
//! functions the Coder renders from — so the picture is the program's *intent*. It
//! cannot catch a broken CNC template, which is a job for a real backplot later; the
//! extraction is deliberately shaped so a G-code interpreter could feed the same
//! [`Scene`] without the renderer knowing the difference.
//!
//! ## Motion is synthesised
//!
//! A drill op is a *point*: the up-and-down lives inside the `drill` primitive, because
//! a `G81` cycle positions and retracts itself. So the vertical motion the operator
//! wants to see is reconstructed here from [`ZProfile`] — rapid across at the retract
//! plane, feed down, rapid back up. Routed ops already carry their moves and are
//! expanded through `routing::spiral_hole` / `slot_route`, arcs tessellated for display.

use serde::Serialize;
use units::Length;

use crate::gcode::plan::{OpKind, Point, StepPlan, ToolBlock, ZProfile};
use crate::gcode::routing::{self, RouteMove};

/// Chord tolerance for tessellating an arc, in millimetres.
///
/// Only the *picture* is chorded — the emitted program keeps its `G2`/`G3`. At 20 µm the
/// facets are far below anything visible at PCB scale, and it keeps a full circle to a
/// few dozen points rather than a fixed high count that would be wasteful on a 0.5 mm
/// hole and coarse on a 50 mm one.
const ARC_CHORD_MM: f64 = 0.02;

/// Bounds on the tessellation, so a degenerate radius cannot produce either a visible
/// polygon or a runaway point count.
const ARC_MIN_SEGMENTS: usize = 8;
const ARC_MAX_SEGMENTS: usize = 240;

/// Per-tool colours, assigned by block order and reused once exhausted.
///
/// Defined here rather than in the renderer so there is one source of truth: a legend in
/// the Tooling tab and the lines in the 3D view have to agree, and they cannot if the
/// list lives in JavaScript. Chosen to stay distinguishable against both themes' panel
/// backgrounds and to survive the common forms of colour blindness — they differ in
/// lightness as well as hue, so they are still tellable apart in greyscale.
pub const TOOL_PALETTE: [u32; 8] = [
    0x4ea3ff, // blue
    0xffb648, // amber
    0x5ddba0, // green
    0xff7a8a, // rose
    0xc08cff, // violet
    0xffe066, // yellow
    0x59d5e0, // cyan
    0xff9f5a, // orange
];

/// The colour for the `index`-th tool block.
///
/// Assigned **here**, once, and carried on the trace ([`ToolTrace::colour`]) — the
/// renderer no longer picks it. That is what lets a trace be filtered out of the payload
/// without recolouring the ones after it: an index into the palette on the JavaScript
/// side would shift the moment anything were hidden, and the legend beside the canvas
/// would start naming the wrong colours.
pub fn tool_colour(index: usize) -> u32 {
    TOOL_PALETTE[index % TOOL_PALETTE.len()]
}

/// A point in machine space, millimetres, Z up. Plain `f64` because this is display
/// geometry on its way out of the type system and into a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ScenePoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl ScenePoint {
    fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    fn at(point: Point, z: Length) -> Self {
        Self::new(point.x.as_mm(), point.y.as_mm(), z.as_mm())
    }
}

/// Whether a run of motion is cutting or just getting somewhere.
///
/// The distinction is the single most informative thing in a toolpath view, and the
/// convention shared by every backplot worth using (Camotics, NC Viewer, LinuxCNC AXIS):
/// rapids thin and muted, feed moves solid and saturated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MoveKind {
    Rapid,
    Feed,
}

/// One continuous run of motion of a single kind.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Polyline {
    pub kind: MoveKind,
    pub points: Vec<ScenePoint>,
}

/// Everything one tool does, in the order it does it.
///
/// One trace per [`ToolBlock`], so a trace *is* the work between two tool changes — which
/// is exactly the unit an operator thinks in ("what does the 0.8 drill do?") and the unit
/// the view colours and toggles.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolTrace {
    pub tool_id: String,
    /// This tool's colour, from [`TOOL_PALETTE`] by block order — the same value the
    /// legend shows, so the two cannot disagree.
    pub colour: u32,
    /// Rack slot, when the assignment placed the tool on one.
    pub slot: Option<u8>,
    pub diameter_mm: f64,
    /// Where the operator (or the ATC) changes to this tool — the block's first entry.
    /// `None` for a block with no ops.
    pub change_at: Option<ScenePoint>,
    pub moves: Vec<Polyline>,
}

impl ToolTrace {
    /// Total number of points across every run — the figure that decides whether the
    /// renderer needs to worry.
    pub fn point_count(&self) -> usize {
        self.moves.iter().map(|m| m.points.len()).sum()
    }
}

/// Segments used to draw a drilled hole's circle in the board solid.
///
/// Twelve is enough that a 0.8 mm hole reads as round at any zoom the board is legible
/// at, and it keeps a 300-hole board to a few thousand extra vertices — which the
/// triangulator does once, at build time.
const HOLE_SEGMENTS: usize = 12;

/// The workpiece: an outline with everything that is removed from it.
///
/// Rendered as an extruded polygon-with-holes, which is a built-in on the renderer's
/// side, so this carries no triangles — just loops. All coordinates are machine
/// millimetres, so the board and the toolpaths share one frame and any misalignment
/// between them is a real misalignment rather than a rendering artefact.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BoardSolid {
    /// The outer boundary, implicitly closed.
    pub outline: Vec<[f64; 2]>,
    /// Everything the board loses: routed cutouts and drilled holes alike.
    ///
    /// One list rather than two because the renderer treats them identically — they are
    /// all just holes in the extruded shape. Drilled holes go in as circles rather than
    /// as separate cylinders precisely so they are *holes* you can see through, instead
    /// of pegs sitting in the board.
    pub openings: Vec<Vec<[f64; 2]>>,
    pub thickness_mm: f64,
    /// The board is in the fixture back-face up, because this step machines its back.
    ///
    /// Decides **which way round the two coloured faces go**, not whether they are drawn:
    /// the board's back is always the red one and its front always the green one, and this
    /// says which of them the spindle is looking at. A back-face step has the board turned
    /// over, so red faces up.
    ///
    /// Worth colouring at all because the artwork is *mirrored* for such a step —
    /// correctly, since that is how the board physically sits — and a mirrored board is
    /// indistinguishable from a right-way-round one unless you already know which you are
    /// looking at. Getting that wrong is a scrapped board.
    pub back_face_up: bool,
}

/// The setup the board sits in: the work zero, the stop the board is registered against,
/// and the locating pins.
///
/// None of this is cut — it is the *frame* the program is written in, drawn so an operator
/// can see it. The gap between the bracket and the board is the room the origin made for
/// the pins, and is the one thing about a pinned job's coordinate frame that is otherwise
/// invisible: the numbers in the program look entirely ordinary either way.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FixtureMark {
    /// Length of each bracket arm, in machine millimetres.
    pub arm_mm: f64,
    /// Which way the arms run from the origin, as `+1`/`-1` per axis: away from the stop
    /// and along the work. With the zero on the board's right-hand edge the work is at
    /// negative X, and so is the arm.
    pub dir_x: f64,
    pub dir_y: f64,
    /// The pin holes as `[x, y, diameter]` in machine millimetres. Empty when the job has
    /// no locating pins.
    ///
    /// Not [`BoardSolid::openings`]: these are holes in the *blank and the backboard*, not
    /// in the board, and drawing them as board openings would put two holes through a
    /// workpiece that does not have them.
    pub pins: Vec<[f64; 3]>,
}

impl BoardSolid {
    /// Appends a drilled hole as a circle of [`HOLE_SEGMENTS`] points.
    pub fn add_hole(&mut self, x: f64, y: f64, diameter: f64) {
        if diameter <= 0.0 {
            return;
        }
        let radius = diameter / 2.0;
        self.openings.push(
            (0..HOLE_SEGMENTS)
                .map(|n| {
                    let angle = std::f64::consts::TAU * (n as f64) / (HOLE_SEGMENTS as f64);
                    [x + radius * angle.cos(), y + radius * angle.sin()]
                })
                .collect(),
        );
    }
}

/// Builds one [`ToolTrace`] per tool block of **one step**, in plan order.
///
/// Blocks arrive already phase-ordered and TSP-ordered, so this only has to walk them.
/// The tool's park position between blocks is not modelled: a trace starts at its first
/// entry, because where the spindle sits during a tool change is the machine's business
/// and drawing a line to it would imply a cutting move that never happens.
///
/// Per step because a step is a physical setup: two steps may be cut on different
/// machines, in different fixtures, with the board turned over between them. Drawing
/// their toolpaths in one scene would compose motions that never coexist.
pub fn trace_step(step: &StepPlan) -> Vec<ToolTrace> {
    step.blocks.iter().enumerate().map(|(index, block)| trace_block(block, index)).collect()
}

/// Walks one block's ops into runs of motion.
///
/// `index` is the block's place in the **step**, which is what picks the colour. Taken as
/// an argument rather than derived later so the colour is fixed to the tool at the moment
/// the trace is built, before anything downstream can filter the list.
fn trace_block(block: &ToolBlock, index: usize) -> ToolTrace {
    let mut moves: Vec<Polyline> = Vec::new();
    // Where the tool is now. `None` until the first op — there is no line to draw from
    // an unknown position.
    let mut at: Option<ScenePoint> = None;

    for op in &block.ops {
        let expanded = expand_op(op.kind.clone(), op.entry, op.exit, op.z, block.diameter);
        for run in expanded {
            // Join the previous run's end to this one's start with a rapid, so the
            // transit between features is visible as the travel the TSP minimised.
            if let (Some(from), Some(&first)) = (at, run.points.first()) {
                if from != first {
                    push_run(&mut moves, Polyline { kind: MoveKind::Rapid, points: vec![from, first] });
                }
            }
            at = run.points.last().copied();
            push_run(&mut moves, run);
        }
    }

    ToolTrace {
        tool_id: block.tool_id.clone(),
        colour: tool_colour(index),
        slot: block.slot,
        diameter_mm: block.diameter.as_mm(),
        change_at: moves.first().and_then(|run| run.points.first().copied()),
        moves,
    }
}

/// Appends a run, merging it into the previous one when they are the same kind and meet.
///
/// Without this a spiral becomes hundreds of two-point runs and the renderer pays for
/// every one of them. Merging is not cosmetic: it is the difference between a handful of
/// line objects per tool and thousands.
fn push_run(moves: &mut Vec<Polyline>, run: Polyline) {
    if run.points.len() < 2 {
        return;
    }
    if let Some(last) = moves.last_mut() {
        if last.kind == run.kind && last.points.last() == run.points.first() {
            last.points.extend(run.points.into_iter().skip(1));
            return;
        }
    }
    moves.push(run);
}

/// One op's motion, as runs.
fn expand_op(
    kind: OpKind,
    entry: Point,
    exit: Point,
    z: ZProfile,
    tool_diameter: Length,
) -> Vec<Polyline> {
    match kind {
        // A point drill's cycle is inside the primitive, so it is reconstructed: arrive
        // at the retract plane, feed down, come back up.
        OpKind::Drill => {
            let top = ScenePoint::at(entry, z.z_retract);
            let bottom = ScenePoint::at(entry, z.z_bottom);
            vec![
                Polyline { kind: MoveKind::Feed, points: vec![top, bottom] },
                Polyline { kind: MoveKind::Rapid, points: vec![bottom, top] },
            ]
        }
        OpKind::RouteHole { hole_diameter } => runs_from_moves(&routing::spiral_hole(
            entry,
            z.z_retract,
            z.z_bottom,
            hole_diameter,
            tool_diameter,
        )),
        OpKind::RouteSlot { width, from_solid } => runs_from_moves(&routing::slot_route(
            entry,
            exit,
            z.z_retract,
            z.z_bottom,
            width,
            tool_diameter,
            from_solid,
        )),
        // A contour span carries its own path; the drop-in and lift-off around it are
        // what make the retaining tabs visible as gaps.
        OpKind::RouteContour { path } => {
            let mut moves = Vec::with_capacity(path.len() + 2);
            if let Some(&first) = path.first() {
                moves.push(RouteMove::Rapid { x: first.x, y: first.y, z: z.z_retract });
                moves.push(RouteMove::Plunge { x: first.x, y: first.y, z: z.z_bottom });
            }
            moves.extend(
                path.iter().skip(1).map(|p| RouteMove::Cut { x: p.x, y: p.y, z: z.z_bottom }),
            );
            if let Some(&last) = path.last() {
                moves.push(RouteMove::Rapid { x: last.x, y: last.y, z: z.z_retract });
            }
            runs_from_moves(&moves)
        }
    }
}

/// Turns a routing move list into runs, tessellating arcs and splitting on kind changes.
fn runs_from_moves(moves: &[RouteMove]) -> Vec<Polyline> {
    let mut runs: Vec<Polyline> = Vec::new();
    let mut here: Option<ScenePoint> = None;

    for mv in moves {
        let (kind, points) = match *mv {
            RouteMove::Rapid { x, y, z } => {
                (MoveKind::Rapid, vec![ScenePoint::new(x.as_mm(), y.as_mm(), z.as_mm())])
            }
            // A plunge and a lateral cut differ only in feed rate, which the picture does
            // not show — both are the tool in the material.
            RouteMove::Plunge { x, y, z } | RouteMove::Cut { x, y, z } => {
                (MoveKind::Feed, vec![ScenePoint::new(x.as_mm(), y.as_mm(), z.as_mm())])
            }
            RouteMove::Arc { x, y, i, j, ccw } => {
                let Some(from) = here else { continue };
                (MoveKind::Feed, arc_points(from, (x.as_mm(), y.as_mm()), (i.as_mm(), j.as_mm()), ccw))
            }
        };

        for point in points {
            match runs.last_mut() {
                Some(run) if run.kind == kind => run.points.push(point),
                _ => {
                    // Start the new run at the current position, or the line from the
                    // previous run would be missing.
                    let mut fresh = Vec::with_capacity(2);
                    if let Some(from) = here {
                        fresh.push(from);
                    }
                    fresh.push(point);
                    runs.push(Polyline { kind, points: fresh });
                }
            }
            here = Some(point);
        }
    }

    runs.retain(|run| run.points.len() >= 2);
    runs
}

/// Tessellates one arc into points, excluding its start (the caller is already there).
///
/// `i`/`j` are the centre offset from the **start**, as GCode defines them. A full circle
/// is the case where the end equals the start, which is why the sweep is forced to a full
/// turn rather than collapsing to zero.
fn arc_points(from: ScenePoint, end: (f64, f64), centre_offset: (f64, f64), ccw: bool) -> Vec<ScenePoint> {
    let centre = (from.x + centre_offset.0, from.y + centre_offset.1);
    let radius = (centre_offset.0.powi(2) + centre_offset.1.powi(2)).sqrt();
    if radius <= f64::EPSILON {
        return vec![ScenePoint::new(end.0, end.1, from.z)];
    }

    let start_angle = (from.y - centre.1).atan2(from.x - centre.0);
    let end_angle = (end.1 - centre.1).atan2(end.0 - centre.0);
    let full = std::f64::consts::TAU;
    let mut sweep = if ccw { end_angle - start_angle } else { start_angle - end_angle };
    sweep = sweep.rem_euclid(full);
    if sweep <= f64::EPSILON {
        sweep = full; // end == start: a full circle, not a zero-length arc
    }

    // Segments from the chord tolerance: the sagitta of a chord subtending θ on radius r
    // is r(1 − cos(θ/2)), so solving for θ gives the coarsest step within tolerance.
    let step = if ARC_CHORD_MM < radius {
        2.0 * (1.0 - ARC_CHORD_MM / radius).acos()
    } else {
        full
    };
    let segments = (sweep / step.max(f64::EPSILON)).ceil() as usize;
    let segments = segments.clamp(ARC_MIN_SEGMENTS, ARC_MAX_SEGMENTS);

    let direction = if ccw { 1.0 } else { -1.0 };
    (1..=segments)
        .map(|n| {
            let angle = start_angle + direction * sweep * (n as f64) / (segments as f64);
            ScenePoint::new(
                centre.0 + radius * angle.cos(),
                centre.1 + radius * angle.sin(),
                from.z,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcode::plan::{AtomicOp, Phase, StepPlan};

    fn pt(x: f64, y: f64) -> Point {
        Point::new(Length::from_mm(x), Length::from_mm(y))
    }

    fn z() -> ZProfile {
        ZProfile {
            z_bottom: Length::from_mm(-2.0),
            z_retract: Length::from_mm(2.0),
            z_feed: None,
        }
    }

    fn op(kind: OpKind, entry: Point, exit: Point) -> AtomicOp {
        AtomicOp {
            phase: Phase::Drill,
            kind,
            tool_id: "t1".into(),
            entry,
            exit,
            z: z(),
            primitive: "x",
            source: "s".into(),
        }
    }

    fn block(diameter: f64, ops: Vec<AtomicOp>) -> ToolBlock {
        ToolBlock {
            tool_id: "t1".into(),
            slot: Some(3),
            diameter: Length::from_mm(diameter),
            ops,
            travel_mm: 0.0,
        }
    }

    fn step_of(blocks: Vec<ToolBlock>) -> StepPlan {
        StepPlan { index: 0, name: "s".into(), blocks, notes: vec![] }
    }

    /// A named block, so a step can hold several distinguishable tools.
    fn named_block(tool_id: &str, diameter: f64) -> ToolBlock {
        ToolBlock {
            tool_id: tool_id.into(),
            ..block(diameter, vec![op(OpKind::Drill, pt(5.0, 5.0), pt(5.0, 5.0))])
        }
    }

    /// Tools take palette colours in block order, and the palette wraps rather than
    /// running out — a step with nine tools must still colour the ninth.
    #[test]
    fn tools_are_coloured_in_block_order_and_the_palette_wraps() {
        let blocks: Vec<ToolBlock> =
            (0..9).map(|n| named_block(&format!("t{n}"), 1.0)).collect();
        let traces = trace_step(&step_of(blocks));

        for (index, trace) in traces.iter().enumerate() {
            assert_eq!(trace.colour, TOOL_PALETTE[index % TOOL_PALETTE.len()], "trace {index}");
        }
        assert_eq!(traces[8].colour, traces[0].colour, "the ninth tool reuses the first colour");
    }

    /// **The reason the colour lives on the trace at all.**
    ///
    /// The 3D view lets a tool be switched off, which drops its trace from the payload.
    /// While the renderer chose colours by position in that list, hiding one tool
    /// silently recoloured every tool after it — and the legend beside the canvas, which
    /// names the colours, would have been describing the previous arrangement. Nothing
    /// about that is visible in a screenshot unless you already know what colour a tool
    /// was before you hid another.
    #[test]
    fn hiding_a_tool_does_not_recolour_the_others() {
        let traces = trace_step(&step_of(vec![
            named_block("drill", 0.8),
            named_block("router", 1.0),
            named_block("vbit", 0.2),
        ]));
        let before: Vec<u32> = traces.iter().map(|t| t.colour).collect();

        // What the payload builder does when the first tool is unticked.
        let shown: Vec<&ToolTrace> = traces.iter().filter(|t| t.tool_id != "drill").collect();

        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].colour, before[1], "the router keeps the colour it had");
        assert_eq!(shown[1].colour, before[2], "and so does the v-bit");
    }

    /// The colour is a property of the trace, so it survives being serialised to the
    /// renderer — which is the only path it travels.
    #[test]
    fn the_colour_reaches_the_renderer_payload() {
        let traces = trace_step(&step_of(vec![named_block("drill", 0.8)]));
        let json = serde_json::to_string(&traces).expect("traces serialise");
        assert!(
            json.contains(&format!("\"colour\":{}", TOOL_PALETTE[0])),
            "the first tool's colour must be in the payload:\n{json}"
        );
    }

    /// A drill op is a point in the plan — the cycle is inside the primitive — so the
    /// down-and-up has to be reconstructed, or the 3D view would show nothing happening
    /// in Z at all.
    #[test]
    fn a_point_drill_becomes_a_visible_plunge_and_retract() {
        let traces = trace_step(&step_of(vec![block(1.0, vec![op(OpKind::Drill, pt(5.0, 5.0), pt(5.0, 5.0))])]));
        assert_eq!(traces.len(), 1);
        let moves = &traces[0].moves;

        assert_eq!(moves.len(), 2, "one feed down, one rapid up: {moves:?}");
        assert_eq!(moves[0].kind, MoveKind::Feed);
        assert_eq!(moves[0].points, vec![ScenePoint::new(5.0, 5.0, 2.0), ScenePoint::new(5.0, 5.0, -2.0)]);
        assert_eq!(moves[1].kind, MoveKind::Rapid);
        assert_eq!(moves[1].points.last(), Some(&ScenePoint::new(5.0, 5.0, 2.0)));
    }

    /// The transit between two features is drawn as a rapid at the retract plane — the
    /// travel the TSP spent its effort minimising, and the thing an operator watches to
    /// judge whether the ordering is sane.
    #[test]
    fn the_transit_between_ops_is_a_rapid_at_the_retract_plane() {
        let traces = trace_step(&step_of(vec![block(
            1.0,
            vec![
                op(OpKind::Drill, pt(0.0, 0.0), pt(0.0, 0.0)),
                op(OpKind::Drill, pt(10.0, 0.0), pt(10.0, 0.0)),
            ],
        )]));
        let moves = &traces[0].moves;
        // down, up+across (merged: both rapid and they meet), down, up.
        let across = moves
            .iter()
            .find(|m| m.kind == MoveKind::Rapid && m.points.iter().any(|p| p.x == 10.0))
            .expect("a rapid reaches the second hole");
        assert!(
            across.points.iter().all(|p| p.z == 2.0 || p.z == -2.0),
            "the transit stays at the retract plane, never through the board"
        );
    }

    /// Consecutive runs of the same kind that meet are merged. A spiral is hundreds of
    /// moves; left unmerged it would be hundreds of line objects for one hole.
    #[test]
    fn runs_of_the_same_kind_are_merged_into_one_polyline() {
        let traces = trace_step(&step_of(vec![block(
            1.0,
            vec![op(
                OpKind::RouteHole { hole_diameter: Length::from_mm(3.2) },
                pt(0.0, 0.0),
                pt(0.0, 0.0),
            )],
        )]));
        let moves = &traces[0].moves;
        assert!(
            moves.len() <= 4,
            "a spiral should be a couple of long runs, got {} — {:?}",
            moves.len(),
            moves.iter().map(|m| (m.kind, m.points.len())).collect::<Vec<_>>()
        );
        assert!(traces[0].point_count() > 50, "and it should actually be tessellated");
    }

    /// Every point of a routed hole stays inside the finished wall, which is the same
    /// invariant `routing` guarantees — so the picture cannot show the tool somewhere
    /// the program will not send it.
    #[test]
    fn a_routed_hole_is_drawn_inside_its_own_wall() {
        let traces = trace_step(&step_of(vec![block(
            1.0,
            vec![op(
                OpKind::RouteHole { hole_diameter: Length::from_mm(3.2) },
                pt(10.0, 20.0),
                pt(10.0, 20.0),
            )],
        )]));
        let reach = (3.2 - 1.0) / 2.0;
        for run in &traces[0].moves {
            for p in &run.points {
                let d = ((p.x - 10.0).powi(2) + (p.y - 20.0).powi(2)).sqrt();
                assert!(d <= reach + 1e-6, "({}, {}) is {d} from centre, past {reach}", p.x, p.y);
            }
        }
    }

    /// A full circle is `end == start` with an I/J offset. Treating its sweep as zero
    /// would drop the whole circle, which is most of what a spiral is.
    #[test]
    fn a_full_circle_arc_tessellates_all_the_way_round() {
        let from = ScenePoint::new(1.0, 0.0, 0.0);
        let points = arc_points(from, (1.0, 0.0), (-1.0, 0.0), true);
        assert!(points.len() >= ARC_MIN_SEGMENTS);
        // Every point is on the unit circle about the origin...
        for p in &points {
            assert!(((p.x * p.x + p.y * p.y).sqrt() - 1.0).abs() < 1e-6);
        }
        // ...and it comes back to where it started.
        let last = points[points.len() - 1];
        assert!((last.x - 1.0).abs() < 1e-6 && last.y.abs() < 1e-6, "closes the circle: {last:?}");
        // A quarter of the way round is a quarter turn, i.e. it went the CCW way.
        let quarter = points[points.len() / 4];
        assert!(quarter.y > 0.0, "counter-clockwise: {quarter:?}");
    }

    /// Chords sit within the tolerance, so a hole is round rather than visibly faceted.
    #[test]
    fn arc_tessellation_stays_within_the_chord_tolerance() {
        let from = ScenePoint::new(20.0, 0.0, 0.0);
        let points = arc_points(from, (20.0, 0.0), (-20.0, 0.0), true);
        let mut previous = from;
        for p in &points {
            let chord = ((p.x - previous.x).powi(2) + (p.y - previous.y).powi(2)).sqrt();
            // Sagitta of a chord c on radius r is r − √(r² − (c/2)²).
            let sagitta = 20.0 - (400.0 - (chord / 2.0).powi(2)).sqrt();
            assert!(sagitta <= ARC_CHORD_MM + 1e-9, "chord {chord} sags {sagitta}");
            previous = *p;
        }
    }

    /// One trace per tool block, carrying the slot and diameter the rack shows, so the
    /// 3D legend and the Tooling tab cannot disagree.
    #[test]
    fn each_tool_block_becomes_one_trace_with_its_rack_identity() {
        let traces = trace_step(&step_of(vec![
            block(0.8, vec![op(OpKind::Drill, pt(0.0, 0.0), pt(0.0, 0.0))]),
            ToolBlock {
                tool_id: "t2".into(),
                slot: None,
                diameter: Length::from_mm(2.0),
                ops: vec![op(OpKind::Drill, pt(1.0, 1.0), pt(1.0, 1.0))],
                travel_mm: 0.0,
            },
        ]));
        assert_eq!(traces.len(), 2);
        assert_eq!((traces[0].slot, traces[0].diameter_mm), (Some(3), 0.8));
        assert_eq!((traces[1].slot, traces[1].diameter_mm), (None, 2.0));
        assert_eq!(
            traces[0].change_at,
            Some(ScenePoint::new(0.0, 0.0, 2.0)),
            "the change is marked where the tool first arrives"
        );
    }

    /// An empty block yields an empty trace rather than being dropped — the rack still
    /// loaded that tool, and a legend entry with no path is informative.
    #[test]
    fn a_block_with_no_ops_yields_an_empty_trace() {
        let traces = trace_step(&step_of(vec![block(1.0, vec![])]));
        assert_eq!(traces.len(), 1);
        assert!(traces[0].moves.is_empty());
        assert_eq!(traces[0].change_at, None);
    }
}

