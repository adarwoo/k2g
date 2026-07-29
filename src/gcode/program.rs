//! Program assembly — turns a step's [`StepPlan`](crate::gcode::plan) into the GCode
//! **body** that slots between the `initialise` header and `conclude` footer
//! (operation-planner.md §7).
//!
//! Walk order, per §7: for each tool block emit `change_tool` (slot, rpm, operator
//! prompt) then `start_spindle`, then the block's ops; a single `stop_spindle` closes
//! the step. Drilling uses the modal `drill` cycle (G81-style), which positions and
//! retracts itself, so no explicit rapid is emitted between holes — the block's ops
//! are already TSP-ordered so consecutive cycles minimise travel.
//!
//! Feeds & speeds resolve **here**, at render time (settled 2026-07-25): a tool's
//! rated feed is scaled with the spindle clamp ([`feeds`]), and a tool missing its
//! feed/speed is a hard [`BodyError`] — the caller turns it into a generation nogo.

use std::collections::BTreeMap;

use gtl::Scope;
use units::{FeedRate, RotationalSpeed};

use crate::gcode::coder::Coder;
use crate::gcode::feeds::{self, FeedsError, FeedsSpeeds, MachineLimits, Motion};
use crate::gcode::plan::{OpKind, StepPlan};
use crate::gcode::routing::{self, RouteMove};

/// The per-step rendering context the body needs beyond the plan: the CNC's operation
/// primitive templates, its spindle and axis limits (for the feed/speed solve), and
/// whether tool changes are automatic (ATC) — a manual machine gets an operator prompt
/// instead.
#[derive(Clone)]
pub struct StepRender {
    pub drill_tpl: String,
    pub change_tool_tpl: String,
    pub start_spindle_tpl: String,
    pub stop_spindle_tpl: String,
    /// Motion primitives used by routed holes (spiral pocket): rapid to position, feed
    /// cut (plunge + radial), and the arc for each full circle.
    pub rapid_move_tpl: String,
    pub linear_cut_tpl: String,
    pub cut_arc_tpl: String,
    pub limits: MachineLimits,
    pub is_atc: bool,
}

/// Everything one step needs to render a **complete, standalone program**: the
/// program-layer primitives and the values they read, taken from *this step's* CNC and
/// fixture, wrapped around the body context.
///
/// Per step because a step is an autonomous setup — it names its own CNC, and a CNC owns
/// the output templates. Two steps may legitimately emit different dialects; one could be
/// Excellon (op-planner §9.2). Held apart from [`StepRender`] rather than folded into it
/// so the body renderer's contract stays honest about what it actually reads.
#[derive(Clone)]
pub struct ProgramRender {
    pub cnc_name: String,
    pub initialise_tpl: String,
    pub conclude_tpl: String,
    /// The CNC's `line_number` primitive. Empty disables numbering.
    pub line_number_tpl: String,
    /// This step's fixture's safe travel height — the header and footer retract to it.
    pub z_safe: units::Length,
    /// Which of the machine's stored zeros this step's fixture sits in, as an ordinal.
    pub work_coordinate_system: u8,
    /// The extension a saved program takes, from the CNC profile — so an Excellon step
    /// is not written as `.nc`.
    pub file_extension: String,
    pub body: StepRender,
}

/// Renders one step into a finished program: `initialise`, the step's body, `conclude`,
/// then this CNC's line numbering over the whole thing.
///
/// **Builds its own [`Coder`].** The Coder deliberately carries the modal unit state that
/// `initialise` establishes (`metric()` and friends) into every later primitive — that is
/// what lets a body emit bare lengths. Sharing one across steps therefore leaks step 1's
/// unit mode into step 2, which is precisely the cross-contamination the per-step model
/// exists to prevent. One program, one Coder.
///
/// Numbering runs here rather than over an assembled job for the same reason: each step is
/// a whole program, so each restarts its `N` sequence at the top.
pub fn render_step_program(
    step: &StepPlan,
    render: &ProgramRender,
    pcb_filename: &str,
    timestamp: &str,
    tool_feeds: &BTreeMap<String, ToolFeed>,
) -> Result<String, BodyError> {
    let coder = Coder::new();

    // `initialise` and `conclude` are program-layer primitives and see the same values: a
    // footer typically retracts to `z_safe`, and either may echo the source file. A fresh
    // scope for each, but the Coder's unit mode persists between them.
    let program_scope = || {
        let mut scope = Scope::new();
        scope.push("pcb_filename", pcb_filename.to_string());
        scope.push("timestamp", timestamp.to_string());
        scope.push("z_safe", render.z_safe);
        scope.push("work_coordinate_system", render.work_coordinate_system as i64);
        scope
    };

    let mut scope = program_scope();
    let header = coder
        .render("initialise", &render.initialise_tpl, &mut scope)
        .map_err(|err| BodyError::Render { primitive: "initialise".into(), message: err.to_string() })?;

    let body = render_step_body(&coder, step, &render.body, tool_feeds)?;

    let mut scope = program_scope();
    let footer = coder
        .render("conclude", &render.conclude_tpl, &mut scope)
        .map_err(|err| BodyError::Render { primitive: "conclude".into(), message: err.to_string() })?;

    // Joined with single newlines (trailing ones trimmed) so an empty or multi-line body
    // never introduces stray blank lines.
    let assembled = [header, body, footer]
        .iter()
        .map(|section| section.trim_end_matches('\n'))
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    number_lines(&coder, &assembled, &render.line_number_tpl)
}

/// A stock tool's identity + rated running values, looked up by tool id when a block
/// is rendered. The rated pair is required — a `None` becomes a [`BodyError::Feeds`].
#[derive(Clone)]
pub struct ToolFeed {
    pub name: String,
    pub feed: Option<FeedRate>,
    pub speed: Option<RotationalSpeed>,
}

/// Why a body could not be rendered.
#[derive(Clone, Debug, PartialEq)]
pub enum BodyError {
    /// A block's tool has no usable feed/speed (the run cannot emit `F`/`S`).
    Feeds { tool: String, message: String },
    /// A primitive template failed to compile/run.
    Render { primitive: String, message: String },
}

impl BodyError {
    /// A single-line, operator-facing message for the generation failure path.
    pub fn message(&self) -> String {
        match self {
            BodyError::Feeds { tool, message } => {
                format!("tool '{tool}' {message} — set it in Stock before generating")
            }
            BodyError::Render { primitive, message } => {
                format!("primitive '{primitive}': {message}")
            }
        }
    }
}

/// Renders one step's body. `coder` carries the engine's modal unit state from the
/// already-rendered `initialise`, so lengths/feeds format in the program's units.
pub fn render_step_body(
    coder: &Coder,
    step: &StepPlan,
    render: &StepRender,
    tool_feeds: &BTreeMap<String, ToolFeed>,
) -> Result<String, BodyError> {
    let mut out = String::new();

    for block in &step.blocks {
        let (name, feed, speed) = match tool_feeds.get(&block.tool_id) {
            Some(tf) => (tf.name.clone(), tf.feed, tf.speed),
            None => (block.tool_id.clone(), None, None),
        };
        // The block commands its spindle speed once, so the solve has to account for every
        // move it will make: a block that drills is bound by Z alone, while anything that
        // routes feeds laterally too. `any` rather than `all` because the binding
        // constraint is the most restrictive one present.
        let motion = if block.ops.iter().any(|op| !matches!(op.kind, OpKind::Drill)) {
            Motion::Routing
        } else {
            Motion::Drilling
        };
        let fs = feeds::resolve(feed, speed, render.limits, motion)
            .map_err(|e| BodyError::Feeds { tool: name.clone(), message: feeds_error_text(e) })?;
        let slot = block.slot.unwrap_or(0) as i64;

        // Tool-block boundary: change_tool then start_spindle (§7). change_tool leads
        // with M05, so it also stops the previous block's spindle.
        let manual_message = if render.is_atc {
            String::new()
        } else {
            format!("(load tool T{slot}: {name})")
        };
        let mut scope = Scope::new();
        scope.push("manual_message", manual_message);
        scope.push("slot", slot);
        scope.push("rpm", fs.rpm);
        out.push_str(&render_one(coder, "change_tool", &render.change_tool_tpl, &mut scope)?);

        let mut scope = Scope::new();
        scope.push("rpm", fs.rpm);
        out.push_str(&render_one(coder, "start_spindle", &render.start_spindle_tpl, &mut scope)?);

        // The block's ops: a point drill is one self-positioning cycle; a routed hole
        // expands into a spiral pocket (rapid → plunge → circles).
        for op in &block.ops {
            match op.kind {
                OpKind::Drill => {
                    let mut scope = Scope::new();
                    scope.push("x", op.entry.x);
                    scope.push("y", op.entry.y);
                    scope.push("z_bottom", op.z.z_bottom);
                    scope.push("z_retract", op.z.z_retract);
                    scope.push("z_feedrate", fs.feed);
                    out.push_str(&render_one(coder, op.primitive, &render.drill_tpl, &mut scope)?);
                }
                OpKind::RouteHole { hole_diameter } => {
                    let moves = routing::spiral_hole(
                        op.entry,
                        op.z.z_retract,
                        op.z.z_bottom,
                        hole_diameter,
                        block.diameter,
                    );
                    for mv in &moves {
                        out.push_str(&render_route_move(coder, render, mv, fs)?);
                    }
                }
                OpKind::RouteContour { ref path } => {
                    // The span carries its own geometry: drop in at its start, feed
                    // through it, lift off at its end. The gap to the next span is the
                    // retaining tab, so the retract between them is not optional.
                    let mut moves = Vec::with_capacity(path.len() + 2);
                    moves.push(RouteMove::Rapid {
                        x: path[0].x,
                        y: path[0].y,
                        z: op.z.z_retract,
                    });
                    moves.push(RouteMove::Plunge {
                        x: path[0].x,
                        y: path[0].y,
                        z: op.z.z_bottom,
                    });
                    moves.extend(path[1..].iter().map(|p| RouteMove::Cut {
                        x: p.x,
                        y: p.y,
                        z: op.z.z_bottom,
                    }));
                    let last = path[path.len() - 1];
                    moves.push(RouteMove::Rapid { x: last.x, y: last.y, z: op.z.z_retract });
                    for mv in &moves {
                        out.push_str(&render_route_move(coder, render, mv, fs)?);
                    }
                }
                OpKind::RouteSlot { width, from_solid } => {
                    // entry/exit are the slot's medial-axis end centres (see `OpKind`).
                    let moves = routing::slot_route(
                        op.entry,
                        op.exit,
                        op.z.z_retract,
                        op.z.z_bottom,
                        width,
                        block.diameter,
                        from_solid,
                    );
                    for mv in &moves {
                        out.push_str(&render_route_move(coder, render, mv, fs)?);
                    }
                }
            }
        }
    }

    // One spindle stop closes the step (the next block/step's change_tool re-stops it,
    // harmlessly). Skipped when the step drilled nothing.
    if !step.blocks.is_empty() {
        let mut scope = Scope::new();
        out.push_str(&render_one(coder, "stop_spindle", &render.stop_spindle_tpl, &mut scope)?);
    }

    Ok(out)
}

/// Renders one move of a routing toolpath through the matching motion primitive:
/// `Rapid`→`rapid_move`, `Plunge`/`Cut`→`linear_cut`, `Arc`→`cut_arc`. Geometry is
/// already in machine coordinates — the Coder only formats it (op-planner §6).
///
/// **Each feed move carries its own `F`.** A `G1` with no feed word runs at whatever is
/// modal — after a drill block that is the *drill's* plunge feed, which is not a feed a
/// router should see. `Plunge` is derated by [`routing::PLUNGE_FEED_FRACTION`], since a
/// tool's one rated feed is its lateral feed.
fn render_route_move(
    coder: &Coder,
    render: &StepRender,
    mv: &RouteMove,
    fs: FeedsSpeeds,
) -> Result<String, BodyError> {
    /// `linear_cut` for a feed move, at the given feed.
    fn linear(
        coder: &Coder,
        render: &StepRender,
        (x, y, z): (units::Length, units::Length, units::Length),
        feed: FeedRate,
        fs: FeedsSpeeds,
    ) -> Result<String, BodyError> {
        let mut s = Scope::new();
        s.push("x", x);
        s.push("y", y);
        s.push("z", z);
        s.push("feedrate", feed);
        // Legacy: templates that restate the spindle speed on the cut line.
        s.push("s", fs.rpm);
        render_one(coder, "linear_cut", &render.linear_cut_tpl, &mut s)
    }

    match *mv {
        RouteMove::Rapid { x, y, z } => {
            let mut s = Scope::new();
            s.push("x", x);
            s.push("y", y);
            s.push("z", z);
            render_one(coder, "rapid_move", &render.rapid_move_tpl, &mut s)
        }
        RouteMove::Plunge { x, y, z } => {
            let plunge = FeedRate::from_mm_per_min(
                fs.feed.as_mm_per_min() * routing::PLUNGE_FEED_FRACTION,
            );
            linear(coder, render, (x, y, z), plunge, fs)
        }
        RouteMove::Cut { x, y, z } => linear(coder, render, (x, y, z), fs.feed, fs),
        RouteMove::Arc { x, y, i, j, ccw } => {
            let mut s = Scope::new();
            // The direction, not the word for it: which G-code names a clockwise arc is
            // the profile's business, so the template branches on this boolean.
            s.push("clockwise", !ccw);
            s.push("x", x);
            s.push("y", y);
            s.push("i", i);
            s.push("j", j);
            s.push("xy_feedrate", fs.feed);
            render_one(coder, "cut_arc", &render.cut_arc_tpl, &mut s)
        }
    }
}

/// Renders one primitive, tagging any engine error with the primitive name.
fn render_one(coder: &Coder, name: &str, tpl: &str, scope: &mut Scope) -> Result<String, BodyError> {
    coder
        .render(name, tpl, scope)
        .map_err(|e| BodyError::Render { primitive: name.to_string(), message: e.to_string() })
}

/// A human phrase for a feeds/speeds failure, completing "tool '<name>' …".
fn feeds_error_text(error: FeedsError) -> String {
    match error {
        FeedsError::MissingFeed => "has no feed rate".to_string(),
        FeedsError::MissingSpeed => "has no spindle speed".to_string(),
        FeedsError::NonPositiveSpeed => "has a spindle speed of zero".to_string(),
    }
}

/// Prefixes every non-blank line of the assembled program with the CNC's `line_number`
/// primitive, rendered with `line` = 1, 2, 3 … and `text` = the line about to be numbered.
///
/// Line numbering is a whole-program concern — no primitive can know its own position —
/// so it runs once here, over the finished program, rather than inside the templates that
/// built it. The **format** is still entirely the profile's: the template decides the
/// word, the increment (`{line * 10}`) and the separator, and ends with a backtick so no
/// newline is emitted between the prefix and the line it numbers.
///
/// `text` is what makes the *decision* the profile's too, not just the format: a template
/// that emits nothing for a line leaves it unnumbered, so a controller that should not see
/// `N` words on its comments says so in its own template rather than asking for a rule
/// here. The line is still emitted by this function either way — the primitive contributes
/// a prefix, never the line itself.
///
/// An empty template disables numbering and the program is returned unchanged — what
/// `line_numbering_increment: 0` used to mean. Blank lines are dropped when numbering, so
/// the numbered program is contiguous.
///
/// A template that fails to render is a [`BodyError::Render`]: silently shipping an
/// unnumbered program to a controller that requires line numbers would be worse than
/// stopping.
pub fn number_lines(coder: &Coder, program: &str, template: &str) -> Result<String, BodyError> {
    if template.trim().is_empty() {
        return Ok(program.to_string());
    }
    let mut out: Vec<String> = Vec::new();
    for (index, line) in program.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let mut scope = Scope::new();
        scope.push("line", index as i64 + 1);
        scope.push("text", line.to_string());
        let prefix = render_one(coder, "line_number", template, &mut scope)?;
        out.push(format!("{prefix}{line}"));
    }
    Ok(out.join("\n"))
}

/// Representative CNC operation primitives, as **test input** for the renderer.
///
/// GCode belongs to the CNC profile, never to generation logic — in production these
/// templates come from the profile via `build_step_render_ctx`. A renderer unit test
/// still needs *something* to render, so the sample lives here in one clearly-labelled
/// place (shared by this module's and the generation worker's tests) rather than being
/// hand-written inline in each test.
#[cfg(test)]
pub(crate) fn sample_step_render(is_atc: bool) -> StepRender {
    StepRender {
        drill_tpl: "`G81 X{x} Y{y} Z{z_bottom} R{z_retract} F{z_feedrate}".to_string(),
        change_tool_tpl: "`{manual_message}\n`T{slot} M06".to_string(),
        start_spindle_tpl: "`S{rpm}\n`M03".to_string(),
        stop_spindle_tpl: "`M05".to_string(),
        rapid_move_tpl: "`G0 X{x} Y{y} Z{z}".to_string(),
        linear_cut_tpl: "`G1 X{x} Y{y} Z{z} F{feedrate}".to_string(),
        cut_arc_tpl: r#"`{if clockwise { "G2" } else { "G3" }} X{x} Y{y} I{i} J{j} F{xy_feedrate}"#
            .to_string(),
        limits: MachineLimits {
            spindle: feeds::SpindleRange::new(
                RotationalSpeed::from_rpm(5_000.0),
                RotationalSpeed::from_rpm(24_000.0),
            ),
            // High enough not to bind, so a test that means to exercise the spindle clamp
            // is not silently also exercising the axis ceiling. The tests that care about
            // the ceiling set it themselves.
            max_feed_xy: FeedRate::from_mm_per_min(1e9),
            max_feed_z: FeedRate::from_mm_per_min(1e9),
        },
        is_atc,
    }
}

/// Sample program-layer primitives (the CNC's `initialise`/`conclude`) for tests, so
/// no generation test hand-writes GCode. Real profiles supply these; see
/// [`sample_step_render`].
#[cfg(test)]
pub(crate) fn sample_initialise_tpl() -> String {
    "`(k2g {pcb_filename} - {timestamp})\nmetric();\n`G0 Z{z_safe}".to_string()
}

/// See [`sample_initialise_tpl`].
#[cfg(test)]
pub(crate) fn sample_conclude_tpl() -> String {
    "`(end of file)".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcode::plan::{AtomicOp, OpKind, Phase, Point, ToolBlock, ZProfile};
    use units::Length;

    fn render_ctx(is_atc: bool) -> StepRender {
        sample_step_render(is_atc)
    }

    fn drill_op(x: f64, y: f64) -> AtomicOp {
        AtomicOp {
            phase: Phase::Drill,
            kind: OpKind::Drill,
            tool_id: "t1".to_string(),
            entry: Point::new(Length::from_mm(x), Length::from_mm(y)),
            exit: Point::new(Length::from_mm(x), Length::from_mm(y)),
            z: ZProfile {
                z_bottom: Length::from_mm(-2.4),
                z_retract: Length::from_mm(5.0),
                z_feed: None,
            },
            primitive: "drill",
            source: "h1".to_string(),
        }
    }

    fn one_block_step() -> StepPlan {
        StepPlan {
            index: 0,
            name: "Step 1".to_string(),
            blocks: vec![ToolBlock {
                tool_id: "t1".to_string(),
                slot: Some(1),
                diameter: Length::from_mm(1.0),
                ops: vec![drill_op(3.0, 4.0), drill_op(10.0, 4.0)],
                travel_mm: 7.0,
            }],
            notes: vec![],
        }
    }

    fn feeds_for(feed: Option<f64>, speed: Option<f64>) -> BTreeMap<String, ToolFeed> {
        let mut m = BTreeMap::new();
        m.insert(
            "t1".to_string(),
            ToolFeed {
                name: "1.0mm drill".to_string(),
                feed: feed.map(FeedRate::from_mm_per_min),
                speed: speed.map(RotationalSpeed::from_rpm),
            },
        );
        m
    }

    #[test]
    fn renders_tool_change_spindle_and_a_drill_per_hole() {
        let coder = Coder::new();
        let body = render_step_body(
            &coder,
            &one_block_step(),
            &render_ctx(true),
            &feeds_for(Some(600.0), Some(12_000.0)),
        )
        .expect("body renders");

        assert!(body.contains("T1 M06"), "tool change on the block's slot");
        assert!(body.contains("M03"), "spindle started");
        assert!(body.contains("S12000"), "rated rpm is within range → passed through");
        // Two ordered drill cycles, with the negative machine-Z depth and R plane.
        assert!(body.contains("G81 X3 Y4 Z-2.4 R5 F600"), "first hole:\n{body}");
        assert!(body.contains("G81 X10 Y4 Z-2.4 R5 F600"), "second hole:\n{body}");
        assert!(body.trim_end().ends_with("M05"), "step ends with a spindle stop");
    }

    #[test]
    fn feed_and_rpm_scale_together_when_the_spindle_clamps() {
        // Rated 100000 rpm @ 10000 mm/min on a 24000-rpm ceiling → 24000 rpm, and the
        // feed scaled by 24000/100000 = 2400 mm/min (chip-load preserved).
        let coder = Coder::new();
        let body = render_step_body(
            &coder,
            &one_block_step(),
            &render_ctx(true),
            &feeds_for(Some(10_000.0), Some(100_000.0)),
        )
        .expect("body renders");
        assert!(body.contains("S24000"), "rpm clamped to the ceiling:\n{body}");
        assert!(body.contains("F2400"), "feed scaled by the same ratio:\n{body}");
    }

    #[test]
    fn a_manual_machine_emits_an_operator_prompt() {
        let coder = Coder::new();
        let body = render_step_body(
            &coder,
            &one_block_step(),
            &render_ctx(false),
            &feeds_for(Some(600.0), Some(12_000.0)),
        )
        .unwrap();
        assert!(body.contains("(load tool T1: 1.0mm drill)"), "manual prompt present:\n{body}");
    }

    #[test]
    fn a_routed_hole_expands_into_a_center_plunge_and_spiral_arcs() {
        let coder = Coder::new();
        // A 3.2mm hole milled with a 1.0mm router at (5,5).
        let step = StepPlan {
            index: 0,
            name: "Route".to_string(),
            blocks: vec![ToolBlock {
                tool_id: "r1".to_string(),
                slot: Some(2),
                diameter: Length::from_mm(1.0),
                ops: vec![AtomicOp {
                    phase: Phase::Route,
                    kind: OpKind::RouteHole { hole_diameter: Length::from_mm(3.2) },
                    tool_id: "r1".to_string(),
                    entry: Point::new(Length::from_mm(5.0), Length::from_mm(5.0)),
                    exit: Point::new(Length::from_mm(5.0), Length::from_mm(5.0)),
                    z: ZProfile {
                        z_bottom: Length::from_mm(-2.1),
                        z_retract: Length::from_mm(5.0),
                        z_feed: None,
                    },
                    primitive: "route_hole",
                    source: "h1".to_string(),
                }],
                travel_mm: 0.0,
            }],
            notes: vec![],
        };
        let body = render_step_body(&coder, &step, &render_ctx(true), &router_feed()).expect("routes");

        assert!(body.contains("G0 X5 Y5 Z5"), "rapid to centre above the work:\n{body}");
        assert!(
            body.contains("G1 X5 Y5 Z-2.1 F200"),
            "plunge at the centre (no island), at a third of the 600 lateral feed:\n{body}"
        );
        // Spiral: a G3 full circle ending on the wall (reach = (3.2-1.0)/2 = 1.1 → X6.1).
        assert!(body.contains("G3 X6.1 Y5 I-1.1 J0 F600"), "finishing lap on the wall:\n{body}");
    }

    /// A 1 mm router in a 2 mm-wide slot lying along +X from (0,0) to (4,0): the axis
    /// pass clears the core, then one stadium lap finishes both walls and both ends.
    #[test]
    fn a_routed_slot_expands_into_an_axis_pass_and_a_stadium_lap() {
        let coder = Coder::new();
        let step = StepPlan {
            index: 0,
            name: "Route".to_string(),
            blocks: vec![ToolBlock {
                tool_id: "r1".to_string(),
                slot: Some(2),
                diameter: Length::from_mm(1.0),
                ops: vec![AtomicOp {
                    phase: Phase::Route,
                    kind: OpKind::RouteSlot { width: Length::from_mm(2.0), from_solid: true },
                    tool_id: "r1".to_string(),
                    entry: Point::new(Length::from_mm(0.0), Length::from_mm(0.0)),
                    exit: Point::new(Length::from_mm(4.0), Length::from_mm(0.0)),
                    z: ZProfile {
                        z_bottom: Length::from_mm(-2.1),
                        z_retract: Length::from_mm(5.0),
                        z_feed: None,
                    },
                    primitive: "route_slot",
                    source: "slot1".to_string(),
                }],
                travel_mm: 0.0,
            }],
            notes: vec![],
        };
        let body = render_step_body(&coder, &step, &render_ctx(true), &router_feed()).expect("routes");

        assert!(body.contains("G1 X0 Y0 Z-2.1 F200"), "plunge on the axis:\n{body}");
        assert!(body.contains("G1 X4 Y0 Z-2.1 F600"), "the axis pass clears the core:\n{body}");
        // Wall at (2.0 − 1.0)/2 = 0.5 either side; the caps are half circles about the
        // axis ends, so their I/J is ±0.5 in Y.
        assert!(body.contains("G3 X0 Y-0.5 I0 J-0.5 F600"), "cap about the (0,0) end:\n{body}");
        assert!(body.contains("G3 X4 Y0.5 I0 J0.5 F600"), "cap about the (4,0) end:\n{body}");
        assert!(body.contains("G0 X4 Y0.5 Z5"), "retracts from where the lap finished:\n{body}");
    }

    /// An outline span drops in at its start, feeds through every vertex, and lifts off
    /// at its end. The lift is what leaves the retaining tab uncut, so it is not optional.
    #[test]
    fn an_outline_span_is_cut_between_a_plunge_and_a_retract() {
        let coder = Coder::new();
        let path: Vec<Point> = [(1.0, 0.0), (10.0, 0.0), (10.0, 5.0)]
            .iter()
            .map(|&(x, y)| Point::new(Length::from_mm(x), Length::from_mm(y)))
            .collect();
        let step = StepPlan {
            index: 0,
            name: "Outline".to_string(),
            blocks: vec![ToolBlock {
                tool_id: "r1".to_string(),
                slot: Some(4),
                diameter: Length::from_mm(2.0),
                ops: vec![AtomicOp {
                    phase: Phase::Route,
                    kind: OpKind::RouteContour { path: path.clone() },
                    tool_id: "r1".to_string(),
                    entry: path[0],
                    exit: path[2],
                    z: ZProfile {
                        z_bottom: Length::from_mm(-2.1),
                        z_retract: Length::from_mm(5.0),
                        z_feed: None,
                    },
                    primitive: "route_contour",
                    source: "outer#0.span0".to_string(),
                }],
                travel_mm: 0.0,
            }],
            notes: vec![],
        };
        let body = render_step_body(&coder, &step, &render_ctx(true), &router_feed()).expect("routes");

        let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
        let cut = lines.iter().position(|l| l.starts_with("G0 X1 Y0 Z5")).expect("rapid to the span start");
        assert_eq!(lines[cut + 1], "G1 X1 Y0 Z-2.1 F200", "plunge at the start");
        assert_eq!(lines[cut + 2], "G1 X10 Y0 Z-2.1 F600");
        assert_eq!(lines[cut + 3], "G1 X10 Y5 Z-2.1 F600", "every vertex is cut through");
        assert_eq!(lines[cut + 4], "G0 X10 Y5 Z5", "lifts off, leaving the tab uncut");
    }

    /// The router used by the routing tests: 600 mm/min rated at a reachable speed, so
    /// the lateral feed renders as 600 and the derated plunge as 200.
    fn router_feed() -> BTreeMap<String, ToolFeed> {
        let mut tf = BTreeMap::new();
        tf.insert(
            "r1".to_string(),
            ToolFeed {
                name: "1mm router".to_string(),
                feed: Some(FeedRate::from_mm_per_min(600.0)),
                speed: Some(RotationalSpeed::from_rpm(18_000.0)),
            },
        );
        tf
    }

    /// The number, its step and its separator all come from the profile's template —
    /// the application only supplies `line`. The template's closing backtick is what
    /// keeps the prefix on the same output line.
    #[test]
    fn line_numbering_is_entirely_the_profiles_template() {
        let coder = Coder::new();
        // Blank lines are dropped; the rest step by whatever arithmetic the profile
        // writes, here the conventional ten.
        assert_eq!(
            number_lines(&coder, "G21\n\nG0 Z5", "`N{line * 10} `").unwrap(),
            "N10 G21\nN20 G0 Z5"
        );
        // A different dialect is a template edit, not a code change.
        assert_eq!(
            number_lines(&coder, "G21\nG0 Z5", "`/{line}:`").unwrap(),
            "/1:G21\n/2:G0 Z5"
        );
        // An empty template disables numbering — what `line_numbering_increment: 0` was.
        assert_eq!(number_lines(&coder, "G21\nG0 Z5", "").unwrap(), "G21\nG0 Z5");
    }

    /// Without the closing backtick the prefix would take the whole line to itself and
    /// push the GCode onto the next one — so the parser rule is load-bearing here, and
    /// this pins it.
    #[test]
    fn a_line_number_template_without_the_closing_backtick_breaks_the_line() {
        let coder = Coder::new();
        assert_eq!(
            number_lines(&coder, "G21", "`N{line * 10} ").unwrap(),
            "N10 \nG21",
            "the emitted newline separates the number from its line"
        );
    }

    /// The line itself is in scope as `text`, so *whether* to number is the profile's
    /// decision as much as how — a template that emits nothing leaves that line bare,
    /// which is how a profile keeps `N` words off its comments. The line is still
    /// emitted: the primitive only ever contributes a prefix.
    #[test]
    fn a_line_number_template_can_read_the_line_and_skip_it() {
        let coder = Coder::new();
        let skip_comments = "if !text.starts_with(\"(\") {\n    `N{line * 10} `\n}";
        assert_eq!(
            number_lines(&coder, "(header)\nG21\n(done)\nG0 Z5", skip_comments).unwrap(),
            "(header)\nN20 G21\n(done)\nN40 G0 Z5",
            "comments pass through unnumbered, and the count still spans every line"
        );
    }

    /// The bundled templates carry real script now, not just substitution, and a Rhai
    /// error in one would otherwise surface as a failed generation in the field — a
    /// profile seeded from a template is never rendered until a job runs.
    #[test]
    fn every_bundled_line_number_template_renders() {
        let coder = Coder::new();
        for (key, yaml) in [
            ("genmitsu_3018", include_str!("../../assets/cnc_templates/genmitsu_3018.yaml")),
            ("masso_g3_with_atc", include_str!("../../assets/cnc_templates/masso_g3_with_atc.yaml")),
            ("masso_g3_no_atc", include_str!("../../assets/cnc_templates/masso_g3_no_atc.yaml")),
            ("batam", include_str!("../../assets/cnc_templates/batam.yaml")),
        ] {
            let value: serde_json::Value =
                serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("[{key}] is not YAML: {e}"));
            let template = value
                .pointer("/primitives/line_number")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("[{key}] has no line_number primitive"));

            let numbered = number_lines(&coder, "(a comment)\nG21", template)
                .unwrap_or_else(|e| panic!("[{key}] line_number failed to render: {e:?}"));
            let lines: Vec<&str> = numbered.lines().collect();
            assert!(lines[1].contains("G21"), "[{key}] lost the line it numbers");
            // Only the Masso profiles opt out of numbering comments; the others are
            // pinned here as numbering everything, so a change to either is deliberate.
            if key.starts_with("masso") {
                assert_eq!(lines[0], "(a comment)", "[{key}] numbered a comment");
            } else {
                assert!(lines[0].starts_with('N'), "[{key}] left a line unnumbered");
            }
        }
    }

    /// A template that cannot render stops generation: quietly shipping an unnumbered
    /// program to a controller that requires numbers is the worse failure.
    #[test]
    fn a_broken_line_number_template_is_a_named_error() {
        let coder = Coder::new();
        let err = number_lines(&coder, "G21", "`N{nope}`").unwrap_err();
        match err {
            BodyError::Render { primitive, .. } => assert_eq!(primitive, "line_number"),
            other => panic!("expected a Render error, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_without_feed_or_speed_is_a_named_error() {
        let coder = Coder::new();
        let err = render_step_body(
            &coder,
            &one_block_step(),
            &render_ctx(true),
            &feeds_for(None, Some(12_000.0)),
        )
        .unwrap_err();
        match err {
            BodyError::Feeds { tool, .. } => assert_eq!(tool, "1.0mm drill"),
            other => panic!("expected a Feeds error, got {other:?}"),
        }
    }
}
