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
use crate::gcode::feeds::{self, FeedsError, FeedsSpeeds, SpindleRange};
use crate::gcode::plan::{OpKind, StepPlan};
use crate::gcode::routing::{self, RouteMove};

/// The per-step rendering context the body needs beyond the plan: the CNC's operation
/// primitive templates, its spindle range (for the feed/speed clamp), and whether tool
/// changes are automatic (ATC) — a manual machine gets an operator prompt instead.
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
    pub spindle: SpindleRange,
    pub is_atc: bool,
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
        let fs = feeds::resolve(feed, speed, render.spindle)
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

/// Renders one move of a routed-hole spiral through the matching motion primitive:
/// `Rapid`→`rapid_move`, `Cut`→`linear_cut` (carrying the spindle rpm), `Arc`→`cut_arc`
/// (carrying the XY feed). Geometry is already in machine coordinates — the Coder only
/// formats it (op-planner §6).
fn render_route_move(
    coder: &Coder,
    render: &StepRender,
    mv: &RouteMove,
    fs: FeedsSpeeds,
) -> Result<String, BodyError> {
    match *mv {
        RouteMove::Rapid { x, y, z } => {
            let mut s = Scope::new();
            s.push("x", x);
            s.push("y", y);
            s.push("z", z);
            render_one(coder, "rapid_move", &render.rapid_move_tpl, &mut s)
        }
        RouteMove::Cut { x, y, z } => {
            let mut s = Scope::new();
            s.push("x", x);
            s.push("y", y);
            s.push("z", z);
            s.push("s", fs.rpm);
            render_one(coder, "linear_cut", &render.linear_cut_tpl, &mut s)
        }
        RouteMove::Arc { x, y, i, j, ccw } => {
            let mut s = Scope::new();
            s.push("arc_cmd", if ccw { "G3".to_string() } else { "G2".to_string() });
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

/// Prefixes every non-blank line of the assembled program with a sequential `N` number,
/// stepping by the CNC's `line_numbering_increment`. Line numbering is a whole-program
/// concern — no primitive can number its own line — so it runs once here, over the
/// finished program, rather than inside any template. An increment of `0` disables it
/// (the program is returned unchanged); otherwise blank lines are dropped so the
/// numbered program is contiguous.
pub fn number_lines(program: &str, increment: u16) -> String {
    if increment == 0 {
        return program.to_string();
    }
    let mut n: u32 = 0;
    program
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            n += u32::from(increment);
            format!("N{n} {line}")
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        linear_cut_tpl: "`G1 X{x} Y{y} Z{z} S{s}".to_string(),
        cut_arc_tpl: "`{arc_cmd} X{x} Y{y} I{i} J{j} F{xy_feedrate}".to_string(),
        spindle: SpindleRange::new(
            RotationalSpeed::from_rpm(5_000.0),
            RotationalSpeed::from_rpm(24_000.0),
        ),
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
        let mut tf = BTreeMap::new();
        tf.insert(
            "r1".to_string(),
            ToolFeed {
                name: "1mm router".to_string(),
                feed: Some(FeedRate::from_mm_per_min(400.0)),
                speed: Some(RotationalSpeed::from_rpm(18_000.0)),
            },
        );
        let body = render_step_body(&coder, &step, &render_ctx(true), &tf).expect("routes");

        assert!(body.contains("G0 X5 Y5 Z5"), "rapid to centre above the work:\n{body}");
        assert!(body.contains("G1 X5 Y5 Z-2.1 S18000"), "plunge at the centre (no island):\n{body}");
        // Spiral: a G3 full circle ending on the wall (reach = (3.2-1.0)/2 = 1.1 → X6.1).
        assert!(body.contains("G3 X6.1 Y5 I-1.1 J0 F400"), "finishing lap on the wall:\n{body}");
    }

    #[test]
    fn number_lines_prefixes_non_blank_lines_and_honours_the_increment() {
        // Blank lines are dropped; the rest step by the increment.
        assert_eq!(number_lines("G21\n\nG0 Z5", 10), "N10 G21\nN20 G0 Z5");
        // Increment 0 disables numbering entirely.
        assert_eq!(number_lines("G21\nG0 Z5", 0), "G21\nG0 Z5");
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
